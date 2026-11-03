//! `doctor`: the checks a task would meet, run without a task or a model call (D17). The
//! harness itself is never started; a small system program runs in its sandbox instead.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::config::Profile;
use crate::sandbox::{Plan, Policy};

/// The address the TLS check fetches unless `--url` names another: it tests the sandbox's
/// trust store, not a provider.
pub const DEFAULT_URL: &str = "https://example.com";

/// What one check found.
#[derive(Debug)]
pub enum Outcome {
    Ok,
    /// Not run, and why.
    Skip(String),
    /// Failed: what is wrong and the fix.
    Fail(String),
}

/// One line of the report.
#[derive(Debug)]
pub struct Check {
    pub name: &'static str,
    pub outcome: Outcome,
}

impl Check {
    fn new(name: &'static str, outcome: Outcome) -> Check {
        Check { name, outcome }
    }

    /// The report line. Names and paths only, never a value.
    pub fn line(&self) -> String {
        match &self.outcome {
            Outcome::Ok => format!("ok    {}", self.name),
            Outcome::Skip(why) => format!("skip  {}: {why}", self.name),
            Outcome::Fail(why) => format!("FAIL  {}: {why}", self.name),
        }
    }

    pub fn failed(&self) -> bool {
        matches!(self.outcome, Outcome::Fail(_))
    }
}

fn fail<E: std::fmt::Display>(error: &E) -> Outcome {
    Outcome::Fail(error.to_string())
}

/// Runs every check for `profile` in `project`, reporting all of them.
///
/// `env_file` is the profile's env file when there is one, so that the fix for an unset variable
/// names it.
pub fn diagnose(
    profile: &Profile,
    project: &Path,
    url: &str,
    unsandboxed: bool,
    env_file: Option<&Path>,
) -> Vec<Check> {
    let mut checks = Vec::new();
    let policy = if unsandboxed {
        Policy::as_written(profile)
    } else {
        match Policy::checked(profile) {
            Ok(policy) => policy,
            Err(error) => {
                checks.push(Check::new("directories", fail(&error)));
                return checks;
            }
        }
    };
    checks.push(Check::new("directories", Outcome::Ok));
    let plan = match Plan::from_policy(policy, project, unsandboxed) {
        Ok(plan) => plan,
        Err(error) => {
            checks.push(Check::new("sandbox", fail(&error)));
            return checks;
        }
    };
    checks.push(Check::new(
        "sandbox",
        if unsandboxed {
            Outcome::Skip("--unsandboxed".to_owned())
        } else {
            Outcome::Ok
        },
    ));
    checks.push(Check::new(
        "harness program",
        match plan.check_program(profile, project) {
            Ok(()) => Outcome::Ok,
            Err(error) => fail(&error),
        },
    ));
    let unset = plan.unset_env();
    checks.push(Check::new(
        "environment",
        if unset.is_empty() {
            Outcome::Ok
        } else {
            let fix = env_file.map_or_else(
                || "export them in this shell".to_owned(),
                |file| {
                    format!(
                        "put the value in {}, which dog-walker loads itself",
                        crate::setup::shell_quote(&file.display().to_string())
                    )
                },
            );
            Outcome::Fail(format!(
                "not set in this shell: {}; {fix}",
                unset.join(", ")
            ))
        },
    ));
    checks.push(Check::new("temporary directory", temp(&plan, project)));
    checks.push(Check::new("tls", tls(&plan, project, url)));
    checks
}

/// A file can be created in the directory the harness would use for temporary files.
fn temp(plan: &Plan, project: &Path) -> Outcome {
    let first: Option<PathBuf> = plan.policy().writable_dirs.first().cloned();
    let Some(first) = first else {
        return Outcome::Skip("the profile lists no writable directory".to_owned());
    };
    let script = r#"d=${TMPDIR:-$1}; f=$d/.dog-walker-doctor.$$; : > "$f" && rm -f "$f""#;
    match run(plan, project, script, &[first.as_os_str()]) {
        Ok((true, _)) => Outcome::Ok,
        Ok((false, said)) => Outcome::Fail(format!(
            "cannot write in the directory the harness would use ({}); set TMPDIR to a directory \
             inside a writable_dirs entry",
            last_line(&said)
        )),
        Err(error) => Outcome::Fail(error),
    }
}

/// A TLS fetch works inside the sandbox, when the profile has a network.
fn tls(plan: &Plan, project: &Path, url: &str) -> Outcome {
    if !plan.policy().network {
        return Outcome::Skip("the profile has no network".to_owned());
    }
    let script =
        r#"command -v curl >/dev/null 2>&1 || exit 127; curl -fsS -o /dev/null --max-time 15 "$1""#;
    match run(plan, project, script, &[std::ffi::OsStr::new(url)]) {
        Ok((true, _)) => Outcome::Ok,
        Ok((false, said)) if said.contains("not found") || said.is_empty() => {
            Outcome::Skip("curl is not available in the sandbox".to_owned())
        }
        Ok((false, said)) => Outcome::Fail(format!(
            "{}; the sandbox may lack the network or a CA store (see docs/setup.md)",
            last_line(&said)
        )),
        Err(error) => Outcome::Fail(error),
    }
}

/// Runs a probe; the outcome is whether it succeeded and what it said on stderr.
fn run(
    plan: &Plan,
    project: &Path,
    script: &str,
    arguments: &[&std::ffi::OsStr],
) -> Result<(bool, String), String> {
    let mut command = plan
        .probe(project, script, arguments)
        .map_err(|error| error.to_string())?;
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot start the probe: {error}"))?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

fn last_line(text: &str) -> &str {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("it failed")
}
