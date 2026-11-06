//! AT34 (R17, D24): the config says whether tasks run in the sandbox; the project's config wins
//! over the global one, and the sandbox is on when neither says.
//!
//! The harness is a stub, and `PATH` holds only the stub, so `bwrap` is never found: a sandboxed
//! run is refused (exit 2) and an unsandboxed one runs, which tells the two apart without bwrap.

// Why: clippy.toml exempts `#[test]` functions from the unwrap denial, but not the helpers of an
// integration-test file; a failing helper should fail the test.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use dog_walker::config;

const PROFILES: &str = "[profiles.alpha]\nharness = \"pi\"\nmodel = \"m\"\ncommand = [\"stub\"]\n\n\
                        [profiles.main]\nharness = \"pi\"\nmodel = \"m\"\nmain = true\n";

struct Setup {
    dir: PathBuf,
    project: PathBuf,
    global: PathBuf,
}

impl Setup {
    fn new(name: &str) -> Setup {
        let dir = std::env::temp_dir().join(format!(
            "dog-walker-sandbox-setting-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let project = dir.join("project");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(dir.join("bin")).unwrap();
        let stub = dir.join("bin").join("stub");
        fs::write(
            &stub,
            "#!/bin/sh\nprintf 'done\\n' > \"$DOG_WALKER_RESULT_PATH\"\n",
        )
        .unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        let project = fs::canonicalize(project).unwrap();
        let global = config::default_path(Some(dir.join("xdg").into()), None).unwrap();
        fs::create_dir_all(global.parent().unwrap()).unwrap();
        Setup {
            dir,
            project,
            global,
        }
    }

    /// Writes the global config, with `top` before the profiles.
    fn global(&self, top: &str) {
        fs::write(&self.global, format!("{top}\n{PROFILES}")).unwrap();
    }

    /// Writes this project's own config, with `top` before the profiles.
    fn project(&self, top: &str) {
        let path = config::project_config_path(&self.global, &self.project).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("{top}\n{PROFILES}")).unwrap();
    }

    fn run(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dog-walker"))
            .args(["run", "alpha", "the brief"])
            .current_dir(&self.project)
            .env("XDG_CONFIG_HOME", self.dir.join("xdg"))
            .env_remove("HOME")
            .env("PATH", self.dir.join("bin"))
            .output()
            .unwrap()
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn handover(project: &Path) -> String {
    let task = fs::read_dir(project.join(".dog-walker").join("tasks"))
        .unwrap()
        .flatten()
        .next()
        .unwrap();
    fs::read_to_string(task.path().join("handover.md")).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// AT34: with no setting anywhere the sandbox is on, so a run without bwrap is refused.
#[test]
fn at34_the_sandbox_is_on_by_default() {
    let setup = Setup::new("default");
    setup.global("");
    let out = setup.run();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("--unsandboxed"), "{}", stderr(&out));
}

// AT34: `sandbox = false` in the global config runs tasks without the sandbox, and the handover
// says so.
#[test]
fn at34_the_global_config_can_turn_the_sandbox_off() {
    let setup = Setup::new("global");
    setup.global("sandbox = false\n");
    let out = setup.run();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(handover(&setup.project).contains("none (--unsandboxed)"));
}

// AT34: a project config that does not say falls back to the global setting.
#[test]
fn at34_a_silent_project_config_uses_the_global_setting() {
    let setup = Setup::new("silent");
    setup.global("sandbox = false\n");
    setup.project("");
    let out = setup.run();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

// AT34: the project's setting wins over the global one, in both directions.
#[test]
fn at34_the_project_setting_wins_over_the_global_one() {
    let on = Setup::new("project-on");
    on.global("sandbox = false\n");
    on.project("sandbox = true\n");
    let out = on.run();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));

    let off = Setup::new("project-off");
    off.global("sandbox = true\n");
    off.project("sandbox = false\n");
    let out = off.run();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

// AT34: only the `sandbox` key of the global config is read for a project that has its own, so
// a global file with no profiles still gives the setting, and a broken one is ignored.
#[test]
fn at34_the_global_fallback_reads_only_the_sandbox_key() {
    let only = Setup::new("only-key");
    fs::write(&only.global, "sandbox = false\n").unwrap();
    only.project("");
    let out = only.run();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let broken = Setup::new("broken-global");
    fs::write(&broken.global, "this is [not toml\n").unwrap();
    broken.project("");
    let out = broken.run();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("--unsandboxed"), "{}", stderr(&out));
}

// AT34: a global path that is not a regular file (a FIFO here) counts as silent and is never
// read, so it cannot make a project that has its own config wait.
#[test]
fn at34_a_global_path_that_is_not_a_file_is_not_read() {
    let setup = Setup::new("fifo");
    let made = Command::new("mkfifo").arg(&setup.global).status().unwrap();
    assert!(made.success());
    setup.project("");
    let mut child = Command::new(env!("CARGO_BIN_EXE_dog-walker"))
        .args(["run", "alpha", "the brief"])
        .current_dir(&setup.project)
        .env("XDG_CONFIG_HOME", setup.dir.join("xdg"))
        .env_remove("HOME")
        .env("PATH", setup.dir.join("bin"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > std::time::Duration::from_secs(20) {
            child.kill().unwrap();
            panic!("dog-walker waits on the global FIFO");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    // The sandbox stays on, so without bwrap on PATH the run is refused.
    assert_eq!(status.code(), Some(2));
}
