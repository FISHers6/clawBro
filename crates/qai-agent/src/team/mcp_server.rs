//! TeamToolServer — MCP (Model Context Protocol) SSE server
//!
//! Exposes two tools to specialist agents:
//!   - `complete_task(task_id, note)`  : marks a task Done and triggers milestone checks
//!   - `block_task(task_id, reason)`   : escalates a blocked task to the Lead via InternalBus
//!
//! Lifecycle:
//!   ```text
//!   TeamToolServer::new(registry, orchestrator, team_id)
//!     .spawn()  →  TeamMcpServerHandle { port, addr, cancellation_token }
//!   ```
//!   Cancel the token (or call `.stop()`) to shut the server down gracefully.
//!
//! Port discovery:
//!   We pre-bind a `TcpListener` on `127.0.0.1:0` so the OS assigns an
//!   ephemeral port.  We retrieve `local_addr()` before handing the listener
//!   to axum, so `TeamMcpServerHandle::port` is always the real port.

use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars,
    tool, tool_router,
    transport::{SseServer, sse_server::SseServerConfig},
};
use serde::Deserialize;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{orchestrator::TeamOrchestrator, registry::TaskRegistry};

// ─── Parameter structs ──────────────────────────────────────────────────────

/// Parameters for the `complete_task` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CompleteTaskParams {
    /// The task ID to mark as done (e.g. "T001").
    pub task_id: String,
    /// A short note summarising what was accomplished.
    pub note: String,
}

/// Parameters for the `block_task` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BlockTaskParams {
    /// The task ID that is blocked (e.g. "T002").
    pub task_id: String,
    /// A description of what is blocking progress.
    pub reason: String,
}

// ─── TeamToolServer ──────────────────────────────────────────────────────────

/// MCP server that exposes task-management tools to specialist agents.
#[derive(Clone)]
pub struct TeamToolServer {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    pub(crate) registry: Arc<TaskRegistry>,
    orchestrator: Arc<TeamOrchestrator>,
    team_id: String,
}

#[tool_router]
impl TeamToolServer {
    pub fn new(
        registry: Arc<TaskRegistry>,
        orchestrator: Arc<TeamOrchestrator>,
        team_id: String,
    ) -> Self {
        Self {
            tool_router: Self::tool_router(),
            registry,
            orchestrator,
            team_id,
        }
    }

    /// Mark a task as complete. Call this when you have finished the work
    /// described in the task specification and all success criteria are met.
    #[tool(description = "Mark a task as complete. Provide the task_id (e.g. T001) and a short completion note.")]
    async fn complete_task(&self, Parameters(p): Parameters<CompleteTaskParams>) -> String {
        match self.orchestrator.handle_specialist_done(&p.task_id, "mcp-tool", &p.note) {
            Ok(()) => format!("Task {} marked done.", p.task_id),
            Err(e) => format!("Error completing task {}: {e}", p.task_id),
        }
    }

    /// Report that a task is blocked and cannot progress without intervention.
    /// This escalates the issue to the Lead agent via the internal bus.
    #[tool(description = "Report a task as blocked. Provide the task_id and the reason it is blocked.")]
    async fn block_task(&self, Parameters(p): Parameters<BlockTaskParams>) -> String {
        let _ = self.registry.reset_claim(&p.task_id);
        if let Err(e) = self.orchestrator.handle_specialist_blocked(&p.task_id, "mcp-tool", &p.reason) {
            tracing::warn!(task_id = %p.task_id, "handle_specialist_blocked error: {e:#}");
        }
        format!("Task {} reported as blocked: {}", p.task_id, p.reason)
    }
}

impl ServerHandler for TeamToolServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(format!(
                "Team task tools for team '{}'. \
                 Use complete_task when you finish a task and block_task when you are stuck.",
                self.team_id
            )),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

// ─── TeamMcpServerHandle ─────────────────────────────────────────────────────

/// Handle returned by [`TeamToolServer::spawn`].
pub struct TeamMcpServerHandle {
    /// TCP port the SSE server is listening on (127.0.0.1).
    pub port: u16,
    /// Full socket address (always 127.0.0.1:{port}).
    pub addr: SocketAddr,
    /// Cancel this token to trigger a graceful shutdown.
    pub cancellation_token: CancellationToken,
    /// Background task that runs the axum server; joins when the CT is cancelled.
    pub task: JoinHandle<()>,
}

impl TeamMcpServerHandle {
    /// Shut the MCP server down and wait for the background task to finish.
    pub async fn stop(self) {
        self.cancellation_token.cancel();
        let _ = self.task.await;
    }
}

// ─── spawn() ─────────────────────────────────────────────────────────────────

impl TeamToolServer {
    /// Start the MCP SSE server on a random loopback port.
    ///
    /// Returns a [`TeamMcpServerHandle`] containing the actual port
    /// and a [`CancellationToken`] that can be used to stop the server.
    pub async fn spawn(self) -> Result<TeamMcpServerHandle> {
        // Bind to port 0 so the OS picks a free ephemeral port.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        let ct = CancellationToken::new();
        let sse_config = SseServerConfig {
            bind: addr,
            sse_path: "/sse".to_string(),
            post_path: "/message".to_string(),
            ct: ct.clone(),
            sse_keep_alive: None,
        };

        // SseServer::new() builds the axum Router but does NOT bind — we
        // supply the already-bound listener ourselves so we own the port.
        let (sse_server, sse_router) = SseServer::new(sse_config);

        // Register the service provider; returns the same CancellationToken.
        let server_ct = sse_server.with_service(move || self.clone());

        // Drive the axum server in a background task.
        let shutdown_ct = server_ct.clone();
        let task = tokio::spawn(async move {
            let server =
                axum::serve(listener, sse_router).with_graceful_shutdown(async move {
                    shutdown_ct.cancelled().await;
                    tracing::info!("TeamMcpServer shutting down");
                });
            if let Err(e) = server.await {
                tracing::error!(error = %e, "TeamMcpServer exited with error");
            }
        });

        tracing::info!(
            addr = %addr,
            "TeamMcpServer started — SSE endpoint: http://{}/sse",
            addr
        );

        Ok(TeamMcpServerHandle {
            port: addr.port(),
            addr,
            cancellation_token: server_ct,
            task,
        })
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

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
        server
            .registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "test task".into(),
                ..Default::default()
            })
            .unwrap();
        server.registry.try_claim("T001", "codex").unwrap();

        let result = server
            .complete_task(Parameters(CompleteTaskParams {
                task_id: "T001".to_string(),
                note: "done".to_string(),
            }))
            .await;
        assert!(result.contains("T001"), "result: {result}");

        let task = server.registry.get_task("T001").unwrap().unwrap();
        assert!(
            matches!(task.status_parsed(), TaskStatus::Done),
            "expected Done, got {:?}",
            task.status_parsed()
        );
    }

    #[tokio::test]
    async fn test_complete_task_unclaimed_is_noop() {
        let (server, _tmp) = make_server();
        server
            .registry
            .create_task(CreateTask {
                id: "T002".into(),
                title: "unclaimed".into(),
                ..Default::default()
            })
            .unwrap();
        // deliberately do NOT claim — mark_done should silently no-op
        let _result = server
            .complete_task(Parameters(CompleteTaskParams {
                task_id: "T002".to_string(),
                note: "oops".to_string(),
            }))
            .await;
        let task = server.registry.get_task("T002").unwrap().unwrap();
        assert!(
            matches!(task.status_parsed(), TaskStatus::Pending),
            "expected Pending, got {:?}",
            task.status_parsed()
        );
    }
}
