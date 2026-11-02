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
  Rollout (#23): two steps, because the ruleset requires the check by name and a pull request
  that replaced its own guard would report nothing at all. The `pull_request_target` workflow
  lands first as `.github/workflows/review-guard-target.yml`, next to the old `pull_request`
  one; a second pull request deletes the old file and gives the new one its name. In between,
  every pull request runs the check twice, and the old run still executes the pull request's
  copy of the workflow, so the hardening is complete only when the second step lands: merge it
  straight away, and merge nothing else in the window that the guard is meant to stop.
  Required CI contexts: `check`, `test` and `review-guard`, required by the repository
  ruleset on `main` (strict: a pull request must be up to date; no bypass actors). The ruleset
  is the owner's; agents do not change it.
  What the guard does not stop: it is a check, not a permission. The ruleset has no
  pull-request rule, so these checks gate a merge only when there is a pull request; whoever
  holds the owner's token (the owner, or an agent acting with it) can remove the label; and
  `check` and `test` run the workflow files the pull request carries. Requiring pull requests
  in the ruleset is the owner's decision.
  Queued runs: observed on #24, with the guard that still ran on `pull_request`. Green with the
  label off, then `needs/review` added: `mergeStateStatus` stayed CLEAN for the couple of
  seconds before GitHub created the new run, and was BLOCKED from the moment it appeared as
  queued, through in-progress, until it finished red. A queued run does not leave a stale green
  check; the green window is only the moment before the run exists. Owner rule: merge only
  after `review-guard` has finished on the current labels.
- Board columns: the ones in `plan`.
- Merge authority: only the owner.
