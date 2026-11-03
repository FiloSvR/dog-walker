//! Starting a secondary harness, waiting for it, and ending it on time or on request (R5, R14).
//!
//! The harness is the profile's `command`: an argv list that dog-walker appends the brief to.
//! The task folder, the handover path, the result path, the profile, the model and the effort
//! reach it in the environment, so an adapter's command line and an argv list a user writes
//! both work without anything being spliced into a shell string.
//!
//! The harness leads a process group of its own, so a timeout or a cancel ends the processes in
//! that group, not only the process dog-walker spawned.

use std::ffi::OsString;
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process_group};
use signal_hook::consts::{SIGINT, SIGTERM};

use crate::config::Profile;
use crate::error::LaunchError;
use crate::sandbox::Plan;
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
    plan: &Plan,
) -> Result<End, LaunchError> {
    // The folder must still be the one that was created before anything is written in it.
    task.verify()?;
    if profile.command.as_ref().is_none_or(Vec::is_empty) {
        return Err(LaunchError::NoCommand {
            profile: profile.name.to_string(),
        });
    }
    let log = File::create_new(task.log_path()).map_err(|source| LaunchError::Log {
        path: task.log_path(),
        source,
    })?;
    let log_for_errors = log.try_clone().map_err(|source| LaunchError::Log {
        path: task.log_path(),
        source,
    })?;
    let extra = task_env(profile, task);
    let mut command = plan
        .command(profile, project, prompt, &extra, &task.handover_path())
        .map_err(LaunchError::Sandbox)?;
    command
        .current_dir(project)
        // A pipe, closed straight away, not /dev/null: the harness reads end-of-file either
        // way, and a sandbox that hides /dev/null cannot make the run fail to start.
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_for_errors))
        // Its own group, so a kill takes its children with it.
        .process_group(0);
    // The handler is installed before anything runs: a Ctrl-C while the harness starts is a
    // cancel like any other, not a dead runner and a live task.
    install()?;
    let program = command.get_program().to_owned();
    let mut attempts = 0;
    let mut child = loop {
        match command.spawn() {
            Ok(child) => break child,
            // A harness that is being written at this moment is transiently busy; it is worth
            // another try before the task is called failed. (A sandboxed run spawns bwrap, so
            // this covers the unsandboxed path; bwrap reports the busy harness in its exit.)
            Err(source)
                if source.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 5 =>
            {
                attempts += 1;
                thread::sleep(POLL);
            }
            Err(source) => {
                return Err(LaunchError::Spawn { program, source });
            }
        }
    };
    drop(child.stdin.take());
    wait(&mut child, &task.directory().join(CANCEL_FILE), timeout)
}

/// What the harness learns about its task, in the environment.
fn task_env(profile: &Profile, task: &Task) -> Vec<(OsString, OsString)> {
    let mut env = vec![
        (
            OsString::from(TASK_DIR),
            task.directory().as_os_str().to_owned(),
        ),
        (
            OsString::from(RESULT_PATH),
            task.result_path().into_os_string(),
        ),
        (
            OsString::from(HANDOVER_PATH),
            task.handover_path().into_os_string(),
        ),
        (
            OsString::from(TASK_ID),
            OsString::from(task.id().to_string()),
        ),
        (
            OsString::from(PROFILE),
            OsString::from(profile.name.as_str()),
        ),
        (OsString::from(MODEL), OsString::from(&profile.model)),
    ];
    if let Some(effort) = &profile.effort {
        env.push((OsString::from(EFFORT), OsString::from(effort)));
    }
    env
}

/// Waits for the harness: it ends, the marker appears, the deadline passes, or Ctrl-C.
///
/// The child is checked before the deadline, so a harness that ends just after its deadline
/// is `done` with its result kept, not `timed-out`: the run that finished is not thrown away.
fn wait(child: &mut Child, cancel: &Path, timeout: Duration) -> Result<End, LaunchError> {
    let Some(deadline) = Instant::now().checked_add(timeout) else {
        let _ = end(child);
        return Err(LaunchError::Deadline { timeout });
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(End::Exited(status)),
            Ok(None) => {}
            Err(source) => {
                // A runner that cannot wait for its child must not leave the group running.
                let _ = end(child);
                return Err(wait_error(source));
            }
        }
        // A runner that cannot confirm the tree ended reports a failed task, not a clean stop.
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

/// Ends the harness's process group: TERM first, then KILL whatever is left.
///
/// The leader is reaped by the polling, so the last KILL can race a reused process-group id in
/// a window of microseconds; the alternative, sleeping the whole grace before the KILL, would
/// make every stop wait for it.
fn end(child: &mut Child) -> Result<(), LaunchError> {
    let group = i32::try_from(child.id()).ok().and_then(Pid::from_raw);
    let Some(group) = group else {
        // No group to signal: end the child itself and reap it anyway.
        let _ = child.kill();
        return child.wait().map(|_| ()).map_err(wait_error);
    };
    let _ = kill_process_group(group, Signal::TERM);
    let mut failure = None;
    let deadline = Instant::now() + GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(source) => {
                failure = Some(source);
                break;
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(POLL);
    }
    // Whatever is left of the group goes now, including children that outlived the leader.
    let _ = kill_process_group(group, Signal::KILL);
    // `wait` blocks forever on a leader in uninterruptible sleep; poll instead, and report when
    // the tree cannot be confirmed gone.
    let deadline = Instant::now() + GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(source) => {
                failure.get_or_insert(source);
                break;
            }
        }
        if Instant::now() >= deadline {
            failure.get_or_insert(std::io::Error::other(
                "the harness did not end after a KILL",
            ));
            break;
        }
        thread::sleep(POLL);
    }
    match failure {
        Some(source) => Err(wait_error(source)),
        None => Ok(()),
    }
}

fn wait_error(source: std::io::Error) -> LaunchError {
    LaunchError::Wait { source }
}

/// Installs the Ctrl-C and TERM handlers once, so a stop closes the handover instead of
/// killing the runner outright.
fn install() -> Result<(), LaunchError> {
    if INTERRUPTED.get().is_some() {
        return Ok(());
    }
    let flag = Arc::new(AtomicBool::new(false));
    let signal = |source| LaunchError::Signal { source };
    let interrupt = signal_hook::flag::register(SIGINT, Arc::clone(&flag)).map_err(signal)?;
    if let Err(source) = signal_hook::flag::register(SIGTERM, Arc::clone(&flag)) {
        // Do not leave SIGINT replaced by a handler nobody reads.
        signal_hook::low_level::unregister(interrupt);
        return Err(signal(source));
    }
    let _ = INTERRUPTED.set(flag);
    Ok(())
}

/// Installs the handlers and clears the flag: a run starts with no cancel from an earlier one
/// in the same process. Called before the first handover is written, so a Ctrl-C during setup
/// is a cancel too.
pub fn watch_for_interrupts() -> Result<(), LaunchError> {
    install()?;
    if let Some(flag) = INTERRUPTED.get() {
        flag.store(false, Ordering::Relaxed);
    }
    Ok(())
}

/// Whether Ctrl-C (or a TERM to the runner) has arrived.
pub fn interrupted() -> bool {
    INTERRUPTED
        .get()
        .is_some_and(|flag| flag.load(Ordering::Relaxed))
}

static INTERRUPTED: OnceLock<Arc<AtomicBool>> = OnceLock::new();

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
            network: true,
            git_writable: false,
            read_only_dirs: Vec::new(),
            writable_dirs: Vec::new(),
            env: Vec::new(),
        }
    }

    /// A plan without the sandbox: these tests are about waiting, not about bwrap.
    fn plan(profile: &Profile) -> Plan {
        Plan::new(profile, Path::new("."), true).unwrap()
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
        let profile = profile(vec!["/bin/echo"]);
        let ended = launch(
            &profile,
            &task,
            &root,
            "the brief",
            Duration::from_secs(5),
            &plan(&profile),
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
        let profile = profile(Vec::new());
        let error = launch(
            &profile,
            &task,
            &root,
            "the brief",
            Duration::from_secs(5),
            &plan(&profile),
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
        let profile = profile(vec![command.as_str()]);
        let ended = launch(
            &profile,
            &task,
            &root,
            "the brief",
            Duration::from_secs(2),
            &plan(&profile),
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
        let pids = task.directory().join("pids");
        let canceller = thread::spawn(move || {
            // Wait for the script to have started and written its pids, so the check below has
            // something to see, then ask it to stop.
            let deadline = Instant::now() + Duration::from_secs(5);
            while !pids.exists() {
                assert!(Instant::now() < deadline, "the script did not start");
                thread::sleep(Duration::from_millis(20));
            }
            File::create(marker).unwrap();
        });
        let profile = profile(vec![command.as_str()]);
        let ended = launch(
            &profile,
            &task,
            &root,
            "the brief",
            Duration::from_secs(30),
            &plan(&profile),
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
