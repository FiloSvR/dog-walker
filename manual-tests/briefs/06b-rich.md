Goal: find out where a configured secret value is redacted in this repo (dog-walker, a Rust CLI). A profile can list `secrets`; their values must not reach the handover, `harness.log` or `result.md`.
Where to look: `run_task` in `src/cli.rs`; `launch()` and the functions it calls to rewrite files, in `src/launch.rs`; and in `src/sandbox.rs` the two functions that replace a secret with `[redacted]`, `redact` (text) and `redact_bytes` (bytes), with the `Policy` methods that hand over the secret values.
Already settled: the redaction happens in those three files and nowhere else; the harness and the adapters never see it. Do not look for it elsewhere.
Skip: `tests/`, `docs/`, `manual-tests/` and every other file in `src/`.
How to verify: read the call sites; no command needs to run.
Constraints: do NOT edit any file. Only write your result file.
Done when: your result lists every place where redaction is applied, as file and function, says for each what it redacts (the brief, the sandbox section, the outcome, the log, the result file), and names the function that does the replacing in each case.
Result format: (1) the answer as a list; (2) the files you changed, or "none"; (3) anything you could not check; (4) anything you are unsure about; (5) the files you opened that this brief did not mention, and why.
