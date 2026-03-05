# Team Mode Activation — Design Document

> **Status:** Approved, ready for implementation
> **Date:** 2026-03-05
> **Context:** Phase 5 built the full team swarm infrastructure (TaskRegistry, Heartbeat,
> TeamOrchestrator, Specialist MCP tools). This document designs the missing activation layer:
> how a Lead Agent creates tasks, triggers execution, and communicates results to users.

---

## Problem Statement

`TeamOrchestrator::start()` is never called at runtime. There is no mechanism for a Lead Agent
to create tasks in the TaskRegistry, no way to activate the Heartbeat, and no way for task
completions to propagate back to the Lead or to IM channels. The entire swarm is dead code.

---

## Design Goals

1. **Natural language activation** — User sends a request in plain language; Lead Agent decides
   how to break it into tasks.
2. **Lead controls the plan** — Lead creates tasks via structured MCP tools, not text markers.
3. **Lead controls confirmation** — Simple tasks start immediately; complex tasks show a plan
   summary and wait for user "yes" before executing.
4. **Dependency-aware scheduling** — Tasks declare `deps`; Gateway resolves the DAG
   automatically; upstream outputs are injected into downstream task context.
5. **Lead as communication hub** — Task completions are routed back to Lead as `TeamNotify`
   turns; Lead decides what to tell users via `post_update`.
6. **Configurable visibility** — `public_updates` setting controls how much the group sees.

---

## Architecture Overview

```
User message (natural language)
        │
        ▼
Lead Agent  ←── LeadMcpServer (per-group singleton, starts with group session)
        │
        │  create_task() × N        ← builds DAG in TaskRegistry
        │  request_confirmation()   ← posts plan to IM, waits for "yes"
        │    OR start_execution()   ← immediate start
        │
        ▼
TeamOrchestrator::start()
        │
        ▼
Heartbeat (60s poll)  ─── find_ready_tasks() ─── dispatch_fn() ──► Specialist
        │                                                                │
        │                                                    complete_task(id, note)
        │                                                                │
        ▼                                                                ▼
Gateway handles completion:                               SpecialistMcpServer
  1. mark_done(task_id, note)
  2. unlock downstream tasks
  3. inject upstream note into downstream task_reminder
  4. push TeamNotify turn to Lead
  5. if all_done → push TeamNotify(all_done) to Lead
        │
        ▼
Lead receives TeamNotify turn
        │  post_update(message)  ← optional, Lead decides
        ▼
IM Channel  (DingTalk / Lark)
```

---

## Component 1: LeadMcpServer

A per-group singleton HTTP/SSE MCP server that starts when the group's Lead Agent session is
first created (i.e., when the group is configured with `interaction = "team"`). It is separate
from `TeamToolServer` (which serves Specialists).

### Lifecycle

| Event | Action |
|-------|--------|
| Group session created, mode = Team | `LeadMcpServer::spawn()` → store port in `OnceLock<u16>` |
| Lead agent run() called | Port injected into `AgentCtx.mcp_server_url` |
| Group session destroyed | `LeadMcpServer::stop()` |

The `LeadMcpServer` uses the same rmcp SSE transport as `TeamToolServer`.
It is wired into `SessionRegistry` via a new `OnceLock<LeadMcpServerHandle>` field.

### Tool Definitions

```rust
/// Register a task in the TaskRegistry. Call multiple times to build a dependency DAG.
/// `deps` is a list of task IDs that must complete before this task becomes ready.
create_task(
    id: String,               // e.g. "T001"
    title: String,
    assignee: String,         // agent name, e.g. "codex"
    spec: String,             // detailed requirements
    deps: Vec<String>,        // task IDs that block this task
    success_criteria: String,
) -> String  // "Task T001 registered."

/// Start executing all registered tasks immediately.
/// Spawns TeamOrchestrator (TaskRegistry + Heartbeat + SpecialistMcpServer).
start_execution() -> String  // "Team execution started. 4 tasks queued."

/// Post the plan summary to the IM channel and pause until user confirms.
/// Gateway listens for the next user message; if it contains yes/是/确认/ok,
/// it automatically calls start_execution(). Any other reply is forwarded to Lead.
request_confirmation(
    plan_summary: String,     // human-readable plan to show the user
) -> String  // "Confirmation requested. Waiting for user reply."

/// Push a message to the IM channel (progress update, milestone, final summary).
post_update(
    message: String,
) -> String  // "Posted."

/// Return a snapshot of all task statuses as a JSON string.
/// Lead can use this to decide whether to intervene or re-assign.
get_task_status() -> String  // JSON: [{"id":"T001","status":"done",...}, ...]

/// Re-assign a task to a different agent (e.g. after a block or quality issue).
/// Only valid for tasks in pending or blocked state.
assign_task(
    task_id: String,
    new_assignee: String,
) -> String  // "Task T001 re-assigned to claude."
```

### State Machine for Confirmation Flow

```
PLANNING  ─── start_execution() ──────────────────► RUNNING
    │
    └─── request_confirmation(summary)
                 │
                 ▼
         AWAITING_CONFIRM
                 │
          (next user message)
                 │
        contains yes/是/确认? ─── yes ──► RUNNING
                 │
                no
                 │
                 ▼
         Lead handles message normally (returns to PLANNING)
```

State is stored in `TeamOrchestrator` as `state: Mutex<TeamState>`.

---

## Component 2: TeamNotify — Lead Feedback Loop

When a Specialist completes or blocks a task, the gateway creates a new `InboundMsg` targeted
at the Lead Agent. This is the equivalent of Claude Code's `SendMessage(recipient=lead)`.

### New MsgSource variant

```rust
pub enum MsgSource {
    Human,
    BotMention,
    Relay,
    Cron,
    Heartbeat,
    TeamNotify,   // ← NEW: gateway → Lead notification
}
```

### TeamNotify message content

**On task completion:**
```
[团队通知] 任务 T001 已完成（执行者：codex）

完成摘要：
{completion_note}

下游任务状态：
- T002（codex）：已解锁，待调度
- T003（claude）：已解锁，待调度

当前进度：1 / 4 完成
```

**On task blocked:**
```
[团队通知] 任务 T002 被阻塞（执行者：codex）

阻塞原因：
{reason}

该任务已重置为 pending，将在下次心跳重新调度。
```

**On all_done:**
```
[团队通知] 所有任务已完成 ✅

完成摘要：
- T001（codex）：{note}
- T002（codex）：{note}
- T003（claude）：{note}
- T004（codex）：{note}

请生成最终汇总并通过 post_update 发送给用户。
```

Lead receives TeamNotify as a normal turn. The system prompt for Lead in Team mode instructs
it to: use `post_update` for user-facing messages, use `get_task_status` to check state, and
use `assign_task` if a task needs re-assignment.

### TeamNotify routing in SessionRegistry

```rust
// In handle_specialist_done() → after mark_done():
if let Some(lead_key) = self.team_orchestrator.get()
    .and_then(|o| o.lead_session_key()) {
    let notify_msg = InboundMsg {
        source: MsgSource::TeamNotify,
        session_key: lead_key,
        content: MsgContent::text(build_task_done_notify(task, completion_note, downstream_status)),
        sender: "gateway".to_string(),
        target_agent: Some(front_bot_name),
        ..
    };
    // Route through normal handle() path — Lead responds, may call post_update
    self.handle(notify_msg).await?;
}
```

---

## Component 3: Dependency Graph & Upstream Result Injection

### DAG resolution (existing, unchanged)

`TaskRegistry::find_ready_tasks()` already returns only tasks whose all deps are `done`.
The Heartbeat already calls this every tick. No changes needed.

### Upstream result injection (new)

When Gateway dispatches a task whose `deps` are non-empty, it collects the `completion_note`
of each completed dependency and appends to the task's `task_reminder`:

```
── 上游任务结果 ──────────────────────────
[T001] 设计 DB schema（codex，已完成）：
{T001.completion_note}

[T002] 实现 User 模型（codex，已完成）：
{T002.completion_note}
─────────────────────────────────────────
```

This is implemented in `TeamSession::build_task_reminder()` — query completed deps from
TaskRegistry and append their notes.

---

## Component 4: Progress Reporting Configuration

```toml
[group.team]
roster = ["codex", "claude"]
max_parallel = 5
public_updates = "lead"      # lead: Lead decides via post_update (default)
                              # auto: Gateway posts brief system notice per task
                              # silent: Only final summary
```

**`public_updates = "lead"` (default)**
- Gateway sends TeamNotify to Lead on every task event
- Lead decides whether to call `post_update`
- Most natural; Lead can add context to each update

**`public_updates = "auto"`**
- Gateway posts a system message to IM on every task completion
- Format: `✅ T001 完成（codex）：{completion_note_first_line}`
- Lead still receives TeamNotify; can add additional commentary

**`public_updates = "silent"`**
- No per-task notifications to IM
- Lead receives TeamNotify for all_done; must post final summary

---

## Component 5: Lead System Prompt Additions (Team Mode)

When `AgentRole::Lead` and `TeamState::PLANNING`, prepend to Layer 0:

```
你是团队协调者。用户的请求需要多个 Agent 协作完成。

步骤：
1. 分析任务，调用 create_task() 定义所有子任务和依赖关系
2. 简单任务（≤3个、无复杂依赖）直接调用 start_execution()
3. 复杂任务先调用 request_confirmation(plan_summary)，等用户确认后再执行
4. 任务执行中你会收到 [团队通知] 消息，用 post_update() 向用户播报关键进度
5. 收到"所有任务已完成"通知后，合成最终结果并调用 post_update() 发给用户

可用工具：create_task, start_execution, request_confirmation, post_update, get_task_status, assign_task
```

When `TeamState::RUNNING`, Layer 0 changes to:

```
团队任务执行中。你会收到 [团队通知] 消息。

- 用 post_update(message) 向用户播报进度
- 用 get_task_status() 查看全局状态
- 用 assign_task(task_id, agent) 重新分配卡住的任务
- 收到"所有任务已完成"通知后，合成最终汇总并 post_update
```

---

## Wiring in main.rs

### New objects to create

```rust
// LeadMcpServer — one per group configured as Team mode
// Starts lazily when Lead's first turn is processed
// OR eagerly at startup for known Team groups

// TeamState — added to TeamOrchestrator
pub enum TeamState { Planning, AwaitingConfirm, Running, Done }
pub state: Mutex<TeamState>

// lead_session_key — stored in TeamOrchestrator
// Set when Lead's first turn is processed in Team mode
pub lead_session_key: OnceLock<SessionKey>
```

### Confirmation listener

When `TeamState::AwaitingConfirm`, the registry's `handle()` intercepts the next user message
for that group's session_key. If it matches `yes|是|确认|ok` (case-insensitive), it:
1. Calls `TeamOrchestrator::activate()`  — sets state to Running, starts Heartbeat + SpecialistMcpServer
2. Returns a synthetic reply: `"收到，开始执行。"` (or forwards to Lead for a richer reply)

If it does NOT match, the message is forwarded to Lead normally (Lead may adjust the plan).

---

## Key Design Decisions

| Decision | Choice | Reason |
|----------|--------|--------|
| Lead creates tasks via MCP tools | LeadMcpServer tools | Structured, no fragile text parsing |
| Specialist notifies Lead on completion | TeamNotify MsgSource → Lead turn | Mirrors Claude Code SendMessage; Lead controls communication |
| Upstream results injected automatically | Gateway appends to task_reminder | Lead not a relay bottleneck; all info available in TaskRegistry |
| Confirmation flow | Lead calls request_confirmation; Gateway listens | Lead decides complexity; user UX is just one "是" |
| LeadMcpServer lifecycle | Per-group, starts with first Lead turn in Team mode | No startup cost for non-Team groups |
| TeamOrchestrator::start() split | `register_tasks()` (called by create_task MCP) + `activate()` (called by start_execution MCP) | Allows incremental task registration before execution starts |

---

## Files to Change (Implementation Scope)

| File | Change |
|------|--------|
| `qai-protocol/src/types.rs` | Add `MsgSource::TeamNotify` |
| `qai-agent/src/team/orchestrator.rs` | Add `TeamState`, `lead_session_key`, `activate()`, `register_task()` |
| `qai-agent/src/team/lead_mcp_server.rs` | **New**: LeadMcpServer with 6 tools |
| `qai-agent/src/team/mod.rs` | Add `pub mod lead_mcp_server` |
| `qai-agent/src/registry.rs` | TeamNotify routing, confirmation interception, Lead MCP injection |
| `qai-agent/src/prompt_builder.rs` | Add Lead Team mode Layer 0 (planning vs running state) |
| `qai-agent/src/team/session.rs` | `build_task_reminder`: inject upstream completion notes |
| `qai-server/src/main.rs` | Wire LeadMcpServer per group, wire TeamNotify dispatch |
| `qai-protocol/src/config.rs` | Add `public_updates` field to `GroupConfig.team` |

---

## Non-Goals (MVP Exclusions)

- `ask_lead()` tool for Specialists (mid-task peer communication) — add post-MVP
- Lead reviewing Specialist output quality before marking done — add post-MVP
- Persistent TaskRegistry across restarts (currently in-memory SQLite) — add post-MVP
- Dynamic agent scaling (spawn new agents based on workload) — Kimi-style, not needed now
