# Architecture

Components, processes and the invariants, each with the test that checks it.
Work in progress: grows as the issues land.

## Components

One crate: a library (`dog_walker`) and a thin binary (`dog-walker`). Errors stay inside their
module; only `cli` turns them into a message and an exit code. Only `output` writes.

| Module | Owns | Status |
|---|---|---|
| `config` | Profiles and the config file: validated `Profile`, `Harness`, `ProfileName`; exactly one main (R1, R2) | built (#3) |
| `output` | Writing to stdout and stderr; the exit codes (R6) | built (#3) |
| `cli` | `profiles` and `instructions`; parsing, errors to exit codes (R21) | built (#3) |
| `error` | `ConfigError` and `Error` | built (#3) |
| `task` | Atomic id allocation and the task folder (R10) | planned (#16) |
| `handover` | TOML front matter, brief and Result; whole-file rewrite (R11, R12) | planned (#16) |
| `status` | The `Status` type and its transitions (R12) | planned (#16) |
| `launch` | Starting the harness through a profile's `command` and waiting (R5) | planned (#16) |

Invariants, each with the test that checks it:

- A config has exactly one main profile and only known keys and harnesses: `at3_*` and `at2_*` in `tests/cli.rs`.
- Nothing but `output` writes, and stdout carries results only: the print lints in `Cargo.toml`, `only_the_output_module_writes` in `tests/architecture.rs` (a tripwire for the common spellings of a write, which the lints do not see: `write!`, `write_all`, `write_fmt`, `flush`, and naming `stdout()` or `stderr()`, or importing them `as` another name, anywhere but `src/main.rs`; only the top-level `src/output.rs` and `src/main.rs` are exempt, checked by `the_exemptions_are_exact_paths`; a write hidden in a macro is not seen), and `at2_missing_model_names_profile_and_field`.
- A row of `profiles` can always be split: `model` and `effort` hold no control, line-separator or bidirectional-control characters (U+2028, U+2029, U+061C, U+200E, U+200F, U+202A-U+202E, U+2066-U+2069), and `-` means no effort: `at2_unprintable_or_reserved_values_are_rejected`, `profiles_rows_have_fixed_columns`.
- No networking or async-runtime crate is a dependency (R19): `deny.toml` bans them (the one list), and `at24_no_networking_crates_in_lockfile` reads that list.

## Task folder

`.dog-walker/tasks/<id>-<slug>/` holds, per task:

- `handover.md`: owned by dog-walker. TOML front matter (id, profile, status), the brief,
  then the Result section. Bound read-only inside the sandbox, so the harness can never
  change the status or the brief.
- `result.md`: the one file the harness is told to write.
- the raw harness output as a log file.

After the run, dog-walker validates `result.md` and copies it into `handover.md` under the
Result heading. A missing or malformed `result.md` gives status `failed` with a reason, the
raw log kept and no invented result (R13).

Invariant: only dog-walker writes the status (R12). Checked by AT15.

## Packaging

The binary is `dog-walker`; the library crate stays internal. Installed with
`cargo install --git https://github.com/FiloSvR/dog-walker` or `cargo install --path .`
from a clone. Prebuilt binaries and distro packages come later.
