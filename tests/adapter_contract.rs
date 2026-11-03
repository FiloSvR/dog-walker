//! The shared adapter contract: every adapter is checked against a fake of its harness (R4).
//!
//! The contract is what an adapter must satisfy, whatever the harness: the program resolves,
//! the argv names the profile's model, the brief is the last argument, the harness runs
//! non-interactively (its stdin is closed), and its result reaches the handover. A fake
//! adapter written here passes the same helper, which is what "a new adapter with no core
//! change" means.

// Why: clippy.toml exempts `#[test]` functions from the unwrap denial, but not the helper
// functions of an integration-test file. A failing helper should fail the test. `expect` is
// not allowed here: nothing in this file needs it, and the index is on an argv the test built.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use dog_walker::adapter::{self, Adapter};
use dog_walker::config::{Harness, Profile, ProfileName};

/// A fake adapter, to prove the seam is adapter-agnostic: it is not in the core's match at
/// all, and the contract helper still holds it to the same rules.
struct FakeAdapter;

impl Adapter for FakeAdapter {
    fn argv(&self, profile: &Profile) -> Vec<OsString> {
        let mut argv = vec![
            OsString::from("fake-harness"),
            OsString::from("--model"),
            OsString::from(&profile.model),
        ];
        if let Some(effort) = &profile.effort {
            argv.push(OsString::from("--effort"));
            argv.push(OsString::from(effort));
        }
        argv
    }
}

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

/// A temporary directory.
fn dir(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("dog-walker-adapter-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

/// A stub harness in `dir`: it records its argv and whether its stdin was a closed pipe, and
/// writes its result where the runner says.
fn stub(dir: &Path, program: &str) -> PathBuf {
    let path = dir.join(program);
    let script = format!(
        "#!/bin/sh\n\
         printf '%s\\n' \"$@\" > {dir}/argv.txt\n\
         if read -r -t 1 line; then echo open > {dir}/stdin.txt; else echo closed > {dir}/stdin.txt; fi\n\
         printf 'the fake %s wrote this\\n' \"$(basename \"$0\")\" > \"$DOG_WALKER_RESULT_PATH\"\n",
        dir = dir.display()
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// The contract, checked against a stub of the harness: the adapter's argv with the brief
/// last (the launcher's own rule), the model and the effort in it, stdin closed, and a result.
fn contract(adapter: &dyn Adapter, harness: Harness, dir: &Path) {
    let mut profile = profile(harness, "claude-sonnet-4-5");
    profile.effort = Some("high".to_owned());
    let argv = adapter::with_brief(&adapter.argv(&profile), "the brief");
    assert!(!argv.is_empty(), "{harness:?}: an empty argv");
    let program = Path::new(&argv[0])
        .file_name()
        .unwrap_or_else(|| panic!("{harness:?}: the program is not a name"))
        .to_owned();
    let stub = stub(dir, &program.to_string_lossy());

    let result = dir.join("result.md");
    let mut child = Command::new(&stub)
        .args(&argv[1..])
        .current_dir(dir)
        .env("DOG_WALKER_RESULT_PATH", &result)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("{harness:?}: the stub does not start: {error}"));
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success(), "{harness:?}: the stub");

    let printed = fs::read_to_string(dir.join("argv.txt")).unwrap();
    let recorded: Vec<&str> = printed.lines().collect();
    assert!(
        recorded.contains(&"claude-sonnet-4-5"),
        "{harness:?}: no model: {printed}"
    );
    assert!(
        recorded.contains(&"high"),
        "{harness:?}: no effort: {printed}"
    );
    // The brief is exactly the last argument, and one argument.
    assert_eq!(recorded.last().copied(), Some("the brief"), "{printed}");
    assert_eq!(
        fs::read_to_string(dir.join("stdin.txt")).unwrap().trim(),
        "closed",
        "{harness:?}: the harness waits on stdin"
    );
    assert!(result.is_file(), "{harness:?}: no result");
    fs::remove_dir_all(dir).unwrap();
}

// AT28 (R4): the Claude Code adapter starts its harness with the profile's model, with the
// brief last and stdin closed, and the runner records the result in the handover.
#[test]
fn at28_the_claude_code_adapter_meets_the_contract() {
    let project = dir("project");
    let config = project.join("config.toml");
    fs::write(
        &config,
        "[profiles.alpha]\nharness = \"claude\"\nmodel = \"claude-sonnet-4-5\"\n\
         effort = \"high\"\n\n\
         [profiles.main]\nharness = \"pi\"\nmodel = \"m\"\nmain = true\n",
    )
    .unwrap();
    stub(&project, "claude");
    let out = Command::new(env!("CARGO_BIN_EXE_dog-walker"))
        .arg("--config")
        .arg(&config)
        .args(["run", "alpha", "the brief", "--unsandboxed"])
        .current_dir(&project)
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
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let handover = fs::read_to_string(
        fs::read_dir(project.join(".dog-walker").join("tasks"))
            .unwrap()
            .flatten()
            .map(|entry| entry.path().join("handover.md"))
            .next()
            .unwrap(),
    )
    .unwrap();
    assert!(
        handover.contains("the fake claude wrote this"),
        "{handover}"
    );
    let printed = fs::read_to_string(project.join("argv.txt")).unwrap();
    let recorded: Vec<&str> = printed.lines().collect();
    assert!(recorded.contains(&"-p"), "{printed}");
    assert!(
        recorded.contains(&"--dangerously-skip-permissions"),
        "{printed}"
    );
    assert!(
        recorded
            .windows(2)
            .any(|w| w == ["--model", "claude-sonnet-4-5"]),
        "{printed}"
    );
    assert_eq!(recorded.last().copied(), Some("the brief"), "{printed}");
    assert_eq!(
        fs::read_to_string(project.join("stdin.txt"))
            .unwrap()
            .trim(),
        "closed"
    );
    fs::remove_dir_all(&project).unwrap();
}

// AT5 (R4): a new adapter needs no core change — the same contract holds it, and it is not in
// the core's list of harnesses at all.
#[test]
fn at5_a_fake_adapter_meets_the_contract_without_a_core_change() {
    contract(&FakeAdapter, Harness::Pi, &dir("fake"));
}

// AT5 (R4): no adapter and no dependency speaks ACP in v1.
#[test]
fn at5_nothing_speaks_acp() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lock = fs::read_to_string(root.join("Cargo.lock")).unwrap();
    for forbidden in ["agent-client-protocol", "acp-", "/acp"] {
        assert!(!lock.contains(forbidden), "{forbidden} is in Cargo.lock");
    }
    let deny = fs::read_to_string(root.join("deny.toml")).unwrap();
    assert!(!deny.to_lowercase().contains("acp"), "{deny}");
}

// AT28 (R4): a harness whose adapter is not written yet is refused before a task exists.
#[test]
fn at28_a_harness_without_an_adapter_is_refused_before_a_task() {
    let project = dir("no-adapter");
    let config = project.join("config.toml");
    fs::write(
        &config,
        "[profiles.alpha]\nharness = \"pi\"\nmodel = \"local/qwen\"\n\n\
         [profiles.main]\nharness = \"claude\"\nmodel = \"m\"\nmain = true\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_dog-walker"))
        .arg("--config")
        .arg(&config)
        .args(["run", "alpha", "the brief", "--unsandboxed"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("`pi`") && err.contains("not written yet"),
        "{err}"
    );
    assert!(
        !project.join(".dog-walker").exists(),
        "a task folder was made"
    );
    fs::remove_dir_all(&project).unwrap();
}
