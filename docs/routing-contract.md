# Routing Contract

## Purpose

`quickai-gateway` now treats routing as a small contract instead of an implicit side effect of `@mention` parsing.

This document defines the current routing precedence and the compatibility rules that the code is expected to preserve.

## Current Precedence

For a normal inbound turn, backend selection now follows this order:

1. explicit `@mention`
2. Team lead fallback for the configured `front_bot`
3. explicit session override from `/backend`
4. explicit routing bindings, in this order:
   - `thread`
   - `scope`
   - `peer`
   - `team`
   - `channel`
   - `default`
5. default roster agent
6. explicit `agent.backend_id` default backend

Notes:

- `@mention` always wins over scope binding.
- `/backend` remains a manual override and is not silently erased by scope binding. It is handled through the synchronous slash control surface, not runtime event flow.
- bindings are only used when there is no explicit `@mention` and no session-level backend override.
- default roster fallback exists to support roster-only deployments where `agent.backend_id` is intentionally unset.

## Binding Model

The current binding model is intentionally small and matches fields that already exist in `InboundMsg` / `SessionKey` today:

- `thread`: exact `channel + scope + thread_ts`
- `scope`: exact `channel? + scope`
- `peer`: `user:` / `group:` scope extraction
- `team`: specialist session prefix `{team_id}:{agent}`
- `channel`: exact inbound channel
- `default`: fallback agent before roster default

Two sources feed the same binding engine:

- derived bindings from `[[group]]` `front_bot`
- derived bindings from exact `[[team_scope]]` `front_bot`
- explicit `[[binding]]` entries in gateway config

Within the same precedence tier, later bindings win.
This is intentional so explicit `[[binding]]` entries can override earlier derived group bindings.

Example:

```toml
[[group]]
scope = "group:lark:chat-123"

[group.mode]
interaction = "solo"
front_bot = "claude"
```

Exact-scope Team wiring uses the same derived binding contract. For example, a Lark DM workbench can bind a `user:*` scope to the Team lead without inventing a second routing system:

```toml
[[team_scope]]
scope = "user:ou_123"

[team_scope.mode]
interaction = "team"
front_bot = "claude"
channel = "lark"
```

This means messages in `group:lark:chat-123` without an explicit `@mention` will resolve to roster agent `claude`, unless the session has already been manually switched with `/backend`.

Explicit bindings can further refine routing:

```toml
[[binding]]
kind = "thread"
agent = "codex"
channel = "dingtalk"
scope = "group:cid_123"
thread_id = "sessionWebhook-1"

[[binding]]
kind = "default"
agent = "claude"
```

## Validation Rules

Gateway config validation now rejects:

- `front_bot` values that do not exist in `[[agent_roster]]`
- `group.team.roster` agent names that do not exist in `[[agent_roster]]`
- `[[binding]]` agents that do not exist in `[[agent_roster]]`
- `[[binding]]` cannot be used without `[[agent_roster]]`

This keeps routing deterministic and avoids runtime-only misconfiguration.

## Team Interaction

Team mode keeps its own role logic:

- Specialist sessions still route by `specialist:{team_id}:{agent}`
- Lead turns still require Team role detection
- `front_bot` in Team mode acts both as:
  - the Lead agent identity
  - the no-mention scope binding target
- Internal Specialist heartbeat turns may resolve through `team` bindings before the generated `target_agent` hint, so a whole team can be rebound to a different specialist backend family without changing orchestrator dispatch format.

Canonical Team semantics still live in `/runtime/team-tools`. Routing only decides which backend receives the turn.
