//! Adapters: how each harness is started, behind one seam (R4).
//!
//! A profile that sets `command` runs exactly that argv; a profile that does not asks its
//! harness's adapter. An adapter for a harness the core knows touches this module and nothing
//! else: the launcher and the sandbox only ever see an argv and a list of variable names.

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
        // Claude Code reads its key from the environment; its config directory is the
        // profile's to list.
        &["ANTHROPIC_API_KEY"]
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

/// The adapter for a harness, when one is written.
pub fn of(harness: Harness) -> Option<&'static dyn Adapter> {
    static CLAUDE: ClaudeCode = ClaudeCode;
    static PI: Pi = Pi;
    static CODEX: Codex = Codex;
    match harness {
        Harness::Claude => Some(&CLAUDE),
        Harness::Pi => Some(&PI),
        Harness::Codex => Some(&CODEX),
        // dsh is its own issue; until then a profile names `command`.
        Harness::Dsh => None,
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
    let missing = || AdapterError::Missing {
        profile: profile.name.to_string(),
        harness: profile.harness,
    };
    let adapter = of(profile.harness).ok_or_else(missing)?;
    let argv = adapter.argv(profile);
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

/// The variables a profile's harness needs by name: its adapter's, and the profile's own.
/// A profile with its own `command` is not running the adapter, so only its own names count.
pub fn env(profile: &Profile) -> Vec<String> {
    let mut names: Vec<String> = if profile.command.as_ref().is_some_and(|c| !c.is_empty()) {
        Vec::new()
    } else {
        of(profile.harness)
            .map(|adapter| {
                adapter
                    .env()
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect()
            })
            .unwrap_or_default()
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
    fn a_harness_without_an_adapter_is_named() {
        let error = argv(&profile(Harness::Dsh, "deepseek")).unwrap_err();
        assert!(error.to_string().contains("`dsh`"), "{error}");
        assert!(error.to_string().contains("not written yet"), "{error}");
    }

    #[test]
    fn the_variables_of_the_adapter_and_the_profile_both_count() {
        let mut profile = profile(Harness::Claude, "m");
        profile.env = vec!["HOME".to_owned()];
        // The adapter's own names come first, then the profile's; a name in both is one name.
        assert_eq!(env(&profile), ["ANTHROPIC_API_KEY", "HOME"]);
        profile.env = vec!["ANTHROPIC_API_KEY".to_owned(), "HOME".to_owned()];
        assert_eq!(env(&profile), ["ANTHROPIC_API_KEY", "HOME"]);
        // A profile with its own `command` is not running the adapter: only its own names.
        profile.env = vec!["HOME".to_owned()];
        profile.command = Some(vec!["/bin/echo".to_owned()]);
        assert_eq!(env(&profile), ["HOME"]);
    }
}
