//! AT33 (R27): `run --worktree` gives the task a git worktree of its own.
//!
//! Each case builds a real git repository and runs the real binary against a stub of the harness
//! on `PATH`.

// Why: clippy.toml exempts `#[test]` functions from the unwrap denial, but not the helpers of an
// integration-test file; a failing helper should fail the test.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    dir: PathBuf,
    repo: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

impl Fixture {
    /// A repository with one committed file, a config with a profile `alpha` that runs the stub
    /// `codex`, and a stub that writes its result, then `$MARK` into `shared.txt`, and prints
    /// whether it is inside a work tree.
    fn new(name: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("dog-walker-wt-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        fs::write(repo.join("tracked.txt"), "committed\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "first"]);
        fs::write(repo.join("untracked.txt"), "not committed\n").unwrap();
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let stub = bin.join("codex");
        fs::write(
            &stub,
            "#!/usr/bin/env bash\n\
             printf '%s\\n' \"$PWD\" > \"$DOG_WALKER_RESULT_PATH\"\n\
             printf '%s\\n' \"${MARK:-none}\" > shared.txt\n\
             printf 'inside: %s\\n' \"$(git rev-parse --is-inside-work-tree 2>&1)\" >> \"$DOG_WALKER_RESULT_PATH\"\n\
             printf 'untracked-visible: %s\\n' \"$([ -e untracked.txt ] && echo yes || echo no)\" >> \"$DOG_WALKER_RESULT_PATH\"\n",
        )
        .unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            dir.join("config.toml"),
            "[profiles.alpha]\nharness = \"codex\"\nmodel = \"m\"\nenv = [\"MARK\"]\n\n\
             [profiles.main]\nharness = \"pi\"\nmodel = \"m\"\nmain = true\n",
        )
        .unwrap();
        Fixture { dir, repo }
    }

    fn run(&self, mark: &str, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dog-walker"))
            .arg("--config")
            .arg(self.dir.join("config.toml"))
            .args(["run", "alpha", "the brief"])
            .args(extra)
            .current_dir(&self.repo)
            .env("MARK", mark)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.dir.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn sandbox_available() -> bool {
    Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--unshare-net", "true"])
        .output()
        .is_ok_and(|out| out.status.success())
}

// AT33 (R27): the task runs in a new worktree outside the repository, at HEAD; what it writes is
// there and not in the checkout; the handover is in the worktree and says so.
#[test]
fn at33_the_task_runs_in_a_worktree_of_its_own() {
    let fixture = Fixture::new("one");
    let out = fixture.run("one", &["--worktree", "--unsandboxed"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let handover = PathBuf::from(stdout(&out));
    assert!(
        handover.starts_with(fixture.dir.join("projects")),
        "{}",
        handover.display()
    );
    let worktree = handover
        .ancestors()
        .find(|dir| dir.join(".git").is_file())
        .unwrap()
        .to_path_buf();
    assert_eq!(
        fs::read_to_string(worktree.join("shared.txt")).unwrap(),
        "one\n"
    );
    assert_eq!(
        fs::read_to_string(worktree.join("tracked.txt")).unwrap(),
        "committed\n"
    );
    // The checkout was not touched, and it did not give its uncommitted file to the task.
    assert!(!fixture.repo.join("shared.txt").exists());
    assert!(!worktree.join("untracked.txt").exists());
    assert_eq!(
        git(&fixture.repo, &["status", "--porcelain"]),
        "?? untracked.txt"
    );
    assert_eq!(git(&fixture.repo, &["worktree", "list"]).lines().count(), 2);
    let text = fs::read_to_string(&handover).unwrap();
    assert!(text.contains("- worktree:"), "{text}");
    assert!(
        text.contains(&git(&fixture.repo, &["rev-parse", "HEAD"])),
        "{text}"
    );
    assert!(
        stderr(&out).contains("git worktree remove --force"),
        "{}",
        stderr(&out)
    );
    // The worktree is the result: it stays.
    assert!(worktree.is_dir());
}

// AT33 (R27): two tasks at once write the same file and neither overwrites the other.
#[test]
fn at33_two_tasks_at_once_do_not_overwrite_each_other() {
    let fixture = Fixture::new("two");
    let first = std::thread::scope(|scope| {
        let a = scope.spawn(|| fixture.run("alpha", &["--worktree", "--unsandboxed"]));
        let b = scope.spawn(|| fixture.run("bravo", &["--worktree", "--unsandboxed"]));
        (a.join().unwrap(), b.join().unwrap())
    });
    let mut marks = Vec::new();
    for out in [&first.0, &first.1] {
        assert_eq!(out.status.code(), Some(0), "{}", stderr(out));
        let handover = PathBuf::from(stdout(out));
        let worktree = handover
            .ancestors()
            .find(|dir| dir.join(".git").is_file())
            .unwrap();
        marks.push((
            worktree.to_path_buf(),
            fs::read_to_string(worktree.join("shared.txt")).unwrap(),
        ));
    }
    assert_ne!(marks[0].0, marks[1].0);
    let mut contents: Vec<&str> = marks.iter().map(|(_, text)| text.as_str()).collect();
    contents.sort_unstable();
    assert_eq!(contents, ["alpha\n", "bravo\n"]);
    assert!(!fixture.repo.join("shared.txt").exists());
}

// AT33 (R27): outside a git repository there is nothing to make a worktree of: the run is refused
// and no task is created.
#[test]
fn at33_without_a_repository_the_run_is_refused() {
    let fixture = Fixture::new("none");
    let plain = fixture.dir.join("plain");
    fs::create_dir_all(&plain).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_dog-walker"))
        .arg("--config")
        .arg(fixture.dir.join("config.toml"))
        .args(["run", "alpha", "the brief", "--worktree", "--unsandboxed"])
        .current_dir(&plain)
        .env(
            "PATH",
            format!(
                "{}:{}",
                fixture.dir.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("GIT_CEILING_DIRECTORIES", &fixture.dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("cannot make a worktree"),
        "{}",
        stderr(&out)
    );
    assert!(!plain.join(".dog-walker").exists());
}

// AT33 (R27): in the sandbox, git works inside the worktree (the repository's `.git` is bound
// read-only) and the task still cannot change the repository.
#[test]
fn at33_git_works_inside_the_sandboxed_worktree() {
    if !sandbox_available() {
        return;
    }
    let fixture = Fixture::new("sandboxed");
    let out = fixture.run("boxed", &["--worktree"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let handover = PathBuf::from(stdout(&out));
    let result = fs::read_to_string(handover.parent().unwrap().join("result.md")).unwrap();
    assert!(result.contains("inside: true"), "{result}");
    assert!(result.contains("untracked-visible: no"), "{result}");
}
