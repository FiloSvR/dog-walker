//! The command line: parses arguments, runs a command, maps the outcome to an exit code.
//! This is the only place that turns an error into a message and an exit code.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::config::{self, Config};
use crate::error::{ConfigError, Error};
use crate::output::{self, exit};

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
}

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
    match execute(&cli, stdout) {
        Ok(()) => exit::SUCCESS,
        Err(error) => {
            let _ = output::line(stderr, &format!("dog-walker: {error}"));
            exit::ERROR
        }
    }
}

fn execute(cli: &Cli, stdout: &mut dyn Write) -> Result<(), Error> {
    match cli.command {
        Command::Instructions => output::line(stdout, INSTRUCTIONS.trim_end())?,
        Command::Profiles => {
            let path = match &cli.config {
                Some(path) => path.clone(),
                None => config::default_path(
                    std::env::var_os("XDG_CONFIG_HOME"),
                    std::env::var_os("HOME"),
                )
                .ok_or(ConfigError::NoConfigPath)?,
            };
            let config = Config::load(&path)?;
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
        }
    }
    Ok(())
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
    fn help_that_can_be_written_exits_0() {
        assert_eq!(
            exit_code(["dog-walker", "--help"], &mut io::sink(), &mut Full),
            exit::SUCCESS
        );
    }
}
