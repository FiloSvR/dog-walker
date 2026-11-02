# Testing and standing rules

Read by `change`, `architect` and `plan`.

## Layers

| Suffix or place | Layer | Use for |
|---|---|---|
| `#[cfg(test)]` modules in `src/` | unit | pure logic: config parsing, status transitions, handover rendering |
| `tests/` | acceptance | the compiled binary run against a fake harness; one test per AT in `docs/requirements.md` |

The fake harness is a small test-only binary with scripted behaviours (write a
result, write a malformed one, hang, read its environment, attempt a forbidden
write, try the network, call `run`). Sandbox tests use the real `bwrap`, since
they check OS-level effects; they are skipped with a loud message when `bwrap`
or user namespaces are missing. Everything runs offline.

## Commands

- One test file or folder: `cargo test --test <name>`, or `cargo test <filter>`. Never the whole suite; CI is the gate.
- Check (lint, types, dependency rules): `cargo clippy --all-targets -- -D warnings && cargo deny check`.
- Format fix: `cargo fmt`.

## Dependencies

Rust, pinned by `rust-toolchain.toml`. Allowed by default: `clap`, `serde`, `toml`,
`serde_json`, and a thin OS library such as `rustix`. No async runtime. Versions
are exact, `Cargo.lock` is committed, and `cargo deny` runs in CI. A new
dependency needs one line in the pull request saying why.

## Standing rules

- Tests run offline.
- No real credentials in the repo.
- No server, daemon or telemetry, and no network request made by dog-walker itself.
