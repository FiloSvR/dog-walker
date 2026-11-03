//! Acceptance tests for `doctor` (#47, D17): the checks a task would meet, run without a task
//! or a model call. They need `bwrap`, like the other sandbox tests.

// Why: clippy.toml exempts `#[test]` functions from the unwrap and panic denials, but not the
// helper functions of an integration-test file. A failing helper should fail the test.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    dir: PathBuf,
    config: PathBuf,
}

impl Fixture {
    /// A project, a writable directory and a profile `alpha` whose `command` is `sh`; `extra`
    /// are more lines for the profile.
    fn new(name: &str, extra: &str) -> Fixture {
        let dir =
            std::env::temp_dir().join(format!("dog-walker-doctor-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("project")).unwrap();
        fs::create_dir_all(dir.join("writable")).unwrap();
        let config = dir.join("config.toml");
        fs::write(
            &config,
            format!(
                "[profiles.boss]\nharness = \"claude\"\nmodel = \"m\"\nmain = true\n\n\
                 [profiles.alpha]\nharness = \"pi\"\nmodel = \"local/qwen\"\n\
                 command = [\"sh\"]\nnetwork = false\n{extra}"
            ),
        )
        .unwrap();
        Fixture { dir, config }
    }

    fn writable(&self) -> PathBuf {
        self.dir.join("writable")
    }

    fn doctor(&self, profile: &str, env: &[(&str, &Path)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dog-walker"));
        command
            .arg("--config")
            .arg(&self.config)
            .args(["doctor", profile])
            .current_dir(self.dir.join("project"))
            .env_remove("DOCTOR_TOKEN")
            .env_remove("TMPDIR");
        for (name, value) in env {
            command.env(name, value);
        }
        command.output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn sandbox_available(test: &str) -> bool {
    let probe = Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--unshare-net", "true"])
        .output();
    if matches!(probe, Ok(ref output) if output.status.success()) {
        return true;
    }
    assert!(
        std::env::var_os("DOG_WALKER_REQUIRE_SANDBOX").is_none_or(|value| value.is_empty()),
        "bwrap or user namespaces are unavailable, but DOG_WALKER_REQUIRE_SANDBOX is set"
    );
    let message = format!("SKIPPED {test}: bwrap or user namespaces unavailable\n");
    let _ = std::io::Write::write_all(&mut std::io::stderr(), message.as_bytes());
    false
}

fn text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

// AT29 (R22): a healthy profile passes every check, exits 0 and shows no value.
#[test]
fn at29_a_healthy_profile_passes() {
    if !sandbox_available("at29_a_healthy_profile_passes") {
        return;
    }
    let fixture = Fixture::new(
        "healthy",
        &format!(
            "env = [\"DOCTOR_TOKEN\", \"TMPDIR\"]\nsecrets = [\"DOCTOR_TOKEN\"]\nwritable_dirs = [\"{}\"]\n",
            Path::new(&fixture_dir("healthy"))
                .join("writable")
                .display()
        ),
    );
    let writable = fixture.writable();
    let out = fixture.doctor(
        "alpha",
        &[
            ("DOCTOR_TOKEN", Path::new("s3cret-value")),
            ("TMPDIR", &writable),
        ],
    );
    let shown = text(&out);
    assert!(out.status.success(), "{shown}");
    assert!(!shown.contains("FAIL"), "{shown}");
    assert!(shown.contains("ok    temporary directory"), "{shown}");
    assert!(
        shown.contains("skip  tls: the profile has no network"),
        "{shown}"
    );
    assert!(!shown.contains("s3cret-value"), "{shown}");
}

fn fixture_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("dog-walker-doctor-{}-{name}", std::process::id()))
}

// AT29: every problem is reported with its fix, not only the first, and the exit is 1.
#[test]
fn at29_every_failure_is_reported_with_a_fix() {
    if !sandbox_available("at29_every_failure_is_reported_with_a_fix") {
        return;
    }
    let fixture = Fixture::new(
        "broken",
        &format!(
            "env = [\"DOCTOR_TOKEN\", \"TMPDIR\"]\nwritable_dirs = [\"{}\"]\n",
            fixture_dir("broken").join("writable").display()
        ),
    );
    // TMPDIR names a directory that does not exist; DOCTOR_TOKEN is unset.
    let missing = fixture.dir.join("writable").join("no-such");
    let out = fixture.doctor("alpha", &[("TMPDIR", &missing)]);
    let shown = text(&out);
    assert_eq!(out.status.code(), Some(1), "{shown}");
    assert!(
        shown.contains("FAIL  environment: not set in this shell: DOCTOR_TOKEN"),
        "{shown}"
    );
    assert!(shown.contains("FAIL  temporary directory"), "{shown}");
    assert!(shown.contains("ok    harness program"), "{shown}");
}

// AT29: when the profile has an env file beside the config, the fix for an unset variable names
// it, so nobody has to search for it.
#[test]
fn at29_the_fix_for_an_unset_variable_names_the_env_file() {
    if !sandbox_available("at29_the_fix_for_an_unset_variable_names_the_env_file") {
        return;
    }
    let fixture = Fixture::new("envfile", "env = [\"DOCTOR_TOKEN\"]\n");
    let file = fixture.dir.join("alpha.env");
    fs::write(&file, "# DOCTOR_TOKEN=\n").unwrap();
    let shown = text(&fixture.doctor("alpha", &[]));
    assert!(shown.contains("set -a; . '"), "{shown}");
    assert!(shown.contains("alpha.env"), "{shown}");
}

// AT29: a directory that does not exist is a failure too.
#[test]
fn at29_a_missing_directory_fails() {
    if !sandbox_available("at29_a_missing_directory_fails") {
        return;
    }
    let fixture = Fixture::new("dirs", "read_only_dirs = [\"/no/such/dir\"]\n");
    let out = fixture.doctor("alpha", &[]);
    let shown = text(&out);
    assert_eq!(out.status.code(), Some(1), "{shown}");
    assert!(shown.contains("FAIL  directories"), "{shown}");
    assert!(shown.contains("/no/such/dir"), "{shown}");
}
