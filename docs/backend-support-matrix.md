# Backend Support Matrix

## Purpose

This document answers a narrower question than [`runtime-backends.md`](/Users/fishers/Desktop/repo/clawbro-openclaw/clawbro-gateway/docs/runtime-backends.md):

- Which backend families are supported today?
- Which runtime capabilities are truly implemented vs only architecturally planned?
- Which channel presentation features are already productized?

It is intentionally conservative. If a capability has not been validated in code and tests, it should not be treated as generally available.

## Capability Levels

- `Complete`: implemented, wired into the host contract, and validated by runtime or end-to-end tests
- `Structured`: implemented through protocol-native events, but not yet validated in a real user-facing product loop
- `Compatible`: supported through host-owned transcript / prompt projection, but not yet parity with the strongest family
- `Not Yet`: not implemented or not honest to claim

## Backend Families

| Family | Current role in ClawBro | Session continuity | Tool lifecycle events | Team contract | Current level |
| --- | --- | --- | --- | --- | --- |
| `quick_ai_native` | Canonical native consumer | Structured host transcript consumed directly by native bridge | Native tool wrapper emits canonical start/completed/failed events | Canonical Team Tool RPC | `Complete` |
| `acp` | Compatibility family with real protocol events | Host-owned transcript projected into ACP session/prompt flow | ACP `ToolCall / ToolCallUpdate` normalized into canonical events | Canonical Team Tool / MCP bridge | `Structured` |
| `open_claw_gateway` | Compatibility family with real gateway protocol | Host-owned transcript projected into OpenClaw chat path | OpenClaw `stream:"tool"` events normalized into canonical events | Team helper bridge + generic tool lifecycle | `Structured` |

## Session and Context Contract

### What is host-owned

ClawBro owns:

- session transcript truth
- recent structured history projection
- filesystem-native context projection
- shared memory and agent memory projections
- team/task shared artifacts

This is the system of record. Backend-local state may exist, but it is not the authoritative source for routing, session truth, or team coordination.

### What backends still own

Each backend family still owns its local execution details:

- `quick_ai_native`: rig/native runtime loop
- `acp`: ACP session/protocol mechanics
- `open_claw_gateway`: OpenClaw run/session behavior behind gateway WS

ClawBro does not try to erase these differences. It normalizes them through host-side contracts.

## Runtime Progress Events

ClawBro now has a host-level runtime progress contract:

- `ToolCallStarted`
- `ToolCallCompleted`
- `ToolCallFailed`

These are emitted into host event flow and then mapped to channel-level presentation.

### Family status

#### `quick_ai_native`

Status: `Complete`

- Tool lifecycle comes from the real tool execution wrapper, not from speculative model chunks
- `call_id` is stable across start/completed/failed
- This is the strongest current implementation

Validated by:

- `cargo test -p clawbro-rust-agent`
- real Lark DM regression with compact progress and final reply

#### `acp`

Status: `Structured`

- Tool lifecycle comes from ACP protocol-native updates:
  - `SessionUpdate::ToolCall`
  - `SessionUpdate::ToolCallUpdate`
- ClawBro normalizes these into canonical runtime events
- This is not text guessing and not fake progress

ACP update compatibility:

- `SessionUpdate::UsageUpdate` — decoded and silently ignored (features: `unstable_session_usage`)
- `SessionUpdate::SessionInfoUpdate` — decoded and silently ignored (features: `unstable_session_info_update`)
- Additive protocol variants no longer fail a prompt turn

Validated by:

- `cargo test -p clawbro-runtime`
- `cargo test -p clawbro-server`
- Echo fixture emits `UsageUpdate` + `SessionInfoUpdate` before text; all ACP E2E tests pass

Current limitation:

- tool labels come from ACP title/fields and are not yet mapped into a host-level canonical tool class

### ACP Backend Identity and Support Levels

The `acp` family supports multiple CLI-backed agents. ClawBro models them in three categories:

#### Category A: Bridge-Backed ACP Backends

Require a dedicated adapter package, not a raw CLI `--acp` flag.

| Backend | `acp_backend` | Launch | Status |
| --- | --- | --- | --- |
| Claude | `claude` | `npx @zed-industries/claude-agent-acp` | `Bridge-backed (e2e-validated)` |
| Codex | `codex` | `npx @zed-industries/codex-acp` | `Bridge-backed (code-supported)` |
| CodeBuddy | `codebuddy` | `npx @tencent-ai/codebuddy-code --acp` | `Bridge-backed (code-supported)` |

These require the adapter npm package to be installed or npx-resolvable.

#### Category B: Generic ACP CLI Backends

Can be launched with a direct command + args. Expected to speak ACP over stdio.

| Backend | `acp_backend` | Example launch | Status |
| --- | --- | --- | --- |
| Qwen Code | `qwen` | `npx @qwen-code/qwen-code --acp` | `Experimental` |
| iFlow | `iflow` | `iflow --acp` | `Experimental` |
| Goose | `goose` | `goose acp` | `Experimental` |
| Kimi | `kimi` | `kimi --acp` | `Experimental` |
| OpenCode | `opencode` | `opencode --acp` | `Experimental` |
| Qoder | `qoder` | `qoder --acp` | `Experimental` |
| Vibe | `vibe` | `vibe --acp` | `Experimental` |
| Custom | `custom` | user-defined | `Experimental` |
| (unspecified) | _(omitted)_ | user-defined | `Experimental` |

`acp_backend` is optional. When omitted, the generic ACP CLI path is used.

#### Category C: Declared but Not Yet Enabled

| Backend | Status | Notes |
| --- | --- | --- |
| Gemini CLI | `Declared` | No validated ACP path in ClawBro; not in the `acp_backend` enum |

**Support level semantics:**

- `Bridge-backed (code-supported)` — adapter package required; bridge bootstrap path and host-side code path exist; backend-specific E2E validation may still be pending
- `Experimental` — config model exists; generic ACP CLI path; no end-to-end validation guarantee
- `Declared` — planned backend identity; no validated adapter path

#### `open_claw_gateway`

Status: `Structured`

- Tool lifecycle comes from OpenClaw gateway `agent` events with:
  - `stream: "tool"`
  - `phase: "start" | "result"`
  - `toolCallId`
  - `name`
  - `isError`
- ClawBro preserves helper-result parsing for team helper JSON, then falls back to generic tool lifecycle parsing

Validated by:

- `cargo test -p clawbro-runtime`
- `cargo test -p clawbro-server`
- OpenClaw source tests run locally after installing dependencies:
  - `src/gateway/server-chat.agent-events.test.ts`
  - `src/tui/tui-event-handlers.test.ts`

Current limitation:

- ClawBro does not yet normalize OpenClaw `phase: "update"` into a host event
- That is acceptable for `final_only` and `progress_compact`, but not enough for a future verbose/debug surface

## External MCP Support

| Family | External MCP config ownership | Current support | Notes |
| --- | --- | --- | --- |
| `quick_ai_native` | `[[backend.external_mcp_servers]]` | `Complete` | native runtime now receives the list through `RuntimeSessionSpec`, connects from inside `clawbro-rust-agent`, and is covered by real SSE MCP integration tests |
| `acp` | `[[backend.external_mcp_servers]]` | `Structured` | ACP merges external SSE MCP servers with the existing `team-tools` bridge |
| `open_claw_gateway` | n/a in this phase | `Not Yet` | OpenClaw still keeps team helper CLI bridge, but does not claim generic external MCP parity |

Important:

- external MCP is a backend capability, not a roster/persona capability
- this phase supports `SSE` only
- `team-tools` MCP and user-configured external MCP are separate paths

This is intentional. It keeps system ownership clear:

- backend owns launch env, protocol family, and external tool transport
- roster owns persona/workspace/mention identity
- group owns routing and team mode, not backend transport capability

## Channel Presentation

ClawBro intentionally separates:

- transcript truth
- runtime progress events
- channel presentation policy

This prevents backend-specific raw tool text from leaking directly into user-facing channels.

### Current presentation modes

- `final_only`
- `progress_compact`

### Current productized scope

| Channel | `final_only` | `progress_compact` | Status |
| --- | --- | --- | --- |
| `Lark` | Yes | Yes | `Complete` |
| `DingTalk` | Yes | Yes | `Structured` |
| `WebSocket` | Raw event stream available | client decides | `Structured`, not productized as IM UX |

Important:

- `progress_compact` is now wired for both Lark and DingTalk
- This is not a runtime limitation
- It is a presentation-layer rollout boundary

## What users can honestly expect today

## Provider Profile Validation Snapshot

Current provider/profile reality is intentionally conservative:

- `quick_ai_native + openai_compatible` is validated end-to-end
- `quick_ai_native + anthropic_compatible` remains vendor-dependent
- `ACP/Claude + official_session` is validated end-to-end
- `ACP/Claude + anthropic_compatible` is validated end-to-end on the DeepSeek Anthropic-compatible path
- `ACP/Codex + local_config_projection + DeepSeek /v1` ACP decode compatibility is resolved; provider-side 404 on `/v1/responses` remains the blocker (requires a Codex-responses-compatible endpoint)
- DeepSeek should currently be treated as an `openai_compatible` target for the native family

Reason:

- the native runtime uses `rig`
- the OpenAI-compatible DeepSeek path validated end-to-end
- the DeepSeek `/anthropic` path did not validate as a stable native Anthropic-compatible streaming target

### Codex Local-Config Projection Constraint

`Codex` now supports two distinct projection paths in ClawBro:

- `acp_auth_projection`
  - for `chatgpt`, `openai_api_key`, and `codex_api_key`
- `local_config_projection`
  - for Codex-native `auth.json + config.toml`
  - used for custom provider/base URL scenarios

However, `local_config_projection` is only usable when the target provider
supports the Codex `responses` API surface.

A generic OpenAI-compatible `/v1` endpoint is not sufficient by itself.

ClawBro has already validated one negative case:

- `Codex + local_config_projection + DeepSeek /v1`
  - projection succeeded
  - isolated `CODEX_HOME` was written correctly
  - upstream provider returned `404 Not Found` for `/v1/responses`

Therefore, `Codex local_config_projection` should currently be treated as:

- `code-supported`
- `provider-dependent`
- requiring explicit E2E validation per vendor

### For `quick_ai_native + Lark`

Users can expect:

- correct short-session continuity
- real tool execution
- one compact progress message such as `⏳ 正在搜索代码`
- one final answer

This is already validated in real Feishu/Lark DM usage.

### For `acp`

Users can expect:

- runtime/tool lifecycle correctness at the host layer
- shared transcript and progress-event contract
- compatibility-family behavior, not native parity

They should not yet expect:

- the same degree of tool-label fidelity as native
- the same level of product-facing validation as native + Lark

### For `open_claw_gateway`

Users can expect:

- real gateway WS integration
- generic tool lifecycle parity at the host event layer
- helper result bridging preserved

They should not yet expect:

- full verbose lifecycle parity with OpenClaw TUI
- product-facing compact progress to have been user-validated in the same way as native + Lark

## Current limitations

The main remaining gaps are:

1. backend tool names are still family-shaped, not yet host-classified
2. OpenClaw `tool update / partial result` is not yet normalized
3. `debug_verbose` presentation mode is not implemented
4. `WebSocket` remains a structured event surface, not an IM compact-progress surface

These are not mainline correctness problems. They are parity and product-surface gaps.

## Recommended interpretation

The honest status of ClawBro today is:

- one shared host contract now exists across `quick_ai_native`, `acp`, and `open_claw_gateway`
- family parity is real enough to build on
- product parity is still led by `quick_ai_native + Lark`

That is strong enough to document and ship, but it is not yet a claim that every backend family exposes identical end-user behavior on every channel.
