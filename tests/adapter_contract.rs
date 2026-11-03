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

use dog_walker::adapter::Adapter;
use dog_walker::config::{Harness, Profile, ProfileName};

/// A fake adapter, to prove the seam is adapter-agnostic: it is not in the core's match at
/// all, and the contract helper still holds it to the same rules.
struct FakeAdapter;

impl Adapter for FakeAdapter {
    fn argv(&self, profile: &Profile) -> Vec<OsString> {
        vec![
            OsString::from("fake-harness"),
            OsString::from("--model"),
            OsString::from(&profile.model),
        ]
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
         echo \"$@\" > {dir}/argv.txt\n\
         if read line; then echo open > {dir}/stdin.txt; else echo closed > {dir}/stdin.txt; fi\n\
         printf 'the fake %s wrote this\\n' \"$(basename \"$0\")\" > \"$DOG_WALKER_RESULT_PATH\"\n",
        dir = dir.display()
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// The contract, checked by running `adapter`'s argv against its stub: the model must reach
/// the harness, the harness must not wait on stdin, and it must write a result.
fn contract(adapter: &dyn Adapter, harness: Harness, dir: &Path) {
    let profile = profile(harness, "claude-sonnet-4-5");
    let argv = adapter.argv(&profile);
    assert!(!argv.is_empty(), "{harness:?}: an empty argv");
    let program = Path::new(&argv[0])
        .file_name()
        .unwrap_or_else(|| panic!("{harness:?}: the program is not a name"))
        .to_owned();
    stub(dir, &program.to_string_lossy());

    let result = dir.join("result.md");
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(dir)
        .env("DOG_WALKER_RESULT_PATH", &result)
        .env(
            "PATH",
            format!(
                "{}:{}",
                dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("{harness:?}: the stub does not start: {error}"));
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success(), "{harness:?}: the stub");

    let printed = fs::read_to_string(dir.join("argv.txt")).unwrap();
    assert!(
        printed.contains("claude-sonnet-4-5"),
        "{harness:?}: no model in the argv: {printed}"
    );
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
        "[profiles.alpha]\nharness = \"claude\"\nmodel = \"claude-sonnet-4-5\"\n\n\
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
    assert!(printed.contains("-p"), "{printed}");
    assert!(printed.contains("--model claude-sonnet-4-5"), "{printed}");
    assert!(printed.trim_end().ends_with("the brief"), "{printed}");
    assert_eq!(
        fs::read_to_string(project.join("stdin.txt"))
            .unwrap()
            .trim(),
        "closed"
    );
    fs::remove_dir_all(&project).unwrap();
}

// AT28 (R4): a new adapter needs no core change — the same contract holds it, and it is not in
// the core's list of harnesses at all.
#[test]
fn at28_a_fake_adapter_meets_the_contract_without_a_core_change() {
    contract(&FakeAdapter, Harness::Pi, &dir("fake"));
}

// AT28 (R4): a harness whose adapter is not written yet says so, and names the profile.
#[test]
fn at28_a_harness_without_an_adapter_says_so() {
    let error = dog_walker::adapter::argv(&profile(Harness::Pi, "local/qwen")).unwrap_err();
    assert!(error.to_string().contains("`pi`"), "{error}");
    assert!(error.to_string().contains("not written yet"), "{error}");
}
