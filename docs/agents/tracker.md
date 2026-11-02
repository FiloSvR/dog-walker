# Tracker, labels and board

Read by `plan`, `change` and `review`.

- Tracker: GitHub, https://github.com/FiloSvR/dog-walker, driven with the `gh` CLI.
  Machine-specific details go in untracked local notes.
- Issue ids: the GitHub issue number.
- Extra labels: none beyond the shared ones defined in `plan`.
- Merge guard: `review-guard`, which fails while `needs/review` is on a pull request and passes
  again once the label is removed, with no new push (the label events start a new run).
  Required CI contexts: `check`, `test` and `review-guard`. The repository ruleset on `main`
  currently only blocks deletion and force-push; requiring these three contexts is a separate,
  owner-approved change to that ruleset.
- Board columns: the ones in `plan`.
- Merge authority: only the owner.
