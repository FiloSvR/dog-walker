Goal: find out how the launcher copes with a harness program that is briefly busy when it is started, in this repo (dog-walker, a Rust CLI).
Where to look: `launch()` in `src/launch.rs`. It spawns the harness in a loop; the case is `std::io::ErrorKind::ExecutableFileBusy`. The constant for the wait is `POLL`, at the top of the same file.
Already settled: the retry is in the launcher and nowhere else (the sandbox and the adapters only build the command). Do not look for it elsewhere.
Skip: everything outside `src/launch.rs`, including `tests/` and `docs/`.
How to verify: read the loop; no command needs to run.
Constraints: do NOT edit any file. Only write your result file.
Done when: your result says where the retry is (file and function), how many retries there are at most, and how long it waits between tries.
Result format: (1) the answer in a few sentences; (2) the files you changed, or "none"; (3) anything you could not check; (4) anything you are unsure about.
