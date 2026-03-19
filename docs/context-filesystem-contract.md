# Context Filesystem Contract

## Purpose

ClawBro uses two cooperating context channels:

- runtime contract
- filesystem-native contract

The runtime contract carries structured fields such as `memory_summary`, `agent_memory`, `team_manifest`, and `task_reminder`.
Those fields are projections of the filesystem-native contract.
The filesystem contract exposes stable context files so different backend families and CLI agents can consume one shared context shape without ClawBro pretending every backend speaks the same transport.

## Current Contract

When present in the resolved persona, workspace, or team roots, ClawBro projects these files into `RuntimeContext.workspace_native_files`:

- `SOUL.md`
- `IDENTITY.md`
- `USER.md`
- `MEMORY.md` for `Solo` and `Lead` turns
- `memory/<channel>_<scope>.md` when scoped memory exists
- `AGENTS.md`
- `CLAUDE.md`
- `HEARTBEAT.md`
- `TEAM.md`
- `CONTEXT.md`
- `TASKS.md`

## Current Loading Rules

- `persona_root` contributes persona files such as `SOUL.md`, `IDENTITY.md`, role-allowed memory files, and optional `USER.md`.
- `workspace_root` contributes workspace files such as `AGENTS.md`, `CLAUDE.md`, optional `USER.md`, and optional `HEARTBEAT.md`.
- `team_root` contributes team-scoped files such as `TEAM.md`, `CONTEXT.md`, `TASKS.md`, and optional `HEARTBEAT.md`.
- The projected contract is deterministic and ordered as: persona files, workspace files, team files.
- If the same filename exists in multiple roots, ClawBro deduplicates by filename in the visible-file projection.

## Role Notes

- `Solo`: receives persona and workspace-native files; no team files unless explicitly in team mode.
- `Lead`: receives persona and workspace-native files plus team-scoped files when team mode is active.
- `Specialist`: receives role-allowed persona files plus team-scoped files. `MEMORY.md` is not automatically exposed to specialists.

This is intentionally conservative: ClawBro standardizes file visibility first, and keeps prompt/runtime semantics separate from raw file projection.

## Projection Rule

ClawBro treats files as the durable source of truth and `RuntimeContext` as the projection layer:

- `system_prompt` is derived support text for runtimes that need structured prompt assembly
- `memory_summary` projects shared contextual memory
- `agent_memory` projects private role-allowed memory
- `team_manifest` projects `TEAM.md`
- `task_reminder` is derived execution-local context

## Why `HEARTBEAT.md` Matters

`HEARTBEAT.md` remains useful as a durable, inspectable Team or workspace checklist file.
It is part of the context filesystem contract, not the declaration source for general scheduled jobs.

In the current scheduler architecture:

- durable scheduled jobs are created through `clawbro schedule ...`
- scheduler state lives in SQLite via the runtime scheduler
- `HEARTBEAT.md` can still guide periodic operational review, but it is not the scheduler control plane

Adding it to the contract still gives ClawBro:

- a stable cross-backend context shape
- a place to evolve heartbeat-native behavior later
- parity with the workspace-native patterns used by comparable systems
