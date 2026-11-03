Goal: find out where a configured secret value is redacted in this repo (dog-walker, a Rust CLI). A profile can list `secrets`; their values must not reach the handover, `harness.log` or `result.md`.
Constraints: do NOT edit any file. Only write your result file.
Done when: your result lists every place where redaction is applied, as file and function, says for each what it redacts (the brief, the sandbox section, the outcome, the log, the result file), and names the function that does the replacing in each case.
Result format: (1) the answer as a list; (2) the files you changed, or "none"; (3) anything you could not check; (4) anything you are unsure about; (5) the files you opened that this brief did not mention, and why.
