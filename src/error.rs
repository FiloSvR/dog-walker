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
    #[error("`limits` key `{key}` is not `harness/model` with a known harness")]
    LimitKey { key: String },
    #[error("`limits` key `{key}` is 0: a limit has to allow at least one task")]
    LimitZero { key: String },
    #[error("exactly one profile must be main, but these are: {names}")]
    ManyMain { names: String },
    #[error("cannot read the project config {}: {source}", path.display())]
    ReadProjectConfig { path: PathBuf, source: io::Error },
    #[error("the project config {} is larger than a policy file should be", path.display())]
    ProjectConfigTooLarge { path: PathBuf },
    #[error("the project config {} is not a regular file inside the project", path.display())]
    UnsafeProjectConfig { path: PathBuf },
    #[error("invalid project config {}:\n{source}", path.display())]
    ProjectConfig {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("no profile named `{name}`: `profiles` lists them")]
    UnknownProfile { name: String },
    #[error("`{profile}` is the main profile: pass --allow-main to delegate to it")]
    MainProfile { profile: String },
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
    #[error(transparent)]
    Adapter(#[from] AdapterError),
    #[error("cannot install the signal handlers: {source}")]
    Signal { source: io::Error },
    #[error("the timeout {timeout:?} does not fit a deadline")]
    Deadline { timeout: std::time::Duration },
    #[error(transparent)]
    Task(#[from] TaskError),
    #[error(transparent)]
    Sandbox(#[from] SandboxError),
    #[error("cannot use the log {}: {source}", path.display())]
    Log { path: PathBuf, source: io::Error },
    #[error("cannot start the harness `{}`: {source}", program.to_string_lossy())]
    Spawn {
        program: OsString,
        source: io::Error,
    },
    #[error("cannot wait for the harness: {source}")]
    Wait { source: io::Error },
}

/// Why a project or task setting was refused: it would loosen the policy it inherits.
#[derive(Debug, thiserror::Error)]
pub enum OverrideError {
    #[error("{origin} cannot loosen `{field}`: {detail}")]
    Loosens {
        origin: String,
        field: &'static str,
        detail: String,
    },
    #[error("{origin}: `{value}` is not a `git` mode (read-only or writable)")]
    Git { origin: String, value: String },
}

/// Why a harness could not be started: no command, and no adapter for it yet.
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error(
        "profile `{profile}` has no `command`, and the `{harness}` adapter is not written yet; \
         set one, or follow the issue for that harness"
    )]
    Missing {
        profile: String,
        harness: crate::config::Harness,
    },
    #[error("the adapter for `{harness}` returned no argv at all")]
    Empty { harness: crate::config::Harness },
}

/// Why the sandbox could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error(transparent)]
    Adapter(#[from] AdapterError),
    #[error(
        "bwrap is not installed: install bubblewrap, or pass --unsandboxed to run without a sandbox"
    )]
    NoTool,
    #[error("cannot resolve the harness `{program}`: {source}")]
    Program { program: String, source: io::Error },
    #[error("cannot mount {}: {source}", path.display())]
    Dir { path: PathBuf, source: io::Error },
    #[error("`{name}` is in the policy's environment and is not set")]
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
    #[error("the read-only directory {} contains the project: it would make the run unwritable", path.display())]
    AboveProject { path: PathBuf },
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
    #[error(
        "a task cannot start tasks: this run is inside one (`{variable}` names a task, or a \
         dog-walker is above it in the process tree); run it from your own shell"
    )]
    Nested { variable: &'static str },
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

/// Why a running slot could not be taken.
#[derive(Debug, thiserror::Error)]
pub enum SlotError {
    #[error("cannot use the run directory {}: {source}", path.display())]
    Dir { path: PathBuf, source: io::Error },
    #[error("cannot write the run record {}: {source}", path.display())]
    Record { path: PathBuf, source: io::Error },
    #[error("the run directory {} is not a directory of its own", path.display())]
    UnsafeDir { path: PathBuf },
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
    Adapter(#[from] AdapterError),
    #[error(transparent)]
    Slot(#[from] SlotError),
    #[error(transparent)]
    Override(#[from] OverrideError),
    #[error(transparent)]
    Handover(#[from] HandoverError),
    #[error(transparent)]
    Cli(#[from] CliError),
    #[error(transparent)]
    Launch(#[from] LaunchError),
}
