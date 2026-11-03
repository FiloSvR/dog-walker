//! `init`: an installer that writes the profiles and tells the main harness about dog-walker
//! (D19).
//!
//! The planning is pure: what to write is computed from the answers, the host's layout and the
//! install directories found, so it is tested without a terminal or a harness. `run_init` asks,
//! shows the plan, and writes it only after a yes. It writes names, never values: no credential
//! is read, copied or asked for, and the env files it creates hold the non-secret lines and a
//! comment for the secret.

// Why: building text with `format!` and `push_str` is what the lint flags, and its advice, `write!`,
// is the spelling that only the `output` module may use (tests/architecture.rs).
#![allow(clippy::format_push_string)]

use std::fs;
use std::io::{BufRead, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::config::Harness;
use crate::error::SetupError;
use crate::output;

/// Marks the block in a harness's instructions file, so a second run finds it.
pub const BEGIN: &str = "<!-- dog-walker:begin -->";
pub const END: &str = "<!-- dog-walker:end -->";
/// What the main harness is told. The text it points to ships in the binary, so it follows the
/// installed version.
pub const LINE: &str =
    "To delegate work to another harness, run `dog-walker instructions` first and follow it.";

/// The roles the installer asks about, and the profile each becomes.
pub const ROLES: [&str; 3] = ["main", "worker", "reviewer"];

/// A harness and model chosen for a role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub harness: Harness,
    pub model: String,
    /// The name of the variable that holds the credential, when the harness needs one.
    pub credential: Option<String>,
}

/// Where things are on this machine.
#[derive(Debug, Clone)]
pub struct Host {
    pub home: PathBuf,
    /// The directory of the config file; each profile's private directory lives under it.
    pub config_dir: PathBuf,
    /// The directories of `PATH`.
    pub path: Vec<PathBuf>,
    /// The project the command runs in: where a project-scoped instructions file goes.
    pub project: PathBuf,
}

/// Whose instructions file the dog-walker line goes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The current project's file: only that project's sessions are told.
    Project,
    /// The main harness's global file: every project's sessions are told.
    Global,
}

impl Scope {
    pub fn parse(text: &str) -> Result<Scope, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "project" | "p" => Ok(Scope::Project),
            "global" | "g" => Ok(Scope::Global),
            other => Err(format!(
                "`{other}` is not a scope: use `project` or `global`"
            )),
        }
    }
}

/// The model suggested when the user does not name one.
pub fn default_model(harness: Harness) -> &'static str {
    match harness {
        Harness::Claude => "claude-sonnet-5-5",
        Harness::Codex => "gpt-6.1-terra",
        Harness::Pi => "openrouter/z-ai/glm-5.3-flash",
        Harness::Dsh => "deepseek",
    }
}

/// The variable a harness's credential is usually in; the user can name another.
pub fn default_credential(harness: Harness, model: &str) -> Option<String> {
    match harness {
        Harness::Claude => Some("CLAUDE_CODE_OAUTH_TOKEN".to_owned()),
        // Codex signs in with a login copied into its private home, not with a variable.
        Harness::Codex => None,
        Harness::Dsh => Some("DEEPSEEK_API_KEY".to_owned()),
        Harness::Pi => {
            let provider = model.split_once('/')?.0;
            let name: String = provider
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_uppercase()
                    } else {
                        '_'
                    }
                })
                .collect();
            // A provider that gives no valid name (it starts with a digit) gets none, and the
            // profile's note says to add the variable by hand.
            let name = format!("{name}_API_KEY");
            check_variable(&name).ok().map(|()| name)
        }
    }
}

/// Rejects text that could not be written safely into a config or an env file.
fn check_text(text: &str, what: &str) -> Result<(), String> {
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err(format!("{what} is empty or has a control character"));
    }
    Ok(())
}

/// A variable name: letters, digits and `_`, not starting with a digit.
pub fn check_variable(text: &str) -> Result<(), String> {
    let valid = !text.is_empty()
        && !text.starts_with(|c: char| c.is_ascii_digit())
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "`{text}` is not a variable name: letters, digits and `_`"
        ))
    }
}

/// Reads `harness[:model]`, or `no` for a role that is skipped.
pub fn parse_pick(text: &str) -> Result<Option<Pick>, String> {
    let text = text.trim();
    if matches!(
        text.to_ascii_lowercase().as_str(),
        "no" | "none" | "n" | "-"
    ) {
        return Ok(None);
    }
    let (name, model) = text.split_once(':').unwrap_or((text, ""));
    let harness = Harness::parse(name.trim())
        .ok_or_else(|| format!("`{name}` is not a harness: claude, codex, pi or dsh"))?;
    let model = match model.trim() {
        "" => default_model(harness).to_owned(),
        given => given.to_owned(),
    };
    check_text(&model, "the model")?;
    let credential = default_credential(harness, &model);
    Ok(Some(Pick {
        harness,
        model,
        credential,
    }))
}

/// A TOML basic string.
fn quote(text: &str) -> String {
    let mut quoted = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            c if c.is_control() => quoted.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

fn quote_list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| quote(item)).collect();
    format!("[{}]", quoted.join(", "))
}

/// A word for a shell that sources the env file.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The first executable called `name` on `path`, with links resolved.
pub fn find_program(name: &str, path: &[PathBuf]) -> Option<PathBuf> {
    path.iter().find_map(|dir| {
        let candidate = dir.join(name);
        let executable = fs::metadata(&candidate)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0);
        if executable {
            fs::canonicalize(&candidate).ok()
        } else {
            None
        }
    })
}

/// The install directory of `harness`, from where its program resolves to. The layouts are the
/// ones `docs/setup.md` names; a layout that does not match gives `None`, and the profile then
/// says to add the directory by hand.
pub fn install_dir(harness: Harness, resolved: &Path) -> Option<PathBuf> {
    let parts: Vec<&str> = resolved
        .iter()
        .map(|part| part.to_str())
        .collect::<Option<_>>()?;
    let prefix = |count: usize| -> PathBuf { parts.iter().take(count).collect() };
    let position = |names: &[&str]| -> Option<usize> {
        parts
            .windows(names.len())
            .position(|window| window == names)
    };
    match harness {
        // ~/.local/share/claude/versions/<v>
        Harness::Claude => position(&["claude", "versions"]).map(|at| prefix(at + 1)),
        // .../node_modules/@openai/codex/bin/codex.js
        Harness::Codex => position(&["@openai", "codex"]).map(|at| prefix(at + 2)),
        // .../node_modules/@deepseek-ai/dsh/lib/bin.js
        Harness::Dsh => position(&["@deepseek-ai", "dsh"]).map(|at| prefix(at + 2)),
        // ~/.pi/agent/bin/pi, with the install beside it
        Harness::Pi => position(&["agent", "bin"]).map(|at| prefix(at + 1).join("install")),
    }
}

/// What to write for one profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfilePlan {
    pub name: String,
    /// The text appended to the config.
    pub toml: String,
    /// Private directories to create (mode 700).
    pub dirs: Vec<PathBuf>,
    /// An env file to create (mode 600), if the profile needs one.
    pub env_file: Option<(PathBuf, String)>,
    /// What is left for the user to do, as commands and names.
    pub notes: Vec<String>,
}

/// The main profile: a harness and a model, never launched by dog-walker.
pub fn plan_main(name: &str, pick: &Pick) -> ProfilePlan {
    let toml = format!(
        "[profiles.{name}]\nharness = {}\nmodel = {}\nmain = true\n",
        quote(pick.harness.as_str()),
        quote(&pick.model)
    );
    ProfilePlan {
        name: name.to_owned(),
        toml,
        dirs: Vec::new(),
        env_file: None,
        notes: Vec::new(),
    }
}

/// A secondary profile: the sandbox part of it, from what the harness needs (`docs/setup.md`).
/// `installed` is the harness's install directory when it was found.
pub fn plan_secondary(
    name: &str,
    pick: &Pick,
    host: &Host,
    installed: Option<&Path>,
) -> ProfilePlan {
    let private = host.config_dir.join(name);
    let tmp = private.join("tmp");
    let env_path = host.config_dir.join(format!("{name}.env"));
    let text = |path: &Path| path.to_string_lossy().into_owned();

    // The variables whose value the env file sets, and the ones the profile names.
    let (file_vars, mut env, dirs): (Vec<(&str, String)>, Vec<String>, Vec<PathBuf>) =
        match pick.harness {
            Harness::Claude => (
                vec![
                    ("CLAUDE_CONFIG_DIR", text(&private)),
                    ("CLAUDE_CODE_TMPDIR", text(&tmp)),
                    ("TMPDIR", text(&tmp)),
                ],
                vec![
                    "CLAUDE_CONFIG_DIR".into(),
                    "CLAUDE_CODE_TMPDIR".into(),
                    "TMPDIR".into(),
                    "HOME".into(),
                ],
                vec![private.clone(), tmp],
            ),
            Harness::Pi => (
                vec![("PI_CODING_AGENT_DIR", text(&private))],
                vec!["PI_CODING_AGENT_DIR".into()],
                vec![private.clone()],
            ),
            Harness::Codex => (
                vec![("CODEX_HOME", text(&private))],
                vec!["CODEX_HOME".into()],
                vec![private.clone()],
            ),
            Harness::Dsh => (
                vec![("DSH_HOME", text(&private)), ("TMPDIR", text(&tmp))],
                vec!["DSH_HOME".into(), "TMPDIR".into()],
                vec![private.clone(), private.join("profiles"), tmp],
            ),
        };
    let mut secrets = Vec::new();
    if let Some(credential) = &pick.credential {
        env.insert(0, credential.clone());
        secrets.push(credential.clone());
    }
    let read_only: Vec<String> = installed.map(text).into_iter().collect();

    let mut toml = format!(
        "[profiles.{name}]\nharness = {}\nmodel = {}\n",
        quote(pick.harness.as_str()),
        quote(&pick.model)
    );
    if pick.harness == Harness::Dsh {
        toml.push_str("dsh_profile = \"headless\"\n");
    }
    toml.push_str(&format!("env = {}\n", quote_list(&env)));
    if !secrets.is_empty() {
        toml.push_str(&format!("secrets = {}\n", quote_list(&secrets)));
    }
    if !read_only.is_empty() {
        toml.push_str(&format!("read_only_dirs = {}\n", quote_list(&read_only)));
    }
    toml.push_str(&format!(
        "writable_dirs = {}\n",
        quote_list(&[text(&private)])
    ));

    let mut env_text = format!(
        "# Variables for the `{name}` profile. Load them in the shell that runs dog-walker:\n\
         #   set -a; . {}; set +a\n",
        shell_quote(&text(&env_path))
    );
    for (variable, value) in &file_vars {
        env_text.push_str(&format!("{variable}={}\n", shell_quote(value)));
    }
    if let Some(credential) = &pick.credential {
        env_text.push_str(&format!(
            "# {credential}=<your credential: uncomment, and keep this file mode 600>\n"
        ));
    }

    let notes = notes_for(name, pick, &private, &env_path, installed.is_some());
    ProfilePlan {
        name: name.to_owned(),
        toml,
        dirs,
        env_file: Some((env_path, env_text)),
        notes,
    }
}

/// The steps the installer leaves to the user, because they need the user's login or secret.
fn notes_for(name: &str, pick: &Pick, private: &Path, env_path: &Path, found: bool) -> Vec<String> {
    // Paths in a command the user will paste are quoted; the file is named plainly in prose.
    let quoted = |path: &Path| shell_quote(&path.to_string_lossy());
    let env = env_path.display();
    let mut notes = Vec::new();
    if !found {
        notes.push(format!(
            "`{}` was not found where this installer looks: add its install directory to \
             `read_only_dirs` of `{name}` by hand (`docs/setup.md`)",
            pick.harness
        ));
    }
    if pick.credential.is_none() && pick.harness != Harness::Codex {
        notes.push(format!(
            "`{name}`: no credential variable could be derived from the model `{}`: add its \
             name to `env` and `secrets` of `{name}` by hand",
            pick.model
        ));
    }
    match pick.harness {
        Harness::Claude => notes.push(format!(
            "`{name}`: run `claude setup-token` in your terminal and put the token in {env} as \
             {} (or use an API key and name that variable)",
            pick.credential.as_deref().unwrap_or("the credential")
        )),
        Harness::Codex => notes.push(format!(
            "`{name}`: copy your login once: `install -m 600 ~/.codex/auth.json {}`",
            quoted(&private.join("auth.json"))
        )),
        Harness::Pi => notes.push(format!(
            "`{name}`: put your key in {env} as {}",
            pick.credential
                .as_deref()
                .unwrap_or("the provider's key variable")
        )),
        Harness::Dsh => notes.push(format!(
            "`{name}`: `cp -r ~/.dsh/profiles/headless {}` and put your key in {env} as {}",
            quoted(&private.join("profiles")),
            pick.credential.as_deref().unwrap_or("DEEPSEEK_API_KEY")
        )),
    }
    notes.push(format!(
        "`{name}`: then `set -a; . {}; set +a` and `dog-walker doctor {name}`",
        quoted(env_path)
    ));
    notes
}

/// What an existing config already has.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Existing {
    pub names: Vec<String>,
    pub main_harness: Option<Harness>,
}

/// Reads the profile names and the main profile's harness of an existing config.
pub fn read_existing(text: &str) -> Result<Existing, String> {
    let table: toml::Table = toml::from_str(text).map_err(|error| error.to_string())?;
    let mut existing = Existing::default();
    let Some(profiles) = table.get("profiles").and_then(toml::Value::as_table) else {
        return Ok(existing);
    };
    for (name, profile) in profiles {
        existing.names.push(name.clone());
        let is_main = profile.get("main").and_then(toml::Value::as_bool) == Some(true);
        if is_main {
            existing.main_harness = profile
                .get("harness")
                .and_then(toml::Value::as_str)
                .and_then(Harness::parse);
        }
    }
    Ok(existing)
}

/// The main harness's instructions file in `scope`, when dog-walker knows where it is. Pi and
/// dsh have none that is verified, so they get the line printed instead.
pub fn instructions_file(harness: Harness, scope: Scope, host: &Host) -> Option<PathBuf> {
    match (harness, scope) {
        (Harness::Claude, Scope::Global) => Some(host.home.join(".claude").join("CLAUDE.md")),
        (Harness::Codex, Scope::Global) => Some(host.home.join(".codex").join("AGENTS.md")),
        (Harness::Claude, Scope::Project) => Some(host.project.join("CLAUDE.md")),
        (Harness::Codex, Scope::Project) => Some(host.project.join("AGENTS.md")),
        (Harness::Pi | Harness::Dsh, _) => None,
    }
}

/// The block that is added to an instructions file.
pub fn block() -> String {
    format!("{BEGIN}\n{LINE}\n{END}\n")
}

/// What to append to `existing` to add the block; `None` when it is there already.
pub fn addition(existing: &str) -> Option<String> {
    if existing.contains(BEGIN) {
        return None;
    }
    let separator = match existing {
        "" => "",
        text if text.ends_with("\n\n") => "",
        text if text.ends_with('\n') => "\n",
        _ => "\n\n",
    };
    Some(format!("{separator}{}", block()))
}

/// What was asked on the command line: a `harness[:model]` or `no` per role, or nothing to ask.
#[derive(Debug, Default)]
pub struct Options {
    pub main: Option<String>,
    pub worker: Option<String>,
    pub reviewer: Option<String>,
    /// `project` or `global`; asked when not given (and the project when `yes` skips asking).
    pub scope: Option<String>,
    pub yes: bool,
}

impl Options {
    fn answer(&self, role: &str) -> Option<&str> {
        match role {
            "main" => self.main.as_deref(),
            "worker" => self.worker.as_deref(),
            _ => self.reviewer.as_deref(),
        }
    }
}

/// Reads one answer; an empty line takes `default`.
fn ask(
    input: &mut dyn BufRead,
    err: &mut dyn Write,
    question: &str,
    default: Option<&str>,
) -> Result<String, SetupError> {
    let shown = default.map_or_else(String::new, |value| format!(" [{value}]"));
    output::text(err, &format!("{question}{shown}: "))?;
    let mut line = String::new();
    let read = input
        .read_line(&mut line)
        .map_err(|source| SetupError::Read { source })?;
    if read == 0 {
        return Err(SetupError::NoAnswer {
            question: question.to_owned(),
        });
    }
    let line = line.trim();
    Ok(match (line.is_empty(), default) {
        (true, Some(default)) => default.to_owned(),
        _ => line.to_owned(),
    })
}

/// The pick for `role`: from the command line, or asked.
fn pick_for(
    role: &str,
    options: &Options,
    detected: &[Harness],
    input: &mut dyn BufRead,
    err: &mut dyn Write,
) -> Result<Option<Pick>, SetupError> {
    if let Some(given) = options.answer(role) {
        return parse_pick(given).map_err(|why| SetupError::Answer {
            role: role.to_owned(),
            why,
        });
    }
    let found = detected
        .iter()
        .map(|harness| harness.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let found = if found.is_empty() {
        "none found on PATH".to_owned()
    } else {
        format!("found: {found}")
    };
    let optional = role != "main";
    let default = if optional {
        Some("no")
    } else {
        detected.first().map(|harness| harness.as_str())
    };
    let end = if optional { ", or `no` to skip" } else { "" };
    let question = format!("{role}: harness (claude, codex, pi, dsh; {found}{end})");
    let harness = loop {
        let answer = ask(input, err, &question, default)?;
        match parse_pick(&answer) {
            Ok(Some(pick)) => break pick.harness,
            Ok(None) if optional => return Ok(None),
            Ok(None) => output::line(err, "the main profile is required")?,
            Err(why) => output::line(err, &why)?,
        }
    };
    let suggested = default_model(harness);
    let model = loop {
        let model = ask(input, err, &format!("{role}: model"), Some(suggested))?;
        match check_text(&model, "the model") {
            Ok(()) => break model,
            Err(why) => output::line(err, &why)?,
        }
    };
    let mut credential = default_credential(harness, &model);
    if optional && let Some(suggested) = credential.clone() {
        credential = loop {
            let name = ask(
                input,
                err,
                &format!("{role}: name of the variable that holds the credential (not its value)"),
                Some(&suggested),
            )?;
            match check_variable(&name) {
                Ok(()) => break Some(name),
                Err(why) => output::line(err, &why)?,
            }
        };
    }
    Ok(Some(Pick {
        harness,
        model,
        credential,
    }))
}

/// Everything `init` will write.
#[derive(Debug)]
struct Plan {
    config: PathBuf,
    profiles: Vec<ProfilePlan>,
    /// The main harness's instructions file and what to append to it.
    instructions: Option<(PathBuf, String)>,
    /// The install directories found, for the preflight.
    installed: Vec<PathBuf>,
    /// Said to the user, not written.
    notes: Vec<String>,
}

/// Asks what is not given, shows the plan, and writes it after a yes. Prompts and the plan go
/// to `err`; the config path goes to `out`.
pub fn run_init(
    options: &Options,
    config_path: &Path,
    host: &Host,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), SetupError> {
    let plan = build(options, config_path, host, input, err)?;
    preflight(&plan, host)?;
    show(&plan, err)?;
    if plan.profiles.is_empty() && plan.instructions.is_none() {
        output::line(err, "dog-walker: nothing to write")?;
    } else if options.yes || confirmed(input, err)? {
        write_plan(&plan)?;
        output::line(err, "dog-walker: written")?;
    } else {
        return Err(SetupError::Declined);
    }
    for note in &plan.notes {
        output::line(err, &format!("  - {note}"))?;
    }
    output::path(out, config_path)?;
    Ok(())
}

fn confirmed(input: &mut dyn BufRead, err: &mut dyn Write) -> Result<bool, SetupError> {
    let answer = ask(input, err, "Write these?  (y/N)", Some("n"))?;
    Ok(matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn build(
    options: &Options,
    config_path: &Path,
    host: &Host,
    input: &mut dyn BufRead,
    err: &mut dyn Write,
) -> Result<Plan, SetupError> {
    let existing = match fs::read_to_string(config_path) {
        Ok(text) => read_existing(&text).map_err(|why| SetupError::Config {
            path: config_path.to_owned(),
            why,
        })?,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Existing::default(),
        Err(source) => {
            return Err(SetupError::Io {
                path: config_path.to_owned(),
                source,
            });
        }
    };
    let detected: Vec<Harness> = Harness::ALL
        .into_iter()
        .filter(|harness| find_program(harness.as_str(), &host.path).is_some())
        .collect();
    let mut plan = Plan {
        config: config_path.to_owned(),
        profiles: Vec::new(),
        instructions: None,
        installed: Vec::new(),
        notes: Vec::new(),
    };
    let mut main_harness = existing.main_harness;
    for role in ROLES {
        if existing.names.iter().any(|name| name == role)
            || (role == "main" && main_harness.is_some())
        {
            plan.notes
                .push(format!("`{role}` is in the config already: left as it is"));
            continue;
        }
        let Some(pick) = pick_for(role, options, &detected, input, err)? else {
            continue;
        };
        if role == "main" {
            main_harness = Some(pick.harness);
            plan.profiles.push(plan_main(role, &pick));
        } else {
            let installed = find_program(pick.harness.as_str(), &host.path)
                .and_then(|resolved| install_dir(pick.harness, &resolved))
                .filter(|dir| dir.is_dir());
            plan.installed.extend(installed.clone());
            plan.profiles
                .push(plan_secondary(role, &pick, host, installed.as_deref()));
        }
    }
    let Some(main_harness) = main_harness else {
        return Err(SetupError::NoMain);
    };
    plan.notes
        .extend(plan.profiles.iter().flat_map(|p| p.notes.clone()));
    let scope = scope_for(main_harness, options, host, input, err)?;
    match instructions_file(main_harness, scope, host) {
        Some(file) => {
            let current = match fs::read_to_string(&file) {
                Ok(text) => text,
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(source) => return Err(SetupError::Io { path: file, source }),
            };
            match addition(&current) {
                Some(add) => plan.instructions = Some((file, add)),
                None => plan.notes.push(format!(
                    "{} already tells the main harness about dog-walker",
                    file.display()
                )),
            }
        }
        None => plan.notes.push(format!(
            "add this line to {main_harness}'s own instructions file: {LINE}"
        )),
    }
    Ok(plan)
}

/// Where the instructions line goes: from the command line, or asked, or the project when
/// `--yes` says not to ask. Not asked for a harness with no file to write.
fn scope_for(
    harness: Harness,
    options: &Options,
    host: &Host,
    input: &mut dyn BufRead,
    err: &mut dyn Write,
) -> Result<Scope, SetupError> {
    if let Some(given) = &options.scope {
        return Scope::parse(given).map_err(|why| SetupError::Answer {
            role: "scope".to_owned(),
            why,
        });
    }
    let (Some(project), Some(global)) = (
        instructions_file(harness, Scope::Project, host),
        instructions_file(harness, Scope::Global, host),
    ) else {
        return Ok(Scope::Project);
    };
    if options.yes {
        return Ok(Scope::Project);
    }
    output::line(
        err,
        &format!(
            "the dog-walker line can go in this project's file ({}) or in your global one ({}), which tells every project",
            project.display(),
            global.display()
        ),
    )?;
    loop {
        let answer = ask(input, err, "scope: project or global", Some("project"))?;
        match Scope::parse(&answer) {
            Ok(scope) => return Ok(scope),
            Err(why) => output::line(err, &why)?,
        }
    }
}

fn show(plan: &Plan, err: &mut dyn Write) -> Result<(), SetupError> {
    output::line(err, "dog-walker will write:")?;
    if !plan.profiles.is_empty() {
        let names: Vec<&str> = plan.profiles.iter().map(|p| p.name.as_str()).collect();
        output::line(
            err,
            &format!(
                "  - profiles {} appended to {}",
                names.join(", "),
                plan.config.display()
            ),
        )?;
    }
    for profile in &plan.profiles {
        for dir in &profile.dirs {
            output::line(err, &format!("  - directory {} (mode 700)", dir.display()))?;
        }
        if let Some((path, _)) = &profile.env_file {
            output::line(
                err,
                &format!(
                    "  - env file {} (mode 600, no secret in it)",
                    path.display()
                ),
            )?;
        }
    }
    if let Some((file, _)) = &plan.instructions {
        // A link is followed on purpose (a dotfiles repo is common); the user sees where.
        let target = fs::canonicalize(file)
            .ok()
            .filter(|real| real != file)
            .map_or_else(String::new, |real| {
                format!(" (a link to {})", real.display())
            });
        output::line(
            err,
            &format!(
                "  - the dog-walker block appended to {}{target}:",
                file.display()
            ),
        )?;
        output::line(err, &format!("      {LINE}"))?;
    }
    Ok(())
}

/// Writes the plan. The config goes last: a profile is recorded only once the directories and
/// files it needs exist, so a run that fails midway is finished by running `init` again.
fn write_plan(plan: &Plan) -> Result<(), SetupError> {
    for profile in &plan.profiles {
        for dir in &profile.dirs {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|source| SetupError::Io {
                    path: dir.clone(),
                    source,
                })?;
            // It may have existed, or changed, since the preflight.
            check_existing(dir, true)?;
        }
        if let Some((path, text)) = &profile.env_file {
            create_private(path, text)?;
        }
    }
    // Read again now: the file may have changed while the user was answering.
    if let Some((file, _)) = &plan.instructions {
        let current = read_or_empty(file)?;
        if let Some(add) = addition(&current) {
            append(file, &add, 0o644)?;
        }
    }
    let mut config = String::new();
    if !plan.profiles.is_empty() {
        let existing = read_or_empty(&plan.config)?;
        let names = read_existing(&existing).map_err(|why| SetupError::Config {
            path: plan.config.clone(),
            why,
        })?;
        for profile in &plan.profiles {
            if names.names.contains(&profile.name) {
                return Err(SetupError::Changed {
                    path: plan.config.clone(),
                    name: profile.name.clone(),
                });
            }
            config.push_str(&format!("\n{}", profile.toml));
        }
        append(&plan.config, &config, 0o644)?;
    }
    Ok(())
}

/// The text of `path`, or nothing when it is not there.
fn read_or_empty(path: &Path) -> Result<String, SetupError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(SetupError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

/// Refuses what would make the setup half-done or the secret exposed, before anything is
/// written: a path that cannot be written into a config or env file as it is, a private
/// directory or env file that exists as the wrong kind of thing or is open to others.
fn preflight(plan: &Plan, host: &Host) -> Result<(), SetupError> {
    let unsafe_path = |path: &Path, why: &str| SetupError::UnsafePath {
        path: path.to_owned(),
        why: why.to_owned(),
    };
    let mut paths: Vec<&Path> = vec![&plan.config, &host.config_dir, &host.home];
    paths.extend(plan.installed.iter().map(PathBuf::as_path));
    for path in paths {
        let Some(text) = path.to_str() else {
            return Err(unsafe_path(
                path,
                "it is not valid UTF-8, which a config needs",
            ));
        };
        if text.chars().any(char::is_control) {
            return Err(unsafe_path(
                path,
                "it has a control character, such as a newline",
            ));
        }
    }
    for profile in &plan.profiles {
        for dir in &profile.dirs {
            check_existing(dir, true)?;
        }
        if let Some((path, _)) = &profile.env_file {
            check_existing(path, false)?;
        }
    }
    Ok(())
}

/// An existing private directory or env file must be what it should be and closed to others.
fn check_existing(path: &Path, directory: bool) -> Result<(), SetupError> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    let exists = |why: &str| SetupError::Exists {
        path: path.to_owned(),
        why: why.to_owned(),
    };
    let right_kind = if directory {
        meta.is_dir()
    } else {
        meta.is_file()
    };
    if !right_kind {
        let kind = if directory {
            "a directory"
        } else {
            "a regular file"
        };
        return Err(exists(&format!(
            "exists and is not {kind} (a link is refused)"
        )));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(exists(
            "exists and is open to others: `chmod 700` (a file: `chmod 600`) and run init again",
        ));
    }
    Ok(())
}

/// Appends `text` to `path`, creating it (and its folder) if needed. A write that fails midway
/// is cut back to the length the file had, so a half-written block or profile is not left for
/// the next run to take as finished.
fn append(path: &Path, text: &str, mode: u32) -> Result<(), SetupError> {
    let io = |source| SetupError::Io {
        path: path.to_owned(),
        source,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io)?;
    }
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(mode)
        .open(path)
        .map_err(io)?;
    let before = file.metadata().map_err(io)?.len();
    let written = file.write_all(text.as_bytes());
    if written.is_err() {
        let _ = file.set_len(before);
    }
    written.map_err(io)
}

/// Creates a mode 600 file; one that exists is left alone, since it may hold the user's secret.
fn create_private(path: &Path, text: &str) -> Result<(), SetupError> {
    let io = |source| SetupError::Io {
        path: path.to_owned(),
        source,
    };
    match fs::OpenOptions::new()
        .append(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(mut file) => {
            let written = file.write_all(text.as_bytes());
            if written.is_err() {
                // A half-written file would be taken for a finished one on the next run.
                let _ = fs::remove_file(path);
            }
            written.map_err(io)
        }
        // One that is there stays, as long as it is the right kind of file and closed to others.
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            check_existing(path, false)
        }
        Err(source) => Err(io(source)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn host() -> Host {
        Host {
            home: PathBuf::from("/home/u"),
            config_dir: PathBuf::from("/home/u/.config/dog-walker"),
            path: Vec::new(),
            project: PathBuf::from("/work/app"),
        }
    }

    fn pick(text: &str) -> Pick {
        parse_pick(text).unwrap().unwrap()
    }

    /// A config of the main profile and `secondary`, parsed by the real parser.
    fn valid(secondary: &ProfilePlan) {
        let main = plan_main("main", &pick("claude"));
        let text = format!("{}\n{}", main.toml, secondary.toml);
        Config::parse(&text, Path::new("test.toml")).unwrap_or_else(|e| panic!("{e}\n{text}"));
    }

    #[test]
    fn a_pick_is_a_harness_with_a_model_or_no() {
        assert_eq!(parse_pick("no").unwrap(), None);
        assert_eq!(parse_pick(" None ").unwrap(), None);
        let claude = pick("claude");
        assert_eq!(claude.model, default_model(Harness::Claude));
        assert_eq!(
            claude.credential.as_deref(),
            Some("CLAUDE_CODE_OAUTH_TOKEN")
        );
        let given = pick("pi:deepseek/deepseek-chat");
        assert_eq!(given.model, "deepseek/deepseek-chat");
        assert_eq!(given.credential.as_deref(), Some("DEEPSEEK_API_KEY"));
        assert!(parse_pick("gpt").is_err());
        assert!(parse_pick("claude:bad\nmodel").is_err());
    }

    #[test]
    fn the_credential_variable_follows_the_provider() {
        assert_eq!(default_credential(Harness::Codex, "m"), None);
        assert_eq!(
            default_credential(Harness::Pi, "openrouter/z-ai/x").as_deref(),
            Some("OPENROUTER_API_KEY")
        );
        assert_eq!(default_credential(Harness::Pi, "no-provider"), None);
        assert!(check_variable("OPENROUTER_API_KEY").is_ok());
        assert!(check_variable("1BAD").is_err());
        assert!(check_variable("a-b").is_err());
        assert!(check_variable("").is_err());
    }

    #[test]
    fn the_install_directory_comes_from_where_the_program_resolves() {
        let dir = |harness, path: &str| install_dir(harness, Path::new(path));
        assert_eq!(
            dir(
                Harness::Claude,
                "/home/u/.local/share/claude/versions/2.1.317"
            ),
            Some(PathBuf::from("/home/u/.local/share/claude"))
        );
        assert_eq!(
            dir(
                Harness::Codex,
                "/home/u/.npm-global/lib/node_modules/@openai/codex/bin/codex.js"
            ),
            Some(PathBuf::from(
                "/home/u/.npm-global/lib/node_modules/@openai/codex"
            ))
        );
        assert_eq!(
            dir(
                Harness::Dsh,
                "/home/u/.npm-global/lib/node_modules/@deepseek-ai/dsh/lib/bin.js"
            ),
            Some(PathBuf::from(
                "/home/u/.npm-global/lib/node_modules/@deepseek-ai/dsh"
            ))
        );
        assert_eq!(
            dir(Harness::Pi, "/home/u/.pi/agent/bin/pi"),
            Some(PathBuf::from("/home/u/.pi/agent/install"))
        );
        // A layout that does not match gives nothing, not a guess.
        assert_eq!(dir(Harness::Claude, "/usr/bin/claude"), None);
    }

    #[test]
    fn every_secondary_profile_is_a_valid_config() {
        for text in [
            "claude",
            "pi:openrouter/z-ai/m",
            "codex",
            "dsh",
            "claude:m with \"quotes\" and \\ slashes",
        ] {
            let plan = plan_secondary("worker", &pick(text), &host(), Some(Path::new("/opt/x")));
            valid(&plan);
        }
    }

    #[test]
    fn a_profile_names_the_variables_and_directories_the_harness_needs() {
        let claude = plan_secondary(
            "reviewer",
            &pick("claude"),
            &host(),
            Some(Path::new("/opt/c")),
        );
        let private = "/home/u/.config/dog-walker/reviewer";
        for want in [
            "\"CLAUDE_CODE_OAUTH_TOKEN\"",
            "\"CLAUDE_CONFIG_DIR\"",
            "\"CLAUDE_CODE_TMPDIR\"",
            "\"TMPDIR\"",
            "\"HOME\"",
            "secrets = [\"CLAUDE_CODE_OAUTH_TOKEN\"]",
            "read_only_dirs = [\"/opt/c\"]",
            &format!("writable_dirs = [\"{private}\"]"),
        ] {
            assert!(claude.toml.contains(want), "{want}\n{}", claude.toml);
        }
        assert!(
            claude
                .dirs
                .contains(&PathBuf::from(format!("{private}/tmp")))
        );
        let dsh = plan_secondary("worker", &pick("dsh"), &host(), None);
        assert!(
            dsh.toml.contains("dsh_profile = \"headless\""),
            "{}",
            dsh.toml
        );
        assert!(!dsh.toml.contains("read_only_dirs"), "{}", dsh.toml);
        assert!(dsh.notes[0].contains("was not found"), "{:?}", dsh.notes);
        let codex = plan_secondary("worker", &pick("codex"), &host(), None);
        assert!(!codex.toml.contains("secrets"), "{}", codex.toml);
    }

    #[test]
    fn the_env_file_holds_no_secret_and_quotes_what_it_sets() {
        let mut odd = host();
        odd.config_dir = PathBuf::from("/home/u/it's here");
        let plan = plan_secondary("worker", &pick("claude"), &odd, None);
        let (path, text) = plan.env_file.unwrap();
        assert_eq!(path, PathBuf::from("/home/u/it's here/worker.env"));
        assert!(
            text.contains("CLAUDE_CONFIG_DIR='/home/u/it'\\''s here/worker'"),
            "{text}"
        );
        // The credential is a comment for the user to fill in, never a value.
        assert!(
            text.lines()
                .any(|line| line.starts_with("# CLAUDE_CODE_OAUTH_TOKEN=")),
            "{text}"
        );
        assert!(
            !text
                .lines()
                .any(|line| line.starts_with("CLAUDE_CODE_OAUTH_TOKEN"))
        );
    }

    #[test]
    fn the_notes_name_what_is_left_to_the_user() {
        let plan = plan_secondary("worker", &pick("codex"), &host(), Some(Path::new("/opt/c")));
        assert!(
            plan.notes.iter().any(|n| n.contains("auth.json")),
            "{:?}",
            plan.notes
        );
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("dog-walker doctor worker")),
            "{:?}",
            plan.notes
        );
    }

    #[test]
    fn an_existing_config_is_read_for_its_names_and_main() {
        let existing = read_existing(
            "[profiles.boss]\nharness='codex'\nmodel='m'\nmain=true\n[profiles.worker]\nharness='pi'\nmodel='m'\n",
        )
        .unwrap();
        assert_eq!(existing.names, ["boss", "worker"]);
        assert_eq!(existing.main_harness, Some(Harness::Codex));
        assert_eq!(read_existing("").unwrap(), Existing::default());
        assert!(read_existing("not = [toml").is_err());
    }

    #[test]
    fn the_instructions_block_is_added_once() {
        let add = addition("").unwrap();
        assert_eq!(add, block());
        assert!(add.contains(LINE));
        let after = format!("# my notes\n{}", addition("# my notes\n").unwrap());
        assert!(
            after.starts_with("# my notes\n\n<!-- dog-walker:begin -->"),
            "{after}"
        );
        assert_eq!(addition(&after), None);
        // No newline at the end of the file: the block still starts on its own line.
        assert!(addition("text").unwrap().starts_with("\n\n<!--"));
    }

    #[test]
    fn only_claude_and_codex_have_a_known_instructions_file() {
        let host = host();
        assert_eq!(
            instructions_file(Harness::Claude, Scope::Global, &host),
            Some(PathBuf::from("/home/u/.claude/CLAUDE.md"))
        );
        assert_eq!(
            instructions_file(Harness::Codex, Scope::Global, &host),
            Some(PathBuf::from("/home/u/.codex/AGENTS.md"))
        );
        assert_eq!(instructions_file(Harness::Pi, Scope::Global, &host), None);
        assert_eq!(instructions_file(Harness::Dsh, Scope::Project, &host), None);
    }

    #[test]
    fn a_provider_that_gives_no_variable_name_gets_a_note_not_a_bad_config() {
        let pick = pick("pi:1provider/m");
        assert_eq!(pick.credential, None);
        let plan = plan_secondary("worker", &pick, &host(), Some(Path::new("/opt/x")));
        valid(&plan);
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("no credential variable")),
            "{:?}",
            plan.notes
        );
    }

    #[test]
    fn a_config_that_gained_a_profile_while_asking_is_not_appended_to() {
        let dir = std::env::temp_dir().join(format!("dog-walker-setup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        let plan = Plan {
            config: config.clone(),
            profiles: vec![plan_secondary(
                "worker",
                &pick("codex"),
                &Host {
                    config_dir: dir.clone(),
                    ..host()
                },
                None,
            )],
            instructions: None,
            installed: Vec::new(),
            notes: Vec::new(),
        };
        // Someone else added `worker` after the plan was made.
        append(
            &config,
            "[profiles.worker]\nharness='pi'\nmodel='m'\n",
            0o644,
        )
        .unwrap();
        let error = write_plan(&plan).unwrap_err();
        assert!(matches!(error, SetupError::Changed { .. }), "{error}");
        assert_eq!(
            fs::read_to_string(&config).unwrap(),
            "[profiles.worker]\nharness='pi'\nmodel='m'\n"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn printed_commands_quote_the_paths_in_them() {
        let mut odd = host();
        odd.config_dir = PathBuf::from("/home/u/my $(touch x) config");
        let plan = plan_secondary("worker", &pick("codex"), &odd, None);
        let joined = plan.notes.join("\n");
        assert!(
            joined.contains("'/home/u/my $(touch x) config/worker.env'"),
            "{joined}"
        );
        assert!(
            joined.contains("'/home/u/my $(touch x) config/worker/auth.json'"),
            "{joined}"
        );
    }

    #[test]
    fn an_env_file_that_appears_open_before_the_write_is_refused() {
        let dir = std::env::temp_dir().join(format!("dog-walker-setup-env-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.env");
        append(&file, "KEEP\n", 0o644).unwrap();
        let error = create_private(&file, "new\n").unwrap_err();
        assert!(matches!(error, SetupError::Exists { .. }), "{error}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "KEEP\n");
        // Closed to others, it stays as it is.
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        create_private(&file, "new\n").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "KEEP\n");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_append_is_an_error_not_a_panic() {
        // /dev/full accepts the open and fails every write with ENOSPC.
        if Path::new("/dev/full").exists() {
            let error = append(Path::new("/dev/full"), "text", 0o644).unwrap_err();
            assert!(matches!(error, SetupError::Io { .. }), "{error}");
        }
    }
}
