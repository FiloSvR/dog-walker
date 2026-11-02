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
   bwrap the harness sees `/bin`, `/sbin`, `/usr`, `/lib`, `/lib64`, `/proc`, `/dev`, its own
   program if that lives elsewhere, the project, and the directories the profile lists. The
   root is remounted read-only, so a write anywhere else fails rather than landing in an empty
   directory, and `/tmp` and the home directory are not mounted at all. The sandbox's `/dev` is
   its own small writable device directory, never the host's. The rejected
   alternative, `--ro-bind / /` with the project writable, leaves the whole host readable,
   while R16 says read and write only inside the project, and hides nothing, which is D2's
   point. The cost is that a harness needing configuration in `$HOME` or scratch in `/tmp`
   must have those directories listed; adapters (#10) can declare them.
   Discussion: issue #4.
