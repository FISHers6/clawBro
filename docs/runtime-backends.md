# Runtime Backends

## Purpose

`quickai-gateway` now separates:

- `Business Control Plane`
- `Agent Runtime / Conductor Plane`

This document describes the runtime side only: backend families, their adapters, and their current status.

Routing precedence is documented separately in [`routing-contract.md`](routing-contract.md).
Validated backend capability levels and current product-facing boundaries are documented separately in [`backend-support-matrix.md`](backend-support-matrix.md).

Current control-plane decomposition in `qai-agent`:

- `routing.rs` resolves turn destination and backend hints
- `context_assembly.rs` builds workspace/persona/shared context
- `slash_service.rs` handles synchronous control commands
- `memory_service.rs` owns `/memory` and related memory control behavior
- `post_turn.rs` applies relay, mention, memory side effects after runtime completion

`SessionRegistry` remains the ingress orchestrator, not the place where slash and memory business logic should accumulate again.

## Canonical Model

All backends are normalized through the same runtime contract:

- `TurnIntent`
- `RuntimeSessionSpec`
- `RuntimeEvent`
- `TurnResult`

All backend-specific protocols are implementation details behind adapters.

## Backend Families

### ACP

Status: active

Implementation:

- adapter: [`qai-runtime/src/acp/adapter.rs`](/Users/fishers/Desktop/repo/quickai-openclaw/quickai-gateway/crates/qai-runtime/src/acp/adapter.rs)
- probe: [`qai-runtime/src/acp/probe.rs`](/Users/fishers/Desktop/repo/quickai-openclaw/quickai-gateway/crates/qai-runtime/src/acp/probe.rs)
- session driver: [`qai-runtime/src/acp/session_driver.rs`](/Users/fishers/Desktop/repo/quickai-openclaw/quickai-gateway/crates/qai-runtime/src/acp/session_driver.rs)

Notes:

- ACP is no longer the only runtime family.
- ACP permission requests now also flow through the shared runtime `ApprovalBroker` instead of being auto-accepted inside the session driver.
- The first interactive approval surface is WebSocket `ResolveApproval`; without a decision, ACP permission requests also fail closed by timeout.

### OpenClaw Gateway

Status: active

Implementation:

- adapter: [`qai-runtime/src/openclaw/adapter.rs`](/Users/fishers/Desktop/repo/quickai-openclaw/quickai-gateway/crates/qai-runtime/src/openclaw/adapter.rs)
- client: [`qai-runtime/src/openclaw/client.rs`](/Users/fishers/Desktop/repo/quickai-openclaw/quickai-gateway/crates/qai-runtime/src/openclaw/client.rs)
- probe: [`qai-runtime/src/openclaw/probe.rs`](/Users/fishers/Desktop/repo/quickai-openclaw/quickai-gateway/crates/qai-runtime/src/openclaw/probe.rs)

Default eligibility:

- `solo = true`
- `relay = true`
- `specialist = false` unless an explicit team helper is configured
- `lead = false` unless an explicit team helper and explicit lead mode are configured
- `native_team = supported but disabled by QuickAI policy` by default

Important:

- In QuickAI Team mode, OpenClaw is currently treated as one backend actor.
- It becomes `SpecialistEligible` only when the backend config provides an explicit `team_helper_command`.
- It becomes `LeadEligible` only when the backend config provides both:
  - `team_helper_command`
  - `lead_helper_mode = true`
- The adapter then:
  - resolves the configured OpenClaw `agent_id`
  - ensures the helper command is allowlisted through `exec.approvals.*`
  - injects the canonical Team Tool Contract into the prompt as backend-native helper commands
- `OpenClaw lead` is constrained:
  - no native sub-team
  - only canonical lead tools
  - no bypass of `TaskRegistry / TeamOrchestrator`
- The runtime transport itself is already real: `OpenClawBackendAdapter` now connects to the gateway WebSocket protocol and normalizes stream events into `RuntimeEvent`.
- OpenClaw `exec.approval.requested` broadcasts are normalized into `RuntimeEvent::ApprovalRequest`.
- QuickAI now exposes a WS approval surface (`ResolveApproval`) backed by a shared runtime `ApprovalBroker`.
- Policy is still fail-closed: if no decision arrives before the approval expires, the runtime resolves it as `deny` to avoid indefinite hangs.
- This keeps OpenClaw equal at the runtime layer without pretending it is ACP.

### QuickAI Native

Status: active, team-capable via canonical Team Tool RPC

Implementation:

- adapter: [`qai-runtime/src/native/adapter.rs`](/Users/fishers/Desktop/repo/quickai-openclaw/quickai-gateway/crates/qai-runtime/src/native/adapter.rs)
- probe: [`qai-runtime/src/native/probe.rs`](/Users/fishers/Desktop/repo/quickai-openclaw/quickai-gateway/crates/qai-runtime/src/native/probe.rs)
- rust agent bridge: [`runtime_bridge.rs`](/Users/fishers/Desktop/repo/quickai-openclaw/quickai-gateway/crates/quickai-agent-sdk/src/runtime_bridge.rs)

Important:

- `quickai-rust-agent` is a first-class participant even though it is not ACP-first by design.
- Native family is no longer limited to `solo/relay`.
- `quickai-rust-agent` now receives `RuntimeSessionSpec`, dynamically registers team tools by role, and calls back into the gateway through the canonical `/runtime/team-tools` endpoint.
- This keeps native family on the same business contract as ACP family without forcing it through ACP MCP.

Team contract note:

- canonical team business semantics now live behind one shared executor in `qai-agent`
- legacy ACP `SharedTeamMcpServer` remains a compatibility adapter only
- adapter methods translate MCP parameters into canonical team tool execution, rather than carrying a second business logic branch

## Config Model

### ACP Backend Identity

The `acp` family supports an optional `acp_backend` field that identifies the specific CLI agent being used. When omitted, the backend is treated as a generic ACP CLI backend.

**Constraints:**
- `acp_backend` is only valid when `family = "acp"`. Other families reject it at config validation.
- `config.toml` does **not** support `${ENV_VAR}` interpolation inside TOML values. All values must be literal strings.
- Gemini is not a validated `acp_backend` in this version.

### Claude via claude-agent-acp (bridge-backed)

This is the active Claude product path.

`quickai-claude-agent` is retained only as a deprecated legacy artifact and is not part of the standard runtime matrix.

```toml
[[backend]]
id = "claude-main"
family = "acp"
acp_backend = "claude"

[backend.launch]
type = "external_command"
command = "npx"
args = ["--yes", "--prefer-offline", "@zed-industries/claude-agent-acp@0.18.0"]

[backend.launch.env]
ANTHROPIC_BASE_URL = "https://api.anthropic.com"
ANTHROPIC_AUTH_TOKEN = "sk-ant-your-key-here"
```

### Codex via codex-acp (bridge-backed)

```toml
[[backend]]
id = "codex-main"
family = "acp"
acp_backend = "codex"

[backend.launch]
type = "external_command"
command = "npx"
args = ["--yes", "--prefer-offline", "@zed-industries/codex-acp@latest"]
```

### CodeBuddy via codebuddy-code (bridge-backed)

```toml
[[backend]]
id = "codebuddy-main"
family = "acp"
acp_backend = "codebuddy"

[backend.launch]
type = "external_command"
command = "npx"
args = ["@tencent-ai/codebuddy-code", "--acp"]
```

### Qwen via generic ACP CLI

```toml
[[backend]]
id = "qwen-main"
family = "acp"
acp_backend = "qwen"

[backend.launch]
type = "external_command"
command = "npx"
args = ["@qwen-code/qwen-code", "--acp"]
```

### iFlow via generic ACP CLI

```toml
[[backend]]
id = "iflow-main"
family = "acp"
acp_backend = "iflow"

[backend.launch]
type = "external_command"
command = "iflow"
args = ["--acp"]
```

### Goose via ACP subcommand path

```toml
[[backend]]
id = "goose-main"
family = "acp"
acp_backend = "goose"

[backend.launch]
type = "external_command"
command = "goose"
args = ["acp"]
```

### Generic or custom ACP backend (no explicit identity)

When `acp_backend` is omitted, QuickAI uses the generic ACP CLI path with no special policy:

```toml
[[backend]]
id = "my-acp-tool"
family = "acp"

[backend.launch]
type = "external_command"
command = "my-acp-tool"
args = ["--acp"]
```

### Primary path (legacy format, still valid)

```toml
[[backend]]
id = "codex-main"
family = "acp"

[backend.launch]
type = "external_command"
command = "codex-acp"
args = ["--stdio"]
```

OpenClaw example:

```toml
[[backend]]
id = "openclaw-main"
family = "open_claw_gateway"

[backend.launch]
type = "gateway_ws"
endpoint = "ws://127.0.0.1:18789"
agent_id = "main"
team_helper_command = "/usr/local/bin/qai-team-cli"
```

OpenClaw constrained leader example:

```toml
[[backend]]
id = "openclaw-lead"
family = "open_claw_gateway"

[backend.launch]
type = "gateway_ws"
endpoint = "ws://127.0.0.1:18789"
agent_id = "main"
team_helper_command = "/usr/local/bin/qai-team-cli"
lead_helper_mode = true
```

Native example:

```toml
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "bundled_command"
```

Launch config note:

- Canonical launch types are `bundled_command` and `external_command`
- Legacy `embedded` and `command` are still accepted as serde aliases for backward-compatible config loading

Rosters can target a catalog backend directly:

```toml
[[agent_roster]]
name = "researcher"
mentions = ["@researcher"]
backend_id = "codex-main"
```

Notes:

- `backend_id` is the new routing target.
- `backend_id` is required for both default agent routing and roster entries.
- There is no engine-centric fallback path in production config anymore.

## External MCP Servers

Current scope:

- `quick_ai_native`: supported
- `acp`: supported
- `open_claw_gateway`: not supported in this phase

Ownership:

- external MCP servers are configured at the `[[backend]]` level
- not at `[[agent_roster]]`
- not at `[[group]]`

Example:

```toml
[[backend]]
id = "claude-main"
family = "acp"

[backend.launch]
type = "external_command"
command = "claude-code"
args = ["--dangerously-skip-permissions"]

[[backend.external_mcp_servers]]
name = "filesystem"
url = "http://127.0.0.1:3001/sse"

[[backend.external_mcp_servers]]
name = "github"
url = "http://127.0.0.1:3002/sse"
```

Contract behavior:

- QuickAI normalizes these into `RuntimeSessionSpec.external_mcp_servers`
- `ACP` merges them with the existing `team-tools` SSE bridge
- `quick_ai_native` receives them over the native JSON runtime session contract and connects from inside `quickai-rust-agent`
- `OpenClaw` keeps its current protocol boundary and does not claim external MCP parity yet

Important:

- this phase supports `SSE` only
- `ToolSurfaceSpec.external_mcp` is now meaningful only as a derived capability bit
- team tools and user-configured external MCP are separate concerns
- external MCP server names must be unique per backend and may not use the reserved name `team-tools`

Why not `OpenClaw` yet:

- QuickAI can normalize OpenClaw runtime events and team helper callbacks
- but current OpenClaw gateway integration still does not expose an equivalent external MCP registration surface for normal chat sessions
- pretending parity here would be dishonest
