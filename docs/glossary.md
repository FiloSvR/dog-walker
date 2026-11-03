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

**Slot**:
The record a running task holds in `.dog-walker/run`, named after its id, with the runner's pid
and start time: it is what a limit counts and what tells another run that the project is
already being written.
_Avoid_: Lock, permit, token

**Queue**:
The wait a task's runner does while the limit for its harness and model is reached: it has a
task folder and the status `created`, and it can be cancelled before it starts.
_Avoid_: Pool, batch, backlog

**Adapter**:
The module that knows how one harness is started: the argv that runs it with a profile's model
and effort, and the variables it reads. A profile's own `command` wins over it.
_Avoid_: Driver, plugin, integration
