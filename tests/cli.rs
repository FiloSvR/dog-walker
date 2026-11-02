//! Acceptance tests for the scaffold (#3): config validation, `profiles`, `instructions`.
//! They run the compiled binary and look only at its exit code and output.

// Why: clippy.toml exempts `#[test]` functions from the unwrap denial, but not the helper
// functions of an integration-test file. A failing helper should fail the test. `expect` is
// not allowed here: nothing in this file needs it.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A unique temporary directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("dog-walker-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dog-walker"))
}

/// Writes `config` to a temp dir and runs `dog-walker --config <file> <args>`.
fn run_with_config(test: &str, config: &str, args: &[&str]) -> Output {
    let dir = TempDir::new(test);
    let file = dir.path().join("config.toml");
    fs::write(&file, config).unwrap();
    binary()
        .arg("--config")
        .arg(&file)
        .args(args)
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

const VALID: &str = r#"
[profiles.alpha]
harness = "pi"
model = "local/qwen"
effort = "medium"

[profiles.zeta]
harness = "claude"
model = "claude-fable-5-1"
effort = "high"
main = true
"#;

// AT2 (R1): a profile missing a required field fails config load and names the field.
#[test]
fn at2_missing_model_names_profile_and_field() {
    let config =
        "[profiles.zeta]\nharness = \"claude\"\nmain = true\n\n[profiles.pi]\nharness = \"pi\"\n";
    let out = run_with_config("at2-model", config, &["profiles"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stdout(&out).is_empty(), "errors must not reach stdout");
    let err = stderr(&out);
    assert!(err.contains("pi"), "names the profile: {err}");
    assert!(err.contains("model"), "names the field: {err}");
}

// AT2 (R1): the same for the harness field.
#[test]
fn at2_missing_harness_names_profile_and_field() {
    let config = "[profiles.zeta]\nmodel = \"m\"\nmain = true\n";
    let out = run_with_config("at2-harness", config, &["profiles"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("zeta") && err.contains("harness"), "{err}");
}

// AT2 (R1): only the four supported harnesses are accepted.
#[test]
fn at2_unknown_harness_is_rejected() {
    let config = "[profiles.zeta]\nharness = \"gemini\"\nmodel = \"m\"\nmain = true\n";
    let out = run_with_config("at2-unknown", config, &["profiles"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("gemini"));
}

// AT2 (R1): a misspelled key is an error, not silently ignored.
#[test]
fn at2_unknown_key_is_rejected() {
    let config =
        "[profiles.zeta]\nharness = \"claude\"\nmodel = \"m\"\nmain = true\nmodle = \"x\"\n";
    let out = run_with_config("at2-key", config, &["profiles"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("modle"));
}

// AT3 (R2): zero main profiles fail config load.
#[test]
fn at3_zero_main_profiles_fail() {
    let config = "[profiles.a]\nharness = \"pi\"\nmodel = \"m\"\n";
    let out = run_with_config("at3-zero", config, &["profiles"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("main"));
}

// AT3 (R2): two main profiles fail config load and name both.
#[test]
fn at3_two_main_profiles_fail() {
    let config = "[profiles.a]\nharness = \"pi\"\nmodel = \"m\"\nmain = true\n\n\
                  [profiles.b]\nharness = \"codex\"\nmodel = \"m\"\nmain = true\n";
    let out = run_with_config("at3-two", config, &["profiles"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("main") && err.contains("`a`") && err.contains("`b`"),
        "{err}"
    );
}

// AT3 (R2): exactly one main loads, and `profiles` lists the main profile first.
#[test]
fn at3_one_main_loads_and_is_listed_first() {
    let out = run_with_config("at3-one", VALID, &["profiles"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(
        lines[0].starts_with("zeta") && lines[0].contains("main"),
        "main first: {text}"
    );
    assert!(lines[1].starts_with("alpha"), "{text}");
    assert!(
        lines[0].contains("claude-fable-5-1") && lines[0].contains("high"),
        "{text}"
    );
}

// R1: a missing config file is an error that names the path.
#[test]
fn missing_config_file_names_the_path() {
    let dir = TempDir::new("missing");
    let file = dir.path().join("nope.toml");
    let out = binary()
        .arg("--config")
        .arg(&file)
        .arg("profiles")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("nope.toml"));
}

// AT27 (R21): `instructions` names `run`, the exit code and the handover location.
#[test]
fn at27_instructions_name_run_exit_code_and_handover() {
    let home = TempDir::new("instructions");
    let out = binary()
        .arg("instructions")
        .env("HOME", home.path())
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("dog-walker run"), "{text}");
    assert!(text.contains("exit code"), "{text}");
    assert!(text.contains(".dog-walker/tasks"), "{text}");
    assert!(text.contains("handover.md"), "{text}");
}

// AT24 (R19): a normal invocation makes no network request of its own: it works in a
// network-less namespace. Skipped, loudly, where bwrap is unavailable.
#[test]
fn at24_works_without_network() {
    let probe = Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--unshare-net", "true"])
        .output();
    if !matches!(probe, Ok(ref o) if o.status.success()) {
        eprintln!("SKIPPED at24_works_without_network: bwrap or user namespaces unavailable");
        return;
    }
    let dir = TempDir::new("at24");
    let file = dir.path().join("config.toml");
    fs::write(&file, VALID).unwrap();
    let out = Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--bind"])
        .arg(dir.path())
        .arg(dir.path())
        .args(["--unshare-net", "--die-with-parent"])
        .arg(env!("CARGO_BIN_EXE_dog-walker"))
        .arg("--config")
        .arg(&file)
        .arg("profiles")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
}

// AT24 (R19): none of the crates `deny.toml` bans is in the dependency tree. The ban list
// lives only in `deny.toml`; this reads it, so the two cannot drift apart.
#[test]
fn at24_no_networking_crates_in_lockfile() {
    let root = env!("CARGO_MANIFEST_DIR");
    let lock = fs::read_to_string(format!("{root}/Cargo.lock")).unwrap();
    let deny: toml::Table =
        toml::from_str(&fs::read_to_string(format!("{root}/deny.toml")).unwrap()).unwrap();
    let banned = deny["bans"]["deny"].as_array().unwrap();
    assert!(banned.len() >= 9, "deny.toml lost its ban list");
    for entry in banned {
        let name = entry["name"].as_str().unwrap();
        assert!(
            !lock.contains(&format!("name = \"{name}\"")),
            "banned crate in Cargo.lock: {name}"
        );
    }
}

// AT2 (R1): a top-level key that is not `profiles` is rejected, not ignored.
#[test]
fn at2_top_level_unknown_key_is_rejected() {
    let out = run_with_config("at2-top", &format!("foo = 1\n{VALID}"), &["profiles"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("foo"));
}

// AT2 (R1): a misspelled `profiles` table is rejected, not read as "no profiles".
#[test]
fn at2_misspelled_profiles_table_is_rejected() {
    let config = "[profile.zeta]\nharness = \"claude\"\nmodel = \"m\"\nmain = true\n";
    let out = run_with_config("at2-table", config, &["profiles"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("profile"));
}

/// A one-profile config whose profile has `extra` lines appended.
fn one_profile(extra: &str) -> String {
    format!("[profiles.zeta]\nharness = \"claude\"\nmain = true\n{extra}\n")
}

// AT2 (R1): blank text fields and an empty command program are rejected, each by name.
#[test]
fn at2_blank_values_are_rejected() {
    for (case, extra, field) in [
        ("blank-model", "model = \"  \"", "model"),
        ("blank-effort", "model = \"m\"\neffort = \"  \"", "effort"),
        (
            "empty-program",
            "model = \"m\"\ncommand = [\"\"]",
            "command",
        ),
    ] {
        let out = run_with_config(case, &one_profile(extra), &["profiles"]);
        assert_eq!(out.status.code(), Some(2), "{case}: {}", stderr(&out));
        let err = stderr(&out);
        assert!(err.contains("zeta") && err.contains(field), "{case}: {err}");
    }
}

// AT2 (R1): control characters in `model` and `effort` are rejected, so a row of
// `profiles` can always be split; `-` is reserved for "no effort".
#[test]
fn at2_unprintable_or_reserved_values_are_rejected() {
    for (case, extra, field) in [
        ("tab-in-model", "model = \"a\\tb\"", "model"),
        ("newline-in-model", "model = \"a\\nb\"", "model"),
        (
            "tab-in-effort",
            "model = \"m\"\neffort = \"x\\ty\"",
            "effort",
        ),
        ("dash-effort", "model = \"m\"\neffort = \"-\"", "effort"),
    ] {
        let out = run_with_config(case, &one_profile(extra), &["profiles"]);
        assert_eq!(out.status.code(), Some(2), "{case}: {}", stderr(&out));
        assert!(stderr(&out).contains(field), "{case}: {}", stderr(&out));
        assert!(stdout(&out).is_empty(), "{case}");
    }
}

// R2: `profiles` prints one row per profile: name, harness, model, effort (`-` if none),
// role, separated by tabs.
#[test]
fn profiles_rows_have_fixed_columns() {
    let config = "[profiles.alpha]\nharness = \"pi\"\nmodel = \"local/qwen\"\n\n\
                  [profiles.zeta]\nharness = \"claude\"\nmodel = \"m\"\neffort = \"high\"\nmain = true\n";
    let out = run_with_config("rows", config, &["profiles"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "zeta\tclaude\tm\thigh\tmain\nalpha\tpi\tlocal/qwen\t-\tsecondary\n"
    );
}

// R6: usage errors exit 2 with the text on stderr and nothing on stdout.
#[test]
fn usage_errors_exit_2_on_stderr() {
    for args in [&["frobnicate"][..], &[][..], &["profiles", "--nope"][..]] {
        let out = binary().args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(stdout(&out).is_empty(), "{args:?}: {}", stdout(&out));
        assert!(!stderr(&out).is_empty(), "{args:?}");
    }
}

// R6: help and version go to stdout, exit 0, nothing on stderr.
#[test]
fn help_and_version_go_to_stdout() {
    for (arg, expect) in [("--help", "instructions"), ("--version", "dog-walker")] {
        let out = binary().arg(arg).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{arg}");
        assert!(stdout(&out).contains(expect), "{arg}: {}", stdout(&out));
        assert!(stderr(&out).is_empty(), "{arg}: {}", stderr(&out));
    }
}

// R1: `--config` is accepted after the subcommand as well as before it.
#[test]
fn config_flag_works_after_the_subcommand() {
    let dir = TempDir::new("flag-after");
    let file = dir.path().join("config.toml");
    fs::write(&file, VALID).unwrap();
    let out = binary()
        .arg("profiles")
        .arg("--config")
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
}
