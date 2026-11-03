# Setting up harnesses

A profile runs inside a sandbox that sees the project, the system's programs and only what the
profile lists: `read_only_dirs`, `writable_dirs` and `env` (variable names, whose values come from
the shell that runs `dog-walker`). So each harness needs its install directory, its credentials
and a place to write, named in its profile. `dog-walker instructions` has a summary per harness;
this page has the steps. After any of them, `dog-walker doctor <profile>` checks the profile
without starting a task or the harness: the directories, the program, the variables (set in this
shell), a write to the temporary directory and a TLS fetch inside the sandbox, with the fix for
each failure.

## The quick way: `dog-walker init`

`dog-walker init` does the setup below for you, once:

    dog-walker init

It asks which harness you open yourself (the main profile, required), then, for a `worker` and
a `reviewer`, which harness and model to use or `no` to skip. It finds each harness's install
directory, writes the profile, creates a private directory and an env file for it, and adds a
line to your main harness's global instructions file (`~/.claude/CLAUDE.md` for Claude Code,
`~/.codex/AGENTS.md` for Codex) so that it reads `dog-walker instructions` before it delegates.
It shows everything it will write and asks before it writes. Pi and dsh have no verified global
file, so for those it prints the line for you to add.

It never asks for, reads or writes a secret. What it leaves to you is printed at the end: put
your key in the env file (or run `claude setup-token`, or copy the Codex login), load the env
file, and run `dog-walker doctor <profile>`. The same answers can be given as options, for a
script: `dog-walker init --main claude --worker pi:openrouter/z-ai/glm-5.3-flash --reviewer no
--yes`. Run again later, it adds only the roles you do not have yet and leaves the rest alone.

The sections below are the same steps by hand, and the reasons for each.

## Telling the main harness about dog-walker by hand

The guidance for the main harness (how to write a brief, run it, read the handover and what a
task cost) ships in the binary: `dog-walker instructions` prints it. `init` adds the line that
points the main harness at it; without `init`, nothing puts it in the main model's context, and
a model that never reads it writes short briefs from habit. Add one line to the file your main
harness reads at the start of a session (`AGENTS.md`, `CLAUDE.md` or the equivalent):

    To delegate work to another harness, run `dog-walker instructions` first and follow it.

That is enough: the text is the tool's own, so it stays in step with the version installed. The
handover then shows what each task used (`tokens`, `turns`, `cost_usd`, where the harness
reports them), which is how you judge your briefs.

## Claude Code with a subscription (no API key)

The Claude Code adapter requires no credential: the profile lists the one it uses in `env`
(and in `secrets`, so dog-walker redacts its value). With a subscription that is a token from
`claude setup-token` instead of an API key.

1. **Create a token**, in your own terminal (it needs your interactive login):

       claude setup-token

   It prints a long-lived token (`sk-ant-oat…`). Treat it as a password: keep it out of the
   repository, chats and shell history.

2. **Keep it in a private file**, outside the repository, mode 600:

       mkdir -p ~/.config/dog-walker/claude-config/tmp
       install -m 600 /dev/null ~/.config/dog-walker/reviewer.env
       cat >> ~/.config/dog-walker/reviewer.env <<EOF
       CLAUDE_CODE_OAUTH_TOKEN=<the token>
       CLAUDE_CONFIG_DIR=$HOME/.config/dog-walker/claude-config
       CLAUDE_CODE_TMPDIR=$HOME/.config/dog-walker/claude-config/tmp
       TMPDIR=$HOME/.config/dog-walker/claude-config/tmp
       EOF

3. **Add the profile** to `~/.config/dog-walker/config.toml` (replace `/home/you`; TOML does not
   expand `~` or `$HOME`):

       [profiles.reviewer]
       harness = "claude"
       model = "claude-sonnet-5-5"
       effort = "high"
       env = ["CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CONFIG_DIR", "CLAUDE_CODE_TMPDIR", "TMPDIR", "HOME"]
       secrets = ["CLAUDE_CODE_OAUTH_TOKEN"]
       read_only_dirs = ["/home/you/.local/share/claude"]
       writable_dirs = ["/home/you/.config/dog-walker/claude-config"]

   The adapter builds the command (`claude -p --dangerously-skip-permissions --model … --effort …
   --`, the brief last). `secrets` makes dog-walker replace the token with `[redacted]` in the
   handover and the log.

4. **Load the variables, then run**, in the same shell:

       set -a; . ~/.config/dog-walker/reviewer.env; set +a
       dog-walker run reviewer "Reply with the word ok."

   The task should end `done` with `ok` in the handover. A tool that starts a fresh shell for
   every command (some agents do) must source the file in the same command as `dog-walker`.

### Why each piece is there

- `CLAUDE_CODE_OAUTH_TOKEN` in `env`: the credential; the adapter asks for none, so the profile does.
- A private `CLAUDE_CONFIG_DIR` in `writable_dirs`: the sandbox does not mount your home, and
  this keeps Claude Code away from `~/.claude` (history, projects). It starts empty, so it is
  not signed in; the token does that.
- `CLAUDE_CODE_TMPDIR` and `TMPDIR` inside it: the sandbox mounts no `/tmp`, and without a temp
  directory Claude Code ends with `EROFS: read-only file system, mkdir '/tmp'`.
- `read_only_dirs` on `~/.local/share/claude`: the install directory (check where `claude`
  resolves with `readlink -f $(which claude)` if yours is elsewhere).

### With an API key instead

Swap `CLAUDE_CODE_OAUTH_TOKEN` for `ANTHROPIC_API_KEY` in `env` and `secrets`, and in the file from
step 2. Everything else stays: the install directory, `CLAUDE_CONFIG_DIR`, the temp variables.

### If it fails

The reason is in `harness.log` next to the handover. `` `X` is in the policy's environment and
is not set `` means a variable is not exported in this shell; `EROFS … '/tmp'` means the temp
variables are missing; `Connection error.` means no network or no CA store in the sandbox.

## Pi

Pi keeps its install under `~/.pi/agent/install` and writes its sessions and auth beside it. Mount
only the install directory read-only, and give Pi a private agent directory to write to, so it
cannot reach your own sessions or logins. The provider key (here OpenRouter) is passed as an
environment variable.

1. Create the agent directory and name it in a private file, with the key:

       mkdir -p ~/.config/dog-walker/pi-agent && chmod 700 ~/.config/dog-walker/pi-agent
       install -m 600 /dev/null ~/.config/dog-walker/pi.env
       cat >> ~/.config/dog-walker/pi.env <<EOF
       OPENROUTER_API_KEY=<your key>
       PI_CODING_AGENT_DIR=$HOME/.config/dog-walker/pi-agent
       EOF
       set -a; . ~/.config/dog-walker/pi.env; set +a

2. Add the profile (replace `/home/you`). The model is Pi's `provider/id` shape, and the effort one
   of Pi's thinking levels (`off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`):

       [profiles.worker]
       harness = "pi"
       model = "openrouter/z-ai/glm-5.3-flash"
       effort = "medium"
       env = ["OPENROUTER_API_KEY", "PI_CODING_AGENT_DIR"]
       secrets = ["OPENROUTER_API_KEY"]
       read_only_dirs = ["/home/you/.pi/agent/install"]
       writable_dirs = ["/home/you/.config/dog-walker/pi-agent"]

   `read_only_dirs` is `<agent>/install`, not all of `<agent>`: the rest holds your sessions and
   credentials. For another provider, list its key (`DEEPSEEK_API_KEY`, …) instead and use its
   name in the model.
3. Check: `dog-walker run worker "Reply with the word ok."` ends `done`.

### If it fails

`EROFS` on `auth.json` means `PI_CODING_AGENT_DIR` is missing or not in `writable_dirs`; a missing
install directory means `read_only_dirs` is wrong; `Connection error.` on Fedora-like systems
needs a dog-walker that mounts `/etc/pki/tls/certs`. The reason is in `harness.log` next to the
handover.

## Codex

Codex keeps its login in its home (`~/.codex/auth.json`, a ChatGPT login, or `OPENAI_API_KEY`),
and the sandbox mounts neither your home nor the real `~/.codex` (history, past sessions,
credentials). So give it a private `CODEX_HOME`:

1. Create it and copy the login into it, once:

       mkdir -p ~/.config/dog-walker/codex-home && chmod 700 ~/.config/dog-walker/codex-home
       install -m 600 ~/.codex/auth.json ~/.config/dog-walker/codex-home/auth.json

   The copy is a credential like the original: keep it mode 600. Codex may rotate its refresh
   token in place, so if your normal Codex asks you to sign in again, copy it again (or log in
   from `CODEX_HOME=~/.config/dog-walker/codex-home codex login`). With an API key, skip the copy and
   list `OPENAI_API_KEY` in `env` and `secrets` instead.
2. Name the directory in a file, and load it before each run:

       echo "CODEX_HOME=$HOME/.config/dog-walker/codex-home" > ~/.config/dog-walker/codex.env
       set -a; . ~/.config/dog-walker/codex.env; set +a

3. Add the profile (replace `/home/you`; the model is any Codex model your login can use):

       [profiles.codex]
       harness = "codex"
       model = "gpt-6.1-terra"
       effort = "low"
       env = ["CODEX_HOME"]
       read_only_dirs = ["/home/you/.npm-global/lib/node_modules/@openai/codex"]
       writable_dirs = ["/home/you/.config/dog-walker/codex-home"]

   `read_only_dirs` is Codex's install directory (`readlink -f $(which codex)` shows where; the
   `node` it needs is a system program and is mounted already). The adapter passes
   `--dangerously-bypass-approvals-and-sandbox` because dog-walker's sandbox is the boundary.
4. Check: `dog-walker run codex "Reply with the word ok."` ends `done`.

## dsh

dsh keeps its provider key in `~/.dsh/.env` (`DEEPSEEK_API_KEY`) and its state in `$DSH_HOME`. Use a
private `DSH_HOME` with the shipped one-task profile, and pass the key as an environment variable:

1. Create the home and the profile, and put the key and the settings dsh needs in a private file:

       D=~/.config/dog-walker
       mkdir -p $D/dsh-home/profiles $D/dsh-home/tmp && chmod 700 $D/dsh-home
       cp -r ~/.dsh/profiles/headless $D/dsh-home/profiles/
       install -m 600 /dev/null $D/dsh.env
       grep '^DEEPSEEK_API_KEY=' ~/.dsh/.env >> $D/dsh.env
       cat >> $D/dsh.env <<EOF
       DSH_HOME=$D/dsh-home
       TMPDIR=$D/dsh-home/tmp
       EOF
       set -a; . $D/dsh.env; set +a

   Do not copy `~/.dsh/.credentials.yaml`: it is a browser-session secret and is not needed.
2. Add the profile (replace `/home/you`):

       [profiles.dsh]
       harness = "dsh"
       dsh_profile = "headless"
       model = "deepseek"          # a label: the model lives in the dsh profile
       env = ["DEEPSEEK_API_KEY", "DSH_HOME", "TMPDIR"]
       secrets = ["DEEPSEEK_API_KEY"]
       read_only_dirs = ["/home/you/.npm-global/lib/node_modules/@deepseek-ai/dsh"]
       writable_dirs = ["/home/you/.config/dog-walker/dsh-home"]

3. Check: `dog-walker run dsh "Reply with the word ok."` ends `done`, and `harness.log` has no
   `ENOENT` or `landlock` lines.

Why the extra variables:

- `TMPDIR`: dsh makes a temp directory for its output spill and its shell sandbox, and the
  sandbox mounts no `/tmp`. Without it the task can still end `done`, but dsh's shell tool is
  broken (`ENOENT … mkdtemp '/tmp/dsh-spill-XXXXXX'`) and the model works around it.
- `DSH_PERMISSION_MODE=danger-full-access`: the adapter sets this itself (D16); you do nothing. dsh's
  default is `workspace-write` with approvals set to `ask`; nobody can answer in a headless run,
  and its own shell sandbox (`landlock`) cannot start inside bwrap (`landlock-run: cannot open
  rule path: /tmp`). This is dsh's documented mode for an outer sandbox, and dog-walker's is the
  boundary (D2). A profile that lists `DSH_PERMISSION_MODE` in `env`, or sets its own `command`,
  gets its own value instead.

## A reviewer that runs cargo

A reviewer that can only read a diff misses what a compiler and one test would show (a test
that cannot fail, a formatting error). To let a reviewer on any harness run cargo, add to its
profile:

1. **The toolchain, read-only.** `rustup` proxies in `~/.cargo/bin` need the whole of `~`, so
   mount one toolchain and put its `bin` first on `PATH`:

       T=$(rustup toolchain list | head -1 | cut -d' ' -f1)   # e.g. stable-x86_64-unknown-linux-gnu
       # read_only_dirs += ["/home/you/.rustup/toolchains/<that name>"]

   `PATH` always reaches the sandbox from your shell, so set it in the profile's env file:
   `PATH=$HOME/.rustup/toolchains/$T/bin:$PATH`.
2. **A cargo home with the crates already downloaded.** Inside the sandbox cargo cannot reach
   `~/.cargo`. Use a directory in the project (for example the `.cargo-home` a `cargo fetch` with
   `CARGO_HOME=$PWD/.cargo-home` fills; keep it in `.gitignore`), and add `CARGO_HOME` and
   `CARGO_NET_OFFLINE=true` to `env` and to the env file. Run that `cargo fetch` again whenever
   `Cargo.lock` changes: a reviewer on an older cargo home fails offline with "package ... is
   unavailable" (found when a new dependency landed, #61).
3. **A temporary directory.** The sandbox has no `/tmp`, and tests that use the filesystem fail
   with a read-only error without one. Create one in the same place and name it:

       mkdir -p $PWD/.cargo-home/tmp
       # env file: TMPDIR=$PWD/.cargo-home/tmp      # and "TMPDIR" in the profile's env

4. **The target directory** is the project's `target/`, writable like the rest of the project.
   Build output, the cargo home and the temporary directory are the only things the reviewer
   writes besides its result, and all three are ignored by git.

`dog-walker doctor <profile>` checks the directories and variables; then ask the reviewer to
run `cargo --version` and one named test. The sandbox cannot nest `bwrap`, so the tests that
need a sandbox report a skip there; CI still runs them.
