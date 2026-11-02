//! Acceptance tests for `run` (#16): the task folder, the handover, the status lifecycle and
//! the exit contract. They run the compiled binary against a fake harness: a small script the
//! profile's `command` points at, which behaves as its first argument says.

// Why: clippy.toml exempts `#[test]` functions from the unwrap and panic denials, but not the
// helper functions of an integration-test file. A failing helper should fail the test.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// The fake harness. Its first argument is the behaviour, the last is the brief dog-walker
/// passes; it writes where `DOG_WALKER_RESULT_PATH` says.
const FAKE_HARNESS: &str = r#"#!/usr/bin/env bash
set -u
mode=${1:-}
if [ "$#" -gt 0 ]; then prompt=${!#}; else prompt=""; fi
result=${DOG_WALKER_RESULT_PATH:?}
case $mode in
  result)
    read -r -t 1 line
    status=$?
    if [ "$status" -eq 1 ]; then stdin="closed"; elif [ "$status" -eq 0 ]; then stdin="open: $line"; else stdin="timeout"; fi
    {
      printf 'prompt: %s\n' "$prompt"
      printf 'stdin: %s\n' "$stdin"
      printf 'task-dir: %s\n' "${DOG_WALKER_TASK_DIR:-}"
      printf 'handover: %s\n' "${DOG_WALKER_HANDOVER_PATH:-}"
      printf 'model: %s\n' "${DOG_WALKER_MODEL:-}"
      printf 'effort: %s\n' "${DOG_WALKER_EFFORT:-<unset>}"
      printf 'profile: %s\n' "${DOG_WALKER_PROFILE:-}"
      printf 'task: %s\n' "${DOG_WALKER_TASK_ID:-}"
      printf 'cwd: %s\n' "$(pwd)"
    } > "$result"
    ;;
  slow)
    sleep 0.2
    printf 'slept first\n' > "$result"
    ;;
  hang)
    sleep 300 &
    echo "$$ $!" > "$DOG_WALKER_TASK_DIR/pids"
    wait
    ;;
  no-result)
    echo "the harness ran and wrote nothing"
    exit 3
    ;;
  result-and-exit-3)
    printf 'a result, but a bad exit\n' > "$result"
    echo "finishing with 3"
    exit 3
    ;;
  empty-result)
    : > "$result"
    ;;
  bad-result)
    printf '\377\376' > "$result"
    ;;
  symlink-result)
    printf 'a secret\n' > "$DOG_WALKER_TASK_DIR/../../secret.txt"
    ln -s "$DOG_WALKER_TASK_DIR/../../secret.txt" "$result"
    ;;
  fifo-result)
    mkfifo "$result"
    ;;
  plant-temp-link)
    ln -s "$DOG_WALKER_TASK_DIR/../../../victim.txt" "$DOG_WALKER_TASK_DIR/.handover.md.tmp"
    ln -s "$DOG_WALKER_TASK_DIR/../../../victim.txt" "$DOG_WALKER_TASK_DIR/.handover.md.$PPID.tmp"
    printf 'a result beside a planted link\n' > "$result"
    ;;
  move-task-dir)
    printf 'a result outside\n' > "$result"
    mv "$DOG_WALKER_TASK_DIR" "$DOG_WALKER_TASK_DIR.moved"
    ln -s "$DOG_WALKER_TASK_DIR.moved" "$DOG_WALKER_TASK_DIR"
    ;;
  stuck-handover)
    printf 'a result\n' > "$result"
    chmod 500 "$DOG_WALKER_TASK_DIR"
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
        Fixture::with_timeout(name, mode, None)
    }

    /// A fixture whose secondary profile has `timeout_minutes` set.
    fn with_timeout(name: &str, mode: &str, timeout_minutes: Option<f64>) -> Self {
        let dir = TempDir::new(name);
        let timeout = timeout_minutes
            .map(|minutes| format!("timeout_minutes = {minutes}\n"))
            .unwrap_or_default();
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
                 effort = \"medium\"\n{timeout}command = [\"{script}\", \"{mode}\"]\n",
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

    fn task_dir(&self) -> PathBuf {
        self.tasks().join(self.only_task())
    }

    /// Waits until the harness has written its pids, then checks that none is left.
    fn assert_tree_gone(&self) {
        let pids = fs::read_to_string(self.task_dir().join("pids")).unwrap();
        assert_eq!(pids.split_whitespace().count(), 2, "{pids}");
        for pid in pids.split_whitespace() {
            let path = PathBuf::from(format!("/proc/{pid}"));
            let deadline = Instant::now() + Duration::from_secs(5);
            while path.exists() {
                assert!(Instant::now() < deadline, "{pid} is still running: {pids}");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    /// Waits until the harness has written its pids into its task folder.
    fn wait_for_pids(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(entries) = fs::read_dir(self.tasks()) {
                for entry in entries.flatten() {
                    if entry.path().join("pids").exists() {
                        return;
                    }
                }
            }
            assert!(Instant::now() < deadline, "the harness did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Starts `run` in the fixture, so a test can cancel it from another process.
fn spawn_run(fixture: &Fixture, args: &[&str]) -> std::process::Child {
    binary()
        .arg("--config")
        .arg(&fixture.config)
        .args(args)
        .current_dir(&fixture.project)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Waits for a spawned run, killing it if it outlives `limit`: a broken timeout or cancel
/// should fail the test, not hang it.
fn wait_with_deadline(mut child: std::process::Child, limit: Duration) -> Output {
    let started = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            panic!("the run did not end within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
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
    assert!(
        handover.contains(&format!(
            "handover: {}",
            fixture
                .tasks()
                .join("0001-do-the-thing")
                .join("handover.md")
                .display()
        )),
        "{handover}"
    );
    // AT6 (R5): it starts in the project directory, not wherever the config lives.
    let project = fs::canonicalize(&fixture.project).unwrap();
    assert!(
        handover.contains(&format!("cwd: {}", project.display())),
        "{handover}"
    );
}

// AT1 (R1): a profile without an effort passes none.
#[test]
fn at1_a_profile_without_effort_passes_none() {
    let fixture = Fixture::new("at1-no-effort", "result");
    let script = fixture.dir.path().join("fake.sh");
    let config = fixture.dir.path().join("no-effort.toml");
    fs::write(
        &config,
        format!(
            "[profiles.zeta]\nharness = \"claude\"\nmodel = \"m\"\nmain = true\n\n\
             [profiles.alpha]\nharness = \"pi\"\nmodel = \"local/qwen\"\n\
             command = [\"{}\", \"result\"]\n",
            script.display()
        ),
    )
    .unwrap();
    let out = binary()
        .arg("--config")
        .arg(&config)
        .args(["run", "alpha", "Do the thing."])
        .current_dir(&fixture.project)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        fixture.handover().contains("effort: <unset>"),
        "{}",
        fixture.handover()
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
            "handover-at-start:\n+++\nid = \"0001\"\nprofile = \"alpha\"\nstatus = \"running\"\n+++\n\n# Task 0001\n\n## Brief\n\nKeep it small.\n"
        ),
        "the snapshot holds the brief before the harness runs: {handover}"
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

// AT16 (R13): a harness that ends badly is a failure even when it left a result.
#[test]
fn at16_a_nonzero_exit_is_a_failure() {
    let fixture = Fixture::new("at16-exit", "result-and-exit-3");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("status = \"failed\""), "{handover}");
    assert!(
        handover.contains("the harness ended with exit status: 3"),
        "{handover}"
    );
    assert!(
        handover.contains("a result, but a bad exit"),
        "the real result is kept, not invented: {handover}"
    );
    assert!(
        fixture.log().contains("finishing with 3"),
        "{}",
        fixture.log()
    );
}

// AT16 (R13): a result that is a symbolic link is refused, not read through.
#[test]
fn at16_a_symlinked_result_is_not_read() {
    let fixture = Fixture::new("at16-symlink", "symlink-result");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("status = \"failed\""), "{handover}");
    assert!(
        handover.contains("the result is not a regular file"),
        "{handover}"
    );
    assert!(!handover.contains("a secret"), "{handover}");
}

// AT16 (R13): a result that is a pipe is refused without the run hanging on it.
#[test]
fn at16_a_fifo_result_is_not_read() {
    let fixture = Fixture::new("at16-fifo", "fifo-result");
    let mut child = binary()
        .arg("--config")
        .arg(&fixture.config)
        .args(["run", "alpha", "Do the thing."])
        .current_dir(&fixture.project)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if started.elapsed() >= std::time::Duration::from_secs(10) {
            let _ = child.kill();
            panic!("the run hung on a FIFO result");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    assert!(
        fixture
            .handover()
            .contains("the result is not a regular file"),
        "{}",
        fixture.handover()
    );
}

// R12: a link planted where the handover is written is not followed, and the victim is kept.
#[test]
fn at15_a_planted_temp_link_is_not_followed() {
    let fixture = Fixture::new("at15-planted", "plant-temp-link");
    fs::create_dir_all(fixture.tasks()).unwrap();
    let victim = fixture.project.join("victim.txt");
    fs::write(&victim, "keep me\n").unwrap();
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep me\n");
    let handover = fixture
        .tasks()
        .join(fixture.only_task())
        .join("handover.md");
    let metadata = fs::symlink_metadata(&handover).unwrap();
    assert!(
        metadata.file_type().is_file(),
        "the handover is a regular file, not a link"
    );
    assert!(fixture.handover().contains("status = \"done\""));
}

// R6: the path is on stdout even when the end state cannot be written.
#[test]
fn at7_the_path_comes_before_the_end_state() {
    let fixture = Fixture::new("at7-stuck", "stuck-handover");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(printed_path(&out).ends_with("handover.md"));
    assert!(stderr(&out).contains("handover"), "{}", stderr(&out));
    // Give the fixture back the right to remove its directory.
    let task = fixture.tasks().join(fixture.only_task());
    fs::set_permissions(&task, fs::Permissions::from_mode(0o700)).unwrap();
}

// R12: a task folder the harness moved away and replaced with a link is not written through.
#[test]
fn at15_a_moved_task_folder_is_refused() {
    let fixture = Fixture::new("at15-moved", "move-task-dir");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("moved or replaced"),
        "{}",
        stderr(&out)
    );
    let moved = fixture
        .tasks()
        .join("0001-do-the-thing.moved")
        .join("handover.md");
    let text = fs::read_to_string(moved).unwrap();
    assert!(
        text.contains("status = \"running\""),
        "the end state was not written through the link: {text}"
    );
}

// AT17 (R14): a task past its timeout has its tree killed and ends `timed-out`.
#[test]
fn at17_a_timeout_kills_the_tree_and_ends_timed_out() {
    let fixture = Fixture::with_timeout("at17", "hang", Some(0.03));
    let started = Instant::now();
    let child = spawn_run(&fixture, &["run", "alpha", "Do the thing."]);
    let out = wait_with_deadline(child, Duration::from_secs(15));
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    assert!(
        started.elapsed() >= Duration::from_millis(1500),
        "the deadline was honoured: {:?}",
        started.elapsed()
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the profile's 0.03 minutes, not the default: {:?}",
        started.elapsed()
    );
    assert!(printed_path(&out).ends_with("handover.md"));
    let handover = fixture.handover();
    assert!(handover.contains("status = \"timed-out\""), "{handover}");
    // The handover text comes from the same timeout, so it pins the 1.8 seconds.
    assert!(
        handover.contains("The task timed out after 1.8 seconds."),
        "{handover}"
    );
    fixture.assert_tree_gone();
}

// AT18 (R14): Ctrl-C kills the tree and ends `cancelled`.
#[test]
fn at18_ctrl_c_kills_the_tree_and_ends_cancelled() {
    let fixture = Fixture::new("at18-int", "hang");
    let child = spawn_run(&fixture, &["run", "alpha", "Do the thing."]);
    fixture.wait_for_pids();
    let killed = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let out = wait_with_deadline(child, Duration::from_secs(15));
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("status = \"cancelled\""), "{handover}");
    assert!(handover.contains("The task was cancelled."), "{handover}");
    assert!(printed_path(&out).ends_with("handover.md"));
    fixture.assert_tree_gone();
}

// AT18 (R14): a TERM to the runner cancels too.
#[test]
fn at18_a_term_to_the_runner_cancels() {
    let fixture = Fixture::new("at18-term", "hang");
    let child = spawn_run(&fixture, &["run", "alpha", "Do the thing."]);
    fixture.wait_for_pids();
    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let out = wait_with_deadline(child, Duration::from_secs(15));
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    assert!(fixture.handover().contains("status = \"cancelled\""));
    fixture.assert_tree_gone();
}

// AT18 (R14): `cancel <id>` from another shell kills the tree and ends `cancelled`.
#[test]
fn at18_cancel_from_another_shell_kills_the_tree() {
    let fixture = Fixture::new("at18-cancel", "hang");
    let child = spawn_run(&fixture, &["run", "alpha", "Do the thing."]);
    fixture.wait_for_pids();
    let out = fixture.run(&["cancel", "0001"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("cancel requested"),
        "{}",
        stdout(&out)
    );
    let out = wait_with_deadline(child, Duration::from_secs(15));
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    assert!(printed_path(&out).ends_with("handover.md"));
    let handover = fixture.handover();
    assert!(handover.contains("status = \"cancelled\""), "{handover}");
    assert!(handover.contains("The task was cancelled."), "{handover}");
    assert!(
        !fixture.task_dir().join(".cancel").exists(),
        "the runner removes the marker"
    );
    fixture.assert_tree_gone();
}

// AT18 (R14): cancelling a finished task changes nothing.
#[test]
fn at18_cancelling_a_finished_task_changes_nothing() {
    let fixture = Fixture::new("at18-done", "result");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let task = fixture.task_dir();
    let before = fs::read_to_string(task.join("handover.md")).unwrap();
    let out = fixture.run(&["cancel", "0001"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("already done"), "{}", stdout(&out));
    assert_eq!(
        fs::read_to_string(task.join("handover.md")).unwrap(),
        before
    );
    assert!(
        !task.join(".cancel").exists(),
        "no marker in a finished task"
    );
}

// R14: `cancel` refuses a tasks root that is a link, like `run` does.
#[test]
fn cancel_refuses_a_symlinked_tasks_root() {
    let fixture = Fixture::new("cancel-symlink", "result");
    let outside = fixture.dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, fixture.project.join(".dog-walker")).unwrap();
    let out = fixture.run(&["cancel", "0001"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("symbolic link"), "{}", stderr(&out));
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
}

// R14: `cancel` names a task it cannot find, and a value that is not an id.
#[test]
fn cancel_names_a_missing_or_bad_id() {
    let fixture = Fixture::new("cancel-bad", "result");
    let out = fixture.run(&["cancel", "0001"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("0001"), "{}", stderr(&out));
    let out = fixture.run(&["cancel", "not-a-number"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("not-a-number"), "{}", stderr(&out));
}

/// Whether a missing sandbox must fail the sandbox tests instead of skipping them.
fn sandbox_required(value: Option<std::ffi::OsString>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

/// The start of every sandbox test: true when `bwrap` works here, a loud skip when it does
/// not, and a failure when the sandbox is required (CI sets `DOG_WALKER_REQUIRE_SANDBOX=1`).
/// A copy of the helper in `tests/cli.rs`, because the two test binaries cannot share it.
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
