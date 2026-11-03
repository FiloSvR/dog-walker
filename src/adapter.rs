//! Adapters: how each harness is started, behind one seam (R4).
//!
//! A profile that sets `command` runs exactly that argv; a profile that does not asks its
//! harness's adapter. Adding a harness touches this module and nothing else: the launcher and
//! the sandbox only ever see an argv and a list of variable names.

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

/// Claude Code, run unattended: `claude -p --model <model> <brief>`.
pub struct ClaudeCode;

impl Adapter for ClaudeCode {
    fn argv(&self, profile: &Profile) -> Vec<OsString> {
        let mut argv: Vec<OsString> = vec!["claude".into(), "-p".into()];
        if !profile.model.is_empty() {
            argv.push("--model".into());
            argv.push(profile.model.clone().into());
        }
        argv
    }
}

/// The adapter for a harness, when one is written.
pub fn of(harness: Harness) -> Option<&'static dyn Adapter> {
    static CLAUDE: ClaudeCode = ClaudeCode;
    match harness {
        Harness::Claude => Some(&CLAUDE),
        // The others are their own issues; until then a profile names `command`.
        Harness::Pi | Harness::Codex | Harness::Dsh => None,
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
        return Err(missing());
    }
    Ok(argv)
}

/// The variables a profile's harness needs by name: its adapter's, and the profile's own.
pub fn env(profile: &Profile) -> Vec<String> {
    let mut names = of(profile.harness)
        .map(|adapter| {
            adapter
                .env()
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
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
        let argv = argv(&profile(Harness::Claude, "claude-sonnet-4-5")).unwrap();
        let argv: Vec<String> = argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            argv,
            ["claude", "-p", "--model", "claude-sonnet-4-5"],
            "{argv:?}"
        );
    }

    #[test]
    fn a_profile_command_wins_over_the_adapter() {
        let mut profile = profile(Harness::Claude, "m");
        profile.command = Some(vec!["/bin/echo".to_owned(), "hi".to_owned()]);
        let argv = argv(&profile).unwrap();
        assert_eq!(argv, ["/bin/echo", "hi"]);
    }

    #[test]
    fn a_harness_without_an_adapter_is_named() {
        let error = argv(&profile(Harness::Pi, "local/qwen")).unwrap_err();
        assert!(error.to_string().contains("`pi`"), "{error}");
        assert!(error.to_string().contains("not written yet"), "{error}");
    }

    #[test]
    fn the_variables_of_the_adapter_and_the_profile_both_count() {
        let mut profile = profile(Harness::Claude, "m");
        profile.env = vec!["HOME".to_owned()];
        assert_eq!(env(&profile), ["HOME"]);
    }
}
