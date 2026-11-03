//! AT30 (R23): the handover records the tokens a harness reports.
//!
//! Each case puts a stub of the harness on `PATH`, named like it and printing what that harness
//! prints, and runs the real binary against it.

// Why: clippy.toml exempts `#[test]` functions from the unwrap denial, but not the helpers of an
// integration-test file; a failing helper should fail the test.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use dog_walker::handover::{self, FrontMatter};

fn dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("dog-walker-usage-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

/// A stub named `program` that writes its result and then prints `output` (a shell-quoted
/// string for `printf '%b'`).
fn stub(dir: &Path, program: &str, output: &str) {
    let path = dir.join(program);
    let script = format!(
        "#!/usr/bin/env bash\n\
         printf 'done\\n' > \"$DOG_WALKER_RESULT_PATH\"\n\
         printf '%b' '{output}'\n"
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Runs `profile` of `config` in `project` with the stubs on `PATH`; the handover's front
/// matter and the run's stderr.
fn run(project: &Path, config: &str) -> (FrontMatter, String) {
    let path = project.join("config.toml");
    fs::write(&path, config).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_dog-walker"))
        .arg("--config")
        .arg(&path)
        .args(["run", "alpha", "the brief", "--unsandboxed"])
        .current_dir(project)
        .env(
            "PATH",
            format!(
                "{}:{}",
                project.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");
    let handover = fs::read_to_string(
        fs::read_dir(project.join(".dog-walker").join("tasks"))
            .unwrap()
            .flatten()
            .map(|entry| entry.path().join("handover.md"))
            .next()
            .unwrap(),
    )
    .unwrap();
    let front = handover::front_matter(&handover).unwrap_or_else(|| panic!("{handover}"));
    (front, stderr)
}

fn config(harness: &str, extra: &str) -> String {
    format!(
        "[profiles.alpha]\nharness = \"{harness}\"\nmodel = \"m\"\n{extra}\n\
         [profiles.main]\nharness = \"pi\"\nmodel = \"m\"\nmain = true\n"
    )
}

// AT30 (R23): Codex's `tokens used` total is recorded and named on the run's last log line.
#[test]
fn at30_codex_usage_is_recorded() {
    let project = dir("codex");
    stub(&project, "codex", "codex\\ntokens used\\n40,317\\ndone\\n");
    let (front, stderr) = run(&project, &config("codex", ""));
    assert_eq!(front.tokens, Some(40317));
    assert_eq!(front.turns, None);
    assert!(stderr.contains("done (40317 tokens)"), "{stderr}");
    fs::remove_dir_all(&project).unwrap();
}

// AT30 (R23): Claude Code's result object gives the tokens of every kind, the turns and the cost.
#[test]
fn at30_claude_usage_is_recorded() {
    let project = dir("claude");
    let result = r#"{"type":"result","num_turns":3,"total_cost_usd":0.5,"usage":{"input_tokens":2,"cache_creation_input_tokens":10,"cache_read_input_tokens":100,"output_tokens":4}}"#;
    stub(&project, "claude", &format!("{result}\\n"));
    let (front, stderr) = run(&project, &config("claude", ""));
    assert_eq!(front.tokens, Some(116));
    assert_eq!(front.turns, Some(3));
    assert_eq!(front.cost_usd, Some(0.5));
    assert!(stderr.contains("done (116 tokens, 3 turns)"), "{stderr}");
    fs::remove_dir_all(&project).unwrap();
}

// AT30 (R23): Pi's usage is summed over its turns.
#[test]
fn at30_pi_usage_is_summed_over_turns() {
    let project = dir("pi");
    let turn = |tokens: u64| {
        format!(
            r#"{{"type":"turn_end","message":{{"usage":{{"totalTokens":{tokens},"cost":{{"total":0.25}}}}}}}}"#
        )
    };
    stub(
        &project,
        "pi",
        &format!("{}\\n{}\\n", turn(1000), turn(500)),
    );
    let (front, _) = run(&project, &config("pi", ""));
    assert_eq!(front.tokens, Some(1500));
    assert_eq!(front.turns, Some(2));
    assert_eq!(front.cost_usd, Some(0.5));
    fs::remove_dir_all(&project).unwrap();
}

// AT30 (R23): a harness that reports nothing readable has no usage, never zero.
#[test]
fn at30_dsh_has_no_usage() {
    let project = dir("dsh");
    stub(&project, "dsh", "tokens used\\n99\\n");
    let (front, stderr) = run(&project, &config("dsh", ""));
    assert_eq!(front.tokens, None);
    assert!(!stderr.contains("tokens)"), "{stderr}");
    fs::remove_dir_all(&project).unwrap();
}

// AT30 (R23): a profile with its own `command` is not running the adapter, so what it prints
// says nothing about usage.
#[test]
fn at30_a_profile_command_has_no_usage() {
    let project = dir("command");
    stub(&project, "mine", "tokens used\\n99\\n");
    let command = format!("command = [\"{}/mine\"]\n", project.display());
    let (front, _) = run(&project, &config("codex", &command));
    assert_eq!(front.tokens, None);
    fs::remove_dir_all(&project).unwrap();
}
