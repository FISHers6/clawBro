# Team Mode Activation — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Connect the dead-code Phase 5 swarm infrastructure to a live runtime. Lead Agent creates tasks via 6 MCP tools, TeamOrchestrator schedules them, Specialist completions route back to Lead via `TeamNotify` turns, Lead posts final results to IM.

**Architecture:** LeadMcpServer (per-group SSE MCP server identical in transport to TeamToolServer) is spawned in main.rs. Lead calls `create_task()` × N then `start_execution()` or `request_confirmation()`. TeamNotify is a new `MsgSource` variant that injects Specialist completion summaries as Lead turns. TeamState machine (Planning→AwaitingConfirm→Running→Done) lives inside `TeamOrchestrator` so `registry.handle()` can intercept the confirmation step.

**Tech Stack:** Rust, rmcp 0.6.4, tokio, rusqlite, DashMap, existing rmcp SSE transport pattern from `mcp_server.rs`.

**Prerequisites:** All Phase 5 tasks (T1–T14) plus MCP T1–T6 are complete (307 tests pass).

---

## Files Overview

| File | Change |
|------|--------|
| `crates/qai-protocol/src/types.rs` | +`MsgSource::TeamNotify` variant |
| `crates/qai-agent/src/team/orchestrator.rs` | +`TeamState`, +`lead_session_key`, +`lead_mcp_server_port`, +`register_task()`, +`activate()`, +`post_message()` |
| `crates/qai-agent/src/team/session.rs` | Extend `build_task_reminder()`: inject upstream completion notes |
| `crates/qai-agent/src/team/lead_mcp_server.rs` | **NEW**: 6 Lead tools |
| `crates/qai-agent/src/team/mod.rs` | +`pub mod lead_mcp_server` |
| `crates/qai-agent/src/registry.rs` | Lead detection + Layer 0 + TeamNotify dispatch + confirmation interceptor |
| `crates/qai-server/src/main.rs` | Spawn LeadMcpServer, wire `lead_mcp_server_port` |

---

## Task 1: Add `MsgSource::TeamNotify` to qai-protocol

**Files:**
- Modify: `crates/qai-protocol/src/types.rs`

**Step 1: Write the failing test**

In `crates/qai-protocol/src/types.rs` tests block, add:

```rust
#[test]
fn test_team_notify_variant_exists() {
    let src = MsgSource::TeamNotify;
    // Must serialize without panic
    let json = serde_json::to_string(&src).unwrap();
    assert!(json.contains("team_notify"));
    // Must deserialize back
    let back: MsgSource = serde_json::from_str(&json).unwrap();
    assert_eq!(back, MsgSource::TeamNotify);
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p qai-protocol test_team_notify_variant_exists 2>&1 | tail -5
```

Expected: `error[E0599]: no variant named \`TeamNotify\``

**Step 3: Add the variant**

In `crates/qai-protocol/src/types.rs`, add after `Heartbeat`:

```rust
    /// Heartbeat,
    /// Gateway → Lead: 通知 Specialist 任务完成 / 全部完成
    TeamNotify,
```

The enum now has 6 variants. `#[serde(rename_all = "snake_case")]` produces `"team_notify"`.

**Step 4: Run test to verify it passes**

```bash
cargo test -p qai-protocol -- --nocapture 2>&1 | tail -5
```

Expected: all tests pass.

**Step 5: Commit**

```bash
git add crates/qai-protocol/src/types.rs
git commit -m "feat(protocol): add MsgSource::TeamNotify for Lead feedback loop"
```

---

## Task 2: TeamOrchestrator — TeamState + register_task() + activate()

**Files:**
- Modify: `crates/qai-agent/src/team/orchestrator.rs`

### Step 1: Write failing tests first

Add to the `#[cfg(test)]` block:

```rust
#[test]
fn test_register_task_increments_registry() {
    let (orch, _tmp) = make_orchestrator();
    let result = orch.register_task(CreateTask {
        id: "T001".into(),
        title: "Write DB schema".into(),
        ..Default::default()
    });
    assert!(result.is_ok());
    assert!(result.unwrap().contains("T001"));
    let task = orch.registry.get_task("T001").unwrap().unwrap();
    assert_eq!(task.title, "Write DB schema");
}

#[test]
fn test_team_state_starts_planning() {
    let (orch, _tmp) = make_orchestrator();
    assert!(matches!(orch.team_state(), TeamState::Planning));
}

#[tokio::test]
async fn test_activate_starts_heartbeat_and_mcp() {
    let (orch, _tmp) = make_orchestrator();
    orch.register_task(CreateTask {
        id: "T001".into(),
        title: "test".into(),
        ..Default::default()
    }).unwrap();
    orch.activate().await.unwrap();
    assert!(matches!(orch.team_state(), TeamState::Running));
    assert!(orch.mcp_server_port.get().is_some());
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test -p qai-agent team_state 2>&1 | tail -10
```

Expected: compile error — `TeamState` not defined.

**Step 3: Add TeamState + new fields**

In `crates/qai-agent/src/team/orchestrator.rs`:

After the existing imports, add:

```rust
/// Team Mode 执行状态机
#[derive(Debug, Clone, PartialEq)]
pub enum TeamState {
    /// Lead 正在通过 create_task() 建立任务图
    Planning,
    /// Lead 调用了 request_confirmation()，等待用户确认
    AwaitingConfirm,
    /// 任务执行中（Heartbeat 运行）
    Running,
    /// 所有任务已完成
    Done,
}
```

In `TeamOrchestrator` struct, add fields after `mcp_server_port`:

```rust
    /// 当前 Team 执行状态（Planning / AwaitingConfirm / Running / Done）
    pub team_state_inner: std::sync::Mutex<TeamState>,
    /// Lead Agent 的 IM session key（设置后用于 TeamNotify 路由）
    pub lead_session_key: std::sync::OnceLock<qai_protocol::SessionKey>,
    /// Bound port of the Lead MCP server (set after spawn in main.rs).
    pub lead_mcp_server_port: std::sync::OnceLock<u16>,
```

In `TeamOrchestrator::new()`, initialize the new fields:

```rust
    // inside the Arc::new(Self { ... }) block, after mcp_server_port:
    team_state_inner: std::sync::Mutex::new(TeamState::Planning),
    lead_session_key: std::sync::OnceLock::new(),
    lead_mcp_server_port: std::sync::OnceLock::new(),
```

**Step 4: Add methods**

After `set_notify_fn()`:

```rust
    // ── Team 状态 ──────────────────────────────────────────────────────────────

    /// 获取当前 TeamState（克隆副本）
    pub fn team_state(&self) -> TeamState {
        self.team_state_inner.lock().unwrap().clone()
    }

    /// 设置 Lead 的 IM session key（由 main.rs 在启动时或首次 Lead 消息时调用）
    pub fn set_lead_session_key(&self, key: qai_protocol::SessionKey) {
        let _ = self.lead_session_key.set(key);
    }

    /// 向 IM 频道发布一条消息（Lead 调用 post_update 时使用）
    pub fn post_message(&self, message: &str) {
        if let (Some(f), Some(scope)) = (self.notify_fn.get(), self.scope.get()) {
            (f)(scope.clone(), message.to_string());
        }
    }

    // ── 增量任务注册（供 LeadMcpServer.create_task 调用）──────────────────────

    /// 在 Planning 阶段注册单个任务。只能在 state == Planning 时调用。
    pub fn register_task(&self, task: super::registry::CreateTask) -> Result<String> {
        let state = self.team_state_inner.lock().unwrap().clone();
        if !matches!(state, TeamState::Planning | TeamState::AwaitingConfirm) {
            anyhow::bail!("Cannot register task: team is already {:?}", state);
        }
        let id = task.id.clone();
        self.registry.create_task(task)?;
        Ok(format!("Task {} registered.", id))
    }

    // ── 激活执行（供 LeadMcpServer.start_execution 调用）────────────────────

    /// 启动 Heartbeat + SpecialistMcpServer，设置 state → Running。
    /// 只允许调用一次（OnceLock guard）。
    pub async fn activate(self: &Arc<Self>) -> Result<String> {
        // Guard: already running?
        if self.mcp_server_port.get().is_some() {
            anyhow::bail!("TeamOrchestrator::activate() called twice");
        }
        // Transition state
        *self.team_state_inner.lock().unwrap() = TeamState::Running;

        // Write TEAM.md if not yet written (minimal manifest)
        let manifest = self.session.read_team_md();
        if manifest.is_empty() {
            let _ = self.session.write_team_md("Team execution started.");
        }

        // Sync TASKS.md snapshot
        self.session.sync_tasks_md(&self.registry)?;

        // Start Heartbeat
        let heartbeat = std::sync::Arc::new(super::heartbeat::OrchestratorHeartbeat::new(
            std::sync::Arc::clone(&self.registry),
            std::sync::Arc::clone(&self.session),
            std::sync::Arc::clone(&self.dispatch_fn),
            self.heartbeat_interval,
        ));
        let handle = tokio::spawn({
            let hb = std::sync::Arc::clone(&heartbeat);
            async move { hb.run().await }
        });
        *self.heartbeat_handle.lock().unwrap() = Some(handle);

        // Start SpecialistMcpServer
        let mcp_srv = super::mcp_server::TeamToolServer::new(
            std::sync::Arc::clone(&self.registry),
            std::sync::Arc::clone(self),
            self.session.team_id.clone(),
        );
        let mcp_handle = mcp_srv.spawn().await?;
        let _ = self.mcp_server_port.set(mcp_handle.port);
        *self.mcp_server_handle.lock().await = Some(mcp_handle);

        let task_count = self.registry.find_ready_tasks()?.len()
            + self.registry.all_tasks()?.iter().filter(|t| !matches!(t.status_parsed(), super::registry::TaskStatus::Done)).count();

        tracing::info!(
            team_id = %self.session.team_id,
            mcp_port = ?self.mcp_server_port.get(),
            "Team activated"
        );
        Ok(format!("Team execution started. {} tasks queued.", task_count))
    }
```

Also update `start()` to delegate to `register_task` + `activate` to avoid code duplication:

```rust
    pub async fn start(self: &Arc<Self>, plan: &TeamPlan) -> Result<()> {
        // Guard against double-start
        if self.mcp_server_port.get().is_some() {
            anyhow::bail!("TeamOrchestrator::start() called twice for team '{}'", self.session.team_id);
        }
        // 1. Write TEAM.md
        self.session.write_team_md(&plan.team_manifest)?;
        // 2. Register all tasks
        for task in &plan.tasks {
            self.register_task(super::registry::CreateTask {
                id: task.id.clone(),
                title: task.title.clone(),
                assignee_hint: task.assignee.clone(),
                deps: task.deps.clone(),
                timeout_secs: 1800,
                spec: task.spec.clone(),
                success_criteria: task.success_criteria.clone(),
            })?;
        }
        // 3+4+5. Activate (syncs TASKS.md, starts Heartbeat + MCP)
        self.activate().await?;
        tracing::info!(team_id = %self.session.team_id, tasks = plan.tasks.len(), "Team started via start()");
        Ok(())
    }
```

Note: `TaskRegistry::all_tasks()` may not exist. Use `registry.find_ready_tasks()?.len()` count or simply hardcode the message. Simplest: just return `"Team execution started."` without a count (avoid needing all_tasks()).

Correction to the activate() return:
```rust
        Ok("Team execution started.".to_string())
```

**Step 5: Run tests**

```bash
cargo test -p qai-agent test_register_task test_team_state test_activate 2>&1 | tail -20
```

Expected: all 3 new tests pass. Existing tests still pass.

**Step 6: Commit**

```bash
git add crates/qai-agent/src/team/orchestrator.rs
git commit -m "feat(team): add TeamState + register_task() + activate() to TeamOrchestrator"
```

---

## Task 3: Upstream Result Injection in build_task_reminder()

**Files:**
- Modify: `crates/qai-agent/src/team/session.rs`

### Step 1: Write failing test

In the `#[cfg(test)]` block, add:

```rust
#[test]
fn test_build_task_reminder_injects_upstream_notes() {
    let (session, _tmp) = make_session();
    let registry = TaskRegistry::new_in_memory().unwrap();

    // T001 is a dependency with a completion note
    registry.create_task(CreateTask {
        id: "T001".into(),
        title: "Design schema".into(),
        ..Default::default()
    }).unwrap();
    registry.try_claim("T001", "codex").unwrap();
    registry.mark_done("T001", "Created users table with uuid pk").unwrap();

    // T002 depends on T001
    registry.create_task(CreateTask {
        id: "T002".into(),
        title: "Implement model".into(),
        deps: vec!["T001".into()],
        ..Default::default()
    }).unwrap();

    let task = registry.get_task("T002").unwrap().unwrap();
    let reminder = session.build_task_reminder(&task, &registry);

    assert!(reminder.contains("上游任务结果"), "must have upstream section header");
    assert!(reminder.contains("T001"), "must mention T001");
    assert!(reminder.contains("Created users table"), "must include T001 completion note");
}
```

**Step 2: Run to verify failure**

```bash
cargo test -p qai-agent test_build_task_reminder_injects_upstream 2>&1 | tail -5
```

Expected: test fails — upstream section not found.

**Step 3: Implement upstream injection**

In `TeamSession::build_task_reminder()`, after building `deps_str` and `blocking_str`, collect upstream notes:

```rust
    // Collect upstream completion notes for deps that are done
    let upstream_notes: Vec<String> = deps
        .iter()
        .filter_map(|dep_id| {
            registry.get_task(dep_id).ok().flatten().and_then(|t| {
                t.completion_note.as_ref().map(|note| {
                    format!("[{}] {}（{}，已完成）：\n{}", dep_id, t.title, t.assignee_hint.as_deref().unwrap_or("unknown"), note)
                })
            })
        })
        .collect();

    let upstream_section = if upstream_notes.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n── 上游任务结果 ──────────────────────────\n{}\n─────────────────────────────────────────",
            upstream_notes.join("\n\n")
        )
    };
```

Then append `upstream_section` to the format string at the very end:

```rust
    format!(
        "...(existing format)...{upstream_section}",
        // all existing named args +
        upstream_section = upstream_section,
    )
```

The full updated format call (showing only the changed tail):

```rust
    format!(
        "══════ 当前任务（自动注入，最高优先级）══════\n\
         任务ID: {id}\n\
         标题: {title}\n\
         详细说明: {spec}\n\
         依赖（已完成）: {deps}\n\
         被阻塞的下游任务: {blocking}\n\
         \n\
         ── 成功标准 ──\n\
         {criteria}\n\
         \n\
         ── 必须遵守 ──\n\
         1. 完成后在回复**最后一行**加 [DONE: {id}] 标记，否则系统不会更新任务状态\n\
         2. 如遇阻塞，在回复最后一行加 [BLOCKED: <原因>] 标记\n\
         3. 重要产出（文件路径、关键发现）写在回复正文\n\
         4. 完成任务时调用工具 `complete_task(task_id, note)` 或输出 `[DONE: {id}]`。\n\
         5. 遇到阻塞时调用工具 `block_task(task_id, reason)` 或输出 `[BLOCKED: reason]`。\n\
         ══════════════════════════════════════════{upstream_section}",
        id = task.id,
        title = task.title,
        spec = task.spec.as_deref().unwrap_or("（无详细说明）"),
        deps = deps_str,
        blocking = blocking_str,
        criteria = task.success_criteria.as_deref().unwrap_or("完成任务说明中描述的工作"),
        upstream_section = upstream_section,
    )
```

**Step 4: Run tests**

```bash
cargo test -p qai-agent session 2>&1 | tail -10
```

Expected: all session tests pass including the new one.

**Step 5: Commit**

```bash
git add crates/qai-agent/src/team/session.rs
git commit -m "feat(team): inject upstream completion notes into Specialist task_reminder"
```

---

## Task 4: Implement LeadMcpServer

**Files:**
- Create: `crates/qai-agent/src/team/lead_mcp_server.rs`
- Modify: `crates/qai-agent/src/team/mod.rs`

### Step 1: Study the existing TeamToolServer pattern

Read `crates/qai-agent/src/team/mcp_server.rs`. The `spawn()` method:
1. Creates `rmcp::ServiceExt` router with `Router::new()` and `.route(tool_handler)`
2. Binds `TcpListener::bind("127.0.0.1:0")`
3. Returns `TeamMcpServerHandle { port }`

The `LeadMcpServer` follows the same pattern exactly.

### Step 2: Write failing test (compilation-level)

Add to `crates/qai-agent/src/team/mod.rs`:

```rust
pub mod lead_mcp_server;
```

Run `cargo check -p qai-agent` — fails because the file doesn't exist.

### Step 3: Create lead_mcp_server.rs with all 6 tools

Create `crates/qai-agent/src/team/lead_mcp_server.rs`:

```rust
//! LeadMcpServer — per-group MCP server for the Lead Agent.
//!
//! Provides 6 tools that allow the Lead Agent to manage the team task lifecycle:
//!   - create_task         : register a task in TaskRegistry
//!   - start_execution     : activate TeamOrchestrator (start Heartbeat + SpecialistMcpServer)
//!   - request_confirmation: post plan to IM + set state → AwaitingConfirm
//!   - post_update         : push a message to the IM channel
//!   - get_task_status     : JSON snapshot of all task statuses
//!   - assign_task         : re-assign a pending/blocked task

use anyhow::Result;
use rmcp::{
    handler::server::tool::{Parameters, ToolCallContext},
    model::{Content, Tool},
    schemars, ServerHandler,
};
use std::sync::Arc;

use super::orchestrator::{TeamOrchestrator, TeamState};
use super::registry::{CreateTask, TaskRegistry};

// ─── LeadToolServer ───────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct LeadToolServer {
    orchestrator: Arc<TeamOrchestrator>,
}

impl LeadToolServer {
    pub fn new(orchestrator: Arc<TeamOrchestrator>) -> Self {
        Self { orchestrator }
    }

    /// Spawn the SSE MCP server on a random port and return its handle.
    pub async fn spawn(self) -> Result<LeadMcpServerHandle> {
        use rmcp::transport::sse_server::{SseServer, SseServerConfig};
        use std::net::SocketAddr;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let addr: SocketAddr = listener.local_addr()?;

        let sse_config = SseServerConfig {
            bind: addr,
            sse_path: "/sse".to_string(),
            post_path: "/message".to_string(),
            ct: tokio_util::sync::CancellationToken::new(),
            ..Default::default()
        };

        let ct = sse_config.ct.clone();
        let server = self;
        let task = tokio::spawn(async move {
            if let Err(e) = SseServer::serve_with_config(sse_config, move || {
                let srv = server.clone();
                async move { Ok(srv) }
            })
            .await
            {
                tracing::error!("LeadMcpServer error: {e}");
            }
        });

        tracing::info!(port = port, "LeadMcpServer started");
        Ok(LeadMcpServerHandle { port, _ct: ct, _task: task })
    }
}

// ─── Tool parameter types ─────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateTaskParams {
    pub id: String,
    pub title: String,
    pub assignee: Option<String>,
    pub spec: Option<String>,
    pub deps: Option<Vec<String>>,
    pub success_criteria: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RequestConfirmationParams {
    pub plan_summary: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PostUpdateParams {
    pub message: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AssignTaskParams {
    pub task_id: String,
    pub new_assignee: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CompleteTaskIdParam {
    pub task_id: String,
}

// ─── ServerHandler impl ───────────────────────────────────────────────────────

impl ServerHandler for LeadToolServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo {
            name: "lead-tools".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            ..Default::default()
        }
    }

    fn list_tools(&self, _cx: &ToolCallContext<'_>) -> Vec<Tool> {
        vec![
            Tool::new("create_task", "注册任务到任务队列，可声明依赖关系（DAG）"),
            Tool::new("start_execution", "立即开始执行所有已注册的任务"),
            Tool::new("request_confirmation", "向用户展示计划摘要并等待确认（yes/是/确认/ok）"),
            Tool::new("post_update", "向 IM 频道发送进度更新或最终摘要"),
            Tool::new("get_task_status", "返回所有任务状态的 JSON 快照"),
            Tool::new("assign_task", "重新分配任务给不同的 agent（仅 pending/blocked 状态）"),
        ]
    }

    async fn call_tool(
        &self,
        name: &str,
        params: serde_json::Value,
        _cx: &ToolCallContext<'_>,
    ) -> rmcp::model::CallToolResult {
        let result = match name {
            "create_task" => self.tool_create_task(params).await,
            "start_execution" => self.tool_start_execution().await,
            "request_confirmation" => self.tool_request_confirmation(params),
            "post_update" => self.tool_post_update(params),
            "get_task_status" => self.tool_get_task_status(),
            "assign_task" => self.tool_assign_task(params),
            other => Err(anyhow::anyhow!("Unknown tool: {}", other)),
        };
        match result {
            Ok(text) => rmcp::model::CallToolResult::success(vec![Content::text(text)]),
            Err(e) => rmcp::model::CallToolResult::error(
                vec![Content::text(e.to_string())],
                false,
            ),
        }
    }
}

// ─── Tool implementations ─────────────────────────────────────────────────────

impl LeadToolServer {
    async fn tool_create_task(&self, params: serde_json::Value) -> Result<String> {
        let p: CreateTaskParams = serde_json::from_value(params)?;
        self.orchestrator.register_task(CreateTask {
            id: p.id.clone(),
            title: p.title,
            assignee_hint: p.assignee,
            deps: p.deps.unwrap_or_default(),
            timeout_secs: 1800,
            spec: p.spec,
            success_criteria: p.success_criteria,
        })
    }

    async fn tool_start_execution(&self) -> Result<String> {
        self.orchestrator.activate().await
    }

    fn tool_request_confirmation(&self, params: serde_json::Value) -> Result<String> {
        let p: RequestConfirmationParams = serde_json::from_value(params)?;
        // Post plan to IM
        self.orchestrator.post_message(&format!(
            "📋 **执行计划**\n\n{}\n\n请回复 **是/确认/yes/ok** 开始执行，或说明需要调整的部分。",
            p.plan_summary
        ));
        // Set state to AwaitingConfirm
        *self.orchestrator.team_state_inner.lock().unwrap() = TeamState::AwaitingConfirm;
        Ok("Confirmation requested. Waiting for user reply.".to_string())
    }

    fn tool_post_update(&self, params: serde_json::Value) -> Result<String> {
        let p: PostUpdateParams = serde_json::from_value(params)?;
        self.orchestrator.post_message(&p.message);
        Ok("Posted.".to_string())
    }

    fn tool_get_task_status(&self) -> Result<String> {
        let tasks = self.orchestrator.registry.all_tasks()?;
        let summaries: Vec<serde_json::Value> = tasks
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id,
                    "title": t.title,
                    "status": t.status_raw,
                    "assignee": t.assignee_hint,
                    "completion_note": t.completion_note,
                })
            })
            .collect();
        Ok(serde_json::to_string_pretty(&summaries)?)
    }

    fn tool_assign_task(&self, params: serde_json::Value) -> Result<String> {
        let p: AssignTaskParams = serde_json::from_value(params)?;
        self.orchestrator
            .registry
            .reassign_task(&p.task_id, &p.new_assignee)?;
        Ok(format!("Task {} re-assigned to {}.", p.task_id, p.new_assignee))
    }
}

// ─── Handle ───────────────────────────────────────────────────────────────────

pub struct LeadMcpServerHandle {
    pub port: u16,
    _ct: tokio_util::sync::CancellationToken,
    _task: tokio::task::JoinHandle<()>,
}

impl LeadMcpServerHandle {
    pub async fn stop(&self) {
        self._ct.cancel();
    }
}
```

Note: `TaskRegistry::all_tasks()` and `TaskRegistry::reassign_task()` don't exist yet — they will be added in sub-steps below.

### Step 3b: Add missing TaskRegistry methods

In `crates/qai-agent/src/team/registry.rs`, add after `mark_done()`:

```rust
    /// Return all tasks (for get_task_status snapshot).
    pub fn all_tasks(&self) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, status, deps_json, assignee_hint, retry_count,
                    timeout_secs, spec, success_criteria, completion_note,
                    created_at, done_at
             FROM tasks ORDER BY created_at ASC"
        )?;
        let tasks = stmt.query_map([], |row| Self::row_to_task(row))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(tasks)
    }

    /// Re-assign a task to a new agent. Only valid when status is 'pending'.
    pub fn reassign_task(&self, task_id: &str, new_assignee: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let rows_changed = conn.execute(
            "UPDATE tasks SET assignee_hint = ?1 WHERE id = ?2 AND status = 'pending'",
            rusqlite::params![new_assignee, task_id],
        )?;
        if rows_changed == 0 {
            anyhow::bail!("Task {} not found or not in pending state", task_id);
        }
        Ok(())
    }
```

You will need to check whether `row_to_task` is a named helper in the existing code or if rows are constructed inline. Look at `get_task()` in registry.rs for the row mapping pattern and reuse it.

### Step 4: Add unit tests for LeadMcpServer

In `lead_mcp_server.rs`, add a `#[cfg(test)]` block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::{bus::InternalBus, heartbeat::DispatchFn, registry::TaskRegistry, session::TeamSession};
    use tempfile::tempdir;

    fn make_server() -> (LeadToolServer, tempfile::TempDir) {
        let tmp = tempdir().unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("test", tmp.path().to_path_buf()));
        let bus = Arc::new(InternalBus::new());
        let dispatch_fn: DispatchFn = Arc::new(|_, _, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            registry, session, bus, dispatch_fn, std::time::Duration::from_secs(3600),
        );
        (LeadToolServer::new(orch), tmp)
    }

    #[tokio::test]
    async fn test_create_task_registers() {
        let (srv, _tmp) = make_server();
        let params = serde_json::json!({"id": "T001", "title": "Setup DB"});
        let result = srv.tool_create_task(params).await.unwrap();
        assert!(result.contains("T001"));
        assert!(result.contains("registered"));
    }

    #[test]
    fn test_get_task_status_json() {
        let (srv, _tmp) = make_server();
        // Register a task first
        let params = serde_json::json!({"id": "T001", "title": "Test"});
        tokio_test::block_on(srv.tool_create_task(params)).unwrap();
        let json_str = srv.tool_get_task_status().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.as_array().unwrap().len() == 1);
        assert_eq!(parsed[0]["id"], "T001");
    }

    #[test]
    fn test_post_update_without_notify_fn_does_not_panic() {
        let (srv, _tmp) = make_server();
        // No notify_fn wired — should be a no-op
        let result = srv.tool_post_update(serde_json::json!({"message": "Hello"}));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Posted.");
    }

    #[test]
    fn test_assign_task_only_pending() {
        let (srv, _tmp) = make_server();
        tokio_test::block_on(srv.tool_create_task(
            serde_json::json!({"id": "T001", "title": "Test"})
        )).unwrap();
        // Should succeed for pending task
        let result = srv.tool_assign_task(
            serde_json::json!({"task_id": "T001", "new_assignee": "claude"})
        );
        assert!(result.is_ok());
        // Should fail for non-existent task
        let result = srv.tool_assign_task(
            serde_json::json!({"task_id": "T999", "new_assignee": "claude"})
        );
        assert!(result.is_err());
    }
}
```

(Add `tokio-test = "0.4"` to `[dev-dependencies]` in `qai-agent/Cargo.toml` if not present.)

**Step 5: Run tests**

```bash
cargo test -p qai-agent lead_mcp_server 2>&1 | tail -20
```

Expected: all 4 new tests pass.

**Step 6: Commit**

```bash
git add crates/qai-agent/src/team/lead_mcp_server.rs crates/qai-agent/src/team/mod.rs crates/qai-agent/src/team/orchestrator.rs crates/qai-agent/src/team/registry.rs
git commit -m "feat(team): add LeadMcpServer with 6 Lead tools (create_task, start_execution, request_confirmation, post_update, get_task_status, assign_task)"
```

---

## Task 5: TeamNotify Routing + Confirmation Interceptor + Lead Layer 0 in registry.rs

**Files:**
- Modify: `crates/qai-agent/src/registry.rs`

This task has 3 sub-parts. All are in the same file.

### Sub-part A: TeamNotify dispatch after Specialist completion

**Where:** In `handle()`, in the post-run hook 1 block (around line 574 where `if early_is_specialist`).

After calling `team_orch.handle_specialist_done(...)`, build the TeamNotify content and spawn a handle() call:

```rust
// After handle_specialist_done succeeds:
if let Some(lead_key) = team_orch.lead_session_key.get().cloned() {
    let notify_content = if team_orch.registry.all_done()? {
        // All tasks done — prompt Lead to synthesize
        let tasks = team_orch.registry.all_tasks().unwrap_or_default();
        let summary: String = tasks.iter()
            .map(|t| format!("- {}（{}）：{}", t.id,
                t.assignee_hint.as_deref().unwrap_or("unknown"),
                t.completion_note.as_deref().unwrap_or("完成")))
            .collect::<Vec<_>>()
            .join("\n");
        *team_orch.team_state_inner.lock().unwrap() = crate::team::orchestrator::TeamState::Done;
        format!(
            "[团队通知] 所有任务已完成 ✅\n\n完成摘要：\n{}\n\n请生成最终汇总并通过 post_update 发送给用户。",
            summary
        )
    } else {
        let done_count = team_orch.registry.all_tasks().unwrap_or_default()
            .iter()
            .filter(|t| t.status_raw == "done")
            .count();
        let total = team_orch.registry.all_tasks().unwrap_or_default().len();
        format!(
            "[团队通知] 任务 {} 已完成（执行者：{}）\n\n完成摘要：\n{}\n\n当前进度：{} / {} 完成",
            task_id, agent, note, done_count, total
        )
    };

    let registry_clone = Arc::clone(/* need self ref */);
    let notify_id = uuid::Uuid::new_v4().to_string();
    let lead_msg = qai_protocol::InboundMsg {
        id: notify_id,
        session_key: lead_key.clone(),
        content: qai_protocol::MsgContent::text(notify_content),
        sender: "gateway".to_string(),
        channel: lead_key.channel.clone(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: team_orch.lead_session_key
            .get()
            .map(|_| {
                // front_bot name from roster — use sender_name from Lead's session
                // Simplest: registry will route via session_key anyway
                None::<String>
            })
            .flatten(),
        source: qai_protocol::MsgSource::TeamNotify,
    };
    // Spawn async so we don't recurse (handle() is not re-entrant for same session)
    // Need self reference — use OnceLock trick or pass via closure
    // *** See implementation note below ***
}
```

**Implementation note for self-reference:** `registry.handle()` is `&self`, not `Arc<Self>`. To spawn `self.handle()`, we need `Arc<Self>`. The existing code passes `registry.clone()` (which is `Arc<SessionRegistry>`) in main.rs for this exact reason. So we need to store an `Arc<Self>` reference somehow.

**Solution:** Add a `self_ref: OnceLock<Arc<SessionRegistry>>` to `SessionRegistry`. Set it in `new()` via `Arc::new_cyclic` or after creation. Then in handle(), use `self.self_ref.get().cloned()` to get Arc<Self> for spawning.

**Simpler alternative:** Use `tokio::spawn` with the closure approach only — but we need the Arc. The existing `registry.clone()` in main.rs IS `Arc<SessionRegistry>`. We can add a `weak_self: Weak<SessionRegistry>` field.

**Actual implementation — use `Arc::new_cyclic`:**

In `SessionRegistry::new()`, change to use `Arc::new_cyclic`:

```rust
    // In new(), replace Arc::new(Self { ... }) with:
    let registry = Arc::new_cyclic(|weak| {
        Self {
            // all existing fields...
            weak_self: weak.clone(),
        }
    });
```

Add field to struct:
```rust
    weak_self: std::sync::Weak<Self>,
```

Then in handle() TeamNotify dispatch:
```rust
    if let Some(arc_self) = self.weak_self.upgrade() {
        tokio::spawn(async move {
            if let Err(e) = arc_self.handle(lead_msg).await {
                tracing::warn!("TeamNotify dispatch error: {e}");
            }
        });
    }
```

### Sub-part B: Confirmation interceptor

**Where:** At the very beginning of `handle()`, after the dedup check, before slash command parsing.

Add:

```rust
    // ── Team Mode confirmation interceptor ──────────────────────────────────────
    // When Lead called request_confirmation(), the next Human message is the user's yes/no.
    if inbound.source == MsgSource::Human {
        if let Some(team_orch) = self.team_orchestrator.get() {
            let state = team_orch.team_state();
            if state == crate::team::orchestrator::TeamState::AwaitingConfirm {
                if let Some(lead_key) = team_orch.lead_session_key.get() {
                    if &inbound.session_key == lead_key {
                        let text_lower = user_text.to_lowercase();
                        let confirmed = ["yes", "是", "确认", "ok", "好的", "开始"]
                            .iter()
                            .any(|kw| text_lower.contains(kw));
                        if confirmed {
                            // Activate execution
                            if let Some(arc_self) = self.weak_self.upgrade() {
                                let orch = std::sync::Arc::clone(team_orch);
                                tokio::spawn(async move {
                                    match orch.activate().await {
                                        Ok(msg) => tracing::info!("Team activated: {}", msg),
                                        Err(e) => tracing::error!("Team activate error: {e}"),
                                    }
                                });
                            }
                            return Ok(Some("收到，开始执行。任务队列已启动。".to_string()));
                        } else {
                            // User said no or gave feedback — reset to Planning, let Lead handle it
                            *team_orch.team_state_inner.lock().unwrap() =
                                crate::team::orchestrator::TeamState::Planning;
                            // Fall through to normal routing (Lead handles the message)
                        }
                    }
                }
            }
        }
    }
```

### Sub-part C: Lead Layer 0 injection

**Where:** In handle(), in the `AgentCtx` construction section, where `early_is_specialist` and `early_agent_role` are computed.

Currently:
```rust
    let early_is_specialist = inbound.source == MsgSource::Heartbeat
        || session_key.channel.as_str() == "team";
    let early_agent_role = if early_is_specialist { AgentRole::Specialist } else { AgentRole::Solo };
```

Replace with:

```rust
    let early_is_specialist = inbound.source == MsgSource::Heartbeat
        || session_key.channel.as_str() == "team";

    let early_is_lead = !early_is_specialist && {
        self.team_orchestrator
            .get()
            .and_then(|o| o.lead_session_key.get())
            .map(|k| k == &session_key)
            .unwrap_or(false)
    };

    let early_agent_role = if early_is_specialist {
        AgentRole::Specialist
    } else if early_is_lead {
        AgentRole::Lead
    } else {
        AgentRole::Solo
    };

    // For Lead turns: set lead_session_key lazily on first contact
    if early_is_lead {
        // Already set — no-op (OnceLock)
    } else if inbound.source == MsgSource::Human || inbound.source == MsgSource::TeamNotify {
        // Detect Lead by front_bot target: if target_agent matches group front_bot and that group is Team mode
        // (Simplified: if we see a TeamNotify source, that session IS the Lead)
        if inbound.source == MsgSource::TeamNotify {
            if let Some(team_orch) = self.team_orchestrator.get() {
                team_orch.set_lead_session_key(session_key.clone());
                // Also set scope for post_update/notify_fn
                team_orch.set_scope(session_key.clone());
            }
        }
    }
```

Build Lead Layer 0 (task_reminder for Lead):

```rust
    // Build Lead Layer 0 when Lead is in Team mode
    let lead_layer_0: Option<String> = if early_is_lead {
        let state = self.team_orchestrator
            .get()
            .map(|o| o.team_state())
            .unwrap_or(crate::team::orchestrator::TeamState::Planning);
        Some(match state {
            crate::team::orchestrator::TeamState::Planning |
            crate::team::orchestrator::TeamState::AwaitingConfirm => {
                "你是团队协调者。用户的请求需要多个 Agent 协作完成。\n\n\
                 步骤：\n\
                 1. 分析任务，调用 create_task() 定义所有子任务和依赖关系\n\
                 2. 简单任务（≤3个、无复杂依赖）直接调用 start_execution()\n\
                 3. 复杂任务先调用 request_confirmation(plan_summary)，等用户确认后再执行\n\
                 4. 任务执行中你会收到 [团队通知] 消息，用 post_update() 向用户播报关键进度\n\
                 5. 收到\"所有任务已完成\"通知后，合成最终结果并调用 post_update() 发给用户\n\n\
                 可用工具：create_task, start_execution, request_confirmation, post_update, get_task_status, assign_task"
                    .to_string()
            }
            crate::team::orchestrator::TeamState::Running |
            crate::team::orchestrator::TeamState::Done => {
                "团队任务执行中。你会收到 [团队通知] 消息。\n\n\
                 - 用 post_update(message) 向用户播报进度\n\
                 - 用 get_task_status() 查看全局状态\n\
                 - 用 assign_task(task_id, agent) 重新分配卡住的任务\n\
                 - 收到\"所有任务已完成\"通知后，合成最终汇总并 post_update"
                    .to_string()
            }
        })
    } else {
        None
    };

    // Override task_reminder for Lead turns
    let effective_task_reminder = if early_is_lead {
        lead_layer_0.as_deref()
    } else {
        early_task_reminder.as_deref()
    };
```

Then in `SystemPromptBuilder { ... }`, use `effective_task_reminder` instead of `early_task_reminder.as_deref()`.

And for Lead MCP URL injection:

```rust
    let mcp_server_url: Option<String> = if early_is_specialist {
        self.team_orchestrator
            .get()
            .and_then(|o| o.mcp_server_port.get().copied())
            .map(|port| format!("http://127.0.0.1:{port}/sse"))
    } else if early_is_lead {
        self.team_orchestrator
            .get()
            .and_then(|o| o.lead_mcp_server_port.get().copied())
            .map(|port| format!("http://127.0.0.1:{port}/sse"))
    } else {
        None
    };
```

**Step — Write failing test for confirmation interceptor**

Add to registry.rs tests (or integration tests):

```rust
#[tokio::test]
async fn test_confirmation_interceptor_yes_activates_team() {
    // This is a unit-level integration test:
    // 1. Create a registry + orchestrator
    // 2. Register tasks, call request_confirmation
    // 3. Send a "是" message — should return "收到，开始执行"
    // 4. Check TeamState transitions to Running
    // Note: activating requires spawning MCP server, so use test heartbeat interval
    // ... (complex test — see verification section below for simpler smoke test)
}
```

For now, a simpler compile-level test is sufficient. The scenario will be verified manually via the E2E step.

**Step: Run cargo check**

```bash
cargo check -p qai-agent 2>&1 | tail -30
```

Fix any compile errors. Common issues:
- `Arc::new_cyclic` for weak_self initialization
- Import `use std::sync::Weak`
- Import TeamState
- `crate::team::orchestrator::TeamState::AwaitingConfirm` etc.

**Step: Run full test suite**

```bash
cargo test -p qai-agent 2>&1 | tail -20
```

Expected: all existing tests plus new ones pass.

**Step 6: Commit**

```bash
git add crates/qai-agent/src/registry.rs
git commit -m "feat(registry): TeamNotify routing, confirmation interceptor, Lead Layer 0 injection"
```

---

## Task 6: Wire LeadMcpServer in main.rs

**Files:**
- Modify: `crates/qai-server/src/main.rs`

### Step 1: Add import

In main.rs, in the `use qai_agent::team::{ ... }` block, add:

```rust
use qai_agent::team::lead_mcp_server::LeadToolServer;
```

### Step 2: Spawn LeadMcpServer after TeamOrchestrator is created

In the `// Wire TeamOrchestrator` block in main.rs, after `registry.set_team_orchestrator(team_orch.clone())`, add:

```rust
        // Spawn LeadMcpServer — provides 6 tools to Lead Agent
        {
            let lead_srv = LeadToolServer::new(Arc::clone(&team_orch));
            match lead_srv.spawn().await {
                Ok(handle) => {
                    let _ = team_orch.lead_mcp_server_port.set(handle.port);
                    tracing::info!(port = handle.port, "LeadMcpServer started");
                    // Keep handle alive for process lifetime (leak is intentional)
                    std::mem::forget(handle);
                }
                Err(e) => {
                    tracing::error!("Failed to start LeadMcpServer: {e}");
                }
            }
        }
```

Note: leaking the handle is intentional — the server should run until process exit. If graceful shutdown is needed later, store it in `AppState`.

### Step 3: Wire lead_session_key from group config

For groups configured with `interaction = "team"`, set the lead session key at startup using the group scope:

```rust
        // Wire lead_session_key from Team groups in config
        for group in &cfg.groups {
            if matches!(group.mode.interaction, qai_server::config::InteractionMode::Team) {
                // Derive session key from group scope + first active channel
                // Group scope is like "group:oc_xxx" — channel is determined at runtime
                // We use a best-effort: check which channels are enabled
                let channel_name = if cfg.channels.dingtalk.as_ref().map(|c| c.enabled).unwrap_or(false) {
                    "dingtalk"
                } else if cfg.channels.lark.as_ref().map(|c| c.enabled).unwrap_or(false) {
                    "lark"
                } else {
                    "ws"
                };
                let lead_key = qai_protocol::SessionKey::new(channel_name, &group.scope);
                team_orch.set_lead_session_key(lead_key.clone());
                team_orch.set_scope(lead_key);
                tracing::info!(scope = %group.scope, "Team group lead_session_key wired");
                break; // MVP: one Team group per Gateway instance
            }
        }
```

### Step 4: Compile check

```bash
cargo check -p qai-server 2>&1 | tail -20
```

Fix any import or type errors.

### Step 5: Run full workspace tests

```bash
cargo test --workspace 2>&1 | tail -20
```

Expected: ≥ 307 tests pass, 0 failures.

**Step 6: Commit**

```bash
git add crates/qai-server/src/main.rs
git commit -m "feat(server): wire LeadMcpServer and lead_session_key in main.rs"
```

---

## Task 7: Verification

### Step 1: Run full test suite

```bash
cargo test --workspace 2>&1 | grep -E "^test |FAILED|error" | tail -40
```

Expected: all tests green. Note the test count (should be ≥ 320 after new tests).

### Step 2: Compile check all crates

```bash
cargo build --workspace 2>&1 | tail -10
```

Expected: `Finished` with no errors.

### Step 3: Manual scenario check

If you have a DingTalk or Lark group configured, test the flow:

1. **Scenario: Simple team task**
   - User: "帮我写一个 user 模型和 JWT 认证系统"
   - Lead calls `create_task(T001, "写 User 模型", codex, deps=[])` via MCP
   - Lead calls `create_task(T002, "实现 JWT", codex, deps=[T001])` via MCP
   - Lead calls `start_execution()` → returns "Team execution started."
   - Heartbeat dispatches T001 to codex
   - codex completes → `[DONE: T001]` → TeamNotify → Lead posts update
   - T002 unlocks → codex completes → `[DONE: T002]` → all_done TeamNotify
   - Lead calls `post_update("任务全部完成！...")` → user sees final message

2. **Scenario: Confirmation flow**
   - User: "帮我重构整个后端"
   - Lead calls `create_task(...)` × N
   - Lead calls `request_confirmation("计划：5个任务...")` → IM shows plan
   - User replies: "是"
   - Registry interceptor activates → Lead continues as Running

### Step 4: Check known bug fix

Verify that the `[BLOCKED:]` task_id extraction bug (pre-existing) is noted:

```rust
// In registry.rs line ~587, the old code:
let task_id_hint = session_key.scope.split(':').next().unwrap_or("unknown");
// This extracts "team_id" from "team_id:agent_name" — WRONG: should be task_id from marker
// The correct fix is: extract task_id from the [BLOCKED: ...] context or from task_reminder
// NOTE: This bug is not in scope for this plan — leave as-is with existing behavior.
```

Document in CLAUDE.md or add a `// TODO(P1-BUG):` comment.

### Step 5: Final commit

```bash
cargo test --workspace 2>&1 | tail -5
git add -p  # review any remaining staged changes
git commit -m "feat(team): complete Team Mode Activation — LeadMcpServer, TeamNotify, confirmation flow, Lead Layer 0"
```

---

## Non-goals (Excluded from this plan)

- `ask_lead()` tool for mid-task Specialist questions — post-MVP
- Lead quality review before marking task done — post-MVP
- TaskRegistry persistence across restarts — post-MVP
- Dynamic agent scaling — post-MVP
- Per-group TeamOrchestrator instances (currently one global) — post-MVP
- Fix `[BLOCKED:]` task_id extraction bug — separate hotfix
