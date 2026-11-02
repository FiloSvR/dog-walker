//! Error types. Each module owns its enum; only `cli` turns them into exit codes.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

use crate::status::Status;

/// Why a config file could not be loaded, or a profile could not be chosen from it.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no config file: pass --config, or set XDG_CONFIG_HOME or HOME")]
    NoConfigPath,
    #[error("cannot read config {}: {source}", path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("invalid config {}:\n{source}", path.display())]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("config {} defines no profiles", path.display())]
    NoProfiles { path: PathBuf },
    #[error("profile `{profile}`: {reason}")]
    Profile { profile: String, reason: String },
    #[error("exactly one profile must be main, but none is: set `main = true` on one")]
    NoMain,
    #[error("exactly one profile must be main, but these are: {names}")]
    ManyMain { names: String },
    #[error("no profile named `{name}`: `profiles` lists them")]
    UnknownProfile { name: String },
    #[error("`{profile}` is the main profile: pass --allow-main to delegate to it")]
    MainProfile { profile: String },
    #[error("profile `{profile}` has no `command` to run (adapters land in #10)")]
    NoCommand { profile: String },
}

/// Why a task could not change status.
#[derive(Debug, thiserror::Error)]
pub enum StatusError {
    #[error("a task in state `{from}` cannot become `{to}`")]
    Transition { from: Status, to: Status },
    #[error("`{to}` is not an end state")]
    NotAnEnd { to: Status },
}

/// Why a task folder could not be created or read.
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("cannot read the tasks directory {}: {source}", path.display())]
    ReadDir { path: PathBuf, source: io::Error },
    #[error("cannot create the task folder {}: {source}", path.display())]
    Create { path: PathBuf, source: io::Error },
    #[error("no task id left: the counter is exhausted")]
    Exhausted,
    #[error("refusing to write through the symbolic link {}", path.display())]
    Symlink { path: PathBuf },
    #[error("the task folder {} was moved or replaced: refusing to touch it", path.display())]
    Moved { path: PathBuf },
    #[error("cannot inspect {}: {source}", path.display())]
    Inspect { path: PathBuf, source: io::Error },
}

/// Why the handover could not be written.
#[derive(Debug, thiserror::Error)]
pub enum HandoverError {
    #[error("cannot write the handover {}: {source}", path.display())]
    Write { path: PathBuf, source: io::Error },
}

/// Why the harness could not be started or waited for.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("cannot install the signal handlers: {source}")]
    Signal { source: io::Error },
    #[error("the timeout {timeout:?} does not fit a deadline")]
    Deadline { timeout: std::time::Duration },
    #[error(transparent)]
    Task(#[from] TaskError),
    #[error(transparent)]
    Sandbox(#[from] SandboxError),
    #[error("profile `{profile}` has no `command` to run")]
    NoCommand { profile: String },
    #[error("cannot open the log {}: {source}", path.display())]
    Log { path: PathBuf, source: io::Error },
    #[error("cannot start the harness `{}`: {source}", program.to_string_lossy())]
    Spawn {
        program: OsString,
        source: io::Error,
    },
    #[error("cannot wait for the harness: {source}")]
    Wait { source: io::Error },
}

/// Why the sandbox could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error(
        "bwrap is not installed: install bubblewrap, or pass --unsandboxed to run without a sandbox"
    )]
    NoTool,
    #[error("profile `{profile}` has no `command` to sandbox")]
    NoCommand { profile: String },
    #[error("cannot resolve the harness `{program}`: {source}")]
    Program { program: String, source: io::Error },
    #[error("cannot mount {}: {source}", path.display())]
    Dir { path: PathBuf, source: io::Error },
    #[error("`{name}` is listed in the profile's `env` and is not set")]
    MissingEnv { name: String },
    #[error("bwrap cannot create a sandbox here: {reason}; pass --unsandboxed to run without one")]
    NoSandbox { reason: String },
    #[error("the directory {} is not a directory", path.display())]
    NotDir { path: PathBuf },
    #[error("a profile cannot list `/` itself; list the directories it needs")]
    RootDir,
    #[error("a profile cannot mount the system directory {}", path.display())]
    SystemDir { path: PathBuf },
    #[error("the directory {} is listed as both read-only and writable", path.display())]
    Overlap { path: PathBuf },
    #[error("the directory {} has a character the handover cannot list", path.display())]
    Unprintable { path: PathBuf },
    #[error("bwrap inside the project cannot be trusted: {}", path.display())]
    BwrapInsideProject { path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("the task is empty: pass the brief as text, with --file, or `-` for stdin")]
    EmptyTask,
    #[error("cannot read the task from {}: {source}", path.display())]
    TaskFile { path: PathBuf, source: io::Error },
    #[error("cannot read the task from stdin: {source}")]
    Stdin { source: io::Error },
    #[error("cannot find the project directory: {source}")]
    Project { source: io::Error },
    #[error("`{id}` is not a task id: it is the number in a task folder's name")]
    BadTaskId { id: String },
    #[error("no task {id} in .dog-walker/tasks")]
    NoTask { id: String },
    #[error("cannot read the handover {}: {source}", path.display())]
    ReadHandover { path: PathBuf, source: io::Error },
    #[error("cannot ask the task to stop: {}: {source}", path.display())]
    Cancel { path: PathBuf, source: io::Error },
}

/// Everything that can make a command fail.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("cannot write output: {0}")]
    Output(#[from] io::Error),
    #[error(transparent)]
    Status(#[from] StatusError),
    #[error(transparent)]
    Task(#[from] TaskError),
    #[error(transparent)]
    Sandbox(#[from] SandboxError),
    #[error(transparent)]
    Handover(#[from] HandoverError),
    #[error(transparent)]
    Cli(#[from] CliError),
    #[error(transparent)]
    Launch(#[from] LaunchError),
}
