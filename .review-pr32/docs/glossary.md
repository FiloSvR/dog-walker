# Glossary

The words dog-walker uses for delegating work between LLM coding tools.

## Language

**Harness**:
A coding tool built around a model that can be run from a command line, such as Claude Code, Codex, Pi or dsh.
_Avoid_: Agent, tool, CLI

**Profile**:
A named configuration of one harness: its model, thinking effort, sandbox settings and timeout.
_Avoid_: Agent config, preset

**Main**:
The profile the user drives, and from which tasks are delegated. Exactly one profile is main.
_Avoid_: Primary, orchestrator, parent

**Secondary**:
A profile that receives delegated tasks.
_Avoid_: Worker, child, sub-agent

**Task**:
One delegated piece of work, run once by a secondary and recorded in its own folder.
_Avoid_: Job, run, ticket

**Brief**:
What the main harness asks of the secondary: goal, context, constraints and done-when.
_Avoid_: Prompt, instructions

**Handover**:
The document, one per task, that holds the brief, the status and the result.
_Avoid_: Report, log, summary

**Result**:
What the secondary writes about its work: what it did, which files it changed, what is open.
_Avoid_: Output, answer

**Status**:
Where a task stands: created, running, done, failed, timed-out or cancelled. Only dog-walker sets it.
_Avoid_: State, phase
