//! The `review-guard` step of `.github/workflows/review-guard-target.yml`, run against a stub
//! `gh` (#23). The step is inline in the workflow because the workflow runs from `main` and must
//! not check out the pull request, so there is no script file to run: this test extracts the
//! `run:` block and executes it, which is how the fail-closed paths are checked without GitHub.
//!
//! The stub applies the `--jq` filter the guard passes with the real `jq`, so the filter that
//! turns a response into labels is under test too.

// Why: clippy.toml exempts `#[test]` functions from the unwrap and panic denials, but not the
// helper functions of an integration-test file. A failing helper should fail the test.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const WORKFLOW: &str = ".github/workflows/review-guard-target.yml";

/// A `gh` stand-in: `STUB_STATUS` is its exit code and `STUB_BODY` its response, on which the
/// `--jq` filter the guard passed is applied.
const GH_STUB: &str = r#"#!/usr/bin/env bash
set -uo pipefail
if [ "${STUB_STATUS:-0}" -ne 0 ]; then
  echo "gh: the API returned an error" >&2
  exit "$STUB_STATUS"
fi
filter=""
previous=""
for argument in "$@"; do
  if [ "$previous" = "--jq" ]; then
    filter=$argument
  fi
  previous=$argument
done
if [ -z "$filter" ]; then
  printf '%s' "${STUB_BODY:-}"
  exit 0
fi
jq -r "$filter" <<<"${STUB_BODY:-}"
"#;

/// A `grep` that cannot read its input, the case the guard must not mistake for "no label".
const GREP_ERROR_STUB: &str = r#"#!/usr/bin/env bash
echo "grep: the input could not be read" >&2
exit 2
"#;

/// A unique temporary directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("dog-walker-guard-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The workflow's single `run: |` block, dedented.
fn guard_script() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(WORKFLOW);
    let text = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.trim() == "run: |")
        .unwrap_or_else(|| panic!("no `run: |` step in {}", path.display()));
    let indent = lines.get(at).unwrap().len() - lines.get(at).unwrap().trim_start().len();
    let mut body: Vec<&str> = Vec::new();
    for line in lines.iter().skip(at + 1) {
        if line.trim().is_empty() {
            body.push("");
        } else if line.len() - line.trim_start().len() > indent {
            body.push(line);
        } else {
            break;
        }
    }
    let strip = body
        .iter()
        .filter(|line| !line.is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut script = String::new();
    for line in body {
        script.push_str(line.get(strip..).unwrap_or(""));
        script.push('\n');
    }
    script
}

/// Runs the guard's script with `stubs` (program name, body) first on `PATH`, and `env` added.
fn run_guard(name: &str, stubs: &[(&str, &str)], env: &[(&str, &str)]) -> Output {
    let dir = TempDir::new(name);
    let script = dir.0.join("guard.sh");
    fs::write(&script, guard_script()).unwrap();
    let bin = dir.0.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for (program, body) in stubs {
        let path = bin.join(program);
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut command = Command::new("bash");
    command
        .arg(&script)
        .env("PATH", path)
        .env("REPO", "owner/repo")
        .env("PR", "1")
        .env("GH_TOKEN", "stub");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The workflow's step really is the guard, not some other block.
#[test]
fn the_extracted_block_is_the_guard() {
    let script = guard_script();
    assert!(script.contains("gh api"), "{script}");
    assert!(script.contains("needs/review"), "{script}");
    assert!(script.contains("grep -qxF"), "{script}");
}

// While the label is on, the check is red even when other labels are present.
#[test]
fn a_pull_request_with_the_label_is_red() {
    let out = run_guard(
        "label-on",
        &[("gh", GH_STUB)],
        &[(
            "STUB_BODY",
            r#"{"labels":[{"name":"area/ci"},{"name":"needs/review"}]}"#,
        )],
    );
    assert_eq!(out.status.code(), Some(1), "stdout: {}", stdout(&out));
    assert!(
        stdout(&out).contains("needs/review is on"),
        "{}",
        stdout(&out)
    );
}

// Without the label the check is green, including for a pull request with no labels at all.
#[test]
fn a_pull_request_without_the_label_is_green() {
    for (name, body) in [
        ("guard-none", r#"{"labels":[]}"#),
        ("guard-other", r#"{"labels":[{"name":"area/ci"}]}"#),
        (
            "guard-similar",
            r#"{"labels":[{"name":"needs/review-old"}]}"#,
        ),
    ] {
        let out = run_guard(name, &[("gh", GH_STUB)], &[("STUB_BODY", body)]);
        assert_eq!(out.status.code(), Some(0), "{body}: {}", stderr(&out));
        assert!(stdout(&out).contains("No review owed"), "{}", stdout(&out));
    }
}

// An API error is red: a review may be owed and the guard cannot tell.
#[test]
fn an_api_error_is_red() {
    let out = run_guard(
        "api-error",
        &[("gh", GH_STUB)],
        &[("STUB_STATUS", "1"), ("STUB_BODY", "{}")],
    );
    assert_eq!(out.status.code(), Some(1), "stdout: {}", stdout(&out));
    assert!(
        stdout(&out).contains("Reading the labels failed"),
        "{}",
        stdout(&out)
    );
}

// A successful call that returns nothing is red, not "no label": the guard cannot tell the
// difference between an empty label list and a response it did not receive.
#[test]
fn an_empty_response_is_red() {
    let out = run_guard("empty-body", &[("gh", GH_STUB)], &[("STUB_BODY", "")]);
    assert_eq!(out.status.code(), Some(1), "stdout: {}", stdout(&out));
    assert!(
        stdout(&out).contains("returned nothing"),
        "{}",
        stdout(&out)
    );
}

// A response without a label list fails the guard closed, even though it printed something.
#[test]
fn a_response_without_labels_is_red() {
    let out = run_guard(
        "no-labels-field",
        &[("gh", GH_STUB)],
        &[("STUB_BODY", "{}")],
    );
    assert_eq!(out.status.code(), Some(1), "stdout: {}", stdout(&out));
    assert!(
        stdout(&out).contains("Reading the labels failed"),
        "{}",
        stdout(&out)
    );
}

// A `grep` failure is red, not "no label found".
#[test]
fn a_grep_error_is_red() {
    let out = run_guard(
        "grep-error",
        &[("gh", GH_STUB), ("grep", GREP_ERROR_STUB)],
        &[("STUB_BODY", r#"{"labels":[]}"#)],
    );
    assert_eq!(out.status.code(), Some(1), "stdout: {}", stdout(&out));
    assert!(stdout(&out).contains("grep exited 2"), "{}", stdout(&out));
}
