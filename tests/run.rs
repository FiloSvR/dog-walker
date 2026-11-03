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
  outside-write)
    outside="$DOG_WALKER_TASK_DIR/../../../../escaped.txt"
    if printf 'escaped\n' > "$outside"; then
      printf 'outside-write: written\n' > "$result"
      exit 1
    fi
    if printf 'x' > "$DOG_WALKER_TASK_DIR/handover.md" 2>/dev/null; then edited=yes; else edited=no; fi
    printf 'outside-write: denied\nedited: %s\n' "$edited" > "$result"
    ;;
  denied-write)
    if printf 'needed\n' > "$DOG_WALKER_TASK_DIR/../../../../needed.txt"; then
      printf 'the write outside the project worked\n' > "$result"
      exit 1
    fi
    echo "the harness needed a directory outside the project" >&2
    exit 1
    ;;
  network)
    if [ -n "${TEST_PORT:-}" ] && (exec 3<>/dev/tcp/127.0.0.1/"$TEST_PORT") 2>/dev/null; then
      local=reachable
    else
      local=unreachable
    fi
    printf 'local: %s\ninterfaces: %s\n' "$local" "$(grep -c ':' /proc/net/dev)" > "$result"
    ;;
  git)
    status() { "$@" > /dev/null 2>&1; echo $?; }
    {
      printf 'log: %s\n' "$(status git log --oneline -1)"
      printf 'diff: %s\n' "$(status git diff --stat)"
      printf 'add: %s\n' "$(status git add .)"
      printf 'commit: %s\n' "$(status git -c user.name=a -c user.email=a@b commit -m sandbox --no-gpg-sign)"
      printf 'push: %s\n' "$(status git push "$DOG_WALKER_TASK_DIR/../../../../outside.git" HEAD:refs/heads/main)"
    } > "$result"
    ;;
  env-report)
    printf 'host: %s\n' "${HOST_SECRET:-<unset>}" > "$result"
    ;;
  extras)
    {
      printf 'env: %s\n' "${TEST_VALUE:-<unset>}"
      printf 'read: %s\n' "$(cat "$TEST_READ/file.txt" 2>&1)"
      printf 'write: %s\n' "$(printf 'written\n' > "$TEST_WRITE/file.txt" 2>&1 && echo ok)"
      printf 'write-read-only: %s\n' "$(printf 'x' > "$TEST_READ/file.txt" 2>/dev/null && echo ok || echo denied)"
      printf 'write-unlisted: %s\n' "$(printf 'x' > "$TEST_WRITE/../unlisted.txt" 2>/dev/null && echo ok || echo denied)"
    } > "$result"
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
    if printf '+++\nid = "9999"\nprofile = "fake"\nstatus = "cancelled"\n+++\n\n# fake\n' > "$handover" 2>/dev/null; then
      edited=yes
    else
      edited=no
    fi
    printf 'edited: %s\n' "$edited" > "$result"
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

    /// A fixture whose secondary profile carries `extra` policy lines.
    fn with_policy(name: &str, mode: &str, extra: &str) -> Self {
        let fixture = Fixture::with_timeout(name, mode, None);
        let text = fs::read_to_string(&fixture.config).unwrap();
        fs::write(&fixture.config, format!("{text}{extra}\n")).unwrap();
        fixture
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

    /// Runs `dog-walker --config <config> <args> --unsandboxed` in the project directory.
    ///
    /// These tests are about the task lifecycle, not about the sandbox; the sandbox has its
    /// own tests below, and they need bwrap.
    fn run(&self, args: &[&str]) -> Output {
        let mut all: Vec<&str> = args.to_vec();
        if all.first() == Some(&"run") {
            all.push("--unsandboxed");
        }
        binary()
            .arg("--config")
            .arg(&self.config)
            .args(&all)
            .current_dir(&self.project)
            .output()
            .unwrap()
    }

    /// Runs the same command with the sandbox, for the tests that are about it.
    fn run_sandboxed(&self, args: &[&str]) -> Output {
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
    /// Kills whatever the hung harness left, best effort: the pids it wrote, if any.
    fn kill_tree(&self) {
        if let Ok(entries) = fs::read_dir(self.tasks()) {
            for entry in entries.flatten() {
                if let Ok(pids) = fs::read_to_string(entry.path().join("pids")) {
                    for pid in pids.split_whitespace() {
                        let _ = Command::new("kill")
                            .args(["-KILL", pid])
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                    }
                }
            }
        }
    }

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
    let mut all: Vec<&str> = args.to_vec();
    if all.first() == Some(&"run") {
        all.push("--unsandboxed");
    }
    binary()
        .arg("--config")
        .arg(&fixture.config)
        .args(&all)
        .current_dir(&fixture.project)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Waits for a spawned run, ending it if it outlives `limit`: a broken timeout or cancel should
/// fail the test, not hang it, and must not leave the harness behind (it leads its own group,
/// so killing the runner does not reach it).
fn wait_with_deadline(
    fixture: &Fixture,
    mut child: std::process::Child,
    limit: Duration,
) -> Output {
    let started = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            fixture.kill_tree();
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
        .args(["run", "alpha", "-", "--unsandboxed"])
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
            "handover-at-start:\n+++\nid = \"0001\"\nprofile = \"alpha\"\nstatus = \"running\"\n+++\n\n# Task 0001\n\n## Sandbox\n\n"
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

// R1: a harness that does not exist is a config error, caught before a task starts.
#[test]
fn at16_a_harness_that_cannot_start_is_a_config_error() {
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
        .args(["run", "alpha", "Do the thing.", "--unsandboxed"])
        .current_dir(&fixture.project)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("/nonexistent/harness"),
        "{}",
        stderr(&out)
    );
    assert!(!fixture.tasks().exists(), "no task folder");
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

// AT24 (R19): a run needs no network: it works in a network-less namespace. The inner run is
// `--unsandboxed` because a sandbox cannot nest user namespaces; the outer bwrap is the
// network-less sandbox here, and the point is that dog-walker itself needs no network.
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
        .args(["run", "alpha", "Do the thing.", "--unsandboxed"])
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
    let out = wait_with_deadline(&fixture, child, Duration::from_secs(15));
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    assert!(
        started.elapsed() >= Duration::from_millis(1500),
        "the deadline was honoured: {:?}",
        started.elapsed()
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
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
    let out = wait_with_deadline(&fixture, child, Duration::from_secs(15));
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
    let out = wait_with_deadline(&fixture, child, Duration::from_secs(15));
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
    let out = wait_with_deadline(&fixture, child, Duration::from_secs(15));
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

// R14: `cancel` on a task whose handover is not written yet still leaves the request.
#[test]
fn cancel_tolerates_a_missing_handover() {
    let fixture = Fixture::new("cancel-no-handover", "result");
    let task = fixture.tasks().join("0001-half");
    fs::create_dir_all(&task).unwrap();
    let out = fixture.run(&["cancel", "0001"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("cancel requested"),
        "{}",
        stdout(&out)
    );
    assert!(task.join(".cancel").exists());
}

// R14: a handover that is a pipe is not read, so `cancel` does not hang on it.
#[test]
fn cancel_does_not_hang_on_a_fifo_handover() {
    let fixture = Fixture::new("cancel-fifo", "result");
    let task = fixture.tasks().join("0001-half");
    fs::create_dir_all(&task).unwrap();
    let fifo = Command::new("mkfifo")
        .arg(task.join("handover.md"))
        .status()
        .unwrap();
    assert!(fifo.success());
    let child = spawn_run(&fixture, &["cancel", "0001"]);
    let out = wait_with_deadline(&fixture, child, Duration::from_secs(10));
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(task.join(".cancel").exists());
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

/// A git repository in `project`, with one commit, so the harness has history to read.
fn init_repo(project: &Path) {
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(project)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    fs::write(project.join("README.md"), "hello\n").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=a",
        "-c",
        "user.email=a@b",
        "commit",
        "-q",
        "--no-gpg-sign",
        "-m",
        "init",
    ]);
}

// AT19 (R15, R16): a write outside the project fails at OS level, whatever the harness says.
#[test]
fn at19_a_write_outside_the_project_is_denied() {
    if !sandbox_available("at19_a_write_outside_the_project_is_denied") {
        return;
    }
    let fixture = Fixture::new("at19", "outside-write");
    let out = fixture.run_sandboxed(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("## Sandbox"), "{handover}");
    assert!(handover.contains("- tool: bwrap"), "{handover}");
    assert!(handover.contains("outside-write: denied"), "{handover}");
    assert!(handover.contains("edited: no"), "{handover}");
    assert!(
        fixture.log().contains("Read-only file system"),
        "the denial is in the harness log: {}",
        fixture.log()
    );
    assert!(
        !fixture.dir.path().join("escaped.txt").exists(),
        "the write escaped the project"
    );
}

// AT20 (R16): the network is there by default and cut with `network = false`. The host's
// listener is the positive control: the test would not pass on a machine with no network.
#[test]
fn at20_the_network_is_there_and_can_be_cut() {
    if !sandbox_available("at20_the_network_is_there_and_can_be_cut") {
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();

    let on = Fixture::with_policy("at20-on", "network", "env = [\"TEST_PORT\"]");
    let out = binary()
        .arg("--config")
        .arg(&on.config)
        .args(["run", "alpha", "Do the thing."])
        .current_dir(&on.project)
        .env("TEST_PORT", &port)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let handover = on.handover();
    assert!(handover.contains("- network: on"), "{handover}");
    assert!(handover.contains("local: reachable"), "{handover}");

    let off = Fixture::with_policy(
        "at20-off",
        "network",
        "network = false\nenv = [\"TEST_PORT\"]",
    );
    let out = binary()
        .arg("--config")
        .arg(&off.config)
        .args(["run", "alpha", "Do the thing."])
        .current_dir(&off.project)
        .env("TEST_PORT", &port)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let handover = off.handover();
    assert!(handover.contains("- network: off"), "{handover}");
    assert!(handover.contains("local: unreachable"), "{handover}");
    // One interface, `lo`: no way out at all.
    assert!(handover.contains("interfaces: 1"), "{handover}");
}

// AT26 (R16a): `.git` is read-only by default: log and diff work, add and commit fail. A push
// to a remote outside the project fails too, since that path is not in the sandbox.
#[test]
fn at26_git_is_read_only_by_default() {
    if !sandbox_available("at26_git_is_read_only_by_default") {
        return;
    }
    let fixture = Fixture::new("at26", "git");
    init_repo(&fixture.project);
    // A real remote just outside the project: the sandbox, not a missing repository, is what
    // makes the push fail.
    let remote = fixture.dir.path().join("outside.git");
    let out = Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&remote)
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = fixture.run_sandboxed(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("- git: read-only"), "{handover}");
    assert!(handover.contains("log: 0"), "{handover}");
    assert!(handover.contains("diff: 0"), "{handover}");
    assert!(!handover.contains("add: 0"), "{handover}");
    assert!(!handover.contains("commit: 0"), "{handover}");
    assert!(!handover.contains("push: 0"), "{handover}");
}

// AT26 (R20): a profile may list `.git` writable, and then the harness can add and commit;
// a push outside still fails.
#[test]
fn at26_git_can_be_writable() {
    if !sandbox_available("at26_git_can_be_writable") {
        return;
    }
    let fixture = Fixture::with_policy("at21-writable", "git", "git = \"writable\"");
    init_repo(&fixture.project);
    let remote = fixture.dir.path().join("outside.git");
    let out = Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&remote)
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = fixture.run_sandboxed(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("- git: writable"), "{handover}");
    assert!(handover.contains("add: 0"), "{handover}");
    assert!(handover.contains("commit: 0"), "{handover}");
    assert!(!handover.contains("push: 0"), "{handover}");
}

// AT25 (R20): extra directories and variables from the profile reach the harness, and the
// handover lists them.
#[test]
fn at25_extra_dirs_and_variables_reach_the_harness() {
    if !sandbox_available("at25_extra_dirs_and_variables_reach_the_harness") {
        return;
    }
    let fixture = Fixture::new("at25", "extras");
    let shared = fixture.dir.path().join("shared");
    let scratch = fixture.dir.path().join("scratch");
    fs::create_dir_all(&shared).unwrap();
    fs::create_dir_all(&scratch).unwrap();
    fs::write(shared.join("file.txt"), "from the shared dir").unwrap();
    let config = fs::read_to_string(&fixture.config).unwrap();
    fs::write(
        &fixture.config,
        format!(
            "{config}read_only_dirs = [\"{}\"]\nwritable_dirs = [\"{}\"]\n\
             env = [\"TEST_VALUE\", \"TEST_READ\", \"TEST_WRITE\"]\n",
            shared.display(),
            scratch.display()
        ),
    )
    .unwrap();
    let out = binary()
        .arg("--config")
        .arg(&fixture.config)
        .args(["run", "alpha", "Do the thing."])
        .current_dir(&fixture.project)
        .env("TEST_VALUE", "hello")
        .env("TEST_READ", &shared)
        .env("TEST_WRITE", &scratch)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("- tool: bwrap"), "{handover}");
    assert!(handover.contains("- network: on"), "{handover}");
    assert!(handover.contains("env: hello"), "{handover}");
    assert!(handover.contains("write-read-only: denied"), "{handover}");
    assert!(handover.contains("write-unlisted: denied"), "{handover}");
    assert!(handover.contains("read: from the shared dir"), "{handover}");
    assert!(handover.contains("write: ok"), "{handover}");
    assert!(
        handover.contains("- environment: TEST_VALUE, TEST_READ, TEST_WRITE, PATH"),
        "{handover}"
    );
    assert!(
        handover.contains(&shared.display().to_string()),
        "{handover}"
    );
    assert!(
        handover.contains(&scratch.display().to_string()),
        "{handover}"
    );
    assert_eq!(
        fs::read_to_string(scratch.join("file.txt")).unwrap(),
        "written\n"
    );
}

// AT23 (R17): without bwrap, `run` refuses and names the opt-out.
#[test]
fn at22_without_bwrap_run_refuses_unless_opt_out() {
    let fixture = Fixture::new("at22", "result");
    let out = binary()
        .arg("--config")
        .arg(&fixture.config)
        .args(["run", "alpha", "Do the thing."])
        .current_dir(&fixture.project)
        .env("PATH", "/nonexistent")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("--unsandboxed"), "{}", stderr(&out));
    assert!(!fixture.tasks().exists());
}

// AT23 (R17): with the explicit opt-out it runs, and the handover records the unsandboxed run.
#[test]
fn at22_without_bwrap_the_opt_out_runs_and_records_it() {
    let fixture = Fixture::new("at22-none", "result");
    let script = fixture.dir.path().join("plain.sh");
    fs::write(
        &script,
        "#!/bin/sh\nprintf 'ran without a sandbox\\n' > \"$DOG_WALKER_RESULT_PATH\"\n",
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let config = fixture.dir.path().join("plain.toml");
    fs::write(
        &config,
        format!(
            "[profiles.zeta]\nharness = \"claude\"\nmodel = \"m\"\nmain = true\n\n\
             [profiles.alpha]\nharness = \"pi\"\nmodel = \"m\"\n\
             command = [\"{}\"]\n",
            script.display()
        ),
    )
    .unwrap();
    let out = binary()
        .arg("--config")
        .arg(&config)
        .args(["run", "alpha", "Do the thing.", "--unsandboxed"])
        .current_dir(&fixture.project)
        .env("PATH", "/nonexistent")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(
        handover.contains("- tool: none (--unsandboxed)"),
        "{handover}"
    );
    assert!(handover.contains("- network: not enforced"), "{handover}");
    assert!(
        handover.contains("- environment: inherited, not cleaned"),
        "{handover}"
    );
    assert!(handover.contains("ran without a sandbox"), "{handover}");
}

// AT8 (R7): an operation outside the policy is denied with no prompt; the task ends failed,
// and the policy and the denial are recorded.
#[test]
fn at8_a_denied_operation_fails_the_task_and_is_traceable() {
    if !sandbox_available("at8_a_denied_operation_fails_the_task_and_is_traceable") {
        return;
    }
    let fixture = Fixture::new("at8", "denied-write");
    let out = fixture.run_sandboxed(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("status = \"failed\""), "{handover}");
    assert!(handover.contains("- tool: bwrap"), "{handover}");
    assert!(handover.contains("- network: on"), "{handover}");
    assert!(
        fixture.log().contains("Read-only file system"),
        "the denial is in the harness log: {}",
        fixture.log()
    );
    assert!(!fixture.dir.path().join("needed.txt").exists());
}

// AT21 (R16): the environment is the allowlist, not the runner's. A secret the runner has is
// invisible to the harness and never reaches the handover.
#[test]
fn at21_a_host_secret_is_invisible() {
    if !sandbox_available("at21_a_host_secret_is_invisible") {
        return;
    }
    let fixture = Fixture::new("at21-secret", "env-report");
    let out = binary()
        .arg("--config")
        .arg(&fixture.config)
        .args(["run", "alpha", "Do the thing."])
        .current_dir(&fixture.project)
        .env("HOST_SECRET", "topsecret")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("host: <unset>"), "{handover}");
    assert!(!handover.contains("topsecret"), "{handover}");
}

// AT1 (R1): the model and effort reach a sandboxed harness too.
#[test]
fn at1_the_model_and_effort_reach_the_sandboxed_harness() {
    if !sandbox_available("at1_the_model_and_effort_reach_the_sandboxed_harness") {
        return;
    }
    let fixture = Fixture::new("at1-sandboxed", "result");
    let out = fixture.run_sandboxed(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("- tool: bwrap"), "{handover}");
    assert!(handover.contains("model: local/qwen"), "{handover}");
    assert!(handover.contains("effort: medium"), "{handover}");
}

// AT17 (R14): the timeout also works inside the sandbox, and the PID namespace takes the
// whole tree with it.
#[test]
fn at17_a_timeout_inside_the_sandbox_cleans_up() {
    if !sandbox_available("at17_a_timeout_inside_the_sandbox_cleans_up") {
        return;
    }
    let fixture = Fixture::with_timeout("at17-sandboxed", "hang", Some(0.03));
    let started = Instant::now();
    let out = fixture.run_sandboxed(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    assert!(started.elapsed() < Duration::from_secs(15));
    assert!(
        fixture.handover().contains("status = \"timed-out\""),
        "{}",
        fixture.handover()
    );
    assert!(
        no_process_runs(&fixture.dir.path().join("fake.sh")),
        "the sandbox tree is still running"
    );
}

/// Whether no process on the host is running `path`: inside a PID namespace the harness sees
/// its own pids, so the host is where the leftovers would be.
fn no_process_runs(path: &Path) -> bool {
    let target = path.display().to_string();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut found = false;
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                if fs::read(entry.path().join("cmdline"))
                    .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(&target))
                {
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Adds lines to the secondary profile of an existing fixture.
fn add_policy(fixture: &Fixture, extra: &str) {
    let text = fs::read_to_string(&fixture.config).unwrap();
    fs::write(&fixture.config, format!("{text}{extra}\n")).unwrap();
}

/// Writes `.dog-walker/config.toml` in the project.
fn project_config(fixture: &Fixture, text: &str) {
    let dir = fixture.project.join(".dog-walker");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), text).unwrap();
}

// AT4 (R3): a project config may tighten the policy it inherits.
#[test]
fn at4_a_project_config_can_tighten() {
    if !sandbox_available("at4_a_project_config_can_tighten") {
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    let fixture = Fixture::with_policy("at4-tight", "network", "env = [\"TEST_PORT\"]");
    project_config(&fixture, "network = false\n");
    let out = binary()
        .arg("--config")
        .arg(&fixture.config)
        .args(["run", "alpha", "Do the thing."])
        .current_dir(&fixture.project)
        .env("TEST_PORT", &port)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let handover = fixture.handover();
    assert!(handover.contains("- network: off"), "{handover}");
    // The sandbox really has no way to the listener the test holds open.
    assert!(handover.contains("local: unreachable"), "{handover}");
}

// AT4 (R3): a project config that loosens is rejected, and the field is named.
#[test]
fn at4_a_project_config_that_loosens_is_named() {
    let fixture = Fixture::with_policy("at4-loose", "result", "network = false");
    project_config(&fixture, "network = true\n");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("project config") && err.contains("network"),
        "{err}"
    );
    assert!(!fixture.tasks().exists());
}

// AT4 (R3): a project config may not drop a read-only directory that protects the project.
#[test]
fn at4_a_project_config_cannot_drop_a_protecting_directory() {
    let fixture = Fixture::new("at4-drop", "result");
    let protected = fixture.project.join("protected");
    fs::create_dir_all(&protected).unwrap();
    add_policy(
        &fixture,
        &format!("read_only_dirs = [\"{}\"]", protected.display()),
    );
    project_config(&fixture, "read_only_dirs = []\n");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("project config") && err.contains("read_only_dirs"),
        "{err}"
    );
    assert!(!fixture.tasks().exists());
}

// AT4 (R3): a project config may not add a directory the profile does not list.
#[test]
fn at4_a_project_config_cannot_add_a_directory() {
    let fixture = Fixture::new("at4-dir", "result");
    project_config(&fixture, "read_only_dirs = [\"/etc\"]\n");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("read_only_dirs"), "{}", stderr(&out));
}

// AT4 (R3): a project config that points out of the project is refused, not read.
#[test]
fn at4_a_symlinked_project_config_is_refused() {
    let fixture = Fixture::new("at4-link", "result");
    let dir = fixture.project.join(".dog-walker");
    fs::create_dir_all(&dir).unwrap();
    let target = fixture.dir.path().join("outside.toml");
    fs::write(&target, "network = false\n").unwrap();
    std::os::unix::fs::symlink(&target, dir.join("config.toml")).unwrap();
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("not a regular file"),
        "{}",
        stderr(&out)
    );
    assert!(!fixture.tasks().exists());
}

// AT4 (R3): a project config with a key that is not a policy field is refused.
#[test]
fn at4_an_unknown_project_key_is_refused() {
    let fixture = Fixture::new("at4-unknown", "result");
    project_config(&fixture, "netwrok = false\n");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("config.toml"), "{}", stderr(&out));
    assert!(!fixture.tasks().exists());
}

// AT4 (R3): without the sandbox the directories are only recorded, so a profile whose
// directories are not on this host still runs.
#[test]
fn at4_unsandboxed_does_not_mount_the_profile_directories() {
    let fixture = Fixture::new("at4-unsandboxed", "result");
    add_policy(
        &fixture,
        "read_only_dirs = [\"/nonexistent/dog-walker-dir\"]",
    );
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        fixture.handover().contains("- directories: not enforced"),
        "{}",
        fixture.handover()
    );
}

// AT4 (R3): a project config with a bad `git` value names the file.
#[test]
fn at4_a_bad_git_mode_names_the_project_config() {
    let fixture = Fixture::new("at4-git-mode", "result");
    project_config(&fixture, "git = \"sometimes\"\n");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("sometimes") && err.contains("config.toml"),
        "{err}"
    );
}

// AT4 (R3): a project config that is a FIFO is refused without being read.
#[test]
fn at4_a_fifo_project_config_is_refused() {
    let fixture = Fixture::new("at4-fifo", "result");
    let dir = fixture.project.join(".dog-walker");
    fs::create_dir_all(&dir).unwrap();
    let made = Command::new("mkfifo")
        .arg(dir.join("config.toml"))
        .output()
        .unwrap();
    assert!(made.status.success());
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("not a regular file"),
        "{}",
        stderr(&out)
    );
    assert!(!fixture.tasks().exists());
}

// AT4 (R3): a project config that is a symlinked directory is refused too.
#[test]
fn at4_a_symlinked_project_config_directory_is_refused() {
    let fixture = Fixture::new("at4-dir-link", "result");
    let outside = fixture.dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("config.toml"), "network = false\n").unwrap();
    std::os::unix::fs::symlink(&outside, fixture.project.join(".dog-walker")).unwrap();
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(!fixture.tasks().exists());
}

// AT4 (R3): a directory the request names must exist: the sandbox would fail to mount it
// after the task folder was made.
#[test]
fn at4_a_request_directory_must_exist() {
    let fixture = Fixture::new("at4-missing-dir", "result");
    project_config(
        &fixture,
        "read_only_dirs = [\"/nonexistent/dog-walker-dir\"]\n",
    );
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("is not a directory"),
        "{}",
        stderr(&out)
    );
    assert!(!fixture.tasks().exists());
}

// AT4 (R3): a directory whose canonical name carries a newline is refused, so the handover
// cannot be forged through a link.
#[test]
fn at4_a_directory_that_cannot_be_shown_is_refused() {
    let fixture = Fixture::new("at4-forge", "result");
    let evil = fixture.project.join("evil\n- network: on");
    fs::create_dir_all(&evil).unwrap();
    std::os::unix::fs::symlink(&evil, fixture.project.join("link")).unwrap();
    project_config(
        &fixture,
        &format!(
            "read_only_dirs = [\"{}\"]\n",
            fixture.project.join("link").display()
        ),
    );
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("read_only_dirs"), "{}", stderr(&out));
    assert!(!fixture.tasks().exists());
}

// AT4 (R3): a project config that is not TOML is named.
#[test]
fn at4_a_malformed_project_config_is_named() {
    let fixture = Fixture::new("at4-bad", "result");
    project_config(&fixture, "network = \n");
    let out = fixture.run(&["run", "alpha", "Do the thing."]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("config.toml"), "{}", stderr(&out));
    assert!(!fixture.tasks().exists());
}

// AT4 (R3): a task request may tighten the network, and may not loosen it.
#[test]
fn at4_a_task_request_may_only_tighten() {
    if !sandbox_available("at4_a_task_request_may_only_tighten") {
        return;
    }
    let fixture = Fixture::new("at4-task", "result");
    let out = fixture.run_sandboxed(&["run", "alpha", "Do the thing.", "--network", "off"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        fixture.handover().contains("- network: off"),
        "{}",
        fixture.handover()
    );

    let fixture = Fixture::with_policy("at4-task-loose", "result", "network = false");
    let out = fixture.run(&["run", "alpha", "Do the thing.", "--network", "on"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("task request") && err.contains("network"),
        "{err}"
    );
    assert!(!fixture.tasks().exists());

    // The same for `.git`: writable is a loosening, read-only is a tightening.
    let out = fixture.run(&["run", "alpha", "Do the thing.", "--git", "writable"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("task request") && err.contains("git"), "{err}");
    assert!(!fixture.tasks().exists());

    let writable = Fixture::with_policy("at4-task-git", "result", "git = \"writable\"");
    let out = writable.run_sandboxed(&["run", "alpha", "Do the thing.", "--git", "read-only"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        writable.handover().contains("- git: read-only"),
        "{}",
        writable.handover()
    );

    // A value the flag does not know is refused by name, not read as `off`.
    for (flag, fixture) in [("--network", "at4-task-bad"), ("--git", "at4-task-bad-git")] {
        let fixture = Fixture::new(fixture, "result");
        let out = fixture.run(&["run", "alpha", "Do the thing.", flag, "maybe"]);
        assert_eq!(out.status.code(), Some(2), "{flag}: {}", stderr(&out));
        assert!(stderr(&out).contains("maybe"), "{flag}: {}", stderr(&out));
        assert!(!fixture.tasks().exists());
    }
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
