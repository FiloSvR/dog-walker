//! The OS-level sandbox: bwrap, and only bwrap (D2, R15, R16).
//!
//! The harness sees the system's programs and libraries, the files a networked process needs,
//! the project, and the directories the profile lists: nothing else. `/tmp`, the home
//! directory and the rest of the host are absent, the root is read-only, and the harness runs
//! in a PID namespace of its own, so nothing it detaches outlives the sandbox.

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
const SYSTEM: [&str; 5] = ["/bin", "/sbin", "/usr", "/lib", "/lib64"];

/// Filesystems a profile may not mount at all, or under: they are the kernel's and the
/// sandbox's own.
const FORBIDDEN_ANY: [&str; 3] = ["/proc", "/sys", "/dev"];

/// Directories a profile may not mount itself: the runtime's and the host's scratch space.
const FORBIDDEN_EXACT: [&str; 2] = ["/run", "/tmp"];

/// The system files a networked harness needs: names, certificates and the loader's cache.
const SYSTEM_FILES: [&str; 8] = [
    "/etc/resolv.conf",
    "/etc/hosts",
    "/etc/nsswitch.conf",
    "/etc/ssl",
    "/etc/ca-certificates",
    "/etc/ld.so.cache",
    "/etc/localtime",
    // The trust store on Fedora and RHEL: `/etc/ssl/certs` there points into it.
    "/etc/pki/ca-trust",
];

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
    /// The policy the profile asks for, with its directories canonical and checked.
    fn checked(profile: &Profile) -> Result<Policy, SandboxError> {
        let read_only_dirs = directories(&profile.read_only_dirs)?;
        let writable_dirs = directories(&profile.writable_dirs)?;
        for dir in &read_only_dirs {
            if writable_dirs.contains(dir) {
                return Err(SandboxError::Overlap { path: dir.clone() });
            }
        }
        Ok(Policy {
            network: profile.network,
            git_writable: profile.git_writable,
            read_only_dirs,
            writable_dirs,
            env: profile.env.clone(),
        })
    }

    /// The policy as written, for a run that does not apply it.
    fn as_written(profile: &Profile) -> Policy {
        Policy {
            network: profile.network,
            git_writable: profile.git_writable,
            read_only_dirs: profile.read_only_dirs.clone(),
            writable_dirs: profile.writable_dirs.clone(),
            env: profile.env.clone(),
        }
    }

    /// What the handover says about the run, one bullet per line.
    pub fn render(&self, sandboxed: bool) -> String {
        if !sandboxed {
            return "- tool: none (--unsandboxed)\n- network: not enforced\n- git: not enforced\n- directories: not enforced\n- environment: inherited, not cleaned".to_owned();
        }
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
        if !names.iter().any(|name| name == "PATH") {
            names.push("PATH".to_owned());
        }
        format!(
            "- tool: bwrap\n- network: {}\n- git: {}\n- writable dirs: {}\n- read-only dirs: {}\n- environment: {}",
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

/// What a run does about the sandbox.
#[derive(Debug)]
pub struct Plan {
    sandboxed: bool,
    policy: Policy,
    bwrap: Option<PathBuf>,
}

impl Plan {
    /// The plan for `profile`: a sandbox unless one was waived, and refused when there is no
    /// usable bwrap.
    pub fn new(profile: &Profile, project: &Path, unsandboxed: bool) -> Result<Plan, SandboxError> {
        if unsandboxed {
            return Ok(Plan {
                sandboxed: false,
                policy: Policy::as_written(profile),
                bwrap: None,
            });
        }
        Ok(Plan {
            sandboxed: true,
            policy: Policy::checked(profile)?,
            bwrap: Some(resolve_bwrap(project)?),
        })
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn sandboxed(&self) -> bool {
        self.sandboxed
    }

    /// Checks what can be checked before a task exists: the harness program resolves and is
    /// executable, and, when sandboxed, the variables the profile lists are set.
    pub fn check(&self, profile: &Profile, project: &Path) -> Result<(), SandboxError> {
        let argv = profile
            .command
            .as_ref()
            .ok_or_else(|| SandboxError::NoCommand {
                profile: profile.name.to_string(),
            })?;
        let (program, _) = argv.split_first().ok_or_else(|| SandboxError::NoCommand {
            profile: profile.name.to_string(),
        })?;
        resolve_program(program, project)?;
        if self.sandboxed {
            self.env()?;
        }
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
            // An inherited `DOG_WALKER_EFFORT` must not survive a profile without one; the
            // task's own variables are applied after, so this cannot remove its value.
            command.env_remove(crate::launch::EFFORT);
            command.args(arguments).arg(prompt);
            for (name, value) in extra {
                command.env(name, value);
            }
            return Ok(command);
        }
        let program = resolve_program(program, project)?;
        let bwrap = self.bwrap.as_ref().ok_or(SandboxError::NoTool)?;
        // No environment of the runner's: with a PID namespace of its own, bwrap is PID 1 and
        // the harness could read `/proc/1/environ`.
        let mut command = Command::new(bwrap);
        command.env_clear();
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
        // Name resolution, TLS roots and the loader's cache, without the rest of `/etc`.
        for file in SYSTEM_FILES {
            let path = Path::new(file);
            if let Ok(target) = fs::canonicalize(path) {
                command.arg("--ro-bind").arg(&target).arg(path);
            }
        }
        // A program outside the project or the system directories is bound by itself, so the
        // rest of its directory stays out of the sandbox.
        if !covers(program, project) {
            command.arg("--ro-bind").arg(program).arg(program);
        }
        command.arg("--proc").arg("/proc");
        command.arg("--dev").arg("/dev");
        // The profile's writable directories are mounted before the project, so one that
        // contains it cannot undo the project bind or its protections.
        for dir in &self.policy.writable_dirs {
            command.arg("--bind").arg(dir).arg(dir);
        }
        command.arg("--bind").arg(project).arg(project);
        // A read-only directory is mounted after the project: a part of the project the
        // profile makes read-only really is read-only.
        for dir in &self.policy.read_only_dirs {
            command.arg("--ro-bind").arg(dir).arg(dir);
        }
        // `.git` read-only, but only while it really is inside the project: a link that
        // escapes is not followed into the sandbox.
        let git = project.join(".git");
        if !self.policy.git_writable
            && git.exists()
            && fs::canonicalize(&git).is_ok_and(|target| target.starts_with(project))
        {
            command.arg("--ro-bind").arg(&git).arg(&git);
        }
        // The handover belongs to dog-walker: the harness may read it, not rewrite its status
        // or brief (R10, R12).
        if handover.exists() {
            command.arg("--ro-bind").arg(handover).arg(handover);
        }
        if !self.policy.network {
            command.arg("--unshare-net");
        }
        // A PID namespace of its own: the host's processes are not visible, and when bwrap
        // (PID 1 in it) ends, everything the harness detached ends with the namespace.
        command.arg("--unshare-pid");
        command.arg("--unshare-uts");
        command.arg("--unshare-ipc");
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

/// Whether bwrap runs and can make a sandbox here.
fn resolve_bwrap(project: &Path) -> Result<PathBuf, SandboxError> {
    let bwrap = resolve_program("bwrap", project).map_err(|_| SandboxError::NoTool)?;
    // A bwrap the project can write is not a sandbox: a previous harness could have left it.
    if bwrap.starts_with(project) {
        return Err(SandboxError::BwrapInsideProject { path: bwrap });
    }
    let probe = Command::new(&bwrap)
        .args(["--ro-bind", "/", "/", "--unshare-net", "true"])
        .env_clear()
        .output();
    match probe {
        Ok(output) if output.status.success() => Ok(bwrap),
        Ok(output) => Err(SandboxError::NoSandbox {
            reason: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        }),
        Err(_) => Err(SandboxError::NoTool),
    }
}

/// The program as an absolute, executable path: a bare name is looked up on `PATH`, a name
/// with a separator in it is a path, relative to the project the harness runs in.
fn resolve_program(program: &str, project: &Path) -> Result<PathBuf, SandboxError> {
    let path = Path::new(program);
    let found = if path.is_absolute() {
        Some(path.to_owned())
    } else if program.contains('/') {
        Some(project.join(path))
    } else {
        env::split_paths(&env::var_os("PATH").unwrap_or_default())
            .map(|dir| dir.join(program))
            .find(|candidate| is_executable(candidate))
    };
    let found = found.ok_or_else(|| SandboxError::Program {
        program: program.to_owned(),
        source: io::Error::new(io::ErrorKind::NotFound, "no such program"),
    })?;
    let found = fs::canonicalize(&found).map_err(|source| SandboxError::Program {
        program: program.to_owned(),
        source,
    })?;
    if !is_executable(&found) {
        return Err(SandboxError::Program {
            program: program.to_owned(),
            source: io::Error::new(io::ErrorKind::PermissionDenied, "not executable"),
        });
    }
    Ok(found)
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// The profile's directories, canonical and checked: real directories, not the root and not
/// one of the system's own mount points.
fn directories(given: &[PathBuf]) -> Result<Vec<PathBuf>, SandboxError> {
    let mut canonical = Vec::with_capacity(given.len());
    for dir in given {
        let path = fs::canonicalize(dir).map_err(|source| SandboxError::Dir {
            path: dir.clone(),
            source,
        })?;
        if !fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
            return Err(SandboxError::NotDir { path });
        }
        if path == Path::new("/") {
            return Err(SandboxError::RootDir);
        }
        if is_system(&path) {
            return Err(SandboxError::SystemDir { path });
        }
        // The handover lists the canonical path; a name that carries a newline or a bidi
        // control there could forge the record.
        if path
            .to_string_lossy()
            .chars()
            .any(crate::config::is_unsafe_char)
        {
            return Err(SandboxError::Unprintable { path });
        }
        canonical.push(path);
    }
    Ok(canonical)
}

/// Whether `path` is one of the system directories the sandbox mounts itself.
fn is_system(path: &Path) -> bool {
    SYSTEM
        .iter()
        .map(Path::new)
        .chain(FORBIDDEN_ANY.iter().map(Path::new))
        .any(|dir| path == dir || path.starts_with(dir))
        || FORBIDDEN_EXACT.iter().map(Path::new).any(|dir| path == dir)
}

/// Whether `path` is inside `project` or one of the system directories.
fn covers(path: &Path, project: &Path) -> bool {
    path.starts_with(project)
        || SYSTEM
            .iter()
            .any(|dir| Path::new(dir).exists() && path.starts_with(dir))
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

    /// A sandbox the test can build: skipped silently when bwrap cannot run here.
    fn plan(profile: &Profile, project: &Path) -> Option<Plan> {
        Plan::new(profile, project, false).ok()
    }

    #[test]
    fn the_policy_lists_what_the_handover_says() {
        let mut profile = profile();
        profile.network = false;
        profile.env = vec!["HOME".to_owned()];
        let project = project("render");
        let shared = project.join("shared");
        fs::create_dir_all(&shared).unwrap();
        profile.writable_dirs = vec![shared.clone()];
        let Some(plan) = plan(&profile, &project) else {
            return;
        };
        let text = plan.policy().render(true);
        assert!(text.contains("- tool: bwrap"), "{text}");
        assert!(text.contains("- network: off"), "{text}");
        assert!(text.contains("- git: read-only"), "{text}");
        assert!(text.contains("- environment: HOME, PATH"), "{text}");
        assert!(
            text.contains(&format!("- writable dirs: {}", shared.display())),
            "{text}"
        );
    }

    #[test]
    fn an_unsandboxed_policy_says_it_is_not_enforced() {
        let text = Policy::as_written(&profile()).render(false);
        assert!(text.contains("- tool: none (--unsandboxed)"), "{text}");
        assert!(text.contains("- network: not enforced"), "{text}");
        assert!(text.contains("- git: not enforced"), "{text}");
        assert!(!text.contains("- network: on"), "{text}");
        assert!(text.contains("- environment: inherited"), "{text}");
    }

    #[test]
    fn an_unsandboxed_plan_is_the_command_itself() {
        let profile = profile();
        let project = project("unsandboxed");
        let plan = Plan::new(&profile, &project, true).unwrap();
        assert!(!plan.sandboxed());
        let command = plan
            .command(&profile, &project, "the brief", &[], Path::new("/none"))
            .unwrap();
        assert_eq!(command.get_program(), "/bin/echo");
        let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
        assert_eq!(args, ["the brief"]);
    }

    #[test]
    fn a_relative_program_is_relative_to_the_project() {
        let profile = profile();
        let project = project("relative");
        let script = project.join("run.sh");
        fs::File::create(&script).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let _plan = Plan::new(&profile, &project, true).unwrap();
        assert_eq!(
            resolve_program("./run.sh", &project).unwrap(),
            fs::canonicalize(&script).unwrap()
        );
        assert!(resolve_program("no-such-program-xyz", &project).is_err());
        fs::remove_dir_all(&project).unwrap();
    }

    #[test]
    fn the_environment_is_the_profile_list_and_path() {
        let mut profile = profile();
        profile.env = vec!["HOME".to_owned()];
        let project = project("env");
        let Some(plan) = plan(&profile, &project) else {
            return;
        };
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
        let project = project("missing-env");
        let Some(plan) = plan(&profile, &project) else {
            return;
        };
        let error = plan.env().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("DOG_WALKER_TEST_UNSET_VARIABLE_XYZ")
        );
    }

    #[test]
    fn the_directory_lists_are_checked() {
        let project = project("dirs");
        assert!(matches!(
            directories(&[PathBuf::from("/")]),
            Err(SandboxError::RootDir)
        ));
        assert!(matches!(
            directories(&[PathBuf::from("/usr")]),
            Err(SandboxError::SystemDir { .. })
        ));
        let file = project.join("file");
        fs::File::create(&file).unwrap();
        assert!(matches!(
            directories(&[file]),
            Err(SandboxError::NotDir { .. })
        ));
        assert!(directories(std::slice::from_ref(&project)).is_ok());
        fs::remove_dir_all(&project).unwrap();
    }

    #[test]
    fn a_read_only_git_is_mounted_over_the_project_and_network_can_be_cut() {
        let project = project("git");
        fs::create_dir_all(project.join(".git")).unwrap();
        let mut profile = profile();
        profile.network = false;
        let Some(plan) = plan(&profile, &project) else {
            return;
        };
        let command = plan
            .command(&profile, &project, "the brief", &[], Path::new("/none"))
            .unwrap();
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let git = project.join(".git").display().to_string();
        assert!(args.contains(&"--unshare-net".to_owned()), "{args:?}");
        assert!(args.contains(&"--unshare-pid".to_owned()), "{args:?}");
        assert!(args.contains(&"--remount-ro".to_owned()), "{args:?}");
        assert!(
            args.windows(3).any(|w| w == ["--ro-bind", &git, &git]),
            "{args:?}"
        );
        // bwrap carries no environment of the runner's: only `--setenv` reaches the harness.
        assert_eq!(command.get_envs().count(), 0);
        fs::remove_dir_all(&project).unwrap();
    }

    #[test]
    fn the_protections_are_mounted_after_the_profile_directories() {
        let project = project("order");
        fs::create_dir_all(project.join(".git")).unwrap();
        let handover = project.join("handover.md");
        fs::File::create(&handover).unwrap();
        let mut profile = profile();
        // A writable directory that contains the project: the project's own bind, the
        // read-only `.git` and the read-only handover must still come after it.
        profile.writable_dirs = vec![project.parent().unwrap().to_path_buf()];
        let Some(plan) = plan(&profile, &project) else {
            return;
        };
        let command = plan
            .command(&profile, &project, "the brief", &[], &handover)
            .unwrap();
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let at = |flag: &str, path: &Path| {
            let path = path.display().to_string();
            args.windows(3)
                .position(|window| window == [flag, &path, &path])
                .unwrap_or_else(|| panic!("{flag} {path} is missing: {args:?}"))
        };
        let writable = at("--bind", project.parent().unwrap());
        let git = at("--ro-bind", &project.join(".git"));
        let handover = at("--ro-bind", &handover);
        assert!(writable < git, "{args:?}");
        assert!(writable < handover, "{args:?}");
        fs::remove_dir_all(&project).unwrap();
    }
}
