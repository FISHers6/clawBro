# Doctor And Status

## Purpose

`ClawBro` now exposes a minimal operations surface for runtime, scheduler, and team visibility.

This is not a full runbook yet. It is the first stable contract for day-2 inspection.

## Endpoints

### `GET /health`

Lightweight health summary for automation and load balancers.

Current output includes:

- `ok`
- `backend_count`
- `unhealthy_backends`
- `active_teams`
- `unhealthy_teams`
- `pending_approvals`

Current health rules:

- backend is healthy when its configured adapter is registered
- running team is healthy when its tool surface is ready

Important:

- `/health` does **not** trigger live backend probe calls
- it reports catalog state plus cached runtime capability state

### `GET /status`

Richer operational snapshot for humans and tooling.

Current output includes:

- gateway-level `ok`
- `pending_approvals`
- backend catalog entries
- adapter registration status
- whether a backend has cached capability data
- cached `CapabilityProfile`
- active team summaries

## Team Summary Fields

Each team summary currently reports:

- `team_id`
- `state`
- `lead_session_key`
- `lead_agent_name`
- `specialists`
- `tool_surface_ready`
- `mcp_port`
- task count breakdown

This is intended to answer:

- is the team only planned or already running
- does it have a reachable tool surface
- how many tasks are pending / claimed / submitted / accepted / done / failed

## Local CLI

### `clawbro status`

Current local status output includes:

- gateway port and mode summary
- backend/provider summary
- channel/auth summary
- gateway running marker
- scheduler enabled/disabled
- scheduler DB path and whether the DB file exists

### `clawbro doctor`

Current local doctor output includes:

- binary presence
- config syntax/schema validation
- environment and channel checks
- runtime directory checks
- gateway process marker
- scheduler DB path and whether the DB file already exists

## Current Limitations

This is intentionally conservative.

- backend liveness is not actively probed on every request
- status relies on cached capability summaries already observed by runtime usage
- channel-specific diagnostics are not included yet
- approval details are not exposed, only pending count

## Next Steps

The next Ops Plane increment should add:

- backend probe refresh on demand
- channel diagnostics
- team tool surface diagnostics
- approval inventory with safe redaction
