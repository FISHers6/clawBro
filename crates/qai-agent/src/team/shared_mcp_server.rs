//! SharedTeamMcpServer — unified MCP SSE server for all team agents.
//!
//! Exposes 8 tools on a single port:
//!   Lead tools:       create_task, start_execution, request_confirmation, post_update,
//!                     get_task_status, assign_task
//!   Specialist tools: complete_task, block_task
//!
//! All agents get the same URL. System prompts determine which tools are appropriate per role.
//!
//! Lifecycle:
//!   ```text
//!   SharedTeamToolServer::new(orchestrator)
//!     .spawn()  →  SharedMcpServerHandle { port }
//!   ```
//!   Cancel the token (or call `.stop()`) to shut the server down gracefully.

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

/// Parameters for `create_task`.
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

/// Parameters for `request_confirmation`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RequestConfirmationParams {
    /// A summary of the plan to present to the user for confirmation.
    pub plan_summary: String,
}

/// Parameters for `post_update`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PostUpdateParams {
    /// The update message to post.
    pub message: String,
}

/// Parameters for `assign_task`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AssignTaskParams {
    /// The task ID to reassign (must be pending).
    pub task_id: String,
    /// The new assignee agent name.
    pub new_assignee: String,
}

/// Parameters for `complete_task`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CompleteTaskParams {
    /// The task ID to mark as done (e.g. "T001").
    pub task_id: String,
    /// A short note summarising what was accomplished.
    pub note: String,
    /// Your agent name (e.g. "codex"). Used to verify you claimed this task.
    pub agent: Option<String>,
}

/// Parameters for `block_task`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BlockTaskParams {
    /// The task ID that is blocked (e.g. "T002").
    pub task_id: String,
    /// A description of what is blocking progress.
    pub reason: String,
    /// Your agent name (for escalation context).
    pub agent: Option<String>,
}

// ─── SharedTeamToolServer ────────────────────────────────────────────────────

/// Unified MCP server: exposes all 8 team tools on one port.
/// Lead agents use the first 6; Specialist agents use the last 2.
#[derive(Clone)]
pub struct SharedTeamToolServer {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    pub orchestrator: Arc<TeamOrchestrator>,
}

#[tool_router]
impl SharedTeamToolServer {
    pub fn new(orchestrator: Arc<TeamOrchestrator>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            orchestrator,
        }
    }

    // ── Lead tools ────────────────────────────────────────────────────────────

    /// Register a new task in the team's task graph during the Planning phase.
    #[tool(description = "Lead only. Register a new task. Provide id, title, and optionally assignee, spec, deps (comma-separated IDs), success_criteria.")]
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

    /// Activate team execution: start the Heartbeat and MCP server becomes live.
    #[tool(description = "Lead only. Start task execution. Call after all tasks are registered with create_task.")]
    async fn start_execution(&self) -> String {
        match self.orchestrator.activate().await {
            Ok(msg) => msg,
            Err(e) => format!("Error starting execution: {e}"),
        }
    }

    /// Request user confirmation before starting execution.
    #[tool(description = "Lead only. Request user confirmation. Posts plan_summary to IM and waits for user reply before execution begins.")]
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
    #[tool(description = "Lead only. Post a message to the IM channel. Use for status updates, progress reports, or questions.")]
    async fn post_update(&self, Parameters(p): Parameters<PostUpdateParams>) -> String {
        self.orchestrator.post_message(&p.message);
        "Posted.".to_string()
    }

    /// Get a JSON snapshot of all tasks and their current statuses.
    #[tool(description = "Lead only. Get current status of all tasks as JSON. Returns an array with id, title, status, assignee, deps.")]
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
    #[tool(description = "Lead only. Reassign a pending task to a new agent. task_id must be pending. Provide new_assignee agent name.")]
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

    // ── Specialist tools ───────────────────────────────────────────────────────

    /// Mark a task as complete.
    #[tool(description = "Specialist only. Mark a task as complete. Provide the task_id, a short completion note, and your agent name.")]
    async fn complete_task(&self, Parameters(p): Parameters<CompleteTaskParams>) -> String {
        // Resolve agent: explicit param first, then extract from claimed status
        let agent = p.agent.as_deref().map(|s| s.to_string()).unwrap_or_else(|| {
            self.orchestrator.registry.get_task(&p.task_id)
                .ok()
                .flatten()
                .and_then(|t| {
                    t.status_raw.strip_prefix("claimed:")
                        .and_then(|s| s.splitn(2, ':').next())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| "unknown".to_string())
        });
        match self.orchestrator.handle_specialist_done(&p.task_id, &agent, &p.note) {
            Ok(()) => format!("Task {} marked done.", p.task_id),
            Err(e) => format!("Error completing task {}: {e}", p.task_id),
        }
    }

    /// Report that a task is blocked.
    #[tool(description = "Specialist only. Report a task as blocked. Provide the task_id, reason, and your agent name.")]
    async fn block_task(&self, Parameters(p): Parameters<BlockTaskParams>) -> String {
        // Resolve agent: explicit param first, then extract from claimed status (mirrors complete_task).
        // Default "specialist" would always fail the ownership check when the actual claimer is a named agent.
        let agent_owned = p.agent.as_deref().map(|s| s.to_string()).unwrap_or_else(|| {
            self.orchestrator.registry.get_task(&p.task_id)
                .ok()
                .flatten()
                .and_then(|t| {
                    t.status_raw.strip_prefix("claimed:")
                        .and_then(|s| s.splitn(2, ':').next())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| "unknown".to_string())
        });
        let agent = agent_owned.as_str();
        // Validate claim ownership before resetting
        let owns_claim = self
            .orchestrator
            .registry
            .is_claimed_by(&p.task_id, agent)
            .unwrap_or(false);
        if !owns_claim {
            tracing::warn!(
                task_id = %p.task_id,
                agent = %agent,
                "block_task rejected: agent does not hold claim"
            );
            return format!(
                "Cannot block task {}: not currently claimed by {}.",
                p.task_id, agent
            );
        }
        if let Err(e) = self.orchestrator.registry.reset_claim(&p.task_id) {
            tracing::warn!(task_id = %p.task_id, "reset_claim error: {e:#}");
        }
        if let Err(e) = self.orchestrator.handle_specialist_blocked(&p.task_id, agent, &p.reason) {
            tracing::warn!(task_id = %p.task_id, "handle_specialist_blocked error: {e:#}");
        }
        format!("Task {} reported as blocked: {}", p.task_id, p.reason)
    }
}

impl ServerHandler for SharedTeamToolServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Shared team tools. \
                 Lead agents use: create_task, start_execution, request_confirmation, post_update, get_task_status, assign_task. \
                 Specialist agents use: complete_task, block_task."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

// ─── SharedMcpServerHandle ───────────────────────────────────────────────────

pub struct SharedMcpServerHandle {
    /// TCP port the SSE server is listening on (127.0.0.1).
    pub port: u16,
    #[allow(dead_code)]
    addr: SocketAddr,
    cancellation_token: CancellationToken,
    task: JoinHandle<()>,
}

impl SharedMcpServerHandle {
    pub async fn stop(self) {
        self.cancellation_token.cancel();
        let _ = self.task.await;
    }
}

// ─── spawn() ─────────────────────────────────────────────────────────────────

impl SharedTeamToolServer {
    /// Start the unified MCP SSE server on a random loopback port.
    pub async fn spawn(self) -> Result<SharedMcpServerHandle> {
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

        let (sse_server, sse_router) = SseServer::new(sse_config);
        let server_ct = sse_server.with_service(move || self.clone());

        let shutdown_ct = server_ct.clone();
        let task = tokio::spawn(async move {
            let server = axum::serve(listener, sse_router)
                .with_graceful_shutdown(async move {
                    shutdown_ct.cancelled().await;
                    tracing::info!("SharedTeamMcpServer shutting down");
                });
            if let Err(e) = server.await {
                tracing::error!(error = %e, "SharedTeamMcpServer exited with error");
            }
        });

        tracing::info!(
            addr = %addr,
            "SharedTeamMcpServer started — SSE endpoint: http://{}/sse",
            addr
        );

        Ok(SharedMcpServerHandle {
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
        heartbeat::DispatchFn,
        registry::{CreateTask as CT, TaskRegistry, TaskStatus},
        session::TeamSession,
    };
    use std::sync::Arc;
    use tempfile::tempdir;

    fn make_server() -> (SharedTeamToolServer, tempfile::TempDir) {
        let tmp = tempdir().unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("test", tmp.path().to_path_buf()));
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(3600),
        );
        (SharedTeamToolServer::new(orch), tmp)
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
        srv.orchestrator
            .registry
            .create_task(CT {
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
    async fn test_complete_task_marks_done() {
        let (srv, _tmp) = make_server();
        srv.orchestrator
            .registry
            .create_task(CT {
                id: "T001".into(),
                title: "test task".into(),
                ..Default::default()
            })
            .unwrap();
        srv.orchestrator.registry.try_claim("T001", "codex").unwrap();

        let result = srv
            .complete_task(Parameters(CompleteTaskParams {
                task_id: "T001".to_string(),
                note: "done".to_string(),
                agent: Some("codex".to_string()),
            }))
            .await;
        assert_eq!(result, "Task T001 marked done.", "unexpected result: {result}");

        let task = srv.orchestrator.registry.get_task("T001").unwrap().unwrap();
        assert!(
            matches!(task.status_parsed(), TaskStatus::Done),
            "expected Done, got {:?}",
            task.status_parsed()
        );
    }

    #[tokio::test]
    async fn test_block_task_resets_to_pending() {
        let (srv, _tmp) = make_server();
        srv.orchestrator
            .registry
            .create_task(CT {
                id: "T002".into(),
                title: "blocked task".into(),
                ..Default::default()
            })
            .unwrap();
        srv.orchestrator.registry.try_claim("T002", "codex").unwrap();

        let result = srv
            .block_task(Parameters(BlockTaskParams {
                task_id: "T002".to_string(),
                reason: "missing dep".to_string(),
                agent: Some("codex".to_string()),
            }))
            .await;

        assert!(result.contains("T002"), "result: {result}");
        let task = srv.orchestrator.registry.get_task("T002").unwrap().unwrap();
        assert!(
            matches!(task.status_parsed(), TaskStatus::Pending),
            "expected Pending after block_task, got {:?}",
            task.status_parsed()
        );
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
    async fn test_request_confirmation_sets_awaiting_state() {
        let (srv, _tmp) = make_server();
        let result = srv
            .request_confirmation(Parameters(RequestConfirmationParams {
                plan_summary: "Do X then Y".into(),
            }))
            .await;
        assert_eq!(result, "Confirmation requested. Waiting for user reply.");
        assert!(matches!(srv.orchestrator.team_state(), TeamState::AwaitingConfirm));
    }

    #[tokio::test]
    async fn test_assign_task_reassigns_pending() {
        let (srv, _tmp) = make_server();
        srv.orchestrator
            .registry
            .create_task(CT {
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
    }
}
