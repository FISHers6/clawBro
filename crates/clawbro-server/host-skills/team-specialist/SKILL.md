---
name: canonical-team-specialist
description: Backend-agnostic team coordination workflow for specialist agents.
---

# Canonical Team Skill

You are a specialist executor. Treat TEAM.md, AGENTS.md, TASKS.md, and task-local artifacts as the source of truth for the assigned task.

## Core Rules

1. Execute the assigned task instead of re-planning the whole team.
2. Use canonical team actions instead of describing execution state only in plain text.
3. Prefer `submit_task_result(task_id, summary, result_markdown)` when work is complete and ready for leader review.
4. Use `checkpoint_task` for intermediate progress, `request_help` for unblock requests, and `block_task` when execution cannot continue.

## CLI Bridge

For CLI-style backends, invoke specialist coordination through `clawbro team-helper`.

- `clawbro team-helper checkpoint-task --task-id T001 --note "..."`
- `clawbro team-helper request-help --task-id T001 --message "..."`
- `clawbro team-helper block-task --task-id T001 --reason "..."`
- `clawbro team-helper submit-task-result --task-id T001 --summary "..." --result-markdown "..."`

The runtime already provides `CLAWBRO_TEAM_TOOL_URL` and `CLAWBRO_SESSION_REF`.
Do not look up tokens or session coordinates manually.

## Specialist Workflow

- Claim context from TEAM.md, AGENTS.md, TASKS.md, and task-local files.
- Report meaningful progress with `checkpoint_task`.
- Submit full final deliverables with `submit_task_result`.
- Ask for help early with `request_help` instead of waiting silently.

## Completion

Before ending a specialist team turn, call at least one canonical specialist action that records progress or a terminal outcome.
