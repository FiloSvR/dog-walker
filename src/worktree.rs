//! `run --worktree`: the task works in a git worktree of its own (D23).
//!
//! Two tasks that write the same files in one checkout overwrite each other, and nothing stops
//! them. A worktree is a second checkout of the same repository, so each task gets its own
//! copy of the committed files, and the main harness reviews and merges what it changed. The
//! worktree stays after the task, because the changes in it are the result.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::CliError;

/// A worktree made for one task.
#[derive(Debug)]
pub struct Worktree {
    /// The worktree's root, what `git worktree remove` takes.
    pub root: PathBuf,
    /// The directory the task runs in: the root, or the same subdirectory the user was in.
    pub project: PathBuf,
    /// The commit it was made at.
    pub commit: String,
}

/// Runs `git` in `dir` with the repository's own environment variables cleared, so that what is
/// asked is the repository `dir` is in.
fn git(dir: &Path, args: &[&str]) -> Result<String, CliError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .map_err(|source| CliError::Worktree {
            why: format!("cannot run git: {source}"),
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(CliError::Worktree {
            why: format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        })
    }
}

/// Makes a worktree of the repository `here` is in, detached at its HEAD, under `base`. The task
/// runs in the same place inside it that `here` is inside the repository. Repository hooks are
/// not run: the checkout is the user's request, not a reason to run a script.
pub fn create(here: &Path, base: &Path) -> Result<Worktree, CliError> {
    let top = PathBuf::from(git(here, &["rev-parse", "--show-toplevel"])?);
    let top = std::fs::canonicalize(&top).map_err(|source| CliError::Worktree {
        why: format!("cannot resolve {}: {source}", top.display()),
    })?;
    let relative = here.strip_prefix(&top).unwrap_or(Path::new(""));
    let commit = git(here, &["rev-parse", "HEAD"])?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(base)
        .map_err(|source| CliError::Worktree {
            why: format!("cannot create {}: {source}", base.display()),
        })?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let root = base.join(format!("w{}-{nanos:09}", std::process::id()));
    let root_text = root.to_string_lossy().into_owned();
    git(
        here,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "worktree",
            "add",
            "--detach",
            &root_text,
            &commit,
        ],
    )?;
    Ok(Worktree {
        project: root.join(relative),
        root,
        commit,
    })
}

/// Takes a worktree back out, after a failure before its task exists; best effort.
pub fn remove(here: &Path, worktree: &Worktree) {
    let root_text = worktree.root.to_string_lossy().into_owned();
    let _ = git(here, &["worktree", "remove", "--force", &root_text]);
}
