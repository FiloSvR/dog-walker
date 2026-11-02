# Tracker, labels and board

Read by `plan`, `change` and `review`.

- Tracker: GitHub, https://github.com/FiloSvR/dog-walker, driven with the `gh` CLI.
  Machine-specific details go in untracked local notes.
- Issue ids: the GitHub issue number.
- Extra labels: none beyond the shared ones defined in `plan`.
- Merge guard: `review-guard`, which fails while `needs/review` is on a pull request and passes
  again once the label is removed, with no new push (the label events start a new run, which
  turns green once a runner picks it up: that can take minutes).
  It runs on `pull_request_target`: GitHub takes the workflow file, and every step in it, from
  `main`, so a pull request cannot switch off the check that gates it. The job checks nothing
  out, runs no pull request code, holds `pull-requests: read` and nothing else, and takes one
  value from the event, the pull request number. It reads the live labels through the API; a
  `gh` failure, an empty response, a response without a label list or a `grep` error makes it
  red, never green. `tests/review_guard.rs` runs the step against a stub `gh` for those paths.
  `check` and `test` stay on `pull_request`: they verify the pull request's code, so they have
  to run it, and a pull request that edits them is caught by the high-risk rule in
  `docs/agents/review.md`, not by the check (#23 decided this).
  How it got here (#23): two pull requests, because the ruleset requires the check by name and
  a pull request that replaced its own guard would have reported nothing at all. #24 added the
  `pull_request_target` workflow next to the old `pull_request` one, under a concurrency group
  of its own (groups are shared across workflows, so sharing the name would have let the two
  cancel each other); #26 deleted the old file and gave the new one its name and group back.
  While both were on `main`, a pull request reported the check twice, and #25, a throwaway
  pull request that replaced the guard with one that always passes, read BLOCKED with
  `needs/review` on: the `main`-sourced run was red, the pull request's own copy green, and the
  gate was not covered. Which of two same-named runs decides is not documented, so the window
  was only as safe as that race; #26 ended it.
  Required CI contexts: `check`, `test` and `review-guard`, required by the repository
  ruleset on `main` (strict: a pull request must be up to date; no bypass actors). The ruleset
  is the owner's; agents do not change it.
  What the guard does not stop: it is a check, not a permission. The ruleset has no
  pull-request rule, so these checks gate a merge only when there is a pull request; whoever
  holds the owner's token (the owner, or an agent acting with it) can remove the label;
  `check` and `test` run the workflow files the pull request carries; and the ruleset matches
  the check by name, so any workflow a pull request carries can add a job of that name, and a
  branch in this repository can ask `ci.yml` for `checks: write` and post the check itself
  (only fork pull requests are forced read-only). The high-risk review in
  `docs/agents/review.md` is what stands behind those. Requiring pull requests in the ruleset
  is the owner's decision.
  Queued runs: observed on #24 and again on #26. Green with the label off, then `needs/review`
  added: `mergeStateStatus` stayed CLEAN for the second or two before GitHub created the new
  run, and was BLOCKED from the moment a run appeared as queued, through in-progress, until it
  finished red. A queued run does not leave a stale green check; the green window is only the
  moment before the run exists. Owner rule: merge only after `review-guard` has finished on the
  current labels.
- Board columns: the ones in `plan`.
- Merge authority: only the owner.
