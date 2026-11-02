# Review

Read by `review`.

## Mechanical checks

CI runs three contexts (`docs/agents/tracker.md`):

- `check`: format, clippy with its denials, `cargo deny`.
- `test`: the whole suite, including the sandbox tests, which fail instead of skipping when
  the runner cannot sandbox. Read the job output: a green `test` with fewer tests than
  before is a finding.
- `review-guard`: red while `needs/review` is on. It is red by design until the review that
  completes the round removes the label. It runs from `main` on `pull_request_target` with no
  checkout, so a pull request cannot make it green by editing it or its workflow.

A green build means those pass; it says nothing about the rules below that tooling cannot see.

## Escape hatches

Any `#[allow(...)]`, `#[expect(...)]` or `unsafe` in the diff, and any change to `Cargo.toml`
`[lints]`, `clippy.toml` or `deny.toml` that loosens a rule. Each needs a comment saying why
and an owner-approved decision entry for `unsafe`. The review judges every one it finds.

## Standing rules and invariants

- The rules under "Standing rules" in `docs/agents/testing.md`, in particular the ones tooling
  cannot enforce: no shell strings, no lossy paths, secrets without `Debug` or `Display`,
  atomic handover writes, atomic task id allocation, process groups, injected time.
- The invariants in `docs/architecture.md`: only dog-walker writes the status (R12); the
  harness cannot touch `handover.md`.
- The decisions in `docs/decisions.md` bind every change: bwrap only (D2), native headless
  commands and no ACP (D4), read-only `.git` by default (D6).
- Status transitions go through the `Status` type, never an assigned string.

## High-risk changes

Kinds that need a second, independent review, on top of the default list:

- anything that builds or changes the sandbox arguments (mounts, namespaces, environment);
- anything that handles secrets, redaction or environment passing;
- anything that changes the handover's ownership or write path.
- anything under `.github/workflows/` and any change to the repository ruleset: `check` and
  `test` run the workflow files a pull request carries, so a pull request can change the checks
  that gate it (`review-guard` runs from `main` and cannot be changed this way). The second
  review is always owed, whatever the first rated.
