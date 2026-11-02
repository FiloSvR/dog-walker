# Delegating a task with dog-walker

You are the main harness. Use `dog-walker` to hand a self-contained task to a secondary
harness, which works in a sandbox and leaves a handover document.

## 1. Write the brief

State, in plain words, what the secondary needs and nothing else:

- **Goal**: what to achieve and why.
- **Context**: what it must know that it cannot find in the repository.
- **Constraints**: what it must not do or touch.
- **Done when**: a checklist you can verify afterwards.

## 2. Run it

    dog-walker run <profile> "<the brief>"

`dog-walker profiles` lists the profiles, the main one first. `run` blocks until the task
ends. It prints the path of the handover on stdout and logs on stderr.

## 3. Read the exit code

- exit code 0: the task ended `done`.
- exit code 1: the task ended `failed`, `timed-out` or `cancelled`.
- exit code 2: bad usage or a config error; no task ran.

Whatever the exit code, stdout holds the handover path when a task was started.

## 4. Read the handover

Each task has its own folder: `.dog-walker/tasks/<id>-<slug>/handover.md`. Its front matter
holds the status; the brief is what you wrote; the Result section is what the secondary
reports: what it did, the files it changed, what is open. Review the changes yourself before
you rely on them. The secondary cannot commit; you do.
