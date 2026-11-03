# Decisions

Numbered entries; each is hard to reverse, surprising without context, and a
real trade-off. Discussion is on the issue named at the end of each entry.

## Decisions

1. **Rust** (D1, 2026-11-02). dog-walker is written in Rust, as a single static-friendly binary
   with a short, vetted list of dependencies and no async runtime. This serves the small,
   auditable, no-server goal (R19) and gives reliable process-tree control for timeouts and
   cancel. Rejected: Python (standard library only) and Node or Bun, easier to start but a
   weaker fit for a security tool that ships as one artifact; Bash, too fragile.
   Discussion: issue #2.
2. **The sandbox is bwrap, and only bwrap, in v1** (D2, 2026-11-02). Landlock only restricts
   access: the harness would still see the host's files, and it cannot clean the environment.
   bwrap hides the rest of the filesystem, can cut the network, clears the environment and dies
   with its parent. Landlock may return later as a second backend behind the same interface.
   Discussion: issue #2.
3. **Network is on by default and switched off per profile** (D3, 2026-11-02). A profile sets
   `network = false` to cut it; the handover records whether a run had network. This reverses
   the more cautious "off by default": hosted-model harnesses (Claude Code, Codex) cannot work
   without network, and a default that every real profile overrides adds noise rather than
   safety. The cost is that a secondary can reach any host unless the profile says otherwise,
   which is why the file-system, git and environment limits stay on by default.
   Host-level allowlisting is out of scope for v1.
   Discussion: issue #2.
4. **Harnesses are launched through their own headless commands, not ACP** (D4, 2026-11-02).
   Each adapter wraps the harness's documented non-interactive mode. ACP adapters would be
   third-party code between dog-walker and the harness, `dsh` has none, and they carry no
   model or thinking-effort settings. The adapter interface leaves room for an ACP adapter later.
   Discussion: issue #2.
5. **Handover front matter is TOML, not YAML** (D5, 2026-11-02). The handover starts with a
   TOML block between `+++` lines, parsed by the `toml` crate already used for config, so no
   YAML dependency. A reader used to YAML front matter will expect `---`; some viewers render
   it as plain text instead of a table. Changing it later means migrating every handover
   already in users' repos.
   Discussion: issue #2.
6. **"No git commit or push" is enforced by a read-only `.git`** (D6, 2026-11-02). Inside the
   sandbox the project is writable but its `.git` is bound read-only, so commits, stashes and
   pushes fail at OS level while reading history still works. The main harness reviews the
   secondary's diff and commits it, which keeps history and authorship under its control.
   A profile can list `.git` as writable to allow commits; push still needs credentials,
   which are not mounted by default. Rejected: hiding `.git` (breaks `git status`) and
   matching command names (bwrap cannot).
   Discussion: issue #2.

7. **Cancel is a marker the runner watches, not a signal from the canceller** (D7, 2026-11-03).
   `cancel <id>` writes `.cancel` into the task folder and exits; the runner, which polls its
   child and that marker, kills the harness's process group and closes the handover as
   `cancelled`. The canceller needs no pid file, no permission to signal the runner and no
   access to its process group, and it cannot hit a recycled pid; a task that has already ended
   is read from its handover and left untouched. The runner installs a Ctrl-C and a TERM
   handler for the same reason: whoever stops the run, the handover is closed. Rejected:
   `kill` from the canceller. Discussion: issue #5.

8. **The sandbox shows the system and the project, and nothing else** (D8, 2026-11-03). Inside
   bwrap the harness sees `/bin`, `/sbin`, `/usr`, `/lib`, `/lib64`, `/proc`, `/dev`, the
   resolver's files and the certificate store, its own program if that lives elsewhere, the
   project, and the directories the profile lists. The
   root is remounted read-only, so a write anywhere else fails rather than landing in an empty
   directory, and `/tmp` and the home directory are not mounted at all. The sandbox's `/dev` is
   its own small writable device directory, never the host's. The rejected
   alternative, `--ro-bind / /` with the project writable, leaves the whole host readable,
   while R16 says read and write only inside the project, and hides nothing, which is D2's
   point. The cost is that a harness needing configuration in `$HOME` or scratch in `/tmp`
   must have those directories listed in its profile (`read_only_dirs`/`writable_dirs`); the
   adapter declares only argv and variable names.
   Discussion: issue #4.

9. **A project or a task may only tighten the policy, and a directory is judged by its mount**
   (D9, 2026-11-03). The profile is the user's policy; a project's
   `.dog-walker/config.toml` and a task's `--network`/`--git` flags are compared field by field
   with it (and a task with the project-tightened policy) and refused when they would widen
   it, naming the field and the origin. A directory is judged by what its mount does: adding a
   read-only directory inside the project or a writable directory takes write access away and
   is allowed, adding one elsewhere is more to read and is not, and dropping one that protects
   the project or a writable directory gives write access back and is refused. Rejected:
   merging the lists, which would let a repository grant itself access to the machine.
   Discussion: issue #6.

10. **Secrets are redacted because they are named, not because they look like secrets**
    (D10, 2026-11-03). A profile lists `secrets = ["ANTHROPIC_API_KEY"]` next to the `env`
    names; every value those names hold in the runner's environment is replaced with
    `[redacted]` in the handover and in `harness.log` once the run ends, and the handover lists
    the names. Guessing by name suffix was rejected: it would either miss a secret with an
    unhelpful name (`env = ["TOKEN"]`) or shred ordinary output (`HOME`), and a list makes the
    decision the user's. The cost is that a value the harness invents or transforms is not
    caught: the sandbox limits what the harness can read, not what it can print, and the
    redaction only removes the exact values it was given. Discussion: issue #8.

11. **The delegation guard is a marker and, outside the sandbox, an ancestry check**
    (D11, 2026-11-03). A `run` refuses when `DOG_WALKER_TASK_DIR` names an existing directory or
    when an ancestor process is a dog-walker. The variable is what a harness can clear, so on
    its own it is a reminder, not a barrier; the ancestry is outside the harness's control,
    but the sandbox gives the harness a PID namespace of its own, where the outer runner is
    not visible, so inside the sandbox only the marker holds. A harness that clears the
    variable and can reach a dog-walker binary (its own repo's `target/`, a system-wide
    install) can therefore still delegate. Rejected: a harder mechanism (a secret fd, a
    sentinel the runner binds read-only), which also needs a channel the harness can drop.
    R8 is read as "a secondary is not a delegator by default", and this guard is what a
    well-behaved harness and a wrapper meet. Discussion: issue #9.

12. **A concurrency limit is advisory, and a slot is a record of the runner's own process**
    (D12, 2026-11-03). `.dog-walker/run/<id>.slot` holds the runner's pid and its start time,
    so a reused pid is not a live runner; another runner counts the records whose runner is
    alive and waits while the count for its harness and model is at the limit. The record is
    written through a temporary file and the directory is mounted read-only inside the sandbox:
    a harness cannot plant, remove or block a record. Two runners can read the same free slot
    and both start, so a limit of one can briefly be two. Rejected: a lock or semaphore that
    would make the limit exact, because it serialises starts and needs a slot count the
    per-harness limits do not have, and because the requirement is a limit that queues best
    effort, not a scheduler. A stale record (the runner died) is cleaned by the next runner
    that sees it. Discussion: issue #7.

13. **An adapter is an argv builder behind one trait, and a profile's own command wins**
    (D13, 2026-11-03). `adapter` builds the harness's argv from the profile (the program, the
    model, the effort, and the flags that make the harness unattended) and declares the
    variables it needs by name; the launcher appends the brief as the last argument, whatever
    the adapter, and closes stdin. A profile with a `command` keeps working: it is the escape
    hatch, what the tests use, and what a harness without an adapter needs. Rejected: an
    adapter that owns the process (spawning, waiting, capturing) — it would duplicate the
    launcher's process-group, timeout and cancel logic, and the seam would no longer be
    testable with a stub on `PATH`. Discussion: issue #10.

14. **The launcher tells the secondary where its result goes, and the Claude adapter requires no credential**
    (D14, 2026-11-03). Found by delegating real tasks. (a) Only the brief reached the harness, so a
    brief that forgot to name `$DOG_WALKER_RESULT_PATH` turned a correct answer into a `failed`
    task with no result. The launcher now appends one fixed line to the brief it passes
    (`launch::prompt`); the handover still records the brief as written. Leaving it to the main
    harness's instructions was rejected: the failure is silent and costs a paid run.
    (b) The Claude adapter demanded `ANTHROPIC_API_KEY`, so a subscription token needed a `command`
    that copied the adapter's argv. It now requires nothing, like Pi: the profile lists its own
    credential (`ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`) in `env` and `secrets`. A profile
    that relied on the adapter adding the key must now list it.

15. **The records keep themselves out of git, and `result.md` is redacted like the log**
    (D15, 2026-11-03). Found by delegating a real task whose `result.md` held the API key: with
    `.dog-walker` not ignored, `git add .` would have committed it. The first task now writes
    `.dog-walker/.gitignore` (`/tasks/`, `/ids/`, `/run/`, itself), which leaves the project's own
    `.gitignore` alone (AT13) and `.dog-walker/config.toml` committable, and `result.md` is
    rewritten after the run with the secret values replaced, like `harness.log`. Rejected:
    `*` in the folder's `.gitignore` (it would hide the project's policy file); leaving
    `result.md` raw because it is the harness's file (the harness has already ended, and a
    secret in a committed file is worse than an edited copy). A secret the harness transforms is
    still not caught, as before.

16. **An adapter can carry fixed environment values**
    (D16, 2026-11-03). dsh has no argv flag for "an outer sandbox is the boundary", only
    `DSH_PERMISSION_MODE=danger-full-access`; without it a headless run has no working shell
    (its own landlock sandbox cannot start inside bwrap, and its approval prompt has no answerer).
    `Adapter::fixed_env` names variables with a value dog-walker sets, as the Claude Code and
    Codex adapters pass their flags. A profile with its own `command` is not running the adapter,
    and one that lists the name in `env` keeps its own (the host's) value. Rejected: leaving it to
    each user's profile (a silent, hard-to-diagnose failure: the task still ends `done`).

17. **`doctor` runs the sandbox's checks with a probe, never the harness**
    (D17, 2026-11-03, #47). Every harness setup failed the same way: silent or cryptic
    (`Connection error.`, `EROFS … '/tmp'`, a missing variable). `dog-walker doctor <profile>`
    reports all checks, one line each (`ok`, `skip`, `FAIL` with the fix), never a value, and
    exits 1 on any failure. It reuses the run's policy and `bwrap` plan but runs `sh` in it: a
    write in `$TMPDIR` (or the first writable directory), and a `curl` fetch of `--url`
    (default `https://example.com`: it tests the trust store, not a provider), skipped when the
    profile has no network. Rejected: launching the harness (a paid, model-dependent call);
    stopping at the first failure (the user fixes one thing per run). dog-walker still opens no
    connection of its own (R19): the probe program does. It creates no home and copies no
    credential.

18. **Usage is read from the harness's log, through the adapter**
    (D18, 2026-11-03, #58). The point of a good brief is that the worker spends fewer tokens, so
    a task has to show what it spent. Each harness says it differently: Codex ends its output
    with `tokens used`; Claude Code with `--output-format json` prints one result object with
    usage, turns and cost; Pi with `--mode json` prints a `turn_end` event per model call; dsh
    prints nothing readable. `Adapter::usage` parses the redacted `harness.log` after the run and
    the handover's front matter gets `tokens`, `turns` and `cost_usd` where known. The two JSON
    flags make those two logs machine output instead of the final message, which is what
    `result.md` is for. A profile with its own `command` has no usage (nothing says what its
    output means). Rejected: counting tokens ourselves (a tokenizer per model, and wrong);
    reading each harness's session files (private formats, and not in the sandbox); recording
    zero for an unreported run (it would read as a cheap task). The numbers are the harness's
    own claim: the log is untrusted output, so they are for judging a brief, not for billing.

19. **`init` is an explicit installer that writes names, never values**
    (D19, 2026-11-03, #70). Setting up a profile by hand failed silently for every harness, and a
    main harness only follows the brief guidance if something points it at
    `dog-walker instructions`. `dog-walker init` asks per role (`main` required, `worker` and
    `reviewer` optional) for a harness and model, finds the install directory from where the
    program resolves, writes the profile's sandbox part (`docs/setup.md` as code), creates a
    private directory (700) and an env file (600) with the non-secret lines, and appends a marked
    block to the main harness's global instructions file. It shows the plan and asks first
    (`--yes` skips), appends to an existing config and never edits it, and creates a file that
    exists never. It asks for the name of the credential variable and never reads, copies or
    writes a value: the token and the Codex login are printed as steps. Only Claude Code
    (`~/.claude/CLAUDE.md`) and Codex (`~/.codex/AGENTS.md`) have a verified global file; for Pi
    and dsh the line is printed. Rejected: a silent step on install or on the first `run` (it
    would write outside the project unasked, against R19's spirit); copying the Codex login
    (a credential copy the user should choose); re-serialising the config (it would drop the
    user's comments); a project `AGENTS.md` (the profiles are user-level, so is the line).
    Hardening from the review of the first version (task 0099): the config is written last, so a
    run that fails midway records no profile that a second run would skip; a preflight refuses,
    before anything is written, a config path with a control character (it would end a comment in
    the env file and run a command when the file is sourced), a non-UTF-8 path (the config would
    name another one), and a private directory or env file that is not what it should be, is a
    link, or is open to others; an env file or directory that is right is left as it is; the
    config and the instructions file are read again just before they are appended to. A link to
    the instructions file or the config is followed on purpose (a dotfiles repo), and the plan
    says where it leads; the installer runs as the user on the user's own files, so a link there
    is the user's. Not done: a lock between two `init` runs at the same time (the second one
    refuses when it finds the profile there).

