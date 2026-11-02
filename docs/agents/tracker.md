# Tracker, labels and board

Read by `plan`, `change` and `review`.

- Tracker: GitHub, https://github.com/FiloSvR/dog-walker, driven with the `gh` CLI.
  Machine-specific details go in untracked local notes.
- Issue ids: the GitHub issue number.
- Extra labels: none beyond the shared ones defined in `plan`.
- Merge guard: `review-guard`, which fails while `needs/review` is on a pull request and passes
  again once the label is removed, with no new push (the label events start a new run).
  Required CI contexts: `check`, `test` and `review-guard`, required by the repository
  ruleset on `main` (strict: a pull request must be up to date; no bypass actors). The ruleset
  is the owner's; agents do not change it.
  What the guard does not stop: it is a check, not a permission. The ruleset has no
  pull-request rule, so these checks gate a merge only when there is a pull request; whoever
  holds the owner's token (the owner, or an agent acting with it) can remove the label; and a
  pull request runs the workflow files it carries, so it can change its own checks (the review
  doc treats that as high-risk, and #23 hardens the guard). Requiring pull requests in the
  ruleset is the owner's decision.
- Board columns: the ones in `plan`.
- Merge authority: only the owner.
