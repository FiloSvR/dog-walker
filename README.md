# dog-walker

Delegate tasks from the LLM harness you work in to other harnesses, sandboxed, and get a
handover document back.

You open one harness yourself (Claude Code, Codex, Pi or dsh): the **main**. When a piece of
work suits a cheaper model, a second opinion, or a review, the main harness runs
`dog-walker run <profile> "<brief>"`. dog-walker starts the **secondary** harness in a
[bubblewrap](https://github.com/containers/bubblewrap) sandbox that sees the project and only
what its profile lists, waits for it, and leaves a record of what was asked, what ran, what
came back and what it cost.

No server, no daemon, no telemetry. It is one binary that runs a command and exits.

## Quick start

You need Linux, `bwrap` (bubblewrap), and one or more harnesses installed. To build you need
Rust 1.99 and a C linker.

    cargo install --path .
    dog-walker init

`init` asks which harness you open yourself, then which harness and model to use for a `worker`
and a `reviewer` (or `no` to skip either). It finds each harness's install directory, writes
the profiles, creates a private directory and an env file for each, and adds a line to your main
harness's instructions file so that it reads `dog-walker instructions` before it delegates. It
first asks whether this is for the project (the default: its own profiles, kept outside the
repository, and its own `CLAUDE.md` or `AGENTS.md`) or global (one config for every project). It
shows what it will write and asks first. For each secondary it asks for the key (typed hidden)
and saves it in that profile's env file, mode 600, which dog-walker loads itself whenever it runs
or checks the profile; for Codex it offers to copy your login. It never reads a key from your
environment, never takes one as an option, and never prints one. Finish with:

    dog-walker doctor worker

`doctor` checks a profile's directories, sandbox, program, variables, a write to the temporary
directory and a TLS fetch inside the sandbox, without starting the harness, and tells you the
fix for each failure. Setup by hand, with the reasons for each piece, is in
[`docs/setup.md`](docs/setup.md).

## Use

    dog-walker run worker "Find where the retry on a busy harness is, and say how many tries."
    dog-walker run worker --file brief.md
    dog-walker run worker --worktree "Refactor the retry."   # its own git worktree
    dog-walker profiles
    dog-walker cancel 0007

`run` blocks until the task ends. It prints the path of the handover on stdout and logs on
stderr, and exits 0 when the task ended `done`, 1 when it ended `failed`, `timed-out` or
`cancelled`, and 2 on a usage or config error.

Each task gets a folder, `.dog-walker/tasks/<id>-<slug>/`, which ignores itself in git:

- `handover.md`: the front matter (id, profile, status, and `tokens`, `turns` and `cost_usd`
  where the harness reports them), the sandbox the task ran under, your brief, and the result.
- `result.md`: what the secondary wrote.
- `harness.log`: its raw output, kept even when the run fails.

The secondary cannot commit (`.git` is read-only unless the profile says otherwise); you review
what it changed and you commit. Tasks that write at the same time share the checkout and can
overwrite each other: give each `--worktree`, a git worktree of its own at HEAD outside the
repository, which stays for you to diff and merge.

## What the sandbox allows

A task sees the project directory and the system's programs. It does not see your home. A
profile lists the extra directories it may read or write (`read_only_dirs`, `writable_dirs`),
the environment variable *names* it receives (`env`; the values come from your shell), the
variables whose values are redacted from the handover and the log (`secrets`), and whether it
has the network. A repository can tighten the policy in `.dog-walker/config.toml`; it cannot
loosen what a profile allows. A task cannot start tasks of its own. If `bwrap` is missing, `run`
refuses unless you pass `--unsandboxed`, and the handover then says so. To run without the
sandbox every time, put `sandbox = false` at the top of the config (a project's own config wins
over the global one; the default is `true`).

## Writing a good brief

The secondary starts with nothing but the brief, and every file it opens costs it tokens. The
main harness has usually paid for exploration already, so the brief is where that is handed over:
the files and line ranges that matter, what is settled, what to skip, how to verify, and the
format of the result. `dog-walker instructions` prints this guidance for the main harness,
including when a short brief is the right one. Measured on one question that spans three files,
a brief with line ranges cost a Pi worker about a fifth of the tokens of a bare one
(`manual-tests/expected.md` has the runs and the caveats).

## Harnesses

Claude Code, Codex, Pi and dsh each have an adapter. A profile can also set its own `command`
to run anything else. Token usage is read from the output of Claude Code, Codex and Pi; dsh
reports none that dog-walker can read.

## Development

    cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo deny check
    cargo test --test <name>

CI runs the whole suite; locally, run one test file at a time. The sandbox tests need a working
`bwrap`. [`AGENTS.md`](AGENTS.md) points to the project's conventions, and `docs/` has the
requirements, architecture, decisions and glossary. `manual-tests/` has briefs that exercise a
real harness and model by hand.

## License

MIT, see [`LICENSE`](LICENSE).
