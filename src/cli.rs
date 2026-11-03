//! The command line: parses arguments, runs a command, maps the outcome to an exit code.
//! This is the only place that turns an error into a message and an exit code.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, ExitStatus};
use std::time::Duration;

use clap::{Parser, Subcommand};

use crate::adapter;
use crate::config::{self, Config, Profile};
use crate::error::{CliError, ConfigError, Error};
use crate::handover::{self, Handover};
use crate::launch::{self, End};
use crate::output::{self, exit};
use crate::sandbox::{self, Plan, Policy, Request};
use crate::slots;
use crate::status::Status;
use crate::task::{Task, TaskId};
use crate::usage::Usage;

const INSTRUCTIONS: &str = include_str!("instructions.md");

#[derive(Debug, Parser)]
#[command(
    name = "dog-walker",
    version,
    about = "Delegate tasks to secondary LLM harnesses"
)]
struct Cli {
    /// Config file (default: $XDG_CONFIG_HOME/dog-walker/config.toml or ~/.config/dog-walker/config.toml)
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List the profiles, the main one first
    Profiles,
    /// Print guidance for the main harness on delegating a task
    Instructions,
    /// Run a task on a secondary harness and wait for it
    Run {
        /// The profile to run the task on
        profile: String,
        /// The brief, or `-` to read it from stdin
        #[arg(value_name = "TASK", conflicts_with = "file")]
        task: Option<String>,
        /// Read the brief from a file
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
        /// Allow delegating to the main profile
        #[arg(long)]
        allow_main: bool,
        /// Run without the sandbox (bwrap): only when the user asks for it
        #[arg(long)]
        unsandboxed: bool,
        /// Ask for a stricter network setting than the profile's
        #[arg(long, value_name = "on|off", value_parser = ["on", "off"])]
        network: Option<String>,
        /// Ask for a stricter `.git` than the profile's
        #[arg(long, value_name = "read-only|writable", value_parser = ["read-only", "writable"])]
        git: Option<String>,
    },
    /// Check that a profile's harness can run, without starting a task or the harness
    Doctor {
        /// The profile to check
        profile: String,
        /// The address the TLS check fetches
        #[arg(long, value_name = "URL", default_value = crate::doctor::DEFAULT_URL)]
        url: String,
        /// Check without the sandbox
        #[arg(long)]
        unsandboxed: bool,
    },
    /// Ask a running task to stop
    Cancel {
        /// The task id, the number in its folder name
        id: String,
    },
}

/// A task runs for half an hour unless its profile says otherwise.
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(30);

/// Runs dog-walker with `args` (including the program name) and returns the exit code.
pub fn run<I, T>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    ExitCode::from(exit_code(args, stdout, stderr))
}

fn exit_code<I, T>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            // Help and version go to stdout with code 0; usage errors to stderr with code 2.
            let text = error.render().to_string();
            let (target, code): (&mut dyn Write, u8) = if error.use_stderr() {
                (stderr, exit::ERROR)
            } else {
                (stdout, exit::SUCCESS)
            };
            return if output::text(target, &text).is_ok() {
                code
            } else {
                exit::ERROR
            };
        }
    };
    match execute(&cli, stdout, stderr) {
        Ok(code) => code,
        Err(error) => {
            let _ = output::line(stderr, &format!("dog-walker: {error}"));
            exit::ERROR
        }
    }
}

fn execute(cli: &Cli, stdout: &mut dyn Write, stderr: &mut dyn Write) -> Result<u8, Error> {
    match &cli.command {
        Command::Instructions => {
            output::line(stdout, INSTRUCTIONS.trim_end())?;
            Ok(exit::SUCCESS)
        }
        Command::Profiles => {
            let config = load(cli)?;
            list(&config, stdout)?;
            Ok(exit::SUCCESS)
        }
        Command::Run {
            profile,
            task,
            file,
            allow_main,
            unsandboxed,
            network,
            git,
        } => {
            let request = Request {
                network: network.as_deref().map(|value| value == "on"),
                git: git.clone(),
                ..Request::default()
            };
            run_task(
                cli,
                profile,
                task.as_deref(),
                file.as_deref(),
                *allow_main,
                *unsandboxed,
                &request,
                stdout,
                stderr,
            )
        }
        Command::Cancel { id } => cancel(id, stdout),
        Command::Doctor {
            profile,
            url,
            unsandboxed,
        } => doctor(cli, profile, url, *unsandboxed, stdout),
    }
}

/// Prints one line per check; exit 1 when any failed.
fn doctor(
    cli: &Cli,
    profile_name: &str,
    url: &str,
    unsandboxed: bool,
    stdout: &mut dyn Write,
) -> Result<u8, Error> {
    let config = load(cli)?;
    let profile = config
        .profiles()
        .iter()
        .find(|profile| profile.name.as_str() == profile_name)
        .ok_or_else(|| ConfigError::UnknownProfile {
            name: profile_name.to_owned(),
        })?;
    let project =
        fs::canonicalize(std::env::current_dir().map_err(|source| CliError::Project { source })?)
            .map_err(|source| CliError::Project { source })?;
    let checks = crate::doctor::diagnose(profile, &project, url, unsandboxed);
    for check in &checks {
        output::line(stdout, &check.line())?;
    }
    Ok(if checks.iter().any(crate::doctor::Check::failed) {
        exit::TASK_NOT_DONE
    } else {
        exit::SUCCESS
    })
}

/// Reads the config the command line points at.
fn load(cli: &Cli) -> Result<Config, ConfigError> {
    let path = match &cli.config {
        Some(path) => path.clone(),
        None => config::default_path(
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        )
        .ok_or(ConfigError::NoConfigPath)?,
    };
    Config::load(&path)
}

/// One row per profile: name, harness, model, effort, role, separated by tabs.
fn list(config: &Config, stdout: &mut dyn Write) -> io::Result<()> {
    for profile in config.profiles() {
        let role = if profile.main { "main" } else { "secondary" };
        let row = format!(
            "{}\t{}\t{}\t{}\t{role}",
            profile.name,
            profile.harness,
            profile.model,
            profile.effort.as_deref().unwrap_or("-"),
        );
        output::line(stdout, &row)?;
    }
    Ok(())
}

/// Creates the task, runs the harness, finalizes the handover and prints its path. The exit
/// code is 0 only when the task ended `done`.
// Why: the runner threads the command line, the flag and both streams through; a struct would
// only move the same fields one indirection further.
#[allow(clippy::too_many_arguments)]
fn run_task(
    cli: &Cli,
    profile_name: &str,
    task: Option<&str>,
    file: Option<&Path>,
    allow_main: bool,
    unsandboxed: bool,
    request: &Request,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<u8, Error> {
    // A task is not a delegator (R8): a `run` from inside one stops here, before a brief is
    // read or a task exists, and the task that spawned it goes on. Two signals: the marker the
    // harness gets, and a dog-walker ancestor, which a harness cannot clear from its own
    // environment. A marker that points nowhere is a stray export, not a task.
    if inside_a_task() {
        return Err(CliError::Nested {
            variable: launch::TASK_DIR,
        }
        .into());
    }
    let config = load(cli)?;
    let profile = config
        .profiles()
        .iter()
        .find(|profile| profile.name.as_str() == profile_name)
        .ok_or_else(|| ConfigError::UnknownProfile {
            name: profile_name.to_owned(),
        })?;
    if profile.main && !allow_main {
        return Err(ConfigError::MainProfile {
            profile: profile.name.to_string(),
        }
        .into());
    }
    // A profile without a `command` runs its harness's adapter (R4).
    adapter::argv(profile)?;
    let brief = brief(task, file)?;
    // From here on a Ctrl-C is a cancel that closes the handover, not a dead runner; before a
    // task exists, the default disposition is the right one.
    launch::watch_for_interrupts()?;
    let project =
        fs::canonicalize(std::env::current_dir().map_err(|source| CliError::Project { source })?)
            .map_err(|source| CliError::Project { source })?;
    // The profile is the policy; the project config and the task may only tighten it (R3).
    // Without the sandbox there is nothing to mount, so the directories are only shown.
    let policy = if unsandboxed {
        Policy::as_written(profile)
    } else {
        Policy::checked(profile)?
    };
    let policy = match config::project_config(&project)? {
        Some(project_config) => {
            let origin = format!("the project config {}", project_config.path().display());
            project_config
                .request()
                .resolve(&policy, &project, &origin)?
        }
        None => policy,
    };
    let policy = request.resolve(&policy, &project, "the task request")?;
    let plan = Plan::from_policy(policy, &project, unsandboxed)?;
    // A sandbox that cannot be set up is a config error, not a failed task.
    plan.check(profile, &project)?;
    // The run directory is the runner's: a symlink there is a config error, not a task.
    slots::check(&project)?;
    // What the record keeps is redacted, down to the folder's name: the harness reads the
    // brief as written, and the task folder is dog-walker's (R18).
    let secrets = plan.policy().secret_values();
    let recorded = sandbox::redact(&brief, &secrets);
    let task = Task::create(&project, &recorded)?;
    let handover_path = task.handover_path();
    // The policy section lists directories, which could contain a secret value; it is
    // redacted like the rest of what dog-walker keeps (R18).
    let sandbox = sandbox::redact(&plan.policy().render(plan.sandboxed()), &secrets);
    let mut handover = Handover::new(&task.id(), &profile.name, &recorded).with_sandbox(&sandbox);
    save(&handover, &task)?;
    // The handover exists, so its path goes out now: whatever happens to the task from here,
    // stdout holds it (R6). The log lines are best effort: a closed stderr is no reason to
    // abandon a task.
    output::path(stdout, &handover_path)?;
    log(stderr, &format!("dog-walker: task {} created", task.id()));
    // A slot per running task: the limit is per harness and model, and the slot's record is
    // what tells the next runner that this task is writing the project too (R9).
    let limit = config.limit(profile);
    let Some(slot) = take_slot(&project, profile, &task, limit, &mut handover, stderr)? else {
        return Ok(exit::TASK_NOT_DONE);
    };
    for other in slot.overlaps() {
        log(
            stderr,
            &format!(
                "dog-walker: warning: task {} overlaps task {other}'s write scope (the project)",
                task.id()
            ),
        );
    }
    handover.start()?;
    save(&handover, &task)?;
    log(
        stderr,
        &format!(
            "dog-walker: task {} running on `{}`",
            task.id(),
            profile.name
        ),
    );
    let timeout = profile.timeout.unwrap_or(DEFAULT_TIMEOUT);
    let prompt = launch::prompt(&brief);
    let (status, outcome) = match launch::launch(profile, &task, &project, &prompt, timeout, &plan)
    {
        Ok(End::Exited(ended)) => outcome(&task, ended),
        Ok(End::TimedOut) => (
            Status::TimedOut,
            format!("The task timed out after {}.", duration_text(timeout)),
        ),
        Ok(End::Cancelled) => (Status::Cancelled, "The task was cancelled.".to_owned()),
        Err(error) => (Status::Failed, format!("The task failed: {error}.")),
    };
    let usage = usage_of(profile, &task);
    handover.record_usage(usage);
    handover.end(status, &sandbox::redact(&outcome, &secrets))?;
    save(&handover, &task)?;
    // Only now that the end state is on disk: a cancel that arrives later reads it and backs
    // off, and one that arrived earlier finds nothing to do.
    let _ = fs::remove_file(task.directory().join(launch::CANCEL_FILE));
    log(stderr, &ended_line(&task, status, usage));
    Ok(if status == Status::Done {
        exit::SUCCESS
    } else {
        exit::TASK_NOT_DONE
    })
}

/// The run's last log line: the end state, and what the harness reported it used.
fn ended_line(task: &Task, status: Status, usage: Option<Usage>) -> String {
    let spent = usage.map_or_else(String::new, |usage| format!(" ({})", usage.summary()));
    format!("dog-walker: task {} {status}{spent}", task.id())
}

/// What the task's harness reported it used. The log was redacted when the run ended, so what
/// is read from it holds no secret.
fn usage_of(profile: &Profile, task: &Task) -> Option<Usage> {
    let log = fs::read(task.log_path()).ok()?;
    adapter::usage(profile, &String::from_utf8_lossy(&log))
}

/// Takes the run's slot; `None` when the wait was cut short, and then the task is ended as
/// cancelled before its harness ever started.
fn take_slot(
    project: &Path,
    profile: &Profile,
    task: &Task,
    limit: Option<usize>,
    handover: &mut Handover,
    stderr: &mut dyn Write,
) -> Result<Option<slots::Slot>, Error> {
    let cancel = task.directory().join(launch::CANCEL_FILE);
    let mut waiting = || {
        log(
            stderr,
            &format!(
                "dog-walker: task {} waiting for a slot (at most {} at once)",
                task.id(),
                limit.unwrap_or_default()
            ),
        );
    };
    let slot = slots::acquire(
        project,
        profile,
        limit,
        &task.id().to_string(),
        &|| launch::interrupted() || cancel.exists(),
        &mut waiting,
    )?;
    if slot.is_none() {
        handover.cancel_before_start("The task was cancelled while it waited for a slot.")?;
        save(handover, task)?;
        // The end state is on disk, so the marker has done its work.
        let _ = std::fs::remove_file(&cancel);
        log(
            stderr,
            &format!("dog-walker: task {} cancelled in the queue", task.id()),
        );
    }
    Ok(slot)
}

/// Whether this process runs inside a task: the marker names a directory, or an ancestor is a
/// dog-walker.
fn inside_a_task() -> bool {
    let marker = std::env::var_os(launch::TASK_DIR).is_some_and(|dir| Path::new(&dir).is_dir());
    if marker {
        return true;
    }
    let mut pid = std::process::id();
    for _ in 0..16 {
        let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
            return false;
        };
        let Some(parent) = status
            .lines()
            .find_map(|line| line.strip_prefix("PPid:"))
            .and_then(|value| value.trim().parse::<u32>().ok())
        else {
            return false;
        };
        if parent == 0 {
            return false;
        }
        let comm = std::fs::read_to_string(format!("/proc/{parent}/comm")).unwrap_or_default();
        if comm.trim() == "dog-walker" {
            return true;
        }
        pid = parent;
    }
    false
}

/// Saves the handover, once the task folder is still the one that was created.
fn save(handover: &Handover, task: &Task) -> Result<(), Error> {
    task.verify()?;
    handover.save(&task.handover_path())?;
    Ok(())
}

/// A timeout as it reads in the handover.
fn duration_text(timeout: Duration) -> String {
    const MINUTE: Duration = Duration::from_secs(60);
    if timeout >= MINUTE && timeout.subsec_nanos() == 0 && timeout.as_secs().is_multiple_of(60) {
        let minutes = timeout.as_secs() / 60;
        return if minutes == 1 {
            "1 minute".to_owned()
        } else {
            format!("{minutes} minutes")
        };
    }
    if timeout == Duration::from_secs(1) {
        return "1 second".to_owned();
    }
    format!("{} seconds", timeout.as_secs_f64())
}

/// Asks a running task to stop, from another shell: a marker the runner watches for. A task
/// that has already ended is left exactly as it is.
fn cancel(id: &str, stdout: &mut dyn Write) -> Result<u8, Error> {
    let id = TaskId::parse(id.trim()).ok_or_else(|| CliError::BadTaskId { id: id.to_owned() })?;
    let project = std::env::current_dir().map_err(|source| CliError::Project { source })?;
    let task = Task::find(&project, id)?.ok_or_else(|| CliError::NoTask { id: id.to_string() })?;
    task.verify()?;
    if let Some(status) = finished_status(&task)? {
        output::line(stdout, &format!("dog-walker: task {id} already {status}"))?;
        return Ok(exit::SUCCESS);
    }
    task.verify()?;
    let marker = task.directory().join(launch::CANCEL_FILE);
    match fs::File::create_new(&marker) {
        Ok(_) => {}
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
        Err(source) => {
            return Err(CliError::Cancel {
                path: marker,
                source,
            }
            .into());
        }
    }
    // The runner may have closed the task between the read and the marker: take the request
    // back rather than leave a marker in a finished task.
    match finished_status(&task) {
        Ok(None) => {}
        Ok(Some(status)) => {
            let _ = fs::remove_file(&marker);
            output::line(stdout, &format!("dog-walker: task {id} already {status}"))?;
            return Ok(exit::SUCCESS);
        }
        Err(error) => {
            let _ = fs::remove_file(&marker);
            return Err(error.into());
        }
    }
    output::line(
        stdout,
        &format!("dog-walker: cancel requested for task {id}"),
    )?;
    Ok(exit::SUCCESS)
}

/// The end status in a task's handover, if it has one. A missing handover, or one that is not
/// a regular file, is not an end: reading it must not follow a link or block on a pipe.
fn finished_status(task: &Task) -> Result<Option<Status>, CliError> {
    let path = task.handover_path();
    let before = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(CliError::ReadHandover { path, source }),
    };
    if !before.file_type().is_file() {
        return Ok(None);
    }
    let mut file = fs::File::open(&path).map_err(|source| CliError::ReadHandover {
        path: path.clone(),
        source,
    })?;
    let opened = file.metadata().map_err(|source| CliError::ReadHandover {
        path: path.clone(),
        source,
    })?;
    if !opened.is_file() || opened.dev() != before.dev() || opened.ino() != before.ino() {
        return Ok(None);
    }
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|source| CliError::ReadHandover { path, source })?;
    Ok(handover::front_matter(&text)
        .and_then(|front| Status::parse(&front.status))
        .filter(|status| status.is_end()))
}

/// One line of log on stderr, if it can be written.
fn log(stderr: &mut dyn Write, line: &str) {
    let _ = output::line(stderr, line);
}

/// The brief: the text given, `-` for stdin, or a file. Empty is an error.
fn brief(task: Option<&str>, file: Option<&Path>) -> Result<String, CliError> {
    let text = match (task, file) {
        (Some("-"), _) => {
            let mut text = String::new();
            io::stdin()
                .read_to_string(&mut text)
                .map_err(|source| CliError::Stdin { source })?;
            text
        }
        (Some(text), _) => text.to_owned(),
        (None, Some(path)) => fs::read_to_string(path).map_err(|source| CliError::TaskFile {
            path: path.to_owned(),
            source,
        })?,
        (None, None) => return Err(CliError::EmptyTask),
    };
    if text.trim().is_empty() {
        return Err(CliError::EmptyTask);
    }
    Ok(text)
}

/// How the task ended: `done` only when the harness ended well and left a good result.
fn outcome(task: &Task, ended: ExitStatus) -> (Status, String) {
    match read_result(task) {
        Ok(result) if ended.success() => (Status::Done, result),
        Ok(result) => (
            Status::Failed,
            format!("The task failed: the harness ended with {ended}.\n\n{result}"),
        ),
        Err(reason) => (
            Status::Failed,
            format!("The task failed: {reason} (the harness ended with {ended})."),
        ),
    }
}

/// The harness's result, or why there is none. A missing, unreadable, empty or non-UTF-8
/// `result.md` never becomes an invented result (R13).
///
/// The result must be a regular file: a symbolic link is a way out of the task folder, and a
/// FIFO would block the read. The inode is compared with the one before opening, so a file
/// swapped in between is refused rather than read.
fn read_result(task: &Task) -> Result<String, String> {
    task.verify().map_err(|error| error.to_string())?;
    let path = task.result_path();
    let before = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Err("the harness left no result".to_owned());
        }
        Err(source) => return Err(format!("the result cannot be read ({source})")),
    };
    if !before.file_type().is_file() {
        return Err("the result is not a regular file".to_owned());
    }
    let mut file =
        fs::File::open(&path).map_err(|source| format!("the result cannot be read ({source})"))?;
    let opened = file
        .metadata()
        .map_err(|source| format!("the result cannot be read ({source})"))?;
    if !opened.is_file() || opened.dev() != before.dev() || opened.ino() != before.ino() {
        return Err("the result changed while it was read".to_owned());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| format!("the result cannot be read ({source})"))?;
    match String::from_utf8(bytes) {
        Ok(text) if !text.trim().is_empty() => Ok(text),
        Ok(_) => Err("the harness left an empty result".to_owned()),
        Err(_) => Err("the result is not valid UTF-8".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    /// A stream that refuses every write, like a full disk.
    struct Full;

    impl Write for Full {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("full"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn help_that_cannot_be_written_exits_2() {
        assert_eq!(
            exit_code(["dog-walker", "--help"], &mut Full, &mut io::sink()),
            exit::ERROR
        );
    }

    #[test]
    fn a_usage_error_that_cannot_be_written_exits_2() {
        assert_eq!(
            exit_code(["dog-walker", "bogus"], &mut io::sink(), &mut Full),
            exit::ERROR
        );
    }

    #[test]
    fn a_result_that_cannot_be_written_exits_2() {
        assert_eq!(
            exit_code(["dog-walker", "instructions"], &mut Full, &mut io::sink()),
            exit::ERROR
        );
    }

    #[test]
    fn durations_read_as_seconds_or_minutes() {
        for (timeout, text) in [
            (Duration::from_secs(1), "1 second"),
            (Duration::from_secs(60), "1 minute"),
            (Duration::from_secs(90), "90 seconds"),
            (Duration::from_mins(30), "30 minutes"),
            (Duration::from_millis(1800), "1.8 seconds"),
        ] {
            assert_eq!(duration_text(timeout), text, "{timeout:?}");
        }
    }

    #[test]
    fn the_default_timeout_is_half_an_hour() {
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_mins(30));
    }

    #[test]
    fn help_that_can_be_written_exits_0() {
        assert_eq!(
            exit_code(["dog-walker", "--help"], &mut io::sink(), &mut Full),
            exit::SUCCESS
        );
    }
}
