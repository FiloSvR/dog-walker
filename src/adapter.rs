//! Adapters: how each harness is started, behind one seam (R4).
//!
//! A profile that sets `command` runs exactly that argv; a profile that does not asks its
//! harness's adapter. An adapter for a harness the core knows touches this module and nothing
//! else: the launcher and the sandbox only ever see an argv and a list of variable names. (A
//! harness that boots a profile of its own, like dsh, also adds a validated profile key.)

use std::ffi::OsString;

use crate::config::{Harness, Profile};
use crate::error::AdapterError;

/// What an adapter provides: the argv that runs its harness, and the variables it needs.
pub trait Adapter {
    /// The argv that runs the harness; the launcher appends the brief as the last argument.
    fn argv(&self, profile: &Profile) -> Vec<OsString>;

    /// The variables the harness needs by name, so the sandbox can pass them through.
    fn env(&self) -> &'static [&'static str] {
        &[]
    }

    /// Variables the harness needs with a fixed value, which dog-walker sets itself: its
    /// documented mode for running inside someone else's sandbox, where the argv has no flag
    /// for it.
    fn fixed_env(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }
}

/// Claude Code, run unattended: `claude -p --dangerously-skip-permissions [--model <model>]
/// [--effort <effort>] -- <brief>`.
///
/// `-p` alone starts in Manual mode, which denies the edits and commands a task needs; the
/// sandbox is the boundary here (D2), so the harness is told to skip its own permission checks.
pub struct ClaudeCode;

impl Adapter for ClaudeCode {
    fn argv(&self, profile: &Profile) -> Vec<OsString> {
        let mut argv: Vec<OsString> = vec![
            "claude".into(),
            "-p".into(),
            "--dangerously-skip-permissions".into(),
        ];
        if !profile.model.is_empty() {
            argv.push("--model".into());
            argv.push(profile.model.clone().into());
        }
        if let Some(effort) = &profile.effort {
            argv.push("--effort".into());
            argv.push(effort.clone().into());
        }
        // The brief is positional: `--` keeps a brief that starts with `-` a brief.
        argv.push("--".into());
        argv
    }

    fn env(&self) -> &'static [&'static str] {
        // Nothing is required: Claude Code signs in with an API key or a subscription token
        // (`CLAUDE_CODE_OAUTH_TOKEN`), and its config directory is the profile's to list, like
        // its credential.
        &[]
    }
}

/// Pi, run unattended: `pi -p --no-session --model <model> [--thinking <effort>] -- <brief>`.
///
/// `--no-session` is what makes it work in the sandbox: the agent directory is read-only
/// there, and Pi would otherwise create a session inside it.
pub struct Pi;

impl Adapter for Pi {
    fn argv(&self, profile: &Profile) -> Vec<OsString> {
        let mut argv: Vec<OsString> = vec!["pi".into(), "-p".into(), "--no-session".into()];
        if !profile.model.is_empty() {
            argv.push("--model".into());
            argv.push(profile.model.clone().into());
        }
        if let Some(effort) = &profile.effort {
            // Pi calls the thinking level what the profile calls the effort.
            argv.push("--thinking".into());
            argv.push(effort.clone().into());
        }
        argv.push("--".into());
        argv
    }

    fn env(&self) -> &'static [&'static str] {
        // Pi finds its agent directory through the home directory; the directory itself (and
        // the provider's key) is the profile's to list.
        &["HOME"]
    }
}

/// Codex, run unattended: `codex exec --dangerously-bypass-approvals-and-sandbox
/// --skip-git-repo-check --ephemeral [-m <model>] [-c model_reasoning_effort="<effort>"] -- <brief>`.
///
/// The bypass is the documented mode for an externally sandboxed environment, which is what
/// bwrap is here (D2), and `--skip-git-repo-check` runs it in a project that is not a
/// repository. `--ephemeral` keeps session files out of the config directory, which still has
/// to be writable (Codex writes its state there) and to be found through `HOME` or
/// `CODEX_HOME` — the adapter declares `HOME`, and the profile lists `CODEX_HOME` or a
/// writable directory.
pub struct Codex;

impl Adapter for Codex {
    fn argv(&self, profile: &Profile) -> Vec<OsString> {
        let mut argv: Vec<OsString> = vec![
            "codex".into(),
            "exec".into(),
            "--dangerously-bypass-approvals-and-sandbox".into(),
            "--skip-git-repo-check".into(),
            "--ephemeral".into(),
        ];
        if !profile.model.is_empty() {
            argv.push("-m".into());
            argv.push(profile.model.clone().into());
        }
        if let Some(effort) = &profile.effort {
            // Codex takes the reasoning effort as a configuration override.
            argv.push("-c".into());
            argv.push(format!("model_reasoning_effort=\"{effort}\"").into());
        }
        argv.push("--".into());
        argv
    }

    fn env(&self) -> &'static [&'static str] {
        // Codex finds `$CODEX_HOME` (or `$HOME/.codex`) through the home directory; the
        // directory itself is the profile's to list, and writable.
        &["HOME"]
    }
}

/// dsh, run unattended: `dsh --profile <dsh_profile> -- -- <brief>`.
///
/// dsh boots one of the user's profiles, and `headless` is the shipped one that answers a task
/// and exits; the model and the tools live in that profile, not in an argv flag, which is what
/// `dsh_profile` names. The launcher takes the first `--`, so the app gets a second one.
pub struct Dsh;

impl Adapter for Dsh {
    fn argv(&self, profile: &Profile) -> Vec<OsString> {
        let name = profile
            .dsh_profile
            .as_ref()
            .map_or("headless", |name| name.as_str());
        // dsh's launcher consumes the first `--` itself, so the app needs a second one to see
        // a brief that starts with a dash as a brief.
        vec![
            "dsh".into(),
            "--profile".into(),
            name.into(),
            "--".into(),
            "--".into(),
        ]
    }

    fn env(&self) -> &'static [&'static str] {
        // dsh finds `$DSH_HOME` (or `$HOME/.dsh`) through the home directory.
        &["HOME"]
    }

    fn fixed_env(&self) -> &'static [(&'static str, &'static str)] {
        // dsh's default is its own shell sandbox (landlock, which cannot start inside bwrap)
        // and an approval prompt nobody can answer in a headless run. This is its documented
        // mode for an outer sandbox, and dog-walker's is the boundary (D2), like the flags the
        // Claude Code and Codex adapters pass.
        &[("DSH_PERMISSION_MODE", "danger-full-access")]
    }
}

/// The adapter of a harness: every harness the core knows has one.
pub fn of(harness: Harness) -> &'static dyn Adapter {
    static CLAUDE: ClaudeCode = ClaudeCode;
    static PI: Pi = Pi;
    static CODEX: Codex = Codex;
    static DSH: Dsh = Dsh;
    match harness {
        Harness::Claude => &CLAUDE,
        Harness::Pi => &PI,
        Harness::Codex => &CODEX,
        Harness::Dsh => &DSH,
    }
}

/// The argv a profile runs: its own `command` when it has one, its adapter's otherwise.
pub fn argv(profile: &Profile) -> Result<Vec<OsString>, AdapterError> {
    if let Some(command) = profile
        .command
        .as_ref()
        .filter(|command| !command.is_empty())
    {
        return Ok(command.iter().map(OsString::from).collect());
    }
    let argv = of(profile.harness).argv(profile);
    if argv.is_empty() {
        return Err(AdapterError::Empty {
            harness: profile.harness,
        });
    }
    Ok(argv)
}

/// The argv the launcher runs: the harness's argv with the brief last. One rule, used by the
/// launcher for a profile's `command` and for an adapter's argv alike.
pub fn with_brief(argv: &[OsString], brief: &str) -> Vec<OsString> {
    let mut argv = argv.to_vec();
    argv.push(brief.into());
    argv
}

/// The variables dog-walker sets for the profile's harness: its adapter's fixed values. A profile
/// with its own `command` is not running the adapter, and one that lists the name in `env`
/// chose its own value (the host's), so neither gets the adapter's.
pub fn fixed_env(profile: &Profile) -> Vec<(String, String)> {
    if profile.command.as_ref().is_some_and(|c| !c.is_empty()) {
        return Vec::new();
    }
    of(profile.harness)
        .fixed_env()
        .iter()
        .filter(|(name, _)| !profile.env.iter().any(|own| own == name))
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

/// The variables a profile's harness needs by name: its adapter's, and the profile's own.
/// A profile with its own `command` is not running the adapter, so only its own names count.
pub fn env(profile: &Profile) -> Vec<String> {
    let mut names: Vec<String> = if profile.command.as_ref().is_some_and(|c| !c.is_empty()) {
        Vec::new()
    } else {
        of(profile.harness)
            .env()
            .iter()
            .map(|name| (*name).to_owned())
            .collect()
    };
    for name in &profile.env {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::ProfileName;

    fn profile(harness: Harness, model: &str) -> Profile {
        Profile {
            name: ProfileName::parse("alpha").unwrap(),
            harness,
            model: model.to_owned(),
            effort: None,
            timeout: None,
            command: None,
            dsh_profile: None,
            main: false,
            network: true,
            git_writable: false,
            read_only_dirs: Vec::new(),
            writable_dirs: Vec::new(),
            env: Vec::new(),
            secrets: Vec::new(),
        }
    }

    #[test]
    fn claude_code_runs_wired_for_unattended_use() {
        let raw = argv(&profile(Harness::Claude, "claude-sonnet-4-5")).unwrap();
        let printed: Vec<String> = raw
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            printed,
            [
                "claude",
                "-p",
                "--dangerously-skip-permissions",
                "--model",
                "claude-sonnet-4-5",
                "--"
            ],
            "{printed:?}"
        );
        // The effort is a flag too.
        let mut high = profile(Harness::Claude, "m");
        high.effort = Some("high".to_owned());
        let printed: Vec<String> = argv(&high)
            .unwrap()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            printed.windows(2).any(|w| w == ["--effort", "high"]),
            "{printed:?}"
        );
        // The brief goes after the `--`, so one that starts with a dash is still a brief.
        let full = with_brief(&argv(&high).unwrap(), "- fix it");
        let last = full.last().unwrap().to_string_lossy().into_owned();
        assert_eq!(last, "- fix it");
    }

    #[test]
    fn a_profile_command_wins_over_the_adapter() {
        let mut profile = profile(Harness::Claude, "m");
        profile.command = Some(vec!["/bin/echo".to_owned(), "hi".to_owned()]);
        let argv = argv(&profile).unwrap();
        assert_eq!(argv, ["/bin/echo", "hi"]);
    }

    #[test]
    fn pi_runs_wired_for_unattended_use() {
        let mut pi = profile(Harness::Pi, "deepseek/deepseek-chat");
        pi.effort = Some("high".to_owned());
        let printed: Vec<String> = argv(&pi)
            .unwrap()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            printed,
            [
                "pi",
                "-p",
                "--no-session",
                "--model",
                "deepseek/deepseek-chat",
                "--thinking",
                "high",
                "--"
            ],
            "{printed:?}"
        );
        // Pi is told where its home is, and a dash-leading brief stays a brief.
        assert_eq!(env(&pi), ["HOME"]);
        let full = with_brief(&argv(&pi).unwrap(), "- fix it");
        assert_eq!(
            full.last().map(|arg| arg.to_string_lossy().into_owned()),
            Some("- fix it".to_owned())
        );
    }

    #[test]
    fn codex_runs_wired_for_unattended_use() {
        let mut codex = profile(Harness::Codex, "gpt-5-codex");
        codex.effort = Some("high".to_owned());
        let printed: Vec<String> = argv(&codex)
            .unwrap()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            printed,
            [
                "codex",
                "exec",
                "--dangerously-bypass-approvals-and-sandbox",
                "--skip-git-repo-check",
                "--ephemeral",
                "-m",
                "gpt-5-codex",
                "-c",
                "model_reasoning_effort=\"high\"",
                "--"
            ],
            "{printed:?}"
        );
        assert_eq!(env(&codex), ["HOME"]);
        let full = with_brief(&argv(&codex).unwrap(), "- fix it");
        assert_eq!(
            full.last().map(|arg| arg.to_string_lossy().into_owned()),
            Some("- fix it".to_owned())
        );
    }

    #[test]
    fn dsh_boots_its_profile_and_keeps_the_brief_positional() {
        let mut dsh = profile(Harness::Dsh, "deepseek");
        dsh.dsh_profile = Some(ProfileName::parse("rescue").unwrap());
        let printed: Vec<String> = argv(&dsh)
            .unwrap()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            printed,
            ["dsh", "--profile", "rescue", "--", "--"],
            "{printed:?}"
        );
        assert_eq!(env(&dsh), ["HOME"]);
        let full = with_brief(&argv(&dsh).unwrap(), "- fix it");
        assert_eq!(
            full.last().map(|arg| arg.to_string_lossy().into_owned()),
            Some("- fix it".to_owned())
        );
        // Without the key, the shipped one-task profile is the default.
        let bare = profile(Harness::Dsh, "deepseek");
        assert_eq!(
            argv(&bare).unwrap(),
            ["dsh", "--profile", "headless", "--", "--"]
        );
    }

    #[test]
    fn the_variables_of_the_adapter_and_the_profile_both_count() {
        let mut profile = profile(Harness::Claude, "m");
        profile.env = vec!["CLAUDE_CODE_OAUTH_TOKEN".to_owned()];
        // Claude Code's adapter requires no credential: the profile names its own.
        assert_eq!(env(&profile), ["CLAUDE_CODE_OAUTH_TOKEN"]);
        // The adapter's own names come first, then the profile's; a name in both is one name
        // (Pi's adapter names `HOME`).
        let mut pi = profile.clone();
        pi.harness = Harness::Pi;
        pi.env = vec!["HOME".to_owned(), "OPENROUTER_API_KEY".to_owned()];
        assert_eq!(env(&pi), ["HOME", "OPENROUTER_API_KEY"]);
        // A profile with its own `command` is not running the adapter: only its own names.
        profile.env = vec!["HOME".to_owned()];
        profile.command = Some(vec!["/bin/echo".to_owned()]);
        assert_eq!(env(&profile), ["HOME"]);
    }

    #[test]
    fn dsh_runs_in_its_outer_sandbox_mode_unless_the_profile_says_otherwise() {
        let mut dsh = profile(Harness::Dsh, "deepseek");
        let mode = (
            "DSH_PERMISSION_MODE".to_owned(),
            "danger-full-access".to_owned(),
        );
        assert_eq!(fixed_env(&dsh), [mode]);
        // The profile lists the name: its own (the host's) value is the one used.
        dsh.env = vec!["DSH_PERMISSION_MODE".to_owned()];
        assert_eq!(fixed_env(&dsh), []);
        // A profile with its own command is not running the adapter.
        dsh.env = Vec::new();
        dsh.command = Some(vec!["/bin/echo".to_owned()]);
        assert_eq!(fixed_env(&dsh), []);
        // The other adapters fix nothing.
        for harness in [Harness::Claude, Harness::Pi, Harness::Codex] {
            assert_eq!(fixed_env(&profile(harness, "m")), []);
        }
    }
}
