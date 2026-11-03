//! AT31 (R25): `init` writes the profiles and the main harness's instructions line, asks before it
//! writes, writes no secret, and never changes what exists.
//!
//! Each test builds a fake home with stub harnesses laid out as the real ones are, and runs the
//! real binary against it.

// Why: clippy.toml exempts `#[test]` functions from the unwrap denial, but not the helpers of an
// integration-test file; a failing helper should fail the test.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Home {
        let path =
            std::env::temp_dir().join(format!("dog-walker-init-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("bin")).unwrap();
        Home(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn config(&self) -> PathBuf {
        self.0.join(".config/dog-walker/config.toml")
    }

    /// A stub harness at `target` (relative to the home), found on `PATH` through a link in `bin`.
    fn harness(&self, name: &str, target: &str) {
        let real = self.0.join(target);
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&real, self.0.join("bin").join(name)).unwrap();
    }

    /// Every harness, laid out as the real ones are.
    fn all_harnesses(&self) {
        self.harness("claude", ".local/share/claude/versions/2.1.0");
        self.harness("codex", "lib/node_modules/@openai/codex/bin/codex.js");
        self.harness("pi", ".pi/agent/bin/pi");
        fs::create_dir_all(self.0.join(".pi/agent/install")).unwrap();
        self.harness("dsh", "lib/node_modules/@deepseek-ai/dsh/lib/bin.js");
    }

    /// Runs `init` globally unless `args` say otherwise, with `stdin` as the user's answers.
    fn init(&self, args: &[&str], stdin: &str) -> Output {
        if args.contains(&"--scope") {
            self.init_raw(args, stdin)
        } else {
            let mut all = vec!["--scope", "global"];
            all.extend_from_slice(args);
            self.init_raw(&all, stdin)
        }
    }

    /// The directory that holds this project's own config and private files.
    fn project_dir(&self) -> PathBuf {
        let projects = self.0.join(".config/dog-walker/projects");
        fs::read_dir(projects)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .next()
            .unwrap()
    }

    /// Runs `dog-walker args` in `cwd` with this home.
    fn dw(&self, args: &[&str], cwd: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dog-walker"))
            .args(args)
            .env_clear()
            .env("HOME", &self.0)
            .env("PATH", self.0.join("bin"))
            .current_dir(cwd)
            .output()
            .unwrap()
    }

    /// Runs `init` with exactly `args`, `stdin` as the user's answers, and this home.
    fn init_raw(&self, args: &[&str], stdin: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_dog-walker"))
            .arg("init")
            .args(args)
            .env_clear()
            .env("HOME", &self.0)
            .env("PATH", self.0.join("bin"))
            // Credentials the installer must never read into anything it writes or prints.
            .env("OPENROUTER_API_KEY", SENTINEL_KEY)
            .env("CLAUDE_CODE_OAUTH_TOKEN", SENTINEL_TOKEN)
            .current_dir(&self.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

const BLOCK: &str = "<!-- dog-walker:begin -->";
const SENTINEL_KEY: &str = "SENTINEL-KEY-8f3a";
const SENTINEL_TOKEN: &str = "SENTINEL-TOKEN-91bc";

/// Every file under `dir`, at any depth.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files(&path));
        } else {
            found.push(path);
        }
    }
    found
}

// AT31 (R25): the answers on the command line write a config the parser accepts, with each
// secondary's install directory found and its sandbox part filled in.
#[test]
fn at31_flags_write_valid_profiles_with_their_sandbox() {
    let home = Home::new("flags");
    home.all_harnesses();
    let out = home.init(
        &[
            "--main",
            "claude",
            "--worker",
            "pi:openrouter/z-ai/glm",
            "--reviewer",
            "codex",
            "--yes",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        home.config().to_string_lossy()
    );
    let config = fs::read_to_string(home.config()).unwrap();
    let want_install = format!("{}/.pi/agent/install", home.path().display());
    assert!(config.contains(&want_install), "{config}");
    assert!(config.contains("\"OPENROUTER_API_KEY\""), "{config}");
    // The real parser accepts it: `profiles` lists main first, then the rest.
    let listed = Command::new(env!("CARGO_BIN_EXE_dog-walker"))
        .arg("--config")
        .arg(home.config())
        .arg("profiles")
        .output()
        .unwrap();
    let rows = String::from_utf8_lossy(&listed.stdout).into_owned();
    assert!(listed.status.success(), "{}", stderr(&listed));
    let names: Vec<&str> = rows.lines().filter_map(|l| l.split('\t').next()).collect();
    assert_eq!(names, ["main", "reviewer", "worker"], "{rows}");
    // Private directories are mode 700, env files mode 600.
    let private = home.path().join(".config/dog-walker/worker");
    assert_eq!(mode(&private), 0o700);
    let env_file = home.path().join(".config/dog-walker/worker.env");
    assert_eq!(mode(&env_file), 0o600);
}

// AT31 (R25): no secret is written or printed. The environment holds a key and a token, and
// the Codex login is a file the installer could copy: none of them is in any file under the home
// (apart from the login's own source) or in either output stream.
#[test]
fn at31_no_secret_is_written() {
    let home = Home::new("secret");
    home.all_harnesses();
    fs::create_dir_all(home.path().join(".codex")).unwrap();
    fs::write(home.path().join(".codex/auth.json"), "SECRET-LOGIN").unwrap();
    let out = home.init(
        &[
            "--main",
            "claude",
            "--worker",
            "pi:openrouter/m",
            "--reviewer",
            "codex",
            "--yes",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let env_file = fs::read_to_string(home.path().join(".config/dog-walker/worker.env")).unwrap();
    assert!(
        env_file
            .lines()
            .any(|l| l.starts_with("# OPENROUTER_API_KEY=")),
        "{env_file}"
    );
    assert!(
        !env_file
            .lines()
            .any(|l| l.starts_with("OPENROUTER_API_KEY")),
        "{env_file}"
    );
    assert!(
        !home
            .path()
            .join(".config/dog-walker/reviewer/auth.json")
            .exists()
    );
    assert!(stderr(&out).contains("auth.json"), "{}", stderr(&out));
    let printed = format!("{}{}", stderr(&out), String::from_utf8_lossy(&out.stdout));
    for secret in [SENTINEL_KEY, SENTINEL_TOKEN, "SECRET-LOGIN"] {
        assert!(
            !printed.contains(secret),
            "{secret} in the output: {printed}"
        );
        for file in files(home.path()) {
            if file.ends_with(".codex/auth.json") {
                continue;
            }
            let text = fs::read_to_string(&file).unwrap_or_default();
            assert!(!text.contains(secret), "{secret} in {}", file.display());
        }
    }
}

// AT31 (R25): the main harness's global file gets the marked line, its other content is kept,
// and a second run does not add it again.
#[test]
fn at31_the_instructions_line_is_added_once_and_the_file_is_kept() {
    let home = Home::new("line");
    home.all_harnesses();
    fs::create_dir_all(home.path().join(".claude")).unwrap();
    let file = home.path().join(".claude/CLAUDE.md");
    fs::write(&file, "# My rules\nBe brief.\n").unwrap();
    let args = [
        "--main",
        "claude",
        "--worker",
        "no",
        "--reviewer",
        "no",
        "--scope",
        "global",
        "--yes",
    ];
    assert_eq!(home.init(&args, "").status.code(), Some(0));
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("# My rules\nBe brief.\n"), "{text}");
    assert_eq!(text.matches(BLOCK).count(), 1, "{text}");
    assert!(
        text.contains("run `dog-walker instructions` first"),
        "{text}"
    );
    let again = home.init(&args, "");
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert_eq!(fs::read_to_string(&file).unwrap(), text);
    assert!(
        stderr(&again).contains("left as it is"),
        "{}",
        stderr(&again)
    );
}

// AT31 (R25): without a scope, `--yes` puts the line in the project's file and leaves the global
// one alone.
#[test]
fn at31_the_line_goes_in_the_project_by_default() {
    let home = Home::new("project");
    home.all_harnesses();
    fs::create_dir_all(home.path().join(".claude")).unwrap();
    let global = home.path().join(".claude/CLAUDE.md");
    fs::write(&global, "# mine\n").unwrap();
    let out = home.init_raw(
        &[
            "--main",
            "claude",
            "--worker",
            "no",
            "--reviewer",
            "no",
            "--yes",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let project = fs::read_to_string(home.path().join("CLAUDE.md")).unwrap();
    assert!(project.contains(BLOCK), "{project}");
    assert_eq!(fs::read_to_string(&global).unwrap(), "# mine\n");
}

// AT31 (R25): with no scope and no `--yes`, the installer asks, naming both files, and the
// answer decides; a bad answer is asked again.
#[test]
fn at31_the_scope_is_asked() {
    let home = Home::new("scopeask");
    home.all_harnesses();
    let out = home.init_raw(
        &["--main", "claude", "--worker", "no", "--reviewer", "no"],
        "everywhere\nglobal\ny\n",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let said = stderr(&out);
    assert!(said.contains("CLAUDE.md"), "{said}");
    assert!(said.contains("is not a scope"), "{said}");
    assert!(
        fs::read_to_string(home.path().join(".claude/CLAUDE.md"))
            .unwrap()
            .contains(BLOCK)
    );
    assert!(!home.path().join("CLAUDE.md").exists());
}

// AT31 (R25, D20): a project set up on its own gets its own config and private files outside the
// repository; that project uses it, the global config is not written, and another directory
// still uses the global one.
#[test]
fn at31_a_project_gets_its_own_config() {
    let home = Home::new("ownconfig");
    home.all_harnesses();
    let project = home.path().join("app");
    fs::create_dir_all(&project).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_dog-walker"))
        .args([
            "init", "--scope", "project", "--main", "claude", "--worker", "codex",
        ])
        .args(["--reviewer", "no", "--yes"])
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", home.path().join("bin"))
        .current_dir(&project)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let own = home.project_dir();
    assert!(
        own.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("app-")
    );
    let config = fs::read_to_string(own.join("config.toml")).unwrap();
    assert!(config.contains("[profiles.worker]"), "{config}");
    assert!(own.join("worker.env").is_file());
    assert_eq!(mode(&own.join("worker")), 0o700);
    assert!(project.join("CLAUDE.md").is_file());
    assert!(!home.config().exists());
    let here = home.dw(&["profiles"], &project);
    assert_eq!(here.status.code(), Some(0), "{}", stderr(&here));
    assert!(String::from_utf8_lossy(&here.stdout).contains("worker"));
    // Another directory has no config of its own and no global one.
    let elsewhere = home.dw(&["profiles"], home.path());
    assert_eq!(elsewhere.status.code(), Some(2), "{}", stderr(&elsewhere));
}

/// A repository `main` with a linked worktree `wt`, laid out as git does, in a fresh folder.
fn repo_with_worktree(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("dog-walker-root-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let main = base.join("main");
    let wt = base.join("wt");
    let admin = main.join(".git/worktrees/wt");
    fs::create_dir_all(&admin).unwrap();
    fs::create_dir_all(wt.join("deep/er")).unwrap();
    fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
    fs::write(admin.join("commondir"), "../..\n").unwrap();
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", wt.join(".git").display()),
    )
    .unwrap();
    (
        base.canonicalize().unwrap(),
        main.canonicalize().unwrap(),
        wt.canonicalize().unwrap(),
    )
}

// D20: a worktree and a subdirectory belong to the main checkout's project.
#[test]
fn a_worktree_and_a_subdirectory_belong_to_the_main_checkouts_project() {
    use dog_walker::config::project_root;
    let (base, main, wt) = repo_with_worktree("ok");
    assert_eq!(project_root(&main), main);
    assert_eq!(project_root(&wt), main);
    assert_eq!(project_root(&wt.join("deep/er")), main);
    fs::remove_dir_all(base).unwrap();
}

// D20: a repository cannot name another checkout as its main one: the main checkout must list the
// worktree and point back at it.
#[test]
fn a_worktree_the_main_checkout_does_not_list_is_its_own_project() {
    use dog_walker::config::project_root;
    let (base, _, wt) = repo_with_worktree("forged");
    fs::write(
        base.join("main/.git/worktrees/wt/gitdir"),
        "/elsewhere/.git\n",
    )
    .unwrap();
    assert_eq!(project_root(&wt), wt);
    fs::remove_dir_all(base).unwrap();
}

// D20: a directory in no repository is its own project.
#[test]
fn a_directory_in_no_repository_is_its_own_project() {
    let dir = std::env::temp_dir().join(format!("dog-walker-root-{}-none", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    assert_eq!(dog_walker::config::project_root(&dir), dir);
    fs::remove_dir_all(dir).unwrap();
}

// AT31 (R25, D20): a linked git worktree and a subdirectory use the project config of the main
// checkout, so the one setup governs every checkout of the repository.
#[test]
fn at31_a_worktree_uses_the_main_checkouts_project_config() {
    let home = Home::new("worktree");
    home.all_harnesses();
    let app = home.path().join("app");
    let admin = app.join(".git/worktrees/wt");
    let wt = home.path().join("wt");
    fs::create_dir_all(&admin).unwrap();
    fs::create_dir_all(wt.join("src")).unwrap();
    fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
    fs::write(admin.join("commondir"), "../..\n").unwrap();
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", wt.join(".git").display()),
    )
    .unwrap();
    let init = Command::new(env!("CARGO_BIN_EXE_dog-walker"))
        .args([
            "init", "--scope", "project", "--main", "claude", "--worker", "codex",
        ])
        .args(["--reviewer", "no", "--yes"])
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", home.path().join("bin"))
        .current_dir(&app)
        .output()
        .unwrap();
    assert_eq!(init.status.code(), Some(0), "{}", stderr(&init));
    for dir in [app.join("src"), wt.clone(), wt.join("src")] {
        fs::create_dir_all(&dir).unwrap();
        let out = home.dw(&["profiles"], &dir);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}: {}",
            dir.display(),
            stderr(&out)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("worker"));
    }
}

// AT31 (R25, D20): a main harness with no instructions file is still set up for the project, and
// gets the line printed.
#[test]
fn at31_pi_main_in_a_project_gets_the_line_printed() {
    let home = Home::new("pimainproject");
    home.all_harnesses();
    let out = home.init_raw(
        &[
            "--scope",
            "project",
            "--main",
            "pi",
            "--worker",
            "no",
            "--reviewer",
            "no",
            "--yes",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stderr(&out).contains("run `dog-walker instructions` first"));
    assert!(home.project_dir().join("config.toml").is_file());
    assert!(!home.path().join("AGENTS.md").exists());
}

// AT31 (R25): Codex as the main harness gets its AGENTS.md.
#[test]
fn at31_codex_main_gets_its_agents_file() {
    let home = Home::new("codex");
    home.all_harnesses();
    let out = home.init(
        &[
            "--main",
            "codex",
            "--worker",
            "no",
            "--reviewer",
            "no",
            "--scope",
            "global",
            "--yes",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = fs::read_to_string(home.path().join(".codex/AGENTS.md")).unwrap();
    assert!(text.contains(BLOCK), "{text}");
}

// AT31 (R25): a main harness with no verified global file gets the line printed, and no file.
#[test]
fn at31_pi_main_gets_the_line_printed() {
    let home = Home::new("pimain");
    home.all_harnesses();
    let out = home.init(
        &[
            "--main",
            "pi",
            "--worker",
            "no",
            "--reviewer",
            "no",
            "--yes",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("run `dog-walker instructions` first"),
        "{}",
        stderr(&out)
    );
    assert!(!home.path().join(".pi/agent/AGENTS.md").exists());
    assert!(!home.path().join(".claude").exists());
}

// AT31 (R25): the user can answer the questions instead, and `no` skips a role.
#[test]
fn at31_the_questions_can_be_answered_on_stdin() {
    let home = Home::new("ask");
    home.all_harnesses();
    // main: harness, model; worker: harness, model, credential variable, its value (left for
    // later); reviewer: no; confirm.
    let answers = "claude\n\npi\nopenrouter/z-ai/glm\nMY_KEY\n\nno\ny\n";
    let out = home.init(&[], answers);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let config = fs::read_to_string(home.config()).unwrap();
    assert!(config.contains("[profiles.main]"), "{config}");
    assert!(config.contains("[profiles.worker]"), "{config}");
    assert!(!config.contains("[profiles.reviewer]"), "{config}");
    assert!(config.contains("\"MY_KEY\""), "{config}");
    assert!(!config.contains("OPENROUTER_API_KEY"), "{config}");
}

// AT31 (R25): a secret typed where a variable name is asked is refused and not printed back.
#[test]
fn at31_a_secret_typed_as_a_variable_name_is_not_echoed() {
    let home = Home::new("echo");
    home.all_harnesses();
    // main: harness, model; worker: harness, model, credential (a key, then the default);
    // reviewer: no; confirm.
    let answers = "claude\n\npi\n\nsk-or-v1-TYPED-BY-MISTAKE\n\n\nno\ny\n";
    let out = home.init(&[], answers);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("not a variable name"),
        "{}",
        stderr(&out)
    );
    assert!(
        !stderr(&out).contains("TYPED-BY-MISTAKE"),
        "{}",
        stderr(&out)
    );
    let written = fs::read_to_string(home.config()).unwrap();
    assert!(!written.contains("TYPED-BY-MISTAKE"), "{written}");
}

// AT31 (R25, D22): a key typed for a secondary is saved in its env file (mode 600) and nowhere
// else: not in the config, not in what the installer prints.
#[test]
fn at31_a_typed_key_is_saved_only_in_the_env_file() {
    let home = Home::new("key");
    home.all_harnesses();
    // main: harness, model; worker: harness, model, credential variable, the key; reviewer: no;
    // confirm.
    let out = home.init(&[], "claude\n\npi\n\n\nTYPED-KEY-5521\nno\ny\n");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let env = home.path().join(".config/dog-walker/worker.env");
    let text = fs::read_to_string(&env).unwrap();
    assert!(
        text.contains("OPENROUTER_API_KEY='TYPED-KEY-5521'"),
        "{text}"
    );
    assert_eq!(mode(&env), 0o600);
    let loaded = dog_walker::envfile::parse(&text);
    assert!(loaded.contains(&("OPENROUTER_API_KEY".to_owned(), "TYPED-KEY-5521".to_owned())));
    let config = fs::read_to_string(home.config()).unwrap();
    assert!(!config.contains("TYPED-KEY-5521"), "{config}");
    assert!(
        config.contains("secrets = [\"OPENROUTER_API_KEY\"]"),
        "{config}"
    );
    assert!(!stderr(&out).contains("TYPED-KEY-5521"), "{}", stderr(&out));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("TYPED-KEY-5521"));
    // Nothing is left for the user to do about the key.
    assert!(!stderr(&out).contains("put your key"), "{}", stderr(&out));
}

// AT31 (R25, D22): the Codex login is copied into the profile's private directory (mode 600)
// when the user agrees, and its content is never printed.
#[test]
fn at31_the_codex_login_is_copied_when_the_user_agrees() {
    let home = Home::new("login");
    home.all_harnesses();
    fs::create_dir_all(home.path().join(".codex")).unwrap();
    fs::write(home.path().join(".codex/auth.json"), "LOGIN-SENTINEL-77").unwrap();
    // main: harness, model; worker: harness, model, copy the login (the default, yes);
    // reviewer: no; confirm.
    let out = home.init(&[], "claude\n\ncodex\n\n\nno\ny\n");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let copy = home.path().join(".config/dog-walker/worker/auth.json");
    assert_eq!(fs::read_to_string(&copy).unwrap(), "LOGIN-SENTINEL-77");
    assert_eq!(mode(&copy), 0o600);
    assert!(
        !stderr(&out).contains("LOGIN-SENTINEL-77"),
        "{}",
        stderr(&out)
    );
    assert!(!stderr(&out).contains("install -m 600"), "{}", stderr(&out));
}

// AT31 (R25): without a yes, nothing is written.
#[test]
fn at31_nothing_is_written_without_a_yes() {
    let home = Home::new("decline");
    home.all_harnesses();
    let out = home.init(
        &["--main", "claude", "--worker", "no", "--reviewer", "no"],
        "n\n",
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(!home.config().exists());
    assert!(!home.path().join(".claude").exists());
    assert!(!home.path().join(".config/dog-walker").exists());
}

// AT31 (R25): a question with no answer to read is an error that writes nothing, not a guess.
#[test]
fn at31_missing_answers_write_nothing() {
    let home = Home::new("eof");
    home.all_harnesses();
    let out = home.init(&["--main", "claude"], "");
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("no answer"), "{}", stderr(&out));
    assert!(!home.config().exists());
}

// AT31 (R25): a bad answer is named, and nothing is written.
#[test]
fn at31_a_bad_answer_is_refused() {
    let home = Home::new("bad");
    home.all_harnesses();
    let out = home.init(&["--main", "gpt", "--yes"], "");
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("not a harness"), "{}", stderr(&out));
    assert!(!home.config().exists());
}

// AT31 (R25): an existing config is only appended to: its profiles stay as they are, and its main
// profile stands.
#[test]
fn at31_an_existing_config_is_never_changed() {
    let home = Home::new("existing");
    home.all_harnesses();
    let original = "# mine\n[profiles.boss]\nharness = \"codex\"\nmodel = \"m\"\nmain = true\n\n\
                    [profiles.worker]\nharness = \"pi\"\nmodel = \"mine\"\n";
    fs::create_dir_all(home.config().parent().unwrap()).unwrap();
    fs::write(home.config(), original).unwrap();
    let out = home.init(
        &[
            "--main",
            "claude",
            "--worker",
            "dsh",
            "--reviewer",
            "claude",
            "--yes",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let config = fs::read_to_string(home.config()).unwrap();
    assert!(config.starts_with(original), "{config}");
    // The existing main and worker stand; only the reviewer is new.
    assert_eq!(config.matches("main = true").count(), 1, "{config}");
    assert_eq!(config.matches("[profiles.worker]").count(), 1, "{config}");
    assert!(config.contains("[profiles.reviewer]"), "{config}");
    // The instructions go to the existing main harness's file (Codex), not the new answer's.
    assert!(home.path().join(".codex/AGENTS.md").exists());
    assert!(!home.path().join(".claude/CLAUDE.md").exists());
}

// AT31 (R25): a harness that is not installed still gets a profile, with a note to add its
// directory by hand, instead of a guess.
#[test]
fn at31_a_missing_harness_is_noted_not_guessed() {
    let home = Home::new("missing");
    home.harness("claude", ".local/share/claude/versions/2.1.0");
    let out = home.init(
        &[
            "--main",
            "claude",
            "--worker",
            "pi",
            "--reviewer",
            "no",
            "--yes",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let config = fs::read_to_string(home.config()).unwrap();
    assert!(!config.contains("read_only_dirs"), "{config}");
    assert!(stderr(&out).contains("was not found"), "{}", stderr(&out));
}

// AT31 (R25): a path with a newline would end a comment in the env file and run a command when
// the file is sourced, so it is refused before anything is written.
#[test]
fn at31_a_control_character_in_the_config_path_is_refused() {
    let home = Home::new("newline");
    home.all_harnesses();
    let bad = home.path().join("evil\nprintf INJECTED #/config.toml");
    let out = home.init(
        &[
            "--config",
            bad.to_str().unwrap(),
            "--main",
            "claude",
            "--worker",
            "pi:openrouter/m",
            "--reviewer",
            "no",
            "--yes",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("control character"),
        "{}",
        stderr(&out)
    );
    assert!(!bad.parent().unwrap().exists());
    assert!(!home.path().join(".claude").exists());
}

// AT31 (R25): something in the way of a private directory stops the run before the config is
// touched, so no profile is recorded that a second run would skip.
#[test]
fn at31_a_blocked_directory_records_no_profile() {
    let home = Home::new("blocked");
    home.all_harnesses();
    fs::create_dir_all(home.path().join(".config/dog-walker")).unwrap();
    fs::write(home.path().join(".config/dog-walker/worker"), "in the way").unwrap();
    let args = [
        "--main",
        "claude",
        "--worker",
        "pi:openrouter/m",
        "--reviewer",
        "no",
        "--yes",
    ];
    let out = home.init(&args, "");
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("not a directory"), "{}", stderr(&out));
    assert!(!home.config().exists());
    assert!(!home.path().join(".claude").exists());
    // Once the obstacle is gone, the same command finishes the job.
    fs::remove_file(home.path().join(".config/dog-walker/worker")).unwrap();
    assert_eq!(home.init(&args, "").status.code(), Some(0));
    assert!(
        fs::read_to_string(home.config())
            .unwrap()
            .contains("[profiles.worker]")
    );
}

// AT31 (R25): a private directory or env file that is open to others, or a link, is refused: the
// installer says the file is mode 600 and the user will put a credential in it.
#[test]
fn at31_open_or_linked_private_paths_are_refused() {
    let args = [
        "--main",
        "claude",
        "--worker",
        "pi:openrouter/m",
        "--reviewer",
        "no",
        "--yes",
    ];
    let home = Home::new("open-dir");
    home.all_harnesses();
    let private = home.path().join(".config/dog-walker/worker");
    fs::create_dir_all(&private).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o755)).unwrap();
    let out = home.init(&args, "");
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("open to others"), "{}", stderr(&out));
    assert!(!home.config().exists());

    let home = Home::new("open-env");
    home.all_harnesses();
    let env_file = home.path().join(".config/dog-walker/worker.env");
    fs::create_dir_all(env_file.parent().unwrap()).unwrap();
    fs::write(&env_file, "KEEP=1\n").unwrap();
    fs::set_permissions(&env_file, fs::Permissions::from_mode(0o644)).unwrap();
    let out = home.init(&args, "");
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("open to others"), "{}", stderr(&out));

    let home = Home::new("link-env");
    home.all_harnesses();
    let env_file = home.path().join(".config/dog-walker/worker.env");
    fs::create_dir_all(env_file.parent().unwrap()).unwrap();
    symlink(home.path().join("elsewhere"), &env_file).unwrap();
    let out = home.init(&args, "");
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("a link is refused"),
        "{}",
        stderr(&out)
    );
    assert!(!home.path().join("elsewhere").exists());
}

// AT31 (R25): an env file the user already has, closed to others, keeps its contents.
#[test]
fn at31_an_existing_private_env_file_keeps_its_contents() {
    let home = Home::new("keep-env");
    home.all_harnesses();
    let env_file = home.path().join(".config/dog-walker/worker.env");
    fs::create_dir_all(env_file.parent().unwrap()).unwrap();
    fs::write(&env_file, "MINE=1\n").unwrap();
    fs::set_permissions(&env_file, fs::Permissions::from_mode(0o600)).unwrap();
    let out = home.init(
        &[
            "--main",
            "claude",
            "--worker",
            "pi:openrouter/m",
            "--reviewer",
            "no",
            "--yes",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(fs::read_to_string(&env_file).unwrap(), "MINE=1\n");
}

// AT31 (R25): an instructions file that is a link (a dotfiles repo) is followed on purpose, and
// the plan says where it leads.
#[test]
fn at31_a_linked_instructions_file_is_followed_and_named() {
    let home = Home::new("linked");
    home.all_harnesses();
    fs::create_dir_all(home.path().join(".claude")).unwrap();
    fs::create_dir_all(home.path().join("dotfiles")).unwrap();
    let real = home.path().join("dotfiles/CLAUDE.md");
    fs::write(&real, "# dotfiles\n").unwrap();
    symlink(&real, home.path().join(".claude/CLAUDE.md")).unwrap();
    let out = home.init(
        &[
            "--main",
            "claude",
            "--worker",
            "no",
            "--reviewer",
            "no",
            "--scope",
            "global",
            "--yes",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(fs::read_to_string(&real).unwrap().contains(BLOCK));
    assert!(stderr(&out).contains("a link to"), "{}", stderr(&out));
}

// AT31 (R25): the file name of the config is checked too, not only the folders above it.
#[test]
fn at31_a_control_character_in_the_config_file_name_is_refused() {
    let home = Home::new("basename");
    home.all_harnesses();
    let bad = home.path().join("cfg\nprintf INJECTED.toml");
    let out = home.init(
        &[
            "--config",
            bad.to_str().unwrap(),
            "--main",
            "claude",
            "--worker",
            "no",
            "--reviewer",
            "no",
            "--yes",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("control character"),
        "{}",
        stderr(&out)
    );
    assert!(!bad.exists());
    assert!(!home.path().join(".claude").exists());
}
