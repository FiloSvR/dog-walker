//! Profiles and the config file that defines them (R1, R2).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::error::ConfigError;

/// The longest timeout a profile may ask for, so a deadline always fits an `Instant`.
const MAX_TIMEOUT: Duration = Duration::from_hours(365 * 24);

/// The harnesses dog-walker can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Codex,
    Pi,
    Dsh,
}

impl Harness {
    pub const ALL: [Harness; 4] = [Harness::Claude, Harness::Codex, Harness::Pi, Harness::Dsh];

    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Pi => "pi",
            Harness::Dsh => "dsh",
        }
    }

    pub fn parse(text: &str) -> Option<Harness> {
        Harness::ALL
            .into_iter()
            .find(|harness| harness.as_str() == text)
    }
}

impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A profile's name: non-empty, and safe to use later in file names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProfileName(String);

impl ProfileName {
    pub fn parse(text: &str) -> Result<Self, String> {
        let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
        if text.is_empty() || !text.chars().all(allowed) || text.starts_with('.') {
            return Err(
                "a profile name uses only letters, digits, `-`, `_` and `.`, and does not start with `.`".to_owned(),
            );
        }
        Ok(Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One validated profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: ProfileName,
    pub harness: Harness,
    pub model: String,
    pub effort: Option<String>,
    /// How long the harness may run; `timeout_minutes` in the file, in minutes.
    pub timeout: Option<Duration>,
    /// Overrides the adapter's default command: an argv list.
    pub command: Option<Vec<String>>,
    /// The dsh profile to boot, when the harness is dsh: dsh's own model lives in it.
    pub dsh_profile: Option<ProfileName>,
    pub main: bool,
    /// Network on (the default) or off inside the sandbox (D3).
    pub network: bool,
    /// `.git` writable, so the harness may commit and add (D6, R20); read-only by default.
    pub git_writable: bool,
    /// Extra directories the harness may read, and the ones it may write (R20).
    pub read_only_dirs: Vec<PathBuf>,
    pub writable_dirs: Vec<PathBuf>,
    /// Names of the environment variables the harness gets, on top of `PATH` (R16, R20).
    pub env: Vec<String>,
    pub secrets: Vec<String>,
}

/// A validated config: at least one profile, exactly one of them main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The main profile first, then the others by name.
    profiles: Vec<Profile>,
    /// How many tasks of a harness and model may run at once, keyed `harness/model`.
    limits: BTreeMap<String, usize>,
    /// `sandbox = false` in the file: run tasks without bwrap. `None` when the file is silent.
    sandbox: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    sandbox: Option<bool>,
    #[serde(default)]
    profiles: BTreeMap<String, RawProfile>,
    #[serde(default)]
    limits: BTreeMap<String, usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    harness: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    timeout_minutes: Option<f64>,
    command: Option<Vec<String>>,
    #[serde(default)]
    main: bool,
    dsh_profile: Option<String>,
    network: Option<bool>,
    git: Option<String>,
    read_only_dirs: Option<Vec<String>>,
    writable_dirs: Option<Vec<String>>,
    env: Option<Vec<String>>,
    secrets: Option<Vec<String>>,
}

/// The largest config file `sandbox_setting` reads.
const MAX_SETTING_FILE: u64 = 1 << 20;

/// The `sandbox` setting of the config at `path`, read alone: the rest of the file is not
/// validated, and a file that is missing, unreadable or not TOML says nothing (`None`), so the
/// global config's profiles cannot stop a project that has its own.
pub fn sandbox_setting(path: &Path) -> Option<bool> {
    // A config is a small regular file: a FIFO, a device or a huge file is not read, so that
    // it cannot block a project that has its own config.
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_SETTING_FILE {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    table.get("sandbox").and_then(toml::Value::as_bool)
}

impl Config {
    /// Reads and validates the config file at `path`.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        Config::parse(&text, path)
    }

    /// Validates config `text`; `origin` is only used in error messages.
    pub fn parse(text: &str, origin: &Path) -> Result<Config, ConfigError> {
        let raw: RawConfig = toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: origin.to_owned(),
            source,
        })?;
        if raw.profiles.is_empty() {
            return Err(ConfigError::NoProfiles {
                path: origin.to_owned(),
            });
        }
        let mut profiles = Vec::with_capacity(raw.profiles.len());
        for (name, profile) in raw.profiles {
            profiles.push(validate(&name, profile)?);
        }
        let mains: Vec<&Profile> = profiles.iter().filter(|profile| profile.main).collect();
        match mains.len() {
            1 => {}
            0 => return Err(ConfigError::NoMain),
            _ => {
                let names: Vec<String> = mains
                    .iter()
                    .map(|profile| format!("`{}`", profile.name))
                    .collect();
                return Err(ConfigError::ManyMain {
                    names: names.join(", "),
                });
            }
        }
        // `false` sorts before `true`, so negate to put the main profile first;
        // the sort is stable, so the others stay in name order.
        profiles.sort_by_key(|profile| !profile.main);
        for (key, limit) in &raw.limits {
            let Some((harness, model)) = key.split_once('/') else {
                return Err(ConfigError::LimitKey { key: key.clone() });
            };
            if harness.is_empty() || model.is_empty() || Harness::parse(harness).is_none() {
                return Err(ConfigError::LimitKey { key: key.clone() });
            }
            if *limit == 0 {
                return Err(ConfigError::LimitZero { key: key.clone() });
            }
        }
        Ok(Config {
            profiles,
            limits: raw.limits,
            sandbox: raw.sandbox,
        })
    }

    /// Whether the file turns the sandbox on or off; `None` when it does not say.
    pub fn sandbox(&self) -> Option<bool> {
        self.sandbox
    }

    /// The profiles, the main one first.
    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    /// How many tasks of this profile's harness and model may run at once, when a limit is
    /// configured: absent means as many as the user starts.
    pub fn limit(&self, profile: &Profile) -> Option<usize> {
        let key = format!("{}/{}", profile.harness.as_str(), profile.model);
        self.limits.get(&key).copied()
    }
}

fn validate(name: &str, raw: RawProfile) -> Result<Profile, ConfigError> {
    let fail = |reason: String| ConfigError::Profile {
        profile: name.to_owned(),
        reason,
    };
    // Parsed before any field is moved out of `raw`.
    let sandbox = sandbox_fields(&raw).map_err(fail)?;

    let profile_name = ProfileName::parse(name).map_err(fail)?;
    let harness_text = raw
        .harness
        .ok_or_else(|| fail("missing required field `harness`".to_owned()))?;
    let harness = Harness::parse(&harness_text).ok_or_else(|| {
        fail(format!(
            "unknown harness `{harness_text}` (expected claude, codex, pi or dsh)"
        ))
    })?;
    let model = raw
        .model
        .ok_or_else(|| fail("missing required field `model`".to_owned()))?;
    check_text("model", &model).map_err(fail)?;
    let dsh_profile = match raw.dsh_profile {
        None => None,
        Some(name) => {
            check_text("dsh_profile", &name).map_err(fail)?;
            if name.starts_with('-') {
                return Err(fail(
                    "`dsh_profile` cannot start with `-`: dsh would read it as an option"
                        .to_owned(),
                ));
            }
            let name = ProfileName::parse(&name).map_err(|reason| {
                fail(format!(
                    "`dsh_profile` must be a dsh profile name: {reason}"
                ))
            })?;
            if name.as_str() == "node_modules" {
                return Err(fail(
                    "`dsh_profile` cannot be `node_modules`: dsh keeps its packages there"
                        .to_owned(),
                ));
            }
            if harness != Harness::Dsh {
                return Err(fail(
                    "`dsh_profile` is only for `harness = \"dsh\"`: it names the dsh profile to boot"
                        .to_owned(),
                ));
            }
            Some(name)
        }
    };
    if let Some(effort) = &raw.effort {
        check_text("effort", effort).map_err(fail)?;
        if effort.trim() == "-" {
            return Err(fail(
                "`effort` cannot be `-`: `profiles` prints that for no effort".to_owned(),
            ));
        }
    }
    let timeout = match raw.timeout_minutes {
        None => None,
        Some(minutes) if !minutes.is_finite() || minutes <= 0.0 => {
            return Err(fail("`timeout_minutes` must be greater than 0".to_owned()));
        }
        Some(minutes) => {
            let seconds = minutes * 60.0;
            let Ok(timeout) = Duration::try_from_secs_f64(seconds) else {
                return Err(fail("`timeout_minutes` is too large".to_owned()));
            };
            if timeout.is_zero() {
                return Err(fail("`timeout_minutes` is too small".to_owned()));
            }
            // A deadline is added to an `Instant`, which has a smaller range than `Duration`;
            // a year is the most a task may ask for, and it keeps that addition total.
            if timeout > MAX_TIMEOUT {
                return Err(fail(
                    "`timeout_minutes` must be at most 525600 (a year)".to_owned(),
                ));
            }
            Some(timeout)
        }
    };
    if raw
        .command
        .as_ref()
        .is_some_and(|argv| argv.first().is_none_or(String::is_empty))
    {
        return Err(fail("`command` must start with a program name".to_owned()));
    }
    Ok(Profile {
        name: profile_name,
        harness,
        model,
        effort: raw.effort,
        timeout,
        command: raw.command,
        dsh_profile,
        main: raw.main,
        network: sandbox.network,
        git_writable: sandbox.git_writable,
        read_only_dirs: sandbox.read_only_dirs,
        writable_dirs: sandbox.writable_dirs,
        env: sandbox.env,
        secrets: sandbox.secrets,
    })
}

/// Characters that make a row of `profiles` unsafe to split or to display: control
/// characters, the Unicode line and paragraph separators, and the bidirectional controls
/// that can reorder how a terminal shows the row. The sandbox refuses them in a directory
/// name too, since the handover lists those.
pub fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}'
                | '\u{2029}'
                | '\u{061C}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        )
}

/// The policy a profile asks for, checked before it becomes a `Profile`.
struct SandboxFields {
    network: bool,
    git_writable: bool,
    read_only_dirs: Vec<PathBuf>,
    writable_dirs: Vec<PathBuf>,
    env: Vec<String>,
    secrets: Vec<String>,
}

fn sandbox_fields(raw: &RawProfile) -> Result<SandboxFields, String> {
    let git_writable = match raw.git.as_deref() {
        None | Some("read-only") => false,
        Some("writable") => true,
        Some(other) => {
            return Err(format!(
                "`git` is `{other}`: expected `read-only` or `writable`"
            ));
        }
    };
    let dirs = |field: &str, given: &Option<Vec<String>>| -> Result<Vec<PathBuf>, String> {
        let mut paths = Vec::new();
        for dir in given.iter().flatten() {
            // A directory name reaches the handover: it must not carry a newline that would
            // add bullets of its own.
            check_text(field, dir)?;
            let path = PathBuf::from(dir);
            if !path.is_absolute() {
                return Err(format!("`{field}` entry `{dir}` is not an absolute path"));
            }
            paths.push(path);
        }
        Ok(paths)
    };
    let read_only_dirs = dirs("read_only_dirs", &raw.read_only_dirs)?;
    let writable_dirs = dirs("writable_dirs", &raw.writable_dirs)?;
    let env = raw.env.clone().unwrap_or_default();
    for name in &env {
        if !is_env_name(name) {
            return Err(format!("`env` entry `{name}` is not a variable name"));
        }
        if name.starts_with("DOG_WALKER_") {
            return Err(format!(
                "`env` entry `{name}` would override a variable dog-walker sets"
            ));
        }
    }
    let secrets = raw.secrets.clone().unwrap_or_default();
    for name in &secrets {
        if !is_env_name(name) {
            return Err(format!("`secrets` entry `{name}` is not a variable name"));
        }
        // A secret the harness does not get is not a secret this run can leak.
        if !env.contains(name) {
            return Err(format!(
                "`secrets` entry `{name}` is not in `env`: list it there too"
            ));
        }
    }
    Ok(SandboxFields {
        network: raw.network.unwrap_or(true),
        git_writable,
        read_only_dirs,
        writable_dirs,
        env,
        secrets,
    })
}

/// An environment variable name: letters, digits and `_`, not starting with a digit.
fn is_env_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
}

/// A text field must hold something printable, so a row of `profiles` can always be split.
fn check_text(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("`{field}` must not be empty"));
    }
    if value.chars().any(is_unsafe_char) {
        return Err(format!(
            "`{field}` must not contain control, line-separator or bidirectional-control characters"
        ));
    }
    Ok(())
}

/// The project's own policy, `.dog-walker/config.toml`, which may only tighten the profile.
pub fn project_config(project: &Path) -> Result<Option<ProjectConfig>, ConfigError> {
    let dir = project.join(".dog-walker");
    let path = dir.join("config.toml");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(ConfigError::ReadProjectConfig { path, source }),
    };
    // A config that points out of the project could read anything the runner can (or be a
    // FIFO that hangs the run): a regular file, opened through its own directory without
    // following a link, without blocking, and read through the descriptor.
    let unsafe_path = ConfigError::UnsafeProjectConfig { path: path.clone() };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(unsafe_path);
    }
    let flags =
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC;
    let read_error = |source: rustix::io::Errno| ConfigError::ReadProjectConfig {
        path: path.clone(),
        source: io::Error::from(source),
    };
    let dir_fd = rustix::fs::open(
        &dir,
        flags | rustix::fs::OFlags::DIRECTORY,
        rustix::fs::Mode::empty(),
    )
    .map_err(read_error)?;
    let canonical_dir =
        fs::canonicalize(&dir).map_err(|source| ConfigError::ReadProjectConfig {
            path: path.clone(),
            source,
        })?;
    if !canonical_dir.starts_with(project) {
        return Err(unsafe_path);
    }
    let fd = rustix::fs::openat(
        &dir_fd,
        "config.toml",
        flags | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
    )
    .map_err(read_error)?;
    let file = fs::File::from(fd);
    let opened = file
        .metadata()
        .map_err(|source| ConfigError::ReadProjectConfig {
            path: path.clone(),
            source,
        })?;
    if !opened.is_file() {
        return Err(unsafe_path);
    }
    let mut text = String::new();
    file.take(MAX_PROJECT_CONFIG + 1)
        .read_to_string(&mut text)
        .map_err(|source| ConfigError::ReadProjectConfig {
            path: path.clone(),
            source,
        })?;
    if text.len() as u64 > MAX_PROJECT_CONFIG {
        return Err(ConfigError::ProjectConfigTooLarge { path });
    }
    let request = toml::from_str(&text).map_err(|source| ConfigError::ProjectConfig {
        path: path.clone(),
        source,
    })?;
    Ok(Some(ProjectConfig { path, request }))
}

/// A project's policy file is small; a file larger than this is not one.
const MAX_PROJECT_CONFIG: u64 = 64 * 1024;

/// A project's policy file: where it is, and what it asks for.
pub struct ProjectConfig {
    path: PathBuf,
    request: crate::sandbox::Request,
}

impl ProjectConfig {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn request(&self) -> &crate::sandbox::Request {
        &self.request
    }
}

/// Where the config file is when `--config` is not given: `$XDG_CONFIG_HOME` if it is an
/// absolute path, otherwise `$HOME/.config`.
/// The project a directory belongs to: the root of its repository, and for a linked git worktree
/// the main checkout, so that every worktree of a repository (and every subdirectory) uses the
/// one project config (D20). A directory that is in no repository is its own project.
///
/// A worktree's `.git` file is the repository's own to name, so it is believed only when the
/// main checkout agrees: the worktree must be listed there, and point back at this very file. A
/// repository cannot name another project's checkout and borrow that project's profiles.
pub fn project_root(dir: &Path) -> PathBuf {
    for ancestor in dir.ancestors() {
        let git = ancestor.join(".git");
        match fs::symlink_metadata(&git) {
            Ok(meta) if meta.is_dir() => return ancestor.to_path_buf(),
            Ok(meta) if meta.is_file() => {
                return main_checkout(ancestor, &git).unwrap_or_else(|| ancestor.to_path_buf());
            }
            _ => {}
        }
    }
    dir.to_path_buf()
}

/// The main checkout that lists `worktree` (whose `.git` is the file `git_file`), if it does.
fn main_checkout(worktree: &Path, git_file: &Path) -> Option<PathBuf> {
    common_git_dir(worktree, git_file)?
        .parent()
        .map(Path::to_path_buf)
}

/// The repository's own `.git` directory that `worktree` (a linked worktree, whose `.git` is the
/// file `git_file`) belongs to, believed only on the terms of `project_root`. A sandbox binds it
/// read-only so that `git` works inside a worktree.
pub fn common_git_dir(worktree: &Path, git_file: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(git_file).ok()?;
    let named = PathBuf::from(text.lines().next()?.strip_prefix("gitdir:")?.trim());
    let gitdir = fs::canonicalize(if named.is_absolute() {
        named
    } else {
        worktree.join(named)
    })
    .ok()?;
    let common =
        fs::canonicalize(gitdir.join(fs::read_to_string(gitdir.join("commondir")).ok()?.trim()))
            .ok()?;
    let listed = gitdir.parent()? == common.join("worktrees");
    let back = fs::canonicalize(fs::read_to_string(gitdir.join("gitdir")).ok()?.trim()).ok()?;
    if !listed || back != fs::canonicalize(git_file).ok()? || common.file_name()? != ".git" {
        return None;
    }
    Some(common)
}

/// The user-owned config of one project: `projects/<name>-<hash>/config.toml` beside the global
/// config, keyed by the project's absolute path. It lives outside the repository on purpose, so
/// that a repository cannot define the profiles that decide what a task may touch (R3).
pub fn project_config_path(global: &Path, project: &Path) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    // FNV-1a: small, and the same on every Rust version, which a stored path needs.
    let hash = project
        .as_os_str()
        .as_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
        });
    let name: String = project
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let name = if name.is_empty() {
        "project".to_owned()
    } else {
        name
    };
    Some(
        global
            .parent()?
            .join("projects")
            .join(format!("{name}-{hash:016x}"))
            .join("config.toml"),
    )
}

pub fn default_path(xdg_config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = match xdg_config_home.map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        _ => PathBuf::from(home.filter(|home| !home.is_empty())?).join(".config"),
    };
    Some(base.join("dog-walker").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_config_is_keyed_by_the_project_and_sits_beside_the_global_one() {
        let global = Path::new("/h/.config/dog-walker/config.toml");
        let a = project_config_path(global, Path::new("/work/app")).unwrap();
        let b = project_config_path(global, Path::new("/other/app")).unwrap();
        assert_ne!(a, b);
        assert_eq!(
            a,
            project_config_path(global, Path::new("/work/app")).unwrap()
        );
        assert!(a.starts_with("/h/.config/dog-walker/projects"));
        assert!(
            a.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("app-")
        );
        assert!(a.ends_with("config.toml"));
        let odd = project_config_path(global, Path::new("/w/my app\n")).unwrap();
        assert!(!odd.to_string_lossy().contains(' '));
        assert!(!odd.to_string_lossy().contains('\n'));
    }

    fn parse(text: &str) -> Result<Config, ConfigError> {
        Config::parse(text, Path::new("test.toml"))
    }

    #[test]
    fn main_profile_comes_first_then_names_in_order() {
        let config = parse(
            "[profiles.b]\nharness='pi'\nmodel='m'\n[profiles.a]\nharness='pi'\nmodel='m'\n\
             [profiles.z]\nharness='claude'\nmodel='m'\nmain=true\n",
        )
        .unwrap();
        let names: Vec<&str> = config.profiles().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["z", "a", "b"]);
    }

    #[test]
    fn optional_fields_are_kept() {
        let config = parse(
            "[profiles.a]\nharness='dsh'\nmodel='m'\neffort='high'\ntimeout_minutes=5\n\
             command=['/bin/x','-y']\nmain=true\n",
        )
        .unwrap();
        let profile = &config.profiles()[0];
        assert_eq!(profile.harness, Harness::Dsh);
        assert_eq!(profile.effort.as_deref(), Some("high"));
        assert_eq!(profile.timeout, Some(Duration::from_secs(300)));
        assert_eq!(
            profile.command.as_deref(),
            Some(&["/bin/x".to_owned(), "-y".to_owned()][..])
        );
    }

    #[test]
    fn bad_timeouts_and_empty_commands_are_rejected() {
        for timeout in ["0", "-1", "nan", "inf", "1.6e17", "1e-300", "525601"] {
            assert!(
                parse(&format!(
                    "[profiles.a]\nharness='pi'\nmodel='m'\nmain=true\ntimeout_minutes={timeout}\n"
                ))
                .is_err(),
                "{timeout}"
            );
        }
        assert!(parse("[profiles.a]\nharness='pi'\nmodel='m'\nmain=true\ncommand=[]\n").is_err());
    }

    #[test]
    fn the_timeout_is_bounded_and_named() {
        let too_large =
            parse("[profiles.a]\nharness='pi'\nmodel='m'\nmain=true\ntimeout_minutes=1e300\n")
                .unwrap_err();
        assert!(
            too_large.to_string().contains("timeout_minutes"),
            "{too_large}"
        );
        let over_a_year =
            parse("[profiles.a]\nharness='pi'\nmodel='m'\nmain=true\ntimeout_minutes=525601\n")
                .unwrap_err();
        assert!(over_a_year.to_string().contains("at most"), "{over_a_year}");
        let a_year =
            parse("[profiles.a]\nharness='pi'\nmodel='m'\nmain=true\ntimeout_minutes=525600\n")
                .unwrap();
        assert_eq!(
            a_year.profiles()[0].timeout,
            Some(Duration::from_hours(365 * 24))
        );
    }

    #[test]
    fn a_timeout_may_have_a_fraction_of_a_minute() {
        let config =
            parse("[profiles.a]\nharness='pi'\nmodel='m'\nmain=true\ntimeout_minutes=0.5\n")
                .unwrap();
        assert_eq!(config.profiles()[0].timeout, Some(Duration::from_secs(30)));
    }

    #[test]
    fn the_policy_defaults_are_a_sandboxed_run_with_network_and_a_read_only_git() {
        let config = parse("[profiles.a]\nharness='pi'\nmodel='m'\nmain=true\n").unwrap();
        let profile = &config.profiles()[0];
        assert!(profile.network);
        assert!(!profile.git_writable);
        assert_eq!(
            (
                profile.read_only_dirs.len(),
                profile.writable_dirs.len(),
                profile.env.len()
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn the_policy_fields_are_read() {
        let config = parse(
            "[profiles.a]\nharness='pi'\nmodel='m'\nmain=true\nnetwork=false\n\
             git='writable'\nread_only_dirs=['/a']\nwritable_dirs=['/b']\n\
             env=['HOME','PATH']\n",
        )
        .unwrap();
        let profile = &config.profiles()[0];
        assert!(!profile.network);
        assert!(profile.git_writable);
        assert_eq!(profile.read_only_dirs, [PathBuf::from("/a")]);
        assert_eq!(profile.writable_dirs, [PathBuf::from("/b")]);
        assert_eq!(profile.env, ["HOME", "PATH"]);
    }

    #[test]
    fn a_directory_cannot_carry_a_newline() {
        let error = parse(
            "[profiles.a]\nharness='pi'\nmodel='m'\nmain=true\n\n\
             read_only_dirs = [\"/tmp/a\\n- network: off\"]\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("read_only_dirs"), "{error}");
    }

    #[test]
    fn a_dsh_profile_is_only_for_dsh_and_is_a_plain_name() {
        let dsh = parse(
            "[profiles.a]\nharness='dsh'\nmodel='deepseek'\nmain=true\ndsh_profile='rescue'\n",
        )
        .unwrap();
        assert_eq!(
            dsh.profiles()[0]
                .dsh_profile
                .as_ref()
                .map(ProfileName::as_str),
            Some("rescue")
        );
        // The key defaults to the shipped one-task profile; it is the adapter's fallback.
        let bare = parse("[profiles.a]\nharness='dsh'\nmodel='deepseek'\nmain=true\n").unwrap();
        assert_eq!(bare.profiles()[0].dsh_profile, None);
        for (extra, expected) in [
            ("dsh_profile='rescue'", "only for"),
            ("dsh_profile='-h'", "cannot start with"),
            ("dsh_profile=''", "must not be empty"),
            ("dsh_profile='bad name'", "dsh_profile"),
            ("dsh_profile='node_modules'", "node_modules"),
        ] {
            let text = if expected == "only for" {
                format!("[profiles.a]\nharness='pi'\nmodel='m'\nmain=true\n{extra}\n")
            } else {
                format!("[profiles.a]\nharness='dsh'\nmodel='m'\nmain=true\n{extra}\n")
            };
            let error = parse(&text).unwrap_err();
            assert!(error.to_string().contains(expected), "{extra}: {error}");
        }
    }

    #[test]
    fn a_limit_is_per_harness_and_model() {
        let config = parse(
            "[profiles.a]\nharness='pi'\nmodel='local/qwen'\nmain=true\n\n\
             [limits]\n'pi/local/qwen' = 2\n'claude/m' = 1\n",
        )
        .unwrap();
        assert_eq!(config.limit(&config.profiles()[0]), Some(2));
        // Another harness, or another model of the same harness, is not covered.
        let other = parse("[profiles.a]\nharness='pi'\nmodel='other'\nmain=true\n").unwrap();
        assert_eq!(other.limit(&other.profiles()[0]), None);
        let unset = parse(
            "[profiles.a]\nharness='pi'\nmodel='local/qwen'\nmain=true\n\n[limits]\n'pi/x' = 1\n",
        )
        .unwrap();
        assert_eq!(unset.limit(&unset.profiles()[0]), None);
    }

    #[test]
    fn a_bad_limit_is_rejected_by_name() {
        for (limits, expected) in [
            ("'pi' = 1", "limits"),
            ("'pi/' = 1", "limits"),
            ("'bogus/x' = 1", "limits"),
            ("'pi/m' = 0", "0"),
        ] {
            let error = parse(&format!(
                "[profiles.a]\nharness='pi'\nmodel='m'\nmain=true\n\n[limits]\n{limits}\n"
            ))
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{limits}: {error}");
        }
    }

    #[test]
    fn a_bad_policy_is_rejected_by_name() {
        for (extra, field) in [
            ("git='sometimes'", "git"),
            ("read_only_dirs=['relative']", "read_only_dirs"),
            ("writable_dirs=['relative']", "writable_dirs"),
            ("env=['1BAD']", "env"),
            ("env=['X']\nsecrets=['Y']", "secrets"),
            ("env=['X']\nsecrets=['1BAD']", "secrets"),
            ("env=['DOG_WALKER_MODEL']", "env"),
        ] {
            let error = parse(&format!(
                "[profiles.a]\nharness='pi'\nmodel='m'\nmain=true\n{extra}\n"
            ))
            .unwrap_err();
            assert!(error.to_string().contains(field), "{extra}: {error}");
        }
    }

    #[test]
    fn profile_names_are_restricted() {
        assert!(ProfileName::parse("pi-local_2.1").is_ok());
        for bad in ["", ".hidden", "a b", "a/b", "é"] {
            assert!(ProfileName::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn no_profiles_is_an_error() {
        assert!(matches!(parse(""), Err(ConfigError::NoProfiles { .. })));
    }

    #[test]
    fn default_path_prefers_an_absolute_xdg_dir_then_home() {
        let some = |s: &str| Some(OsString::from(s));
        assert_eq!(
            default_path(some("/x"), some("/h")),
            Some(PathBuf::from("/x/dog-walker/config.toml"))
        );
        assert_eq!(
            default_path(some("relative"), some("/h")),
            Some(PathBuf::from("/h/.config/dog-walker/config.toml"))
        );
        assert_eq!(
            default_path(None, some("/h")),
            Some(PathBuf::from("/h/.config/dog-walker/config.toml"))
        );
        assert_eq!(default_path(None, None), None);
        assert_eq!(default_path(None, some("")), None);
    }
}
