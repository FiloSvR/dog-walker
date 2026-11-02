# Architecture

Components, processes and the invariants, each with the test that checks it.
Work in progress: grows as the issues land.

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
