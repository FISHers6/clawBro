# Backend Support Matrix

## Purpose

This document answers a narrower question than [`runtime-backends.md`](/Users/fishers/Desktop/repo/quickai-openclaw/quickai-gateway/docs/runtime-backends.md):

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

| Family | Current role in QuickAI | Session continuity | Tool lifecycle events | Team contract | Current level |
| --- | --- | --- | --- | --- | --- |
| `quick_ai_native` | Canonical native consumer | Structured host transcript consumed directly by native bridge | Native tool wrapper emits canonical start/completed/failed events | Canonical Team Tool RPC | `Complete` |
| `acp` | Compatibility family with real protocol events | Host-owned transcript projected into ACP session/prompt flow | ACP `ToolCall / ToolCallUpdate` normalized into canonical events | Canonical Team Tool / MCP bridge | `Structured` |
| `open_claw_gateway` | Compatibility family with real gateway protocol | Host-owned transcript projected into OpenClaw chat path | OpenClaw `stream:"tool"` events normalized into canonical events | Team helper bridge + generic tool lifecycle | `Structured` |

## Session and Context Contract

### What is host-owned

QuickAI owns:

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

QuickAI does not try to erase these differences. It normalizes them through host-side contracts.

## Runtime Progress Events

QuickAI now has a host-level runtime progress contract:

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

- `cargo test -p quickai-rust-agent`
- real Lark DM regression with compact progress and final reply

#### `acp`

Status: `Structured`

- Tool lifecycle comes from ACP protocol-native updates:
  - `SessionUpdate::ToolCall`
  - `SessionUpdate::ToolCallUpdate`
- QuickAI normalizes these into canonical runtime events
- This is not text guessing and not fake progress

Validated by:

- `cargo test -p qai-runtime`
- `cargo test -p qai-server`

Current limitation:

- tool labels come from ACP title/fields and are not yet mapped into a host-level canonical tool class

#### `open_claw_gateway`

Status: `Structured`

- Tool lifecycle comes from OpenClaw gateway `agent` events with:
  - `stream: "tool"`
  - `phase: "start" | "result"`
  - `toolCallId`
  - `name`
  - `isError`
- QuickAI preserves helper-result parsing for team helper JSON, then falls back to generic tool lifecycle parsing

Validated by:

- `cargo test -p qai-runtime`
- `cargo test -p qai-server`
- OpenClaw source tests run locally after installing dependencies:
  - `src/gateway/server-chat.agent-events.test.ts`
  - `src/tui/tui-event-handlers.test.ts`

Current limitation:

- QuickAI does not yet normalize OpenClaw `phase: "update"` into a host event
- That is acceptable for `final_only` and `progress_compact`, but not enough for a future verbose/debug surface

## Channel Presentation

QuickAI intentionally separates:

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

The honest status of QuickAI today is:

- one shared host contract now exists across `quick_ai_native`, `acp`, and `open_claw_gateway`
- family parity is real enough to build on
- product parity is still led by `quick_ai_native + Lark`

That is strong enough to document and ship, but it is not yet a claim that every backend family exposes identical end-user behavior on every channel.
