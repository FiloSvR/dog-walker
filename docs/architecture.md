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
| `cli` | `profiles`, `instructions`, `run` and `cancel`; parsing, errors to exit codes (R5, R6, R14, R21) | built (#3, #16, #5) |
| `error` | One error enum per module, and `Error` | built (#3, #16) |
| `task` | Atomic id allocation and the task folder (R10) | built (#16) |
| `handover` | TOML front matter, brief and Result; whole-file rewrite (R11, R12) | built (#16) |
| `status` | The `Status` type and its transitions (R12) | built (#16) |
| `launch` | Starting the harness, waiting, and ending its process group (R5, R14) | built (#16, #5) |

Invariants, each with the test that checks it:

- A config has exactly one main profile and only known keys and harnesses: `at3_*` and `at2_*` in `tests/cli.rs`.
- Nothing but `output` writes, and stdout carries results only: the print lints in `Cargo.toml`, `only_the_output_module_writes` in `tests/architecture.rs` (a tripwire for the common spellings of a write, which the lints do not see: `write!`, `write_all`, `write_fmt`, `flush`, and naming `stdout()` or `stderr()`, or importing them `as` another name, anywhere but `src/main.rs`; the top-level `src/output.rs` and `src/main.rs` are exempt, and `src/handover.rs` may use `write_all` on the file it creates, checked by `the_exemptions_are_exact_paths`; a write hidden in a macro is not seen), and `at2_missing_model_names_profile_and_field`.
- A row of `profiles` can always be split: `model` and `effort` hold no control, line-separator or bidirectional-control characters (U+2028, U+2029, U+061C, U+200E, U+200F, U+202A-U+202E, U+2066-U+2069), and `-` means no effort: `at2_unprintable_or_reserved_values_are_rejected`, `profiles_rows_have_fixed_columns`.
- No networking or async-runtime crate is a dependency (R19): `deny.toml` bans them (the one list), and `at24_no_networking_crates_in_lockfile` reads that list.
- A task's status moves `created`, `running`, then exactly one end state, and an end never changes: `created_becomes_running_and_nothing_else_does`, `only_a_running_task_can_end`, `an_end_state_cannot_be_an_end_of_itself` in `src/status.rs`.
- Task ids are zero-padded counters that are never handed out twice: `folders_are_numbered_in_order_and_are_private`, `the_next_id_follows_the_highest_marker`, `concurrent_creations_never_share_an_id`, `an_id_is_not_reused_after_its_folder_goes`, `at13_ids_are_counters_and_gitignore_is_untouched`.
- The handover's front matter is TOML, and its brief and result survive an edit by the harness: `front_matter_is_toml`, `at14_the_handover_is_complete_before_the_harness_starts`, `at15_a_status_edited_by_the_harness_is_overwritten`.

## Task folder

`.dog-walker/tasks/<id>-<slug>/` holds, per task:

- `handover.md`: owned by dog-walker. TOML front matter (id, profile, status), the brief,
  then the Result section. Inside the sandbox (#4) it will be bound read-only; until then the
  guarantee is the whole-file rewrite after the run, which drops an edit by the harness.
- `result.md`: the one file the harness is told to write.
- `harness.log`: the harness's stdout and stderr, raw and untruncated.

After the run, dog-walker validates `result.md` and copies it into `handover.md` under the
Result heading. A missing, empty, unreadable or non-UTF-8 `result.md` gives status `failed`
with a reason, the raw log kept and no invented result (R13).

An id is allocated by creating a marker file for it under `.dog-walker/ids/` with `create_new`,
retrying with the next id when the name is taken: the marker is atomic, so two tasks started at
once cannot take the same counter, and it is never removed, so deleting a task folder does not
free its id for reuse. The task folder is then created as `<id>-<slug>`, mode 0700, and `.dog-walker`, `tasks` and
`ids` are created 0700 too (an existing one is left as it is). A symbolic link where any of
them belongs is refused, and after the run the task folder is checked to still be the one that
was created, so a folder the harness moves away or replaces with a link is not written through.

Invariant: only dog-walker writes the status (R12), and it rewrites the whole document from
what it holds in memory, so a harness that edits the status or the brief loses the edit:
`at15_a_status_edited_by_the_harness_is_overwritten`, `at15_the_real_end_state_wins`. The
handover's temporary file is created with `create_new`, so a link planted in the task folder
cannot redirect the write (`at15_a_planted_temp_link_is_not_followed`); the result must be a
regular file, so a link or a pipe there is refused, not read (`at16_a_symlinked_result_is_not_read`,
`at16_a_fifo_result_is_not_read`).

## The harness environment

The harness is the profile's `command`, an argv list with the brief appended as its last
argument, started in the project directory, with stdin closed and its output in `harness.log`.
It reads, from the environment:

- `DOG_WALKER_TASK_DIR`, `DOG_WALKER_HANDOVER_PATH`, `DOG_WALKER_RESULT_PATH`, `DOG_WALKER_TASK_ID`;
- `DOG_WALKER_PROFILE`, `DOG_WALKER_MODEL`, and `DOG_WALKER_EFFORT` when the profile sets one.

Adapters (#10) will build that command line from the profile; until then a profile needs a
`command` to run, and `run` says so before it creates a task. The environment is inherited for
now; the allowlist and the cleaning are #8.

Checks: `at1_*`, `at6_*`, `at7_*`, `at16_a_harness_that_cannot_start_fails_the_task`. A
harness that ends with a non-zero status is a failed task, even when it left a result
(`at16_a_nonzero_exit_is_a_failure`), and the handover path is printed as soon as the handover
exists, so a failure to write the end state still leaves it on stdout
(`at7_the_path_comes_before_the_end_state`).

## Stopping a task

The harness leads a process group of its own, so a stop takes the processes in that group. `run`
waits by polling the child and ends the tree when the profile's timeout passes (30 minutes by
default; `timeout_minutes` takes a fraction of a minute), when Ctrl-C or a TERM interrupts the
runner, or when `cancel <id>` writes `.cancel` into the task folder. The group gets TERM, the
runner waits for the leader up to two seconds, then the group gets KILL. The handover is closed
as `timed-out` or `cancelled`, and the runner removes the marker once the end state is on disk.
`cancel`, run in the project directory, reads the handover first and writes nothing to a task
that has already ended.

Checks: `at17_a_timeout_kills_the_tree_and_ends_timed_out`,
`at18_ctrl_c_kills_the_tree_and_ends_cancelled`,
`at18_cancel_from_another_shell_kills_the_tree`,
`at18_cancelling_a_finished_task_changes_nothing`, and the unit tests
`a_timeout_ends_the_whole_tree` and `a_cancel_marker_ends_the_whole_tree` in `src/launch.rs`.

## Packaging

The binary is `dog-walker`; the library crate stays internal. Installed with
`cargo install --git https://github.com/FiloSvR/dog-walker` or `cargo install --path .`
from a clone. Prebuilt binaries and distro packages come later.
