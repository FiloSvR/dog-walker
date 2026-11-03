//! The command line: parses arguments, runs a command, maps the outcome to an exit code.
//! This is the only place that turns an error into a message and an exit code.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
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
use crate::setup;
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
        /// Run in a new git worktree of this repository, at its HEAD, so that tasks running at
        /// once do not write the same files; the worktree stays for you to review
        #[arg(long)]
        worktree: bool,
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
    /// Set up profiles and tell the main harness about dog-walker, asking what it needs
    Init {
        /// The harness you open yourself, as `harness[:model]`
        #[arg(long, value_name = "HARNESS[:MODEL]")]
        main: Option<String>,
        /// A secondary for tasks, as `harness[:model]`, or `no`
        #[arg(long, value_name = "HARNESS[:MODEL]|no")]
        worker: Option<String>,
        /// A secondary for reviews, as `harness[:model]`, or `no`
        #[arg(long, value_name = "HARNESS[:MODEL]|no")]
        reviewer: Option<String>,
        /// Set up this `project` (its own config and instructions file) or `global`ly; asked when
        /// not given
        #[arg(long, value_name = "project|global")]
        scope: Option<String>,
        /// Write without asking to confirm (the line then goes in the project's file)
        #[arg(long)]
        yes: bool,
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
        Command::Init {
            main,
            worker,
            reviewer,
            scope,
            yes,
        } => init(
            cli,
            [main.as_deref(), worker.as_deref(), reviewer.as_deref()],
            scope.as_deref(),
            *yes,
            stdout,
            stderr,
        ),
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
            worktree,
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
                *worktree,
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

/// Where this project's worktrees go: beside its config, outside the repository.
fn worktree_dir(cli: &Cli, project: &Path) -> Result<PathBuf, ConfigError> {
    let global = match &cli.config {
        Some(path) => path.clone(),
        None => global_config_path()?,
    };
    config::project_config_path(&global, &config::project_root(project))
        .and_then(|path| path.parent().map(|dir| dir.join("worktrees")))
        .ok_or(ConfigError::NoConfigPath)
}

/// The sandbox plan for `profile` in `project`. The profile is the policy; the project config
/// and the task may only tighten it (R3).
fn plan_for(
    profile: &config::Profile,
    project: &Path,
    unsandboxed: bool,
    request: &Request,
) -> Result<Plan, Error> {
    // Without the sandbox there is nothing to mount, so the directories are only shown.
    let policy = if unsandboxed {
        Policy::as_written(profile)
    } else {
        Policy::checked(profile)?
    };
    let policy = match config::project_config(project)? {
        Some(project_config) => {
            let origin = format!("the project config {}", project_config.path().display());
            project_config
                .request()
                .resolve(&policy, project, &origin)?
        }
        None => policy,
    };
    let policy = request.resolve(&policy, project, "the task request")?;
    let plan = Plan::from_policy(policy, project, unsandboxed)?;
    // A sandbox that cannot be set up is a config error, not a failed task.
    plan.check(profile, project)?;
    Ok(plan)
}

/// The directory the task runs in: `project`, or with `--worktree` a new worktree of it.
fn enter(
    cli: &Cli,
    project: PathBuf,
    worktree: bool,
) -> Result<(PathBuf, Option<crate::worktree::Worktree>), Error> {
    if !worktree {
        return Ok((project, None));
    }
    let made = crate::worktree::create(&project, &worktree_dir(cli, &project)?)?;
    Ok((made.project.clone(), Some(made)))
}

/// Creates the task folder in `project`. A worktree made for it is taken back out when that
/// fails: there is no task to point at it.
fn open_task(
    project: &Path,
    here: &Path,
    recorded: &str,
    worktree: Option<&crate::worktree::Worktree>,
) -> Result<Task, Error> {
    // The run directory is the runner's: a symlink there is a config error, not a task.
    let created = slots::check(project)
        .map_err(Error::from)
        .and_then(|()| Task::create(project, recorded).map_err(Error::from));
    if created.is_err()
        && let Some(made) = worktree
    {
        crate::worktree::remove(here, made);
    }
    created
}

fn log_worktree(stderr: &mut dyn Write, task: &Task, made: &crate::worktree::Worktree) {
    log(
        stderr,
        &format!(
            "dog-walker: task {} works in the worktree {} (remove it with `git worktree remove --force {}` when done)",
            task.id(),
            made.root.display(),
            crate::setup::shell_quote(&made.root.to_string_lossy())
        ),
    );
}

/// Set once the env file is loaded, so that the process started for it does not start another.
const ENV_LOADED: &str = "DOG_WALKER_ENV_LOADED";

/// The profile's env file, `<profile>.env` beside the config, when there is one.
fn env_file_of(cli: &Cli, profile: &str) -> Result<Option<PathBuf>, ConfigError> {
    Ok(config_path(cli)?
        .parent()
        .map(|dir| dir.join(format!("{profile}.env")))
        .filter(|file| file.is_file()))
}

/// Loads the profile's env file (D21): the variables it sets replace the shell's, and the rest of
/// the shell's stay. A process cannot change its own environment without `unsafe`, so this starts
/// dog-walker again, in place, with the variables set; it returns only when there is nothing to
/// load.
fn adopt_env_file(cli: &Cli, profile: &str) -> Result<(), Error> {
    if std::env::var_os(ENV_LOADED).is_some() {
        return Ok(());
    }
    let Some(file) = env_file_of(cli, profile)? else {
        return Ok(());
    };
    let env_error = |source| CliError::EnvFile {
        path: file.clone(),
        source,
    };
    let variables = crate::envfile::parse(&fs::read_to_string(&file).map_err(env_error)?);
    if variables.is_empty() {
        return Ok(());
    }
    let program = std::env::current_exe().map_err(env_error)?;
    let error = std::process::Command::new(program)
        .args(std::env::args_os().skip(1))
        .envs(variables)
        .env(ENV_LOADED, "1")
        .exec();
    Err(env_error(error).into())
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
    adopt_env_file(cli, profile.name.as_str())?;
    let env_file = env_file_of(cli, profile.name.as_str())?;
    let unsandboxed = unsandboxed || sandbox_off_by_config(cli, &config)?;
    let checks = crate::doctor::diagnose(profile, &project, url, unsandboxed, env_file.as_deref());
    for check in &checks {
        output::line(stdout, &check.line())?;
    }
    Ok(if checks.iter().any(crate::doctor::Check::failed) {
        exit::TASK_NOT_DONE
    } else {
        exit::SUCCESS
    })
}

/// Where the config is: the command line's, or the default.
fn config_path(cli: &Cli) -> Result<PathBuf, ConfigError> {
    if let Some(path) = &cli.config {
        return Ok(path.clone());
    }
    let global = global_config_path()?;
    // A project that was set up with its own config uses it, and only it (D20).
    Ok(project_config_path(&global)
        .filter(|path| path.is_file())
        .unwrap_or(global))
}

/// Whether tasks run without the sandbox because a config says so: the config in use, else the
/// global one, else no. The command line's `--unsandboxed` is separate, and only adds to this.
fn sandbox_off_by_config(cli: &Cli, config: &Config) -> Result<bool, ConfigError> {
    if let Some(sandbox) = config.sandbox() {
        return Ok(!sandbox);
    }
    if cli.config.is_some() {
        return Ok(false);
    }
    let global = global_config_path()?;
    if config_path(cli)? == global || !global.is_file() {
        return Ok(false);
    }
    Ok(Config::load(&global)?.sandbox() == Some(false))
}

fn global_config_path() -> Result<PathBuf, ConfigError> {
    config::default_path(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
    .ok_or(ConfigError::NoConfigPath)
}

/// Where this directory's own config would be, whether or not it exists.
fn project_config_path(global: &Path) -> Option<PathBuf> {
    let here = fs::canonicalize(std::env::current_dir().ok()?).ok()?;
    config::project_config_path(global, &config::project_root(&here))
}

/// Reads the config the command line points at.
fn load(cli: &Cli) -> Result<Config, ConfigError> {
    Config::load(&config_path(cli)?)
}

/// `init` (D19): asks what the command line did not say, then writes the profiles and the
/// main harness's instructions line.
fn init(
    cli: &Cli,
    [main, worker, reviewer]: [Option<&str>; 3],
    scope: Option<&str>,
    yes: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<u8, Error> {
    let configs = if let Some(path) = &cli.config {
        setup::Configs {
            global: path.clone(),
            project: path.clone(),
        }
    } else {
        let global = global_config_path()?;
        let project = project_config_path(&global).ok_or(ConfigError::NoConfigPath)?;
        setup::Configs { global, project }
    };
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or(ConfigError::NoConfigPath)?;
    let host = setup::Host {
        home,
        config_dir: PathBuf::new(),
        path: std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect(),
        project: fs::canonicalize(std::env::current_dir().map_err(|source| {
            crate::error::SetupError::Io {
                path: PathBuf::from("."),
                source,
            }
        })?)
        .map_err(|source| crate::error::SetupError::Io {
            path: PathBuf::from("."),
            source,
        })?,
    };
    let options = setup::Options {
        main: main.map(str::to_owned),
        worker: worker.map(str::to_owned),
        reviewer: reviewer.map(str::to_owned),
        scope: scope.map(str::to_owned),
        hide: std::io::IsTerminal::is_terminal(&io::stdin()),
        yes,
    };
    setup::run_init(
        &options,
        &configs,
        &host,
        &mut io::stdin().lock(),
        stdout,
        stderr,
    )?;
    Ok(exit::SUCCESS)
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
    worktree: bool,
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
    adopt_env_file(cli, profile.name.as_str())?;
    let brief = brief(task, file)?;
    // From here on a Ctrl-C is a cancel that closes the handover, not a dead runner; before a
    // task exists, the default disposition is the right one.
    launch::watch_for_interrupts()?;
    let project =
        fs::canonicalize(std::env::current_dir().map_err(|source| CliError::Project { source })?)
            .map_err(|source| CliError::Project { source })?;
    let unsandboxed = unsandboxed || sandbox_off_by_config(cli, &config)?;
    let plan = plan_for(profile, &project, unsandboxed, request)?;
    // With `--worktree` the task gets a checkout of its own, made only now that the setup has
    // been checked: a refusal above leaves nothing behind (D23).
    let here = project.clone();
    let (project, worktree) = enter(cli, project, worktree)?;
    // What the record keeps is redacted, down to the folder's name: the harness reads the
    // brief as written, and the task folder is dog-walker's (R18).
    let secrets = plan.policy().secret_values();
    let recorded = sandbox::redact(&brief, &secrets);
    let task = open_task(&project, &here, &recorded, worktree.as_ref())?;
    let handover_path = task.handover_path();
    // The policy section lists directories, which could contain a secret value; it is
    // redacted like the rest of what dog-walker keeps (R18).
    let sandbox = sandbox::redact(&plan.policy().render(plan.sandboxed()), &secrets);
    let sandbox = match &worktree {
        // The task runs there, not in the checkout the user is in; the changes are in it.
        Some(made) => format!(
            "{sandbox}\n- worktree: {} (detached at {}; the uncommitted changes of the checkout it was made from are not in it)",
            made.root.display(),
            made.commit
        ),
        None => sandbox,
    };
    let mut handover = Handover::new(&task.id(), &profile.name, &recorded).with_sandbox(&sandbox);
    save(&handover, &task)?;
    // The handover exists, so its path goes out now: whatever happens to the task from here,
    // stdout holds it (R6). The log lines are best effort: a closed stderr is no reason to
    // abandon a task.
    output::path(stdout, &handover_path)?;
    log(stderr, &format!("dog-walker: task {} created", task.id()));
    if let Some(made) = &worktree {
        log_worktree(stderr, &task, made);
    }
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
