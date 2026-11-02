//! The OS-level sandbox: bwrap, and only bwrap (D2, R15, R16).
//!
//! The harness sees the project, the system's programs and libraries, and nothing else: no
//! home directory, no `/var`, no host `/tmp`. Inside, the project is writable, `.git` is
//! read-only unless the profile says otherwise, and the network is there unless it is turned
//! off. The environment is cleared and rebuilt from the profile's list plus `PATH`.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Profile;
use crate::error::SandboxError;

/// The parts of the host a sandboxed process needs in order to run at all, read-only.
const SYSTEM: [&str; 6] = ["/bin", "/sbin", "/usr", "/lib", "/lib32", "/lib64"];

/// The environment a profile's harness gets, and the handover records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub network: bool,
    pub git_writable: bool,
    pub read_only_dirs: Vec<PathBuf>,
    pub writable_dirs: Vec<PathBuf>,
    pub env: Vec<String>,
}

impl Policy {
    /// What the handover says about the run, one bullet per line.
    pub fn render(&self, sandboxed: bool) -> String {
        let list = |paths: &[PathBuf]| {
            if paths.is_empty() {
                "none".to_owned()
            } else {
                paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        };
        let mut names = self.env.clone();
        if names.is_empty() {
            names.push("PATH".to_owned());
        }
        format!(
            "- tool: {}\n- network: {}\n- git: {}\n- writable dirs: {}\n- read-only dirs: {}\n- environment: {}",
            if sandboxed { "bwrap" } else { "none" },
            if self.network { "on" } else { "off" },
            if self.git_writable {
                "writable"
            } else {
                "read-only"
            },
            list(&self.writable_dirs),
            list(&self.read_only_dirs),
            names.join(", "),
        )
    }
}

impl From<&Profile> for Policy {
    fn from(profile: &Profile) -> Policy {
        Policy {
            network: profile.network,
            git_writable: profile.git_writable,
            read_only_dirs: profile.read_only_dirs.clone(),
            writable_dirs: profile.writable_dirs.clone(),
            env: profile.env.clone(),
        }
    }
}

/// What a run does about the sandbox.
#[derive(Debug)]
pub struct Plan {
    sandboxed: bool,
    policy: Policy,
}

impl Plan {
    /// The plan for `profile`: a sandbox unless one was waived or bwrap is missing.
    pub fn new(profile: &Profile, unsandboxed: bool) -> Result<Plan, SandboxError> {
        let sandboxed = if unsandboxed {
            false
        } else if available() {
            true
        } else {
            return Err(SandboxError::NoTool);
        };
        Ok(Plan {
            sandboxed,
            policy: Policy::from(profile),
        })
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn sandboxed(&self) -> bool {
        self.sandboxed
    }

    /// Checks that the sandbox can be set up, before a task exists: the program resolves, the
    /// extra directories exist, and the variables the profile lists are set.
    pub fn check(&self, profile: &Profile) -> Result<(), SandboxError> {
        if !self.sandboxed {
            return Ok(());
        }
        let argv = profile
            .command
            .as_ref()
            .ok_or_else(|| SandboxError::NoCommand {
                profile: profile.name.to_string(),
            })?;
        let (program, _) = argv.split_first().ok_or_else(|| SandboxError::NoCommand {
            profile: profile.name.to_string(),
        })?;
        resolve_program(program)?;
        for dir in self
            .policy
            .read_only_dirs
            .iter()
            .chain(&self.policy.writable_dirs)
        {
            canonical(dir)?;
        }
        self.env()?;
        Ok(())
    }

    /// The command for the harness: the profile's argv with the brief last, under bwrap when
    /// the run is sandboxed, with the environment the harness gets.
    pub fn command(
        &self,
        profile: &Profile,
        project: &Path,
        prompt: &str,
        extra: &[(OsString, OsString)],
        handover: &Path,
    ) -> Result<Command, SandboxError> {
        let argv = profile
            .command
            .as_ref()
            .ok_or_else(|| SandboxError::NoCommand {
                profile: profile.name.to_string(),
            })?;
        let (program, arguments) = argv.split_first().ok_or_else(|| SandboxError::NoCommand {
            profile: profile.name.to_string(),
        })?;
        if !self.sandboxed {
            let mut command = Command::new(program);
            command.args(arguments).arg(prompt);
            for (name, value) in extra {
                command.env(name, value);
            }
            command.env_remove(crate::launch::EFFORT);
            return Ok(command);
        }
        let program = resolve_program(program)?;
        let mut command = Command::new("bwrap");
        self.sandbox_args(&mut command, project, &program, extra, handover)?;
        command.arg(&program).args(arguments).arg(prompt);
        Ok(command)
    }

    /// The mounts, namespaces, directory and environment bwrap sets up before the program.
    fn sandbox_args(
        &self,
        command: &mut Command,
        project: &Path,
        program: &Path,
        extra: &[(OsString, OsString)],
        handover: &Path,
    ) -> Result<(), SandboxError> {
        for dir in SYSTEM {
            let path = Path::new(dir);
            if path.exists() {
                command.arg("--ro-bind").arg(path).arg(path);
            }
        }
        // A program outside the project or the system directories is bound by itself, so the
        // rest of its directory stays out of the sandbox.
        if !covers(program, project) {
            command.arg("--ro-bind").arg(program).arg(program);
        }
        command.arg("--proc").arg("/proc");
        command.arg("--dev").arg("/dev");
        command.arg("--bind").arg(project).arg(project);
        let git = project.join(".git");
        if !self.policy.git_writable && git.exists() {
            command.arg("--ro-bind").arg(&git).arg(&git);
        }
        // The handover belongs to dog-walker: the harness may read it, not rewrite its status
        // or brief (R10, R12).
        if handover.exists() {
            command.arg("--ro-bind").arg(handover).arg(handover);
        }
        for dir in &self.policy.read_only_dirs {
            let dir = canonical(dir)?;
            command.arg("--ro-bind").arg(&dir).arg(&dir);
        }
        for dir in &self.policy.writable_dirs {
            let dir = canonical(dir)?;
            command.arg("--bind").arg(&dir).arg(&dir);
        }
        if !self.policy.network {
            command.arg("--unshare-net");
        }
        // Everything not mounted above is an empty directory in a writable root; a write there
        // would "succeed" into nothing. Read-only makes it fail, as the policy says.
        command.arg("--remount-ro").arg("/");
        command.arg("--die-with-parent");
        command.arg("--chdir").arg(project);
        command.arg("--clearenv");
        for (name, value) in self.env()?.iter().chain(extra) {
            command.arg("--setenv").arg(name).arg(value);
        }
        Ok(())
    }

    /// The variables the profile passes through, `PATH` always among them.
    fn env(&self) -> Result<Vec<(OsString, OsString)>, SandboxError> {
        let mut names = self.policy.env.clone();
        if !names.iter().any(|name| name == "PATH") {
            names.push("PATH".to_owned());
        }
        let mut pairs = Vec::with_capacity(names.len());
        for name in names {
            match env::var_os(&name) {
                Some(value) => pairs.push((OsString::from(&name), value)),
                // A `PATH` that is not set still has to name the system's programs.
                None if name == "PATH" => pairs.push((
                    OsString::from(&name),
                    OsString::from("/usr/local/bin:/usr/bin:/bin"),
                )),
                None => return Err(SandboxError::MissingEnv { name }),
            }
        }
        Ok(pairs)
    }
}

/// Whether bwrap can be run at all.
pub fn available() -> bool {
    Command::new("bwrap")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// The program as an absolute path inside the sandbox: a bare name is looked up on `PATH`.
fn resolve_program(program: &str) -> Result<PathBuf, SandboxError> {
    let path = Path::new(program);
    let found = if path.is_absolute() {
        Some(path.to_owned())
    } else {
        env::split_paths(&env::var_os("PATH").unwrap_or_default())
            .map(|dir| dir.join(program))
            .find(|candidate| is_executable(candidate))
    };
    let found = found.ok_or_else(|| SandboxError::Program {
        program: program.to_owned(),
        source: io::Error::new(io::ErrorKind::NotFound, "no such program"),
    })?;
    fs::canonicalize(&found).map_err(|source| SandboxError::Program {
        program: program.to_owned(),
        source,
    })
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// Whether `path` is inside `project` or one of the system directories.
fn covers(path: &Path, project: &Path) -> bool {
    path.starts_with(project)
        || SYSTEM
            .iter()
            .any(|dir| Path::new(dir).exists() && path.starts_with(dir))
}

fn canonical(path: &Path) -> Result<PathBuf, SandboxError> {
    fs::canonicalize(path).map_err(|source| SandboxError::Dir {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::{Harness, ProfileName};

    fn profile() -> Profile {
        Profile {
            name: ProfileName::parse("alpha").unwrap(),
            harness: Harness::Pi,
            model: "m".to_owned(),
            effort: None,
            timeout: None,
            command: Some(vec!["/bin/echo".to_owned()]),
            main: false,
            network: true,
            git_writable: false,
            read_only_dirs: Vec::new(),
            writable_dirs: Vec::new(),
            env: Vec::new(),
        }
    }

    fn project(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("dog-walker-sandbox-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::canonicalize(root).unwrap()
    }

    #[test]
    fn the_policy_lists_what_the_handover_says() {
        let mut profile = profile();
        profile.network = false;
        profile.writable_dirs = vec![PathBuf::from("/cache")];
        profile.env = vec!["HOME".to_owned()];
        let text = Policy::from(&profile).render(true);
        assert!(text.contains("- tool: bwrap"), "{text}");
        assert!(text.contains("- network: off"), "{text}");
        assert!(text.contains("- git: read-only"), "{text}");
        assert!(text.contains("- writable dirs: /cache"), "{text}");
        assert!(text.contains("- environment: HOME"), "{text}");
        assert!(
            Policy::from(&profile)
                .render(false)
                .contains("- tool: none")
        );
    }

    #[test]
    fn an_unsandboxed_plan_is_the_command_itself() {
        let plan = Plan::new(&profile(), true).unwrap();
        assert!(!plan.sandboxed());
        let command = plan
            .command(
                &profile(),
                &project("unsandboxed"),
                "the brief",
                &[],
                Path::new("/none"),
            )
            .unwrap();
        assert_eq!(command.get_program(), "/bin/echo");
        let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
        assert_eq!(args, ["the brief"]);
    }

    #[test]
    fn the_environment_is_the_profile_list_and_path() {
        let mut profile = profile();
        profile.env = vec!["HOME".to_owned()];
        let plan = Plan::new(&profile, false).unwrap();
        if !available() {
            return;
        }
        // `HOME` is set in this test's environment, so both names resolve.
        let pairs = plan.env().unwrap();
        let names: Vec<String> = pairs
            .iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["HOME", "PATH"]);
    }

    #[test]
    fn a_listed_variable_that_is_not_set_is_named() {
        let mut profile = profile();
        profile.env = vec!["DOG_WALKER_TEST_UNSET_VARIABLE_XYZ".to_owned()];
        let plan = Plan::new(&profile, false).unwrap();
        if !available() {
            return;
        }
        let error = plan.env().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("DOG_WALKER_TEST_UNSET_VARIABLE_XYZ")
        );
    }

    #[test]
    fn a_read_only_git_is_mounted_over_the_project_and_network_can_be_cut() {
        if !available() {
            return;
        }
        let project = project("git");
        fs::create_dir_all(project.join(".git")).unwrap();
        let mut profile = profile();
        profile.network = false;
        let plan = Plan::new(&profile, false).unwrap();
        let command = plan
            .command(&profile, &project, "the brief", &[], Path::new("/none"))
            .unwrap();
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let git = project.join(".git").display().to_string();
        assert!(args.contains(&"--unshare-net".to_owned()), "{args:?}");
        assert!(args.contains(&"--remount-ro".to_owned()), "{args:?}");
        assert!(
            args.windows(3).any(|w| w == ["--ro-bind", &git, &git]),
            "{args:?}"
        );
        fs::remove_dir_all(&project).unwrap();
    }

    #[test]
    fn a_sandboxed_plan_starts_with_bwrap_and_ends_with_the_prompt() {
        if !available() {
            return;
        }
        let plan = Plan::new(&profile(), false).unwrap();
        assert!(plan.sandboxed());
        let project = project("bwrap");
        let command = plan
            .command(&profile(), &project, "the brief", &[], Path::new("/none"))
            .unwrap();
        assert_eq!(command.get_program(), "bwrap");
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.first().unwrap(), "--ro-bind");
        assert!(args.contains(&"--clearenv".to_owned()), "{args:?}");
        assert!(args.contains(&"--setenv".to_owned()), "{args:?}");
        assert_eq!(args.last().unwrap(), "the brief");
        assert!(args.contains(&"--bind".to_owned()), "{args:?}");
        fs::remove_dir_all(&project).unwrap();
    }
}
