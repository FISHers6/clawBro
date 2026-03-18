# Team MCP Tool Server Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace unreliable prompt-marker task completion (`[DONE: Txxx]`) with structured MCP tool calls (`complete_task` / `block_task`), keeping prompt-markers as a silent fallback.

**Architecture:** A per-team singleton HTTP MCP Server (rmcp) starts with `TeamOrchestrator::start()` and stops with `::stop()`. Its port is stored in `TeamOrchestrator`. On each Specialist turn, `registry.rs` reads the port and injects it into `AgentCtx`. `AcpEngine` checks if the ACP agent declared `mcpCapabilities.http = true` at initialize-time; if so, it appends an `McpServerHttp` entry to `NewSessionRequest.mcp_servers`. The existing prompt-marker hooks remain as a no-op fallback (since `mark_done` only acts on `claimed` status, double-calls safely do nothing).

**Tech Stack:** Rust, `rmcp = "0.8.3"` (HTTP MCP server), `agent-client-protocol = "0.9"` (ACP client), `tokio`, existing `TaskRegistry` + `TeamOrchestrator`.

---

## Background: Why This Matters

Currently the Specialist LLM must embed `[DONE: T003]` in its free-text output. Problems:
- LLM may forget, misformat, or hallucinate the task ID → task stuck or wrong task completed
- Text parsing is fragile by nature

With MCP tool calls, the LLM calls `complete_task({task_id: "T003", note: "..."})` as a structured function call. The ACP agent's built-in tool-calling loop handles retry and validation. The gateway's `TeamToolServer` receives the call and calls `TaskRegistry::mark_done()` directly.

**Compatibility:** `mark_done()` already requires `status LIKE 'claimed%'`, so if the tool call succeeds first, a subsequent prompt-marker attempt simply warns + skips. No changes to existing fallback hooks needed.

---

## Key Files

| File | Role |
|------|------|
| `crates/clawbro-agent/src/team/mcp_server.rs` | **New**: HTTP MCP server (ServerHandler) |
| `crates/clawbro-agent/src/team/mod.rs` | Add `pub mod mcp_server` |
| `crates/clawbro-agent/src/team/orchestrator.rs` | Add `mcp_server_handle` field; start/stop lifecycle |
| `crates/clawbro-agent/src/traits.rs` | Add `mcp_server_url: Option<String>` to `AgentCtx` |
| `crates/clawbro-agent/src/registry.rs` | Inject `mcp_server_url` into `AgentCtx` for Specialist turns |
| `crates/clawbro-agent/src/acp_engine.rs` | Read `mcp_capabilities.http` from initialize response; populate `mcp_servers` |
| `crates/clawbro-agent/Cargo.toml` | Add `rmcp = "0.8.3"` |

---

## Task 1: Add rmcp dependency and verify API surface

**Files:**
- Modify: `crates/clawbro-agent/Cargo.toml`

**Step 1: Add dependency**

In `crates/clawbro-agent/Cargo.toml` under `[dependencies]`, add:
```toml
rmcp = { version = "0.8.3", features = ["server", "transport-sse-server"] }
```

**Step 2: Check available API (do not write code yet)**

Run:
```bash
cd /path/to/clawBro-gateway && cargo doc -p clawbro-agent --no-deps 2>&1 | tail -20
```

Then check the rmcp types by running:
```bash
cargo add rmcp@0.8.3 --features server,transport-sse-server -p clawbro-agent 2>&1
cargo check -p clawbro-agent 2>&1 | head -30
```

Specifically verify these types exist (they may differ by version):
- `rmcp::ServerHandler` trait
- `rmcp::tool` proc-macro
- `rmcp::service::RequestContext`
- SSE transport: check `rmcp::transport::sse_server` module path

> **Note:** If API differs, adjust `mcp_server.rs` accordingly. The core pattern is always: struct + `#[tool(tool_box)]` impl + `impl ServerHandler`.

**Step 3: Verify compilation**

```bash
cargo check -p clawbro-agent 2>&1 | grep -E "^error"
```
Expected: 0 errors (rmcp added, no code using it yet).

**Step 4: Commit**

```bash
git add crates/clawbro-agent/Cargo.toml Cargo.lock
git commit -m "chore(clawbro-agent): add rmcp dependency for team MCP server"
```

---

## Task 2: Implement TeamToolServer (mcp_server.rs)

**Files:**
- Create: `crates/clawbro-agent/src/team/mcp_server.rs`
- Modify: `crates/clawbro-agent/src/team/mod.rs`

**Step 1: Create `team/mcp_server.rs`**

The server exposes two tools and spawns an HTTP listener on a random OS port.

```rust
//! TeamToolServer — per-team MCP Server (HTTP/SSE)
//!
//! Exposes two tools to Specialist agents:
//!   complete_task(task_id, note) — calls TaskRegistry::mark_done()
//!   block_task(task_id, reason) — calls TeamOrchestrator::handle_specialist_blocked()
//!
//! 生命周期: TeamOrchestrator::start() 启动，::stop() 关闭
//! 通信方式: HTTP (mcpCapabilities.http = true 的 ACP agents 使用此路径)

use std::net::SocketAddr;
use std::sync::Arc;
use anyhow::Result;
use tokio::task::JoinHandle;

use super::orchestrator::TeamOrchestrator;
use super::registry::TaskRegistry;

// ─── ServerHandler impl ───────────────────────────────────────────────────────
//
// rmcp proc-macro approach: struct fields are available via self inside tool methods.
// Clone required by rmcp ServerHandler.
//

#[derive(Clone)]
pub struct TeamToolServer {
    pub registry: Arc<TaskRegistry>,
    pub orchestrator: Arc<TeamOrchestrator>,
    /// The agent_name is not yet known at server creation time; it is filled
    /// per-call via `extract_agent_name()` which reads the request context.
    /// For simplicity we fall back to "unknown" — the important thing is the
    /// task_id, not which agent completed it (registry validates claimed status).
    pub team_id: String,
}

/// Running HTTP MCP server handle (returned by `spawn()`).
pub struct TeamMcpServerHandle {
    pub port: u16,
    pub addr: SocketAddr,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl TeamMcpServerHandle {
    /// Stop the HTTP MCP server gracefully.
    pub fn stop(self) {
        let _ = self.shutdown_tx.send(());
        self.task.abort();
    }
}

impl TeamToolServer {
    pub fn new(
        registry: Arc<TaskRegistry>,
        orchestrator: Arc<TeamOrchestrator>,
        team_id: String,
    ) -> Self {
        Self { registry, orchestrator, team_id }
    }

    /// Spawn an HTTP MCP server on a random OS-assigned port.
    /// Returns the handle (contains port + shutdown channel).
    pub async fn spawn(self) -> Result<TeamMcpServerHandle> {
        use rmcp::transport::sse_server::SseServer;

        // Bind to any available port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let port = addr.port();

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let server_self = self;

        let task = tokio::spawn(async move {
            let server = rmcp::McpServer::new(server_self);
            // SseServer::serve takes a TcpListener and runs until shutdown
            // NOTE: exact API depends on rmcp version; adjust if needed.
            // Pattern for rmcp 0.8.x:
            if let Err(e) = SseServer::serve_with_shutdown(listener, server, async {
                let _ = shutdown_rx.await;
            }).await {
                tracing::error!("TeamMcpServer exited with error: {e:#}");
            }
        });

        tracing::info!(port = port, "TeamMcpServer started on 127.0.0.1:{port}");
        Ok(TeamMcpServerHandle { port, addr, shutdown_tx, task })
    }
}

// ─── Tool implementations ─────────────────────────────────────────────────────

#[rmcp::tool(tool_box)]
impl TeamToolServer {
    /// Mark the current task as completed.
    /// Call this when you have fully finished the work described in your task spec.
    #[rmcp::tool(description = "Mark the assigned task as done. Call exactly once when all work is complete.")]
    async fn complete_task(
        &self,
        #[rmcp::tool(param, description = "The task ID shown in your task reminder, e.g. T003")]
        task_id: String,
        #[rmcp::tool(param, description = "Brief completion note (what you did, output artifact)")]
        note: String,
    ) -> String {
        match self.registry.mark_done(&task_id, &note) {
            Ok(()) => {
                // Also trigger orchestrator milestone check
                if let Err(e) = self.orchestrator.handle_specialist_done(&task_id, "mcp-tool", &note) {
                    tracing::warn!(task_id = %task_id, "handle_specialist_done error: {e:#}");
                }
                format!("Task {task_id} marked done.")
            }
            Err(e) => format!("Error marking task done: {e}"),
        }
    }

    /// Report that the current task is blocked and cannot proceed.
    /// Lead will be notified for escalation.
    #[rmcp::tool(description = "Report task as blocked when you cannot proceed without external input.")]
    async fn block_task(
        &self,
        #[rmcp::tool(param, description = "The task ID shown in your task reminder, e.g. T003")]
        task_id: String,
        #[rmcp::tool(param, description = "Clear reason why you are blocked; what input you need")]
        reason: String,
    ) -> String {
        if let Err(e) = self.orchestrator.handle_specialist_blocked(&task_id, "mcp-tool", &reason) {
            tracing::warn!(task_id = %task_id, "handle_specialist_blocked error: {e:#}");
        }
        format!("Task {task_id} reported as blocked: {reason}")
    }
}

#[rmcp::tool(tool_box)]
impl rmcp::ServerHandler for TeamToolServer {}
```

> **⚠️ rmcp API note:** The exact proc-macro path (`rmcp::tool` vs `#[tool]` with `use rmcp::tool`) and transport API (`SseServer::serve_with_shutdown` signature) depends on the installed version. Run `cargo doc -p clawbro-agent --open` after adding the dependency to verify. Adjust the proc-macro attribute paths and `SseServer` call accordingly.

**Step 2: Add module to `team/mod.rs`**

```rust
pub mod bus;
pub mod heartbeat;
pub mod mcp_server;    // ← add this line
pub mod orchestrator;
pub mod registry;
pub mod session;
```

**Step 3: Compile-check (no tests yet)**

```bash
cargo check -p clawbro-agent 2>&1 | grep -E "^error"
```

Fix any proc-macro path errors by consulting `cargo doc` output for rmcp.

**Step 4: Write unit test for tool logic (no HTTP, direct struct call)**

In `mcp_server.rs` `#[cfg(test)]` section:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use crate::team::{
        bus::InternalBus,
        heartbeat::DispatchFn,
        orchestrator::TeamOrchestrator,
        registry::{CreateTask, TaskRegistry, TaskStatus},
        session::TeamSession,
    };

    fn make_server() -> (TeamToolServer, tempfile::TempDir) {
        let tmp = tempdir().unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("test", tmp.path().to_path_buf()));
        let bus = Arc::new(InternalBus::new());
        let dispatch_fn: DispatchFn = Arc::new(|_, _, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            Arc::clone(&registry),
            session,
            bus,
            dispatch_fn,
            std::time::Duration::from_secs(3600),
        );
        let server = TeamToolServer::new(registry, orch, "test-team".to_string());
        (server, tmp)
    }

    #[tokio::test]
    async fn test_complete_task_marks_done() {
        let (server, _tmp) = make_server();
        server.registry.create_task(CreateTask {
            id: "T001".into(),
            title: "test task".into(),
            ..Default::default()
        }).unwrap();
        server.registry.try_claim("T001", "codex").unwrap();

        let result = server.complete_task("T001".to_string(), "done".to_string()).await;
        assert!(result.contains("T001"));

        let task = server.registry.get_task("T001").unwrap().unwrap();
        assert!(matches!(task.status_parsed(), TaskStatus::Done));
    }

    #[tokio::test]
    async fn test_complete_task_unclaimed_is_noop() {
        let (server, _tmp) = make_server();
        server.registry.create_task(CreateTask {
            id: "T002".into(),
            title: "unclaimed".into(),
            ..Default::default()
        }).unwrap();
        // NOT claimed — mark_done should silently skip
        let result = server.complete_task("T002".to_string(), "oops".to_string()).await;
        // No panic; task stays pending
        let task = server.registry.get_task("T002").unwrap().unwrap();
        assert!(matches!(task.status_parsed(), TaskStatus::Pending));
        let _ = result; // result text doesn't matter
    }
}
```

**Step 5: Run tests**

```bash
cargo test -p clawbro-agent team::mcp_server 2>&1
```
Expected: 2 tests pass.

**Step 6: Commit**

```bash
git add crates/clawbro-agent/src/team/mcp_server.rs crates/clawbro-agent/src/team/mod.rs
git commit -m "feat(team): add TeamToolServer — MCP server for complete_task/block_task tool calls"
```

---

## Task 3: Wire TeamMcpServer lifecycle into TeamOrchestrator

**Files:**
- Modify: `crates/clawbro-agent/src/team/orchestrator.rs`

**Step 1: Add `mcp_server_handle` field**

In `TeamOrchestrator` struct, add:
```rust
/// Running HTTP MCP server handle (Some after start(), taken on stop()).
mcp_server_handle: tokio::sync::Mutex<Option<super::mcp_server::TeamMcpServerHandle>>,
/// Bound port of the running MCP server (set once after start()).
pub mcp_server_port: std::sync::OnceLock<u16>,
```

In `new()`, initialize:
```rust
mcp_server_handle: tokio::sync::Mutex::new(None),
mcp_server_port: std::sync::OnceLock::new(),
```

Note: `heartbeat_handle` uses `std::sync::Mutex`; `mcp_server_handle` uses `tokio::sync::Mutex` because `stop()` on the handle is called from async context in `::stop()`.

**Step 2: Spawn MCP server in `start()`**

At the END of `TeamOrchestrator::start()`, after spawning the heartbeat, add (note: `start()` is currently sync — change it to `async fn start()`):

```rust
// Spawn per-team MCP server
let mcp_srv = super::mcp_server::TeamToolServer::new(
    Arc::clone(&self.registry),
    // Need Arc<Self>: orchestrator passes itself. But start() takes &self not Arc<Self>.
    // Solution: accept Arc<TeamOrchestrator> as arg or store self-Arc.
    // See Step 3 for the resolution.
    todo!(),
    self.session.team_id.clone(),
);
let handle = mcp_srv.spawn().await?;
let _ = self.mcp_server_port.set(handle.port);
*self.mcp_server_handle.lock().await = Some(handle);
```

> **Design note:** `TeamToolServer` needs an `Arc<TeamOrchestrator>` to call `handle_specialist_done/blocked`. But `start()` is called on `Arc<TeamOrchestrator>` (since `new()` returns `Arc<Self>`). Solution: change `start()` signature from `pub fn start(&self, ...)` to `pub fn start(self: &Arc<Self>, ...)`.

**Step 3: Change `start()` signature to `async fn start(self: &Arc<Self>, plan: &TeamPlan)`**

Full replacement of `start()` signature and MCP spawn block:

```rust
pub async fn start(self: &Arc<Self>, plan: &TeamPlan) -> Result<()> {
    // ... existing steps 1-4 unchanged ...

    // 5. 启动 TeamMcpServer（per-team singleton）
    let mcp_srv = super::mcp_server::TeamToolServer::new(
        Arc::clone(&self.registry),
        Arc::clone(self),
        self.session.team_id.clone(),
    );
    let handle = mcp_srv.spawn().await?;
    let _ = self.mcp_server_port.set(handle.port);
    *self.mcp_server_handle.lock().await = Some(handle);

    tracing::info!(
        team_id = %self.session.team_id,
        tasks = plan.tasks.len(),
        mcp_port = ?self.mcp_server_port.get(),
        "Team started"
    );
    Ok(())
}
```

**Step 4: Stop MCP server in `stop()`**

Change `stop()` to `async fn stop()` and add:

```rust
pub async fn stop(&self) -> Result<()> {
    // Stop heartbeat (unchanged)
    if let Some(handle) = self.heartbeat_handle.lock().unwrap().take() {
        handle.abort();
    }
    // Stop MCP server
    if let Some(handle) = self.mcp_server_handle.lock().await.take() {
        handle.stop();
        tracing::info!(team_id = %self.session.team_id, "TeamMcpServer stopped");
    }
    // Cleanup InternalBus + archive (unchanged)
    self.bus.cleanup_team(&self.session.team_id);
    self.session.archive()?;
    tracing::info!(team_id = %self.session.team_id, "Team stopped and archived");
    Ok(())
}
```

**Step 5: Fix callers of `start()` and `stop()`**

Search for all callers and add `.await`:
```bash
grep -rn "\.start(" crates/ --include="*.rs" | grep -v "test"
grep -rn "\.stop("  crates/ --include="*.rs" | grep -v "test"
```

The only production callers should be in `main.rs` or slash command handlers. Update them to `.await?`.

For tests inside `orchestrator.rs`, the existing sync tests that call `orch.start(&plan).unwrap()` need to become `#[tokio::test] async fn` and use `.await.unwrap()`.

**Step 6: Run tests**

```bash
cargo test -p clawbro-agent team::orchestrator 2>&1
```
Expected: all orchestrator tests pass.

**Step 7: Commit**

```bash
git add crates/clawbro-agent/src/team/orchestrator.rs
git commit -m "feat(orchestrator): spawn per-team HTTP MCP server on start(), stop on stop()"
```

---

## Task 4: Add `mcp_server_url` to `AgentCtx` + inject in `registry.rs`

**Files:**
- Modify: `crates/clawbro-agent/src/traits.rs`
- Modify: `crates/clawbro-agent/src/registry.rs`

**Step 1: Add field to AgentCtx**

In `traits.rs`, add to `AgentCtx`:
```rust
/// URL of the running TeamMcpServer (e.g. "http://127.0.0.1:54321").
/// Set only for Specialist turns when TeamOrchestrator is wired and running.
pub mcp_server_url: Option<String>,
```

**Step 2: Inject in `registry.rs` handle()**

In the block where `effective_workspace` and `team_dir` are computed (after the `early_is_specialist` check), add:

```rust
let mcp_server_url: Option<String> = if early_is_specialist {
    self.team_orchestrator
        .get()
        .and_then(|o| o.mcp_server_port.get().copied())
        .map(|port| format!("http://127.0.0.1:{port}"))
} else {
    None
};
```

Then add to `AgentCtx` construction:
```rust
let ctx = AgentCtx {
    // ... existing fields ...
    mcp_server_url,
};
```

**Step 3: Compile-check**

```bash
cargo check -p clawbro-agent 2>&1 | grep "^error"
```

Any `AgentCtx` construction that doesn't include `mcp_server_url` will now fail to compile. Fix each by adding `mcp_server_url: None` where not applicable (non-Specialist paths and tests).

**Step 4: Run tests**

```bash
cargo test --workspace 2>&1 | grep -E "FAILED|^test result"
```
Expected: 0 failures.

**Step 5: Commit**

```bash
git add crates/clawbro-agent/src/traits.rs crates/clawbro-agent/src/registry.rs
git commit -m "feat(ctx): add mcp_server_url to AgentCtx; inject for Specialist turns"
```

---

## Task 5: Extend AcpEngine to pass mcp_servers in NewSessionRequest

**Files:**
- Modify: `crates/clawbro-agent/src/acp_engine.rs`

**Step 1: Capture `initialize()` response to read `mcp_capabilities`**

Change:
```rust
conn.initialize(
    acp::InitializeRequest::new(acp::ProtocolVersion::V1).client_info(
        acp::Implementation::new("clawbro-gateway", env!("CARGO_PKG_VERSION")),
    ),
)
.await
.map_err(|e| anyhow::anyhow!("ACP initialize failed: {e:?}"))?;
```

To:
```rust
let init_resp = conn.initialize(
    acp::InitializeRequest::new(acp::ProtocolVersion::V1).client_info(
        acp::Implementation::new("clawbro-gateway", env!("CARGO_PKG_VERSION")),
    ),
)
.await
.map_err(|e| anyhow::anyhow!("ACP initialize failed: {e:?}"))?;

let supports_http_mcp = init_resp
    .capabilities
    .mcp
    .as_ref()
    .map(|c| c.http)
    .unwrap_or(false);
```

> **Note:** Verify the exact field path: `init_resp.capabilities.mcp.http` or similar. Run `cargo doc -p agent-client-protocol --open` and search for `InitializeResponse`. The JSON schema field is `"mcp": { "http": bool, "sse": bool }`.

**Step 2: Build `mcp_servers` list for `NewSessionRequest`**

Replace:
```rust
let sess = conn
    .new_session(acp::NewSessionRequest::new(session_root))
    .await
    .map_err(|e| anyhow::anyhow!("ACP new_session failed: {e:?}"))?;
```

With:
```rust
let mut mcp_servers: Vec<acp::McpServer> = Vec::new();
if supports_http_mcp {
    if let Some(ref url) = ctx.mcp_server_url {
        // Exact variant name depends on agent-client-protocol version.
        // Check: acp::McpServer variants via `cargo doc`.
        // Pattern A (flat enum):
        //   acp::McpServer::Http { name: ..., url: ..., headers: vec![], meta: None }
        // Pattern B (tuple enum):
        //   acp::McpServer::Http(acp::McpServerHttp { name: ..., url: ..., ... })
        // Use whichever compiles. Example (adjust to match):
        mcp_servers.push(acp::McpServer::Http {
            name: "team-tools".to_string(),
            url: url.clone(),
            headers: vec![],
            meta: None,
        });
        tracing::debug!(url = %url, "Injecting team-tools MCP server into ACP session");
    }
}

let sess = conn
    .new_session(acp::NewSessionRequest {
        mcp_servers,
        cwd: session_root,
        meta: None,
    })
    .await
    .map_err(|e| anyhow::anyhow!("ACP new_session failed: {e:?}"))?;
```

**Step 3: Compile-check**

```bash
cargo check -p clawbro-agent 2>&1 | grep "^error"
```

Adjust `McpServer` variant syntax based on compiler error. The compiler will tell you the exact enum shape.

**Step 4: Write integration smoke test**

In `acp_engine.rs` tests (or a new file), verify that when `mcp_server_url` is set and `supports_http_mcp` is true, the mcp_servers vec is populated. This is tricky to test without a real ACP agent; instead write a unit test for the mcp_servers construction logic:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_mcp_servers_empty_when_no_url() {
        // When mcp_server_url is None, mcp_servers must be empty
        let url: Option<String> = None;
        let supports = true;
        let servers = build_mcp_servers(supports, url.as_deref());
        assert!(servers.is_empty());
    }

    #[test]
    fn test_mcp_servers_empty_when_no_http_capability() {
        let url = Some("http://127.0.0.1:9999".to_string());
        let supports = false;
        let servers = build_mcp_servers(supports, url.as_deref());
        assert!(servers.is_empty());
    }

    #[test]
    fn test_mcp_servers_populated_when_url_and_capability() {
        let url = Some("http://127.0.0.1:9999".to_string());
        let supports = true;
        let servers = build_mcp_servers(supports, url.as_deref());
        assert_eq!(servers.len(), 1);
    }
}
```

Extract the mcp_servers construction into a pure function `build_mcp_servers(supports_http: bool, url: Option<&str>) -> Vec<acp::McpServer>` inside `acp_engine.rs` to enable unit testing.

**Step 5: Run tests**

```bash
cargo test -p clawbro-agent acp_engine 2>&1
cargo test --workspace 2>&1 | grep -E "FAILED|^test result"
```
Expected: new tests pass, no regressions.

**Step 6: Commit**

```bash
git add crates/clawbro-agent/src/acp_engine.rs
git commit -m "feat(acp_engine): inject team-tools MCP server into NewSessionRequest for Specialist turns"
```

---

## Task 6: End-to-end verification and cleanup

**Step 1: Run full test suite**

```bash
cargo test --workspace 2>&1 | grep -E "FAILED|^test result"
```
Expected: 0 failures, count ≥ previous baseline.

**Step 2: Verify fallback still works**

The prompt-marker fallback requires no code changes — `mark_done` with `LIKE 'claimed%'` guard already handles double-call. Verify by reading the relevant test:

```bash
cargo test -p clawbro-agent test_create_and_claim_task 2>&1
```

**Step 3: Document system prompt update**

In the Specialist's `task_reminder` (built by `TeamSession::build_task_reminder()`), add a line explaining the available MCP tools. Edit `crates/clawbro-agent/src/team/session.rs` in `build_task_reminder()`:

```rust
// After the existing format! string, append:
"完成任务时调用工具 `complete_task(task_id, note)` 或输出 `[DONE: {task_id}]`。\n\
 遇到阻塞时调用工具 `block_task(task_id, reason)` 或输出 `[BLOCKED: reason]`。"
```

This tells the LLM about both paths.

**Step 4: Final commit**

```bash
git add crates/clawbro-agent/src/team/session.rs
git commit -m "docs(team): update task_reminder to mention MCP tools alongside marker fallback"
```

---

## Verification Checklist

After all tasks complete:

- [ ] `cargo test --workspace` → 0 failures
- [ ] `TeamMcpServer::spawn()` binds to a random port (check log output)
- [ ] `complete_task` tool directly calls `TaskRegistry::mark_done()` (validated by unit test)
- [ ] `block_task` tool calls `handle_specialist_blocked()` (validated by unit test)
- [ ] `mcp_server_port` is `None` before `start()`, `Some(port)` after
- [ ] `AcpEngine` skips MCP server injection when `supports_http_mcp = false`
- [ ] Prompt-marker fallback still works for agents that don't support HTTP MCP
- [ ] `TeamOrchestrator::stop()` shuts down the HTTP listener (no leaked port)

---

## Risk Register

| Risk | Mitigation |
|------|------------|
| rmcp 0.8.x API differs from 0.6.x | Consult `cargo doc` after adding dep; adjust proc-macro paths |
| ACP agents may not advertise `http` capability | Graceful skip: no MCP server injected, prompt-markers still work |
| `NewSessionRequest` McpServer variant names unknown | Compiler error will show exact enum shape; adjust |
| `start()` → `async` breaks callers | Search with grep, fix each to `.await` |
| Port collision (unlikely, OS-assigned) | `TcpListener::bind("127.0.0.1:0")` — OS picks free port, no collision |
