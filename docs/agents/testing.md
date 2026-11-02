# Testing and standing rules

Read by `change`, `architect` and `plan`.

## Layers

| Suffix or place | Layer | Use for |
|---|---|---|
| `#[cfg(test)]` modules in `src/` | unit | pure logic: config parsing, status transitions, handover rendering |
| `tests/` | acceptance | the compiled binary run against a fake harness; one test per AT in `docs/requirements.md` |

`tests/review_guard.rs` is another exception, like `tests/architecture.rs`: it runs the
`review-guard` step, which lives inline in `.github/workflows/review-guard.yml`, against
a stub `gh` and the real `jq`.

The fake harness is a small script that a test writes and a profile's `command` points at:
its first argument is the behaviour (write a result, write a malformed one, do nothing, hang —
it writes its pid and its child's into `pids` — edit the handover) and the brief arrives as its
last argument, so `tests/run.rs` exercises the
same launcher a user configures. It is not part of the binary, and there are no test-only
variants in it.

Sandbox tests use the real `bwrap`, since
they check OS-level effects; they are skipped with a loud message when `bwrap`
or user namespaces are missing, unless `DOG_WALKER_REQUIRE_SANDBOX` is set to any
non-empty value, `0` included (CI sets `1`), in which case a missing sandbox fails them. Every sandbox
test starts with the `sandbox_available` helper, copied in `tests/cli.rs` and `tests/run.rs`
(the test binaries cannot share it); today the tests are `at24_works_without_network` and
`at24_a_run_works_without_network`. A skipped test still reports `ok`; its SKIPPED line is printed to stderr, which cargo
shows even for a passing test. Everything runs offline. Only the acceptance tests
carry an AT id in the name, and a failing test prints the fake harness's output.

## Commands

- One test file or folder: `cargo test --test <name>`, or `cargo test <filter>`. Never the whole suite; CI is the gate.
- Check (format, lint, dependency rules): `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo deny check`.
- Format fix: `cargo fmt`.
- Tools: `cargo install --locked cargo-deny`, and `jq` for `tests/review_guard.rs`.
  Building any crate needs a C linker (`gcc`).

## CI

`.github/workflows/ci.yml` runs on every pull request and every push to `main`:

- `check`: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo deny check`.
- `test`: the whole suite, `cargo test --no-fail-fast`.

The sandbox tests run on the runner, not skipped: the `test` job installs `bubblewrap` and
switches off Ubuntu's AppArmor restriction on unprivileged user namespaces, and sets
`DOG_WALKER_REQUIRE_SANDBOX=1`, which turns a skip into a failure, so a runner that cannot
sandbox is noticed. Third-party actions are not used; the two official ones (`actions/checkout`,
`actions/cache`) are pinned to a commit. Only downloaded crates and the `cargo-deny` binary are
cached, never build output, so a poisoned cache cannot become a build.

## Dependencies

Rust 2024, pinned by `rust-toolchain.toml` and `rust-version`. Allowed by default:
`clap`, `serde`, `toml`, `serde_json`, `thiserror`, a thin OS library such as `rustix`, and
`signal-hook` for the Ctrl-C flag. No async runtime and no `anyhow`. Every crate is added with
`default-features = false` and only the features it needs; versions are exact,
`Cargo.lock` is committed, and `cargo deny` runs in CI. A new dependency needs
one line in the pull request saying why.

## Standing rules

General:

- Tests run offline. No real credentials in the repo.
- No server, daemon or telemetry, and no network request made by dog-walker itself.
- Linux only: anything else is a `compile_error!`.

Enforced by tooling (`Cargo.toml` `[lints]`, `clippy.toml`, `deny.toml`; the check command above):

- `#![forbid(unsafe_code)]`. Needing `unsafe` is a decision entry first; syscalls go through a safe crate.
- clippy denies `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `dbg_macro`,
  `indexing_slicing`, `print_stdout` and `print_stderr` (tests are exempt), and warns on
  `pedantic` with a short allow-list.
- All output goes through one module: stdout is exactly the handover path (R6).

Followed by `change`, checked by `review`:

- **Parse, don't validate.** Config and CLI input become validated types at the boundary
  (`TaskId`, `ProfileName`, `Status`). A status transition is a method returning a `Result`.
- **Errors.** One error enum per module (`thiserror`). Only `main` turns errors into exit
  codes and messages. No `Box<dyn Error>` across modules and no panic on user input.
- **No shell.** Processes are built with `Command` and explicit arguments, never `sh -c` with
  interpolated text. Paths stay `Path` or `OsStr`, never lossy strings. Paths are canonicalized
  before they become a mount, and a symlink that escapes the project is rejected.
- **Secrets.** A type that holds a secret has no derived `Debug` or `Display`. Logs and
  handovers show environment variable names, never values.
- **Files.** The handover is written to a temp file and renamed into place. Task ids are
  allocated by an atomic `create_dir` retry loop. Task folders are created with mode 0700.
- **Processes.** A child gets its own process group; cancel and timeout kill the group;
  nothing is left as a zombie.
- **Time and randomness** are injected, so tests are deterministic. The timeout and cancel
  tests are the exception: the deadline is the thing under test, so they use the real clock
  with short deadlines (seconds) instead of a manual one. The launch tests also install the
  Ctrl-C and TERM handlers in the test process, so `cargo test --lib` ignores Ctrl-C once they
  have run.
- **Visibility.** `pub(crate)` by default; `pub` only where the binary or an integration test needs it.
- **Comments** explain why, not what. A `TODO` carries an issue number.
