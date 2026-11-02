//! Starting a secondary harness and waiting for it (R5).
//!
//! The harness is the profile's `command`: an argv list that dog-walker appends the brief to.
//! The task folder, the handover path, the result path, the profile, the model and the effort
//! reach it in the environment, so an adapter's command line and an argv list a user writes
//! both work without anything being spliced into a shell string.

use std::fs::File;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

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

/// Runs the harness for `task` in `project` with `prompt` as its last argument, its output in
/// the task's log, and returns how it ended. Blocks until it does.
pub fn launch(
    profile: &Profile,
    task: &Task,
    project: &Path,
    prompt: &str,
) -> Result<ExitStatus, LaunchError> {
    let argv = profile
        .command
        .as_ref()
        .ok_or_else(|| LaunchError::NoCommand {
            profile: profile.name.to_string(),
        })?;
    let (program, arguments) = argv.split_first().ok_or_else(|| LaunchError::NoCommand {
        profile: profile.name.to_string(),
    })?;
    let log = File::create(task.log_path()).map_err(|source| LaunchError::Log {
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
        .stderr(Stdio::from(log_for_errors));
    if let Some(effort) = &profile.effort {
        command.env(EFFORT, effort);
    }
    let mut child = command.spawn().map_err(|source| LaunchError::Spawn {
        program: program.into(),
        source,
    })?;
    drop(child.stdin.take());
    child.wait().map_err(|source| LaunchError::Wait { source })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
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
            timeout_minutes: None,
            command: Some(command.into_iter().map(str::to_owned).collect()),
            main: false,
        }
    }

    // The brief is the last argument, in the project directory, with output in the log.
    #[test]
    fn the_brief_is_the_last_argument_and_the_output_is_the_log() {
        let root = project("echo");
        let task = Task::create(&root, "a brief").unwrap();
        let ended = launch(&profile(vec!["/bin/echo"]), &task, &root, "the brief").unwrap();
        assert!(ended.success());
        assert_eq!(fs::read_to_string(task.log_path()).unwrap(), "the brief\n");
        fs::remove_dir_all(&root).unwrap();
    }

    // Without a command there is nothing to start.
    #[test]
    fn a_profile_without_a_command_is_an_error() {
        let root = project("no-command");
        let task = Task::create(&root, "a brief").unwrap();
        let error = launch(&profile(Vec::new()), &task, &root, "the brief").unwrap_err();
        assert!(matches!(error, LaunchError::NoCommand { .. }), "{error}");
        fs::remove_dir_all(&root).unwrap();
    }
}
