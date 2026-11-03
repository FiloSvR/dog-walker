# Delegating a task with dog-walker

You are the main harness. Use `dog-walker` to hand a self-contained task to a secondary
harness, which works in a sandbox and leaves a handover document.

## 1. Write the brief

State, in plain words, what the secondary needs and nothing else:

- **Goal**: what to achieve and why.
- **Context**: what it must know that it cannot find in the repository.
- **Constraints**: what it must not do or touch.
- **Done when**: a checklist you can verify afterwards.

dog-walker adds one line after your brief, telling the secondary to write its result to
`$DOG_WALKER_RESULT_PATH`; you do not need to. The handover records your brief as you wrote it.

## 2. Run it

    dog-walker run <profile> "<the brief>"
    dog-walker run <profile> --file brief.md
    dog-walker run <profile> -              # the brief on stdin

A profile sets a `command` to say exactly what to run; without one, its harness's adapter
builds the command line (Claude Code, Pi, Codex and dsh).
A Claude Code profile lists its own credential in `env` and `secrets`: `ANTHROPIC_API_KEY`, or
`CLAUDE_CODE_OAUTH_TOKEN` from `claude setup-token` for a subscription. It also needs Claude's
install directory in `read_only_dirs`, a private `CLAUDE_CONFIG_DIR` in `writable_dirs`, and a
`CLAUDE_CODE_TMPDIR` and `TMPDIR` inside that directory: the sandbox has no `/tmp`, and Claude
Code fails without one. `docs/setup.md` has the steps.

A Pi profile's model is Pi's `provider/id` shape (`deepseek/deepseek-chat`), its effort is one
of Pi's thinking levels (`off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`), and its
sandbox needs Pi's install directory (`<agent>/install`, not all of `<agent>`) in
`read_only_dirs` and its provider key in `env`.

A Codex profile's model is a Codex model (`gpt-5-codex`), its effort a `model_reasoning_effort`
(`minimal`, `low`, `medium`, `high`), and its sandbox needs Codex's install directory in
`read_only_dirs` and a writable configuration directory (`CODEX_HOME`, or `$HOME/.codex`) in
`writable_dirs`, with its key in `env`; `--ephemeral` keeps session files out of it.

A dsh profile names the dsh profile to boot with `dsh_profile` (`headless` is the shipped
one-task profile and the default); dsh's model and tools live in that profile, so `model` and
`effort` are only labels there. The adapter sets `DSH_PERMISSION_MODE=danger-full-access` itself,
since dog-walker's sandbox is the boundary. Its sandbox needs dsh's install directory in
`read_only_dirs` and `HOME` (the adapter declares it); a custom `DSH_HOME` also goes in `env`, and
the directory it names (default `$HOME/.dsh`) in `writable_dirs`, as does dsh's provider key in
`env`, and a `TMPDIR` inside it, since the sandbox has no `/tmp`. `docs/setup.md` has the steps.

`dog-walker profiles` lists the profiles, the main one first. `run` blocks until the task
ends. It prints the path of the handover on stdout and logs on stderr. Running the main
profile itself needs `--allow-main`.

The task runs in a sandbox: it sees the project, the system's programs, and only the
directories and variables its profile lists (`read_only_dirs`, `writable_dirs`, `env`). A
profile can also name `secrets`: the values those variables hold (they must also be in `env`)
are replaced with `[redacted]` in the handover and the log, though a value the harness invents
or transforms is not caught. A
profile can cut the network (`network = false`) and make `.git` writable
(`git = "writable"`). If bwrap is missing, `run` refuses; `--unsandboxed` runs without it and
the handover says so.

Several tasks can run at once. The user's config can limit them per harness and model with a
top-level table:

```toml
[limits]
"pi/local/qwen" = 1
```

A run over its limit waits for a slot, says so on stderr, and starts when one frees; while it
waits it has a task folder and the status `created`, and `cancel <id>` or Ctrl-C ends it before
it starts. With no limit, any number run. Two tasks running at once write the same project, so
the second is warned with both ids.

A task should not start tasks of its own: a `run` called from inside one is refused by the
marker it gets (and, outside the sandbox, by the dog-walker above it in the process tree), and
the task that called it is unaffected.

A repository can tighten the policy in `.dog-walker/config.toml` (the same keys: `network`,
`git`, `read_only_dirs`, `writable_dirs`, `env`), and a task can tighten `network` and `git`
with `--network off` and `--git read-only`. Neither can loosen what the profile allows: a
read-only directory can be added only inside the project, a writable directory, or one the
profile already keeps read-only.

A task is stopped when its profile's `timeout_minutes` passes (30 minutes by default; it
takes a fraction of a minute, so `0.5` is 30 seconds, up to 525600 for a year), or sooner with
`dog-walker cancel <id>` from another shell in the project directory. Either way the harness
and the processes in its process group are killed, and the handover says `timed-out` or
`cancelled`.

## 3. Read the exit code

- exit code 0: the task ended `done`.
- exit code 1: the task ended `failed`, `timed-out` or `cancelled`.
- exit code 2: bad usage, a config error, or a failure after the task was created.

Whatever the exit code, stdout holds the handover path when a task was started.

## 4. Read the handover

Each task has its own folder: `.dog-walker/tasks/<id>-<slug>/handover.md`. Its front matter
holds the status; the brief is what you wrote; the Result section is what the secondary
reports: what it did, the files it changed, what is open. Next to it, `result.md` is what the
secondary wrote and `harness.log` is its raw output, kept even when the run fails. Review the
changes yourself before you rely on them. The secondary cannot commit; you do.
