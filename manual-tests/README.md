# Manual worker tests

Real harness, real model, real sandbox: what `tests/` cannot cover (it only uses a fake
harness). Each brief in `briefs/` is delegated to the `worker` profile and the outcome is
compared with `expected.md`. The model is not deterministic, so the expectations are checks
on the handover and the repo, not on exact wording. These tests cost API calls; run them by
hand, never in CI.

## Setup (once)

1. `cargo build`, and use `target/debug/dog-walker` (or install it).
2. `bwrap` installed, and a harness with a provider key (the example uses Pi and OpenRouter, set up as in [`docs/setup.md`](../docs/setup.md#pi):
   `OPENROUTER_API_KEY` exported in your shell).
3. `mkdir -p ~/.config/dog-walker/pi-agent`
4. `sed "s|@HOME@|$HOME|g" manual-tests/config.example.toml > ~/.config/dog-walker/config.toml`
   (or pass `--config <file>` to every command). Check with `dog-walker profiles`.
5. `export PI_CODING_AGENT_DIR=$HOME/.config/dog-walker/pi-agent`
   The profile forwards the name only; the value must be set in the shell that runs `dog-walker`.

Then check the profile before spending a run: `dog-walker doctor worker` prints one line per
check (directories, sandbox, program, variables, a temp write, a TLS fetch) and exits 1 with the
fix when something is wrong. Do this for every profile you use below.

## Writing a brief for a worker

A worker that is told only "write your result to $DOG_WALKER_RESULT_PATH" tends to write
"ok". A reviewer then has to rebuild what changed from the diff (found delegating #48). End each
brief with a result format, as `briefs/01` and `briefs/04` do:

    Result format: (1) what you did, in a few sentences; (2) the files you changed, one per
    line; (3) anything you could not do or check (for example, you could not run cargo); (4)
    anything you are unsure about; (5) the files you opened that the brief did not mention, and
    why.

The fifth item is the feedback loop: each task names what the brief left out, so the next brief
can say it.

## Claude Code reviewer (optional)

The `reviewer` profile in the example config runs Claude Code with a subscription token. Set it
up with [`docs/setup.md`](../docs/setup.md#claude-code-with-a-subscription-no-api-key), then
`set -a; . ~/.config/dog-walker/reviewer.env; set +a` before each session.

## Codex and dsh (optional)

The `codex` and `dsh` profiles in the example config run the same briefs on those harnesses.
Set them up with [`docs/setup.md`](../docs/setup.md#codex), then source `codex.env` or `dsh.env`
before the run (for example `01-readonly-review` on each).

## Run

From the repo root (a git repository; `.dog-walker/` is ignored):

    target/debug/dog-walker run worker --file manual-tests/briefs/01-readonly-review.md
    echo "exit=$?"

`stdout` is the handover path; read it, `result.md` and `harness.log` next to it, then check
`git status`. Compare with the matching section of `expected.md`. Remove what a test left
(`git clean -n` first) before the next one.

## Tests

| Brief | Checks |
|---|---|
| `01-readonly-review` | the happy path: a `done` task, a result in the handover, no source changed |
| `02-sandbox-probe` | the sandbox from inside: project writes work, outside writes, `git commit` and a secret do not leak |
| `03-no-result-path` | a brief that never names the result file still ends `done`: the launcher adds it |
| `04-review-worker-diff` | the full loop: a `reviewer` reviews a `worker`'s uncommitted change from its handover |
| `06a-thin` and `06b-rich` | the same comparison on a question spread over three files; the one that shows a saving on Pi and Codex |
| `05a-thin` and `05b-rich` | the same question with a bare brief and with one that hands over what the main harness knows: the handover's `tokens`, `turns` and `cost_usd` show what the second saves |

If a test fails before the model answers, the cause is usually setup: `harness.log` says
which (a missing install directory, `Connection error.` for a missing CA store or key).

## The full loop (test 04)

1. Delegate a small change to `worker` with a brief of your own (a tight one: files it may touch,
   no commit). Note the task folder `run` prints, and check `git diff`.
2. Fill in the review brief and run it on `reviewer`:

       sed "s|@HANDOVER@|<path/to/worker/task>/handover.md|" manual-tests/briefs/04-review-worker-diff.md > /tmp/review.md
       dog-walker run reviewer --file /tmp/review.md

3. Read the verdict, then decide yourself: the reviewer only reports, and nothing is committed.

## Does a richer brief save tokens (tests 05 and 06)

The brief is the only way the main harness can hand over what it has already learned, so a
brief that leaves it out makes the worker pay again (R23). Each pair asks one question (`05` is
small, `06` spans three files and shows more), and the rich brief also says where to look, what
is settled, what to skip and how to verify. Run each three times on the same profile (a model's
cost varies from run to run), then read what each task used:

    for b in 06a-thin 06b-rich; do
      for i in 1 2 3; do target/debug/dog-walker run worker --file manual-tests/briefs/$b.md; done
    done
    grep -H '^\(tokens\|turns\|cost_usd\) ' .dog-walker/tasks/*/handover.md | tail -18

Compare the medians of the two briefs, not single runs. See `expected.md` for the reading. A
harness that reports no usage (dsh, or a profile with its own `command`) shows nothing here;
use Pi, Codex or Claude Code. For Claude Code compare `cost_usd`: its `tokens` count the
cached input it re-reads on every turn.
