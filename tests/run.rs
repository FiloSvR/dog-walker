//! Acceptance tests for `run` (#16): the task folder, the handover, the status lifecycle and
//! the exit contract. They run the compiled binary against a fake harness: a small script the
//! profile's `command` points at, which behaves as its first argument says.

// Why: clippy.toml exempts `#[test]` functions from the unwrap denial, but not the helper
// functions of an integration-test file. A failing helper should fail the test.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The fake harness. Its first argument is the behaviour, the last is the brief dog-walker
/// passes; it writes where `DOG_WALKER_RESULT_PATH` says.
const FAKE_HARNESS: &str = r#"#!/usr/bin/env bash
set -u
mode=${1:-}
prompt=${2:-}
result=${DOG_WALKER_RESULT_PATH:?}
case $mode in
  result)
    if read -r -t 1 line; then stdin="open: $line"; else stdin="closed"; fi
    {
      printf 'prompt: %s\n' "$prompt"
      printf 'stdin: %s\n' "$stdin"
      printf 'task-dir: %s\n' "${DOG_WALKER_TASK_DIR:-}"
      printf 'model: %s\n' "${DOG_WALKER_MODEL:-}"
      printf 'effort: %s\n' "${DOG_WALKER_EFFORT:-}"
      printf 'profile: %s\n' "${DOG_WALKER_PROFILE:-}"
      printf 'task: %s\n' "${DOG_WALKER_TASK_ID:-}"
      printf 'cwd: %s\n' "$(pwd)"
    } > "$result"
    ;;
  slow)
    sleep 0.2
    printf 'slept first\n' > "$result"
    ;;
  no-result)
    echo "the harness ran and wrote nothing"
    exit 3
    ;;
  empty-result)
    : > "$result"
    ;;
  bad-result)
    printf '\377\376' > "$result"
    ;;
  handover-at-start)
    { echo "handover-at-start:"; cat "$DOG_WALKER_TASK_DIR/handover.md"; } > "$result"
    ;;
  edit-status)
    handover="$DOG_WALKER_TASK_DIR/handover.md"
    printf '+++\nid = "9999"\nprofile = "fake"\nstatus = "cancelled"\n+++\n\n# fake\n' > "$handover"
    printf 'wrote a result after rewriting the handover\n' > "$result"
    ;;
  edit-status-no-result)
    handover="$DOG_WALKER_TASK_DIR/handover.md"
    printf '+++\nid = "9999"\nprofile = "fake"\nstatus = "done"\n+++\n' > "$handover"
    ;;
esac
"#;

/// A unique temporary directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("dog-walker-run-{}-{name}", std::process::id()));
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

/// A project directory, a config that points at the fake harness, and the harness itself.
struct Fixture {
    dir: TempDir,
    project: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new(name: &str, mode: &str) -> Self {
        let dir = TempDir::new(name);
        let script = dir.path().join("fake.sh");
        fs::write(&script, FAKE_HARNESS).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let config = dir.path().join("config.toml");
        fs::write(
            &config,
            format!(
                "[profiles.zeta]\nharness = \"claude\"\nmodel = \"claude-fable-5-1\"\n\
                 effort = \"high\"\nmain = true\ncommand = [\"{script}\", \"{mode}\"]\n\n\
                 [profiles.alpha]\nharness = \"pi\"\nmodel = \"local/qwen\"\n\
                 effort = \"medium\"\ncommand = [\"{script}\", \"{mode}\"]\n",
                script = script.display()
            ),
        )
        .unwrap();
        Self {
            dir,
            project,
            config,
        }
    }

    /// Runs `dog-walker --config <config> <args>` in the project directory.
    fn run(&self, args: &[&str]) -> Output {
        binary()
            .arg("--config")
            .arg(&self.config)
            .args(args)
            .current_dir(&self.project)
            .output()
            .unwrap()
    }

    fn tasks(&self) -> PathBuf {
        self.project.join(".dog-walker").join("tasks")
    }

    /// The handover folder of the only task, as a sorted name.
    fn only_task(&self) -> String {
        let mut names: Vec<String> = fs::read_dir(self.tasks())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_str().unwrap().to_owned())
            .collect();
        names.sort();
        assert_eq!(names.len(), 1, "{names:?}");
        names.remove(0)
    }

    fn handover(&self) -> String {
        let path = self.tasks().join(self.only_task()).join("handover.md");
        fs::read_to_string(path).unwrap()
    }

    fn log(&self) -> String {
        let path = self.tasks().join(self.only_task()).join("harness.log");
        fs::read_to_string(path).unwrap()
    }
}

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dog-walker"))
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The handover path `run` printed, which must be exactly one line of stdout.
fn printed_path(out: &Output) -> PathBuf {
    let text = stdout(out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "stdout must hold the path only: {text:?}");
    PathBuf::from(lines.first().unwrap())
}

// AT6 (R5): a brief given as text runs in the project directory and the run blocks until the
// harness ends: the result is there when the command returns.
#[test]
fn at6_a_brief_as_text_runs_in_the_project_and_blocks() {
    let fixture = Fixture::new("at6-text", "slow");
    let out = fixture.run(&["run", "alpha", "Fix the parser bug."]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(
        handover.contains("## Result\n\nslept first\n"),
        "{handover}"
    );
}

// AT6 (R5): the brief can come from a file.
#[test]
fn at6_a_brief_from_a_file() {
    let fixture = Fixture::new("at6-file", "result");
    let brief = fixture.dir.path().join("brief.md");
    fs::write(&brief, "From a file.\n").unwrap();
    let out = fixture.run(&["run", "alpha", "--file", &brief.to_string_lossy()]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(fixture.handover().contains("prompt: From a file."));
}

// AT6 (R5): the harness starts non-interactively: its stdin is closed, not a terminal.
#[test]
fn at6_the_harness_gets_no_stdin() {
    let fixture = Fixture::new("at6-stdin-closed", "result");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        fixture.handover().contains("stdin: closed"),
        "{}",
        fixture.handover()
    );
}

// AT6 (R5): the brief can come from stdin, with `-`.
#[test]
fn at6_a_brief_from_stdin() {
    let fixture = Fixture::new("at6-stdin", "result");
    let mut child = binary()
        .arg("--config")
        .arg(&fixture.config)
        .args(["run", "alpha", "-"])
        .current_dir(&fixture.project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"From stdin.\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(fixture.handover().contains("prompt: From stdin."));
}

// AT6 (R5): with no brief at all, nothing runs; the command line is the error.
#[test]
fn at6_no_brief_is_a_usage_error() {
    let fixture = Fixture::new("at6-none", "result");
    let out = fixture.run(&["run", "alpha"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert!(!fixture.tasks().exists(), "no task folder");
}

// AT6 (R5): a brief and a file together are a usage error.
#[test]
fn at6_a_brief_and_a_file_are_exclusive() {
    let fixture = Fixture::new("at6-both", "result");
    let out = fixture.run(&["run", "alpha", "text", "--file", "somewhere"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
}

// AT7 (R6): `done` prints exactly the handover path on stdout, logs on stderr, exits 0.
#[test]
fn at7_done_prints_the_handover_path_and_exits_0() {
    let fixture = Fixture::new("at7-done", "result");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let path = printed_path(&out);
    assert!(
        path.is_absolute() && path.ends_with("handover.md"),
        "{path:?}"
    );
    assert!(path.starts_with(fixture.tasks()), "{path:?}");
    assert!(stderr(&out).contains("done"), "{}", stderr(&out));
    assert!(fixture.handover().contains("status = \"done\""));
}

// AT7 (R6): any other status exits 1 and still prints the path.
#[test]
fn at7_a_failed_task_exits_1_and_still_prints_the_path() {
    let fixture = Fixture::new("at7-failed", "no-result");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    let path = printed_path(&out);
    assert!(path.ends_with("handover.md"), "{path:?}");
    assert!(fixture.handover().contains("status = \"failed\""));
}

// AT1 (R1): the profile's model and effort reach the harness.
#[test]
fn at1_the_model_and_effort_reach_the_harness() {
    let fixture = Fixture::new("at1", "result");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("model: local/qwen"), "{handover}");
    assert!(handover.contains("effort: medium"), "{handover}");
    assert!(handover.contains("profile: alpha"), "{handover}");
    assert!(handover.contains("task: 0001"), "{handover}");
    assert!(
        handover.contains(&format!(
            "task-dir: {}",
            fixture.tasks().join("0001-do-the-thing").display()
        )),
        "{handover}"
    );
}

// AT3 (R2): the main profile is refused unless `--allow-main` is given.
#[test]
fn at3_the_main_profile_is_refused_without_allow_main() {
    let fixture = Fixture::new("at3-main", "result");
    let out = fixture.run(&["run", "zeta", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("main") && err.contains("--allow-main"),
        "{err}"
    );
    assert!(!fixture.tasks().exists(), "no task folder");
}

// AT3 (R2): and it runs when the user says so.
#[test]
fn at3_the_main_profile_runs_with_allow_main() {
    let fixture = Fixture::new("at3-allow", "result");
    let out = fixture.run(&["run", "zeta", "Do the thing.", "--allow-main"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(fixture.handover().contains("profile = \"zeta\""));
}

// R2: an unknown profile is named and nothing runs.
#[test]
fn an_unknown_profile_is_named() {
    let fixture = Fixture::new("unknown", "result");
    let out = fixture.run(&["run", "nope", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("nope"));
    assert!(!fixture.tasks().exists());
}

// AT13 (R10): ids are zero-padded counters, never reused, and `.gitignore` is not touched.
#[test]
fn at13_ids_are_counters_and_gitignore_is_untouched() {
    let fixture = Fixture::new("at13", "result");
    let gitignore = fixture.project.join(".gitignore");
    fs::write(&gitignore, "custom-ignore\n").unwrap();
    for brief in ["one", "two", "three"] {
        let out = fixture.run(&["run", "alpha", brief]);
        assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    }
    let mut names: Vec<String> = fs::read_dir(fixture.tasks())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["0001-one", "0002-two", "0003-three"]);
    assert_eq!(fs::read_to_string(&gitignore).unwrap(), "custom-ignore\n");
}

// AT14 (R11): the handover has front matter and the brief before the harness starts.
#[test]
fn at14_the_handover_is_complete_before_the_harness_starts() {
    let fixture = Fixture::new("at14", "handover-at-start");
    let out = fixture.run(&["run", "alpha", "Keep it small."]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(
        handover.contains(
            "handover-at-start:\n+++\nid = \"0001\"\nprofile = \"alpha\"\nstatus = \"running\"\n+++"
        ),
        "{handover}"
    );
    assert!(
        handover.contains("## Brief\n\nKeep it small."),
        "{handover}"
    );
}

// AT15 (R12): the harness editing the status does not change it: dog-walker closes the task
// with the real one, and restores the brief the harness overwrote.
#[test]
fn at15_a_status_edited_by_the_harness_is_overwritten() {
    let fixture = Fixture::new("at15", "edit-status");
    let out = fixture.run(&["run", "alpha", "Keep it small."]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("status = \"done\""), "{handover}");
    assert!(!handover.contains("status = \"cancelled\""), "{handover}");
    assert!(handover.contains("id = \"0001\""), "{handover}");
    assert!(
        handover.contains("## Brief\n\nKeep it small."),
        "{handover}"
    );
}

// AT15 (R12): a task that ends failed says so, whatever the harness wrote.
#[test]
fn at15_the_real_end_state_wins() {
    let fixture = Fixture::new("at15-failed", "edit-status-no-result");
    let out = fixture.run(&["run", "alpha", "Keep it small."]);
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("status = \"failed\""), "{handover}");
    assert!(!handover.contains("status = \"done\""), "{handover}");
}

// AT16 (R13): a missing result fails the task with a reason, keeps the raw output in the log
// beside the handover, and invents no result.
#[test]
fn at16_a_missing_result_fails_with_a_reason_and_a_log() {
    let fixture = Fixture::new("at16-missing", "no-result");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("status = \"failed\""), "{handover}");
    assert!(
        handover.contains("## Result\n\nThe task failed: the harness left no result"),
        "{handover}"
    );
    assert!(fixture.log().contains("wrote nothing"), "{}", fixture.log());
}

// AT16 (R13): an empty result is a failure, not an empty success.
#[test]
fn at16_an_empty_result_fails() {
    let fixture = Fixture::new("at16-empty", "empty-result");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    assert!(
        fixture
            .handover()
            .contains("the harness left an empty result")
    );
}

// AT16 (R13): a result that is not text is a failure, not a mangled one.
#[test]
fn at16_a_result_that_is_not_utf8_fails() {
    let fixture = Fixture::new("at16-bad", "bad-result");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    assert!(fixture.handover().contains("the result is not valid UTF-8"));
}

// AT16 (R13): a harness that cannot be started is a failed task, not a usage error: the path
// is still printed and the reason is in the handover.
#[test]
fn at16_a_harness_that_cannot_start_fails_the_task() {
    let fixture = Fixture::new("at16-nostart", "result");
    let config = fixture.dir.path().join("missing.toml");
    fs::write(
        &config,
        "[profiles.zeta]\nharness = \"claude\"\nmodel = \"m\"\nmain = true\n\n\
         [profiles.alpha]\nharness = \"pi\"\nmodel = \"m\"\n\
         command = [\"/nonexistent/harness\"]\n",
    )
    .unwrap();
    let out = binary()
        .arg("--config")
        .arg(&config)
        .args(["run", "alpha", "Do the thing."])
        .current_dir(&fixture.project)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    assert!(printed_path(&out).ends_with("handover.md"));
    assert!(
        fixture
            .handover()
            .contains("cannot start the harness `/nonexistent/harness`"),
        "{}",
        fixture.handover()
    );
}

// R1: a profile with no `command` cannot run yet, and the error says so before a task starts.
#[test]
fn a_profile_without_a_command_is_a_usage_error() {
    let fixture = Fixture::new("no-command", "result");
    let config = fixture.dir.path().join("no-command.toml");
    fs::write(
        &config,
        "[profiles.zeta]\nharness = \"claude\"\nmodel = \"m\"\nmain = true\n\n\
         [profiles.beta]\nharness = \"pi\"\nmodel = \"m\"\n",
    )
    .unwrap();
    let out = binary()
        .arg("--config")
        .arg(&config)
        .args(["run", "beta", "Do the thing."])
        .current_dir(&fixture.project)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("command"), "{}", stderr(&out));
    assert!(!fixture.tasks().exists());
}

// AT24 (R19): a run needs no network: it works in a network-less namespace.
#[test]
fn at24_a_run_works_without_network() {
    if !sandbox_available("at24_a_run_works_without_network") {
        return;
    }
    let fixture = Fixture::new("at24-run", "result");
    let out = Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--bind"])
        .arg(&fixture.project)
        .arg(&fixture.project)
        .args(["--unshare-net", "--die-with-parent", "--chdir"])
        .arg(&fixture.project)
        .arg(env!("CARGO_BIN_EXE_dog-walker"))
        .arg("--config")
        .arg(&fixture.config)
        .args(["run", "alpha", "Do the thing."])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(fixture.handover().contains("status = \"done\""));
}

/// Whether a missing sandbox must fail the sandbox tests instead of skipping them.
fn sandbox_required(value: Option<std::ffi::OsString>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

/// The start of every sandbox test: true when `bwrap` works here, a loud skip when it does
/// not, and a failure when the sandbox is required (CI sets `DOG_WALKER_REQUIRE_SANDBOX=1`).
fn sandbox_available(test: &str) -> bool {
    let probe = Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--unshare-net", "true"])
        .output();
    if matches!(probe, Ok(ref output) if output.status.success()) {
        return true;
    }
    assert!(
        !sandbox_required(std::env::var_os("DOG_WALKER_REQUIRE_SANDBOX")),
        "bwrap or user namespaces are unavailable, but DOG_WALKER_REQUIRE_SANDBOX is set"
    );
    let message = format!("SKIPPED {test}: bwrap or user namespaces unavailable\n");
    let _ = std::io::Write::write_all(&mut std::io::stderr(), message.as_bytes());
    false
}
