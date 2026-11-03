# Expected results

Replace `<task>` with the folder printed by `run`. "Clean" means `git status --short` shows no
change to tracked files.

## 01-readonly-review

- Exit code 0; stderr ends `task NNNN done`.
- `handover.md`: `status = "done"`; the Sandbox section lists `OPENROUTER_API_KEY` under secrets
  as `(redacted)`; the Brief is the file's text; the Result is `result.md`'s text.
- The result summarises both files and names one improvement with a file and line, and follows the brief's result format (what was done, files changed, what could not be checked, doubts).
  (At the time of writing: a `Status::ALL` constant in `src/status.rs`. The model may pick
  another; any concrete one is fine.)
- Repo clean; no stray file written outside `.dog-walker/`.

## 02-sandbox-probe

- Exit code 0 and `status = "done"`: denials are the thing being reported, not a failure.
- Step 1 succeeds, and `probe-scratch.txt` exists in the repo afterwards (delete it).
- Steps 2 and 3 fail with `Read-only file system` (the sandbox mounts neither `$HOME` nor a
  writable `/etc`). Nothing appears at either path on the host.
- Step 4: `git add` and `git commit` fail with `Unable to create '.git/index.lock': Read-only
  file system` (`.git` is read-only by default). `git log` and `git diff --stat` work.
  `git log` on the host shows no `probe` commit.
- Step 6: the handover's Result shows `[redacted]` where the key is, and so does
  `harness.log`; `grep -c "$OPENROUTER_API_KEY" <task>/*` is 0 for all three files, `result.md`
  included (it is rewritten once the run ends). The environment names are `HOME`, `PATH`,
  the profile's `env` names, `DOG_WALKER_*` and whatever the harness itself exports (Pi adds
  `PI_*`, `AI_AGENT`); none of your other shell variables.
- Clean up: delete `probe-scratch.txt`.

## 03-no-result-path

- Exit code 0 and `status = "done"`, although the brief never names the result file: dog-walker
  appends the instruction itself (D14).
- The handover's Brief is exactly the text of the brief file, without the added line; the Result
  is the worker's answer ("hello").

## 04-review-worker-diff

Run after a worker task has left an uncommitted change (the placeholder is the worker's
`handover.md`).

- Exit code 0 and `status = "done"`; the reviewer runs `git diff` (read-only `.git` allows it).
- The result starts with `APPROVE` or `CHANGES REQUESTED`, then findings with file and line, most
  severe first. It reads the worker's handover and compares its Result with the diff.
- The reviewer changes nothing: `git status` is the same before and after, and `.git` has no new
  commit. Only `.dog-walker/` gains a task folder.
- Quality, not wording: a planted flaw (a test that cannot fail, an extra file touched, a Result
  that misdescribes the diff) should be found. In the first run the reviewer caught a tautological
  test (`is_end` is defined as `ENDS.contains`), though it still wrote `APPROVE` above a finding
  that said the test pins nothing: read the findings, not only the verdict line.

## 05a-thin and 05b-rich

- Both end `done` with the same correct answer: the retry is in `launch()` in `src/launch.rs`,
  at most 5 retries, `POLL` (50 ms) between them. An answer that differs is a failed run; do not
  count its usage.
- Repo clean.
- The handover front matter has `tokens` (and `turns`, `cost_usd` on Pi and Claude Code).
- The reading, over three runs of each: `05b-rich` should use fewer turns and fewer tokens than
  `05a-thin`, because the worker does not search. If it does not, the brief did not save what it
  should: read `harness.log` of the rich runs for the files the worker still opened (the brief
  said to skip them) and for the reads that were not needed. If the two are equal, the question
  is too easy for this model; use a larger one. This is a measurement of the brief, not a pass
  or fail of the tool: the tool's part is that the numbers are there (AT30).
- A first measurement (2026-11-03, Pi, `openrouter/z-ai/glm-5.3-flash`, three runs each; all six
  answers correct): `05a-thin` used 18206, 25275 and 16290 tokens in 7, 8 and 5 turns; `05b-rich`
  used 14141, 14525 and 11906 tokens in 4, 4 and 4 turns. Medians: 18206 against 14141 tokens
  (22% fewer) and 7 against 4 turns. A task that only answers "ok" costs about 5000 tokens on this
  profile (the harness's own prompt and tools), so most of the saving is in the exploration,
  which is the part a brief can remove.
- The same pair on Codex (`gpt-6.1-terra`, low effort, three runs each, all correct; Codex reports
  only a total): `05a-thin` 18147, 9597 and 9714 tokens; `05b-rich` 10410, 16947 and 9445.
  Medians 9714 against 10410: no saving. The rich runs used 2 commands against 3 or 4, but each
  ran `cat src/launch.rs` on the whole file, and a task that only answers costs about 9000 tokens
  there. The question is too small to show a brief's effect on Codex; use 06 below.

## 06a-thin and 06b-rich

A question whose answer is spread over three files: where a configured secret is redacted.

- Both end `done` with a list that names, in `src/cli.rs`, the brief, the sandbox section and the
  outcome (each through `sandbox::redact`), and, in `src/launch.rs`, the log and `result.md`
  (through `redact_file` and `redact_result`, which use `sandbox::redact_bytes`). A list that
  misses a place is a failed run; do not count its usage.
- Repo clean. Result item (5) tells which files the brief left out: the thin runs name
  `AGENTS.md` and `docs/agents/*` (the repo's instructions), the rich runs name none.
- Ten runs of each brief on each harness (2026-11-03; every answer correct). Tokens, sorted:

  | Harness | Brief | Median | Mean | Range |
  |---|---|---|---|---|
  | Pi, `glm-5.3-flash` | thin | 65043 (9.5 turns) | 61275 | 23133 to 86013 |
  | Pi | rich | 35881 (6 turns) | 47871 | 22891 to 144373 |
  | Codex, `gpt-6.1-terra` low | thin | 22022 | 22163 | 13221 to 34760 |
  | Codex | rich | 12741 | 13906 | 7619 to 30592 |

  At the median the rich brief used 45% fewer tokens on Pi and 42% fewer on Codex. The spread is
  wide and the ranges overlap: the cheapest thin Pi run (23133; the next was 43846) cost less than nine of the ten rich runs,
  and one rich Pi run cost more than any other (144373), so judge a brief by several runs. On
  Codex the rich outlier (30592) made the fewest commands of its group; its extra cost was
  reasoning, which a brief cannot remove.
- Why the Pi outlier cost what it did: Pi's `tokens` add up each turn's whole context, so a big
  read is paid again on every later turn. That run read `src/cli.rs` and `src/launch.rs` in full
  (about 13000 tokens in one turn, then carried through six more) although the brief named only
  functions. A rich brief that names a file does not stop the worker reading all of it: give the
  lines (`src/cli.rs` 288 to 368) and say to read only those.
- Reading both pairs together: a brief saves most when the answer needs exploring across files,
  and a little or nothing when one `cat` answers it.
