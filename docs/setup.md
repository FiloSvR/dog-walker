# Setting up harnesses

A profile runs inside a sandbox that sees the project, the system's programs and only what the
profile lists: `read_only_dirs`, `writable_dirs` and `env` (variable names, whose values come from
the shell that runs `dog-walker`). So each harness needs its install directory, its credentials
and a place to write, named in its profile. `dog-walker instructions` has a summary per harness;
this page has the steps.

## Claude Code with a subscription (no API key)

The Claude Code adapter reads `ANTHROPIC_API_KEY` and `run` refuses when it is unset. With a
Claude subscription there is no key, so the profile sets `command` instead (a profile with its
own `command` does not get the adapter's variables) and authenticates with a token.

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
       command = ["claude", "-p", "--dangerously-skip-permissions", "--model", "claude-sonnet-5-5", "--effort", "high", "--"]
       env = ["CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CONFIG_DIR", "CLAUDE_CODE_TMPDIR", "TMPDIR", "HOME"]
       secrets = ["CLAUDE_CODE_OAUTH_TOKEN"]
       read_only_dirs = ["/home/you/.local/share/claude"]
       writable_dirs = ["/home/you/.config/dog-walker/claude-config"]

   `command` repeats the adapter's arguments (`claude -p --dangerously-skip-permissions --model …
   --effort … --`, the brief is appended last), so keep it in step with `src/adapter.rs`.
   `secrets` makes dog-walker replace the token with `[redacted]` in the handover and the log.

4. **Load the variables, then run**, in the same shell:

       set -a; . ~/.config/dog-walker/reviewer.env; set +a
       dog-walker run reviewer "Reply ok. Write your result to the file named by \$DOG_WALKER_RESULT_PATH, and nowhere else."

   The task should end `done` with `ok` in the handover. A tool that starts a fresh shell for
   every command (some agents do) must source the file in the same command as `dog-walker`.

### Why each piece is there

- `command`: the adapter would refuse for want of `ANTHROPIC_API_KEY`.
- A private `CLAUDE_CONFIG_DIR` in `writable_dirs`: the sandbox does not mount your home, and
  this keeps Claude Code away from `~/.claude` (history, projects). It starts empty, so it is
  not signed in; the token does that.
- `CLAUDE_CODE_TMPDIR` and `TMPDIR` inside it: the sandbox mounts no `/tmp`, and without a temp
  directory Claude Code ends with `EROFS: read-only file system, mkdir '/tmp'`.
- `read_only_dirs` on `~/.local/share/claude`: the install directory (check where `claude`
  resolves with `readlink -f $(which claude)` if yours is elsewhere).

### With an API key instead

Drop `command`, the token and the `TMPDIR` lines; list `ANTHROPIC_API_KEY` in `env` and
`secrets`, and keep the install directory, `CLAUDE_CONFIG_DIR` and `writable_dirs`.

### If it fails

The reason is in `harness.log` next to the handover. `` `X` is in the policy's environment and
is not set `` means a variable is not exported in this shell; `EROFS … '/tmp'` means the temp
variables are missing; `Connection error.` means no network or no CA store in the sandbox.
