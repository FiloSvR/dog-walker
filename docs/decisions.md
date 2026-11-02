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
