//! Starting a secondary harness, waiting for it, and ending it on time or on request (R5, R14).
//!
//! The harness is the profile's `command`: an argv list that dog-walker appends the brief to.
//! The task folder, the handover path, the result path, the profile, the model and the effort
//! reach it in the environment, so an adapter's command line and an argv list a user writes
//! both work without anything being spliced into a shell string.
//!
//! The harness leads a process group of its own, so a timeout or a cancel ends everything it
//! started, not only the process dog-walker spawned.

use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process_group};
use signal_hook::consts::{SIGINT, SIGTERM};

use crate::config::Profile;
use crate::error::LaunchError;
use crate::task::Task;

/// The task folder the harness may write in.
pub const TASK_DIR: &str = "DOG_WALKER_TASK_DIR";
/// Where the harness writes its result.
pub const RESULT_PATH: &str = "DOG_WALKER_RESULT_PATH";
/// The handover the main harness reads; the secondary must not write it.
pub const HANDOVER_PATH: &str = "DOG_WALKER_HANDOVER_PATH";
/// The number of the task, as printed in the handover.
pub const TASK_ID: &str = "DOG_WALKER_TASK_ID";
/// The profile's name.
pub const PROFILE: &str = "DOG_WALKER_PROFILE";
/// The profile's model.
pub const MODEL: &str = "DOG_WALKER_MODEL";
/// The profile's thinking effort, when it has one.
pub const EFFORT: &str = "DOG_WALKER_EFFORT";

/// The file `cancel` creates in a task folder, and `run` watches for.
pub const CANCEL_FILE: &str = ".cancel";

/// How often the runner looks: this is how long a cancel or a timeout takes to be noticed.
const POLL: Duration = Duration::from_millis(50);

/// How long the process tree gets to end after TERM before it gets KILL.
const GRACE: Duration = Duration::from_secs(2);

/// How a task ended, as the runner saw it.
#[derive(Debug)]
pub enum End {
    /// The harness ended by itself.
    Exited(ExitStatus),
    /// The deadline passed: the tree was killed.
    TimedOut,
    /// Ctrl-C, or a cancel from another shell: the tree was killed.
    Cancelled,
}

/// Runs the harness for `task` in `project` with `prompt` as its last argument and its output
/// in the task's log. Blocks until it ends, its deadline passes, or it is cancelled.
pub fn launch(
    profile: &Profile,
    task: &Task,
    project: &Path,
    prompt: &str,
    timeout: Duration,
) -> Result<End, LaunchError> {
    // The folder must still be the one that was created before anything is written in it.
    task.verify()?;
    let argv = profile
        .command
        .as_ref()
        .ok_or_else(|| LaunchError::NoCommand {
            profile: profile.name.to_string(),
        })?;
    let (program, arguments) = argv.split_first().ok_or_else(|| LaunchError::NoCommand {
        profile: profile.name.to_string(),
    })?;
    // `create_new`: the task folder is new, and a link planted at the log's name would send
    // the harness's output wherever it points.
    let log = File::create_new(task.log_path()).map_err(|source| LaunchError::Log {
        path: task.log_path(),
        source,
    })?;
    let log_for_errors = log.try_clone().map_err(|source| LaunchError::Log {
        path: task.log_path(),
        source,
    })?;
    let mut command = Command::new(program);
    command
        .args(arguments)
        .arg(prompt)
        .current_dir(project)
        .env(TASK_DIR, task.directory())
        .env(RESULT_PATH, task.result_path())
        .env(HANDOVER_PATH, task.handover_path())
        .env(TASK_ID, task.id().to_string())
        .env(PROFILE, profile.name.as_str())
        .env(MODEL, &profile.model)
        .env_remove(EFFORT)
        // A pipe, closed straight away, not /dev/null: the harness reads end-of-file either
        // way, and a sandbox that hides /dev/null cannot make the run fail to start.
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_for_errors))
        // Its own group, so a kill takes its children with it.
        .process_group(0);
    if let Some(effort) = &profile.effort {
        command.env(EFFORT, effort);
    }
    // The handler is installed before anything runs: a Ctrl-C while the harness starts is a
    // cancel like any other, not a dead runner and a live task.
    let _ = interrupted();
    let mut child = command.spawn().map_err(|source| LaunchError::Spawn {
        program: program.into(),
        source,
    })?;
    drop(child.stdin.take());
    wait(&mut child, &task.directory().join(CANCEL_FILE), timeout)
}

/// Waits for the harness: it ends, the marker appears, the deadline passes, or Ctrl-C.
fn wait(child: &mut Child, cancel: &Path, timeout: Duration) -> Result<End, LaunchError> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(wait_error)? {
            return Ok(End::Exited(status));
        }
        if cancel.exists() || interrupted() {
            end(child)?;
            return Ok(End::Cancelled);
        }
        if Instant::now() >= deadline {
            end(child)?;
            return Ok(End::TimedOut);
        }
        thread::sleep(POLL);
    }
}

/// Ends the harness's whole process group: TERM first, KILL when it does not go.
fn end(child: &mut Child) -> Result<(), LaunchError> {
    // A pid that does not fit an `i32` cannot be a process group; there is nothing to kill.
    let Ok(group) = i32::try_from(child.id()) else {
        return Ok(());
    };
    let Some(group) = Pid::from_raw(group) else {
        return Ok(());
    };
    let _ = kill_process_group(group, Signal::TERM);
    let deadline = Instant::now() + GRACE;
    while child.try_wait().map_err(wait_error)?.is_none() {
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(POLL);
    }
    // Whatever is left of the group goes now, including children that outlived the leader.
    let _ = kill_process_group(group, Signal::KILL);
    let _ = child.wait().map_err(wait_error)?;
    Ok(())
}

fn wait_error(source: std::io::Error) -> LaunchError {
    LaunchError::Wait { source }
}

/// Whether Ctrl-C (or a TERM to the runner) has arrived. Installs the handlers on first use.
fn interrupted() -> bool {
    static INTERRUPTED: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    INTERRUPTED
        .get_or_init(|| {
            let flag = Arc::new(AtomicBool::new(false));
            let _ = signal_hook::flag::register(SIGINT, Arc::clone(&flag));
            let _ = signal_hook::flag::register(SIGTERM, Arc::clone(&flag));
            flag
        })
        .load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use crate::config::{Harness, ProfileName};

    fn project(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("dog-walker-launch-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn profile(command: Vec<&str>) -> Profile {
        Profile {
            name: ProfileName::parse("alpha").unwrap(),
            harness: Harness::Pi,
            model: "local/model".to_owned(),
            effort: Some("high".to_owned()),
            timeout: None,
            command: Some(command.into_iter().map(str::to_owned).collect()),
            main: false,
        }
    }

    /// A script that runs `body` and can be a profile's command.
    fn script(root: &Path, name: &str, body: &str) -> String {
        let path = root.join(name);
        let mut file = fs::File::create(&path).unwrap();
        // `io::copy`, not `write_all`: the architecture tripwire keeps that spelling for the
        // output module and the handover.
        std::io::copy(&mut body.as_bytes(), &mut file).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.to_str().unwrap().to_owned()
    }

    /// The tree the script leaves behind: the harness and a child that must die with it.
    const HANGING: &str =
        "#!/bin/sh\nsleep 30 &\necho \"$$ $!\" > \"$DOG_WALKER_TASK_DIR/pids\"\nwait\n";

    fn gone(pid: &str) -> bool {
        let path = PathBuf::from(format!("/proc/{pid}"));
        let deadline = Instant::now() + Duration::from_secs(3);
        while path.exists() {
            assert!(Instant::now() < deadline, "{pid} is still there");
            thread::sleep(Duration::from_millis(20));
        }
        true
    }

    // The brief is the last argument, in the project directory, with output in the log.
    #[test]
    fn the_brief_is_the_last_argument_and_the_output_is_the_log() {
        let root = project("echo");
        let task = Task::create(&root, "a brief").unwrap();
        let ended = launch(
            &profile(vec!["/bin/echo"]),
            &task,
            &root,
            "the brief",
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(matches!(ended, End::Exited(status) if status.success()));
        assert_eq!(fs::read_to_string(task.log_path()).unwrap(), "the brief\n");
        fs::remove_dir_all(&root).unwrap();
    }

    // Without a command there is nothing to start.
    #[test]
    fn a_profile_without_a_command_is_an_error() {
        let root = project("no-command");
        let task = Task::create(&root, "a brief").unwrap();
        let error = launch(
            &profile(Vec::new()),
            &task,
            &root,
            "the brief",
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(matches!(error, LaunchError::NoCommand { .. }), "{error}");
        fs::remove_dir_all(&root).unwrap();
    }

    // A deadline ends the harness and the child it started.
    #[test]
    fn a_timeout_ends_the_whole_tree() {
        let root = project("timeout");
        let task = Task::create(&root, "a brief").unwrap();
        let command = script(&root, "hang.sh", HANGING);
        let ended = launch(
            &profile(vec![command.as_str()]),
            &task,
            &root,
            "the brief",
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(matches!(ended, End::TimedOut), "{ended:?}");
        let pids = fs::read_to_string(task.directory().join("pids")).unwrap();
        for pid in pids.split_whitespace() {
            assert!(gone(pid), "{pid}");
        }
        fs::remove_dir_all(&root).unwrap();
    }

    // A cancel from another shell is noticed and ends the tree.
    #[test]
    fn a_cancel_marker_ends_the_whole_tree() {
        let root = project("cancel");
        let task = Task::create(&root, "a brief").unwrap();
        let command = script(&root, "hang.sh", HANGING);
        let marker = task.directory().join(CANCEL_FILE);
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            File::create(marker).unwrap();
        });
        let ended = launch(
            &profile(vec![command.as_str()]),
            &task,
            &root,
            "the brief",
            Duration::from_secs(30),
        )
        .unwrap();
        canceller.join().unwrap();
        assert!(matches!(ended, End::Cancelled), "{ended:?}");
        let pids = fs::read_to_string(task.directory().join("pids")).unwrap();
        for pid in pids.split_whitespace() {
            assert!(gone(pid), "{pid}");
        }
        fs::remove_dir_all(&root).unwrap();
    }
}
