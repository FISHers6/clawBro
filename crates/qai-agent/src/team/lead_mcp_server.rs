//! LeadMcpServer — per-group MCP server for the Lead Agent.
//!
//! Provides 6 tools: create_task, start_execution, request_confirmation,
//! post_update, get_task_status, assign_task.

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

use super::orchestrator::{TeamOrchestrator, TeamState};
use super::registry::CreateTask;

// ─── Parameter structs ──────────────────────────────────────────────────────

/// Parameters for the `create_task` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateTaskParams {
    /// Unique task ID (e.g. "T001").
    pub id: String,
    /// Short human-readable title.
    pub title: String,
    /// Agent to assign the task to (e.g. "codex", "claude").
    pub assignee: Option<String>,
    /// Detailed specification of what the agent must do.
    pub spec: Option<String>,
    /// Comma-separated dependency task IDs (e.g. "T001,T002").
    pub deps: Option<String>,
    /// Success criteria the agent must meet.
    pub success_criteria: Option<String>,
}

/// Parameters for the `request_confirmation` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RequestConfirmationParams {
    /// A summary of the plan to present to the user for confirmation.
    pub plan_summary: String,
}

/// Parameters for the `post_update` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PostUpdateParams {
    /// The update message to post.
    pub message: String,
}

/// Parameters for the `assign_task` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AssignTaskParams {
    /// The task ID to reassign (must be pending).
    pub task_id: String,
    /// The new assignee agent name.
    pub new_assignee: String,
}

// ─── LeadToolServer ───────────────────────────────────────────────────────────

/// MCP server that exposes team-management tools to the Lead agent.
#[derive(Clone)]
pub struct LeadToolServer {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    pub orchestrator: Arc<TeamOrchestrator>,
}

#[tool_router]
impl LeadToolServer {
    pub fn new(orchestrator: Arc<TeamOrchestrator>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            orchestrator,
        }
    }

    /// Register a new task in the team's task graph during the Planning phase.
    /// Use this to build the task dependency graph before calling start_execution.
    #[tool(description = "Register a new task. Provide id, title, and optionally assignee, spec, deps (comma-separated IDs), success_criteria.")]
    async fn create_task(&self, Parameters(p): Parameters<CreateTaskParams>) -> String {
        let deps: Vec<String> = p
            .deps
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();

        let task = CreateTask {
            id: p.id.clone(),
            title: p.title,
            assignee_hint: p.assignee,
            deps,
            timeout_secs: 1800,
            spec: p.spec,
            success_criteria: p.success_criteria,
        };

        match self.orchestrator.register_task(task) {
            Ok(msg) => msg,
            Err(e) => format!("Error registering task {}: {e}", p.id),
        }
    }

    /// Activate team execution: start the Heartbeat and SpecialistMcpServer.
    /// Call this after all tasks have been registered (and confirmed if needed).
    #[tool(description = "Start task execution. Activates the Heartbeat and SpecialistMcpServer. Call after all tasks are registered.")]
    async fn start_execution(&self) -> String {
        match self.orchestrator.activate().await {
            Ok(msg) => msg,
            Err(e) => format!("Error starting execution: {e}"),
        }
    }

    /// Request user confirmation before starting execution.
    /// This posts the plan summary to the IM channel and pauses the team.
    #[tool(description = "Request user confirmation. Posts plan_summary to IM and waits for user reply before execution begins.")]
    async fn request_confirmation(
        &self,
        Parameters(p): Parameters<RequestConfirmationParams>,
    ) -> String {
        let formatted = format!("**Plan for confirmation:**\n\n{}", p.plan_summary);
        self.orchestrator.post_message(&formatted);
        *self.orchestrator.team_state_inner.lock().unwrap() = TeamState::AwaitingConfirm;
        "Confirmation requested. Waiting for user reply.".to_string()
    }

    /// Post a status update or message to the IM channel.
    #[tool(description = "Post a message to the IM channel. Use for status updates, progress reports, or questions.")]
    async fn post_update(&self, Parameters(p): Parameters<PostUpdateParams>) -> String {
        self.orchestrator.post_message(&p.message);
        "Posted.".to_string()
    }

    /// Get a JSON snapshot of all tasks and their current statuses.
    #[tool(description = "Get current status of all tasks as JSON. Returns an array of task objects with id, title, status, assignee, deps.")]
    async fn get_task_status(&self) -> String {
        match self.orchestrator.registry.all_tasks() {
            Ok(tasks) => {
                let arr: Vec<serde_json::Value> = tasks
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "id": t.id,
                            "title": t.title,
                            "status": t.status_raw,
                            "assignee": t.assignee_hint,
                            "deps": t.deps(),
                            "retry_count": t.retry_count,
                            "completion_note": t.completion_note,
                        })
                    })
                    .collect();
                serde_json::to_string_pretty(&arr).unwrap_or_else(|e| format!("Error: {e}"))
            }
            Err(e) => format!("Error fetching tasks: {e}"),
        }
    }

    /// Reassign a pending task to a different agent.
    #[tool(description = "Reassign a pending task to a new agent. task_id must be in pending state. Provide new_assignee agent name.")]
    async fn assign_task(&self, Parameters(p): Parameters<AssignTaskParams>) -> String {
        match self
            .orchestrator
            .registry
            .reassign_task(&p.task_id, &p.new_assignee)
        {
            Ok(()) => format!("Task {} reassigned to {}.", p.task_id, p.new_assignee),
            Err(e) => format!("Error reassigning task {}: {e}", p.task_id),
        }
    }
}

impl ServerHandler for LeadToolServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Lead agent tools. Use create_task to build the task graph, \
                 request_confirmation to get user approval, start_execution to begin, \
                 post_update for status messages, get_task_status to monitor progress, \
                 and assign_task to reassign pending tasks."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

// ─── LeadMcpServerHandle ─────────────────────────────────────────────────────

/// Handle returned by [`LeadToolServer::spawn`].
pub struct LeadMcpServerHandle {
    /// TCP port the SSE server is listening on (127.0.0.1).
    pub port: u16,
    /// Full socket address (always 127.0.0.1:{port}).
    #[allow(dead_code)]
    addr: SocketAddr,
    /// Cancel this token to trigger a graceful shutdown.
    cancellation_token: CancellationToken,
    /// Background task that runs the axum server; joins when the CT is cancelled.
    task: JoinHandle<()>,
}

impl LeadMcpServerHandle {
    /// Shut the Lead MCP server down and wait for the background task to finish.
    pub async fn stop(self) {
        self.cancellation_token.cancel();
        let _ = self.task.await;
    }
}

// ─── spawn() ─────────────────────────────────────────────────────────────────

impl LeadToolServer {
    /// Start the Lead MCP SSE server on a random loopback port.
    ///
    /// Returns a [`LeadMcpServerHandle`] containing the actual port
    /// and a [`CancellationToken`] that can be used to stop the server.
    pub async fn spawn(self) -> Result<LeadMcpServerHandle> {
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
                    tracing::info!("LeadMcpServer shutting down");
                });
            if let Err(e) = server.await {
                tracing::error!(error = %e, "LeadMcpServer exited with error");
            }
        });

        tracing::info!(
            addr = %addr,
            "LeadMcpServer started — SSE endpoint: http://{}/sse",
            addr
        );

        Ok(LeadMcpServerHandle {
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
    use crate::team::{
        bus::InternalBus, heartbeat::DispatchFn, registry::TaskRegistry, session::TeamSession,
    };
    use tempfile::tempdir;

    fn make_server() -> (LeadToolServer, tempfile::TempDir) {
        let tmp = tempdir().unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("test", tmp.path().to_path_buf()));
        let bus = Arc::new(InternalBus::new());
        let dispatch_fn: DispatchFn = Arc::new(|_, _, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            registry,
            session,
            bus,
            dispatch_fn,
            std::time::Duration::from_secs(3600),
        );
        (LeadToolServer::new(orch), tmp)
    }

    #[tokio::test]
    async fn test_create_task_registers() {
        let (srv, _tmp) = make_server();
        let result = srv
            .create_task(Parameters(CreateTaskParams {
                id: "T001".into(),
                title: "Setup DB".into(),
                assignee: None,
                spec: None,
                deps: None,
                success_criteria: None,
            }))
            .await;
        assert!(result.contains("T001"), "result: {result}");
        assert!(result.contains("registered"), "result: {result}");
    }

    #[tokio::test]
    async fn test_get_task_status_json() {
        let (srv, _tmp) = make_server();
        // Register a task via registry directly
        srv.orchestrator
            .registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "Test".into(),
                ..Default::default()
            })
            .unwrap();
        let json_str = srv.get_task_status().await;
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 1);
        assert_eq!(parsed[0]["id"], "T001");
    }

    #[tokio::test]
    async fn test_post_update_without_notify_fn_does_not_panic() {
        let (srv, _tmp) = make_server();
        let result = srv
            .post_update(Parameters(PostUpdateParams {
                message: "Hello".into(),
            }))
            .await;
        assert_eq!(result, "Posted.");
    }

    #[tokio::test]
    async fn test_assign_task_pending() {
        let (srv, _tmp) = make_server();
        srv.orchestrator
            .registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "A".into(),
                ..Default::default()
            })
            .unwrap();
        let result = srv
            .assign_task(Parameters(AssignTaskParams {
                task_id: "T001".into(),
                new_assignee: "claude".into(),
            }))
            .await;
        assert!(result.contains("T001"), "result: {result}");
        assert!(result.contains("claude"), "result: {result}");
        let task = srv
            .orchestrator
            .registry
            .get_task("T001")
            .unwrap()
            .unwrap();
        assert_eq!(task.assignee_hint.as_deref(), Some("claude"));
    }

    #[tokio::test]
    async fn test_request_confirmation_sets_awaiting_state() {
        let (srv, _tmp) = make_server();
        let result = srv
            .request_confirmation(Parameters(RequestConfirmationParams {
                plan_summary: "Do X then Y".into(),
            }))
            .await;
        assert_eq!(result, "Confirmation requested. Waiting for user reply.");
        let state = srv.orchestrator.team_state();
        assert!(
            matches!(state, TeamState::AwaitingConfirm),
            "expected AwaitingConfirm, got {:?}",
            state
        );
    }
}
