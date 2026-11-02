# Requirements: delegating tasks between LLM harnesses

## Goal

Let a developer with several LLM harnesses delegate a task from a main harness (for example Claude Code) to a secondary one (Pi, Claude Code, dsh, Codex) in a clear, controlled and trackable way. Every task leaves a handover document in the repo.

## Scope

A Linux CLI, `dog-walker`. Main-to-secondary delegation only. Blocking runs. No server, daemon or telemetry. ACP is not used in v1; the adapter interface leaves room for it later.

## Problem

Different models work best in different harnesses: Claude in Claude Code, DeepSeek in dsh, others in Pi or Codex. Moving a task between them today is manual, uncontrolled and leaves no record.

## Solution

Profiles in a config file (harness, model, thinking effort, sandbox, timeout), one marked main. The main harness calls `dog-walker run <profile>` with a brief. The secondary runs unattended inside an OS-level sandbox and writes its result into a handover document that dog-walker owns and tracks.

## Requirements

**Configuration**
- R1 Config defines harness profiles: harness (Pi, Claude Code, dsh, Codex), model, thinking effort, sandbox policy, timeout.
- R2 Exactly one profile is main; the others are secondary. `run` rejects the main profile as a target unless `--allow-main` is given; `list` shows the main profile first.
- R3 Config is user-owned. A project config or a task may only tighten the sandbox, never loosen it.
- R4 Each harness sits behind an adapter; adding one touches nothing else. ACP is allowed later, not used in v1.

**Running a task**
- R5 `run <profile>` with a task (text, file or stdin) starts the secondary non-interactively in the project directory and blocks until it ends.
- R6 `run` prints the handover path on stdout, logs to stderr, exits 0 only if the task is `done`.
- R7 The secondary never prompts; anything outside its policy fails the task.
- R8 No delegation back: a secondary cannot start tasks.
- R9 Concurrent tasks are allowed by default; a limit can be set per harness and model; a task over the limit is queued; overlapping write scopes produce a warning.

**Handover**
- R10 `.dog-walker/tasks/<id>-<slug>/handover.md`, ids a zero-padded counter per repo. Tracked or ignored through the user's `.gitignore`.
- R11 dog-walker writes the brief (goal, context, constraints, done-when) and TOML front matter (between `+++` lines) with the id, the profile and the status; the secondary writes the result (what it did, files changed, what is open).
- R12 Status: created, running, then done, failed, timed-out or cancelled. Only dog-walker writes it.
- R13 A missing or malformed result marks the task failed with a reason; raw output stays in a log beside the handover; no result is invented.
- R14 A task can be cancelled (Ctrl-C on `run`, or `cancel <id>`) and times out after the profile's timeout (default 30 minutes, configurable per harness and model).

**Security**
- R15 The sandbox is enforced at OS level (bwrap), not by harness flags.
- R16 Default policy: read and write only inside the project directory; network on, switched off per profile with `network = false`; no git commit or push; cleaned environment with an allowlist.
- R17 If the sandbox tool is missing, the run is refused unless the user explicitly opts out.
- R18 Configured secret values are redacted from the handover and logs.
- R21 `dog-walker instructions` prints a short text for the main harness: how to write a brief, call `run`, read the exit code and find the handover.
- R19 No server, daemon or telemetry; dog-walker makes no network request itself.
- R16a By default the project's `.git` is read-only inside the sandbox, so commit and push fail at OS level while `git log` and `git diff` work. A profile can list `.git` as writable (R20); push still fails without credentials.
- R20 A profile can list extra accessible directories (read-only or writable) and the environment variables passed through, on top of what its adapter declares. The handover records the network setting, the extra directories and the variable names.

## Testing decisions

Acceptance tests run offline against a fake harness: no real model, no network. The seam is the CLI: command in, handover file, exit code and sandbox effects out.

## Out of scope

Delegation back from a secondary. A background or pollable mode. ACP. Cost budgets. A UI.

## Done when (acceptance tests)

- [ ] AT1 (R1) A profile's model and effort reach the launched harness.
- [ ] AT2 (R1) A profile missing a required field fails config load and names the field.
- [ ] AT3 (R2) Zero or two main profiles fail config load; exactly one loads. `run` on the main profile is rejected unless `--allow-main` is given.
- [ ] AT4 (R3) A project or task setting that loosens the policy is rejected and named; a stricter one is accepted.
- [ ] AT5 (R4) A new fake adapter works with no core change; no adapter or dependency speaks ACP.
- [ ] AT6 (R5) A task given as text, file or stdin starts non-interactively in the project directory and `run` blocks until it ends.
- [ ] AT7 (R6) `done`: stdout is exactly the handover path, logs on stderr, exit 0. Any other status: non-zero exit, path still on stdout.
- [ ] AT8 (R7) A permission request outside policy is denied with no prompt; the task ends failed and the handover records the violated rule.
- [ ] AT9 (R8) A secondary calling `run` is refused and its task is unaffected.
- [ ] AT10 (R9) With no limit, three concurrent tasks run in separate task folders.
- [ ] AT11 (R9) With a limit of 1 for a harness and model, a second task is queued and starts when the first ends; other harnesses are unaffected.
- [ ] AT12 (R9) Two running tasks with overlapping write scopes produce a warning naming both ids.
- [ ] AT13 (R10) A new task creates its handover path; ids are zero-padded counters, never reused; `.gitignore` is untouched.
- [ ] AT14 (R11) The handover has front matter and a brief before the harness starts, and the result after a successful run.
- [ ] AT15 (R12) Status follows created, running, then exactly one end state; a status edited by the harness is overwritten with the real one.
- [ ] AT16 (R13) A missing or malformed result gives failed with a reason, raw output in a log beside the handover, and no invented result.
- [ ] AT17 (R14) A task past its timeout (default 30 minutes, per harness and model) has its process tree killed and ends timed-out.
- [ ] AT18 (R14) Ctrl-C on `run` and `cancel <id>` each kill the process tree and end cancelled; cancelling a finished task changes nothing.
- [ ] AT19 (R15, R16) A write outside the project directory fails at OS level whatever the harness flags say; the denial is recorded in the handover.
- [ ] AT20 (R16) With `network = false` the network is unreachable; under the default policy `git commit` and `git push` fail; denials are recorded.
- [ ] AT21 (R16) A non-allowlisted environment secret is invisible to the harness; an allowlisted variable is visible.
- [ ] AT22 (R17) Without bwrap, `run` refuses; with the explicit opt-out it runs and the handover records the unsandboxed run.
- [ ] AT23 (R18) A configured secret value is absent from the handover and logs.
- [ ] AT24 (R19) A normal run opens no listening socket, starts no daemon and makes no network request of its own.
- [ ] AT26 (R16a) By default `git commit`, `git add` and `git push` fail inside the sandbox while `git log` and `git diff` work; with `.git` listed writable in the profile, `git commit` works.
- [ ] AT27 (R21) `dog-walker instructions` prints text that names `run`, the exit code and the handover location.
- [ ] AT25 (R20) Extra directories and passed variables from a profile are available to the harness; the handover lists them, the network setting and the variable names.
