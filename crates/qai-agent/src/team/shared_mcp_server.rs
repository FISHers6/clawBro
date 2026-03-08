//! SharedTeamMcpServer — ACP-family MCP adapter for team tools.
//!
//! This is the legacy ACP-family tool surface, not the canonical multi-backend
//! Team Tool Contract.
//!
//! Exposes 8 legacy tools on a single port:
//!   Lead tools:       create_task, start_execution, request_confirmation, post_update,
//!                     get_task_status, assign_task
//!   Specialist tools: complete_task, block_task
//!
//! Canonical v1.1 tools are also exposed for ACP-family agents:
//!   Lead tools:       accept_task, reopen_task
//!   Specialist tools: checkpoint_task, submit_task_result, request_help
//!
//! All ACP agents get the same URL. System prompts determine which tools are appropriate per role.
//! Future family-agnostic semantics (`checkpoint_task`, `submit_task_result`,
//! `accept_task`, `reopen_task`, `request_help`) live in `qai-runtime::tool_bridge`
//! and will be mapped into this adapter as compatibility shims.
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
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::{sse_server::SseServerConfig, SseServer},
    ServerHandler,
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

/// Parameters for `checkpoint_task`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CheckpointTaskParams {
    pub task_id: String,
    pub note: String,
    pub agent: Option<String>,
}

/// Parameters for `submit_task_result`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SubmitTaskResultParams {
    pub task_id: String,
    pub summary: String,
    pub agent: Option<String>,
}

/// Parameters for `accept_task`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AcceptTaskParams {
    pub task_id: String,
    pub by: Option<String>,
}

/// Parameters for `reopen_task`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReopenTaskParams {
    pub task_id: String,
    pub reason: String,
    pub by: Option<String>,
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

/// Parameters for `request_help`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RequestHelpParams {
    pub task_id: String,
    pub message: String,
    pub agent: Option<String>,
}

// ─── SharedTeamToolServer ────────────────────────────────────────────────────

/// ACP-family MCP server exposing the current legacy team tools on one port.
/// Lead ACP agents use the first 6; Specialist ACP agents use the last 2.
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

    /// Legacy ACP completion mapping.
    /// Future canonical semantics are expected to flow through:
    /// `submit_task_result` -> leader `accept_task` / `reopen_task`.
    fn complete_task_legacy(&self, task_id: &str, agent: &str, note: &str) -> Result<()> {
        self.orchestrator
            .handle_specialist_done(task_id, agent, note)
    }

    /// Legacy ACP blocked/help mapping.
    /// Future canonical semantics are expected to flow through:
    /// `block_task` / `request_help` -> leader triage or reassignment.
    fn block_task_legacy(&self, task_id: &str, agent: &str, reason: &str) -> Result<()> {
        self.orchestrator
            .handle_specialist_blocked(task_id, agent, reason)
    }

    fn resolve_claimed_agent(&self, task_id: &str, explicit: Option<&str>) -> String {
        explicit
            .map(ToOwned::to_owned)
            .or_else(|| {
                self.orchestrator
                    .registry
                    .get_task(task_id)
                    .ok()
                    .flatten()
                    .and_then(|t| {
                        t.status_raw
                            .strip_prefix("claimed:")
                            .and_then(|s| s.splitn(2, ':').next())
                            .map(|s| s.to_string())
                    })
            })
            .unwrap_or_else(|| "unknown".to_string())
    }

    // ── Lead tools ────────────────────────────────────────────────────────────

    /// Register a new task in the team's task graph during the Planning phase.
    #[tool(
        description = "Lead only. Register a new task. Provide id, title, and optionally assignee, spec, deps (comma-separated IDs), success_criteria."
    )]
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
    #[tool(
        description = "Lead only. Start task execution. Call after all tasks are registered with create_task."
    )]
    async fn start_execution(&self) -> String {
        match self.orchestrator.activate().await {
            Ok(msg) => msg,
            Err(e) => format!("Error starting execution: {e}"),
        }
    }

    /// Request user confirmation before starting execution.
    #[tool(
        description = "Lead only. Request user confirmation. Posts plan_summary to IM and waits for user reply before execution begins."
    )]
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
    #[tool(
        description = "Lead only. Post a message to the IM channel. Use for status updates, progress reports, or questions."
    )]
    async fn post_update(&self, Parameters(p): Parameters<PostUpdateParams>) -> String {
        self.orchestrator.post_message(&p.message);
        "Posted.".to_string()
    }

    /// Get a JSON snapshot of all tasks and their current statuses.
    #[tool(
        description = "Lead only. Get current status of all tasks as JSON. Returns an array with id, title, status, assignee, deps."
    )]
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
    #[tool(
        description = "Lead only. Reassign a pending task to a new agent. task_id must be pending. Provide new_assignee agent name."
    )]
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

    /// Accept a submitted task result.
    #[tool(
        description = "Lead only. Accept a submitted task result after review. Provide task_id and optionally your leader name in by."
    )]
    async fn accept_task(&self, Parameters(p): Parameters<AcceptTaskParams>) -> String {
        let by = p.by.as_deref().unwrap_or("leader");
        match self.orchestrator.accept_submitted_task(&p.task_id, by) {
            Ok(()) => format!("Task {} accepted by {}.", p.task_id, by),
            Err(e) => format!("Error accepting task {}: {e}", p.task_id),
        }
    }

    /// Reopen a submitted/accepted/done task.
    #[tool(
        description = "Lead only. Reopen a task back to pending. Provide task_id, reason, and optionally your leader name in by."
    )]
    async fn reopen_task(&self, Parameters(p): Parameters<ReopenTaskParams>) -> String {
        let by = p.by.as_deref().unwrap_or("leader");
        match self
            .orchestrator
            .reopen_submitted_task(&p.task_id, &p.reason, by)
        {
            Ok(()) => format!("Task {} reopened by {}.", p.task_id, by),
            Err(e) => format!("Error reopening task {}: {e}", p.task_id),
        }
    }

    // ── Specialist tools ───────────────────────────────────────────────────────

    /// Mark a task as complete.
    #[tool(
        description = "Specialist only. Mark a task as complete. Provide the task_id, a short completion note, and your agent name."
    )]
    async fn complete_task(&self, Parameters(p): Parameters<CompleteTaskParams>) -> String {
        let agent = self.resolve_claimed_agent(&p.task_id, p.agent.as_deref());
        match self.complete_task_legacy(&p.task_id, &agent, &p.note) {
            Ok(()) => format!("Task {} marked done.", p.task_id),
            Err(e) => format!("Error completing task {}: {e}", p.task_id),
        }
    }

    /// Report a progress checkpoint without changing task state.
    #[tool(
        description = "Specialist only. Report a checkpoint update for the claimed task. Provide task_id, note, and optionally your agent name."
    )]
    async fn checkpoint_task(&self, Parameters(p): Parameters<CheckpointTaskParams>) -> String {
        let agent = self.resolve_claimed_agent(&p.task_id, p.agent.as_deref());
        match self
            .orchestrator
            .handle_specialist_checkpoint(&p.task_id, &agent, &p.note)
        {
            Ok(()) => format!("Checkpoint recorded for task {}.", p.task_id),
            Err(e) => format!("Error recording checkpoint for task {}: {e}", p.task_id),
        }
    }

    /// Submit a completed task result for lead acceptance.
    #[tool(
        description = "Specialist only. Submit task results for lead review. Provide task_id, summary, and optionally your agent name."
    )]
    async fn submit_task_result(
        &self,
        Parameters(p): Parameters<SubmitTaskResultParams>,
    ) -> String {
        let agent = self.resolve_claimed_agent(&p.task_id, p.agent.as_deref());
        match self
            .orchestrator
            .handle_specialist_submitted(&p.task_id, &agent, &p.summary)
        {
            Ok(()) => format!("Task {} submitted for review.", p.task_id),
            Err(e) => format!("Error submitting task {}: {e}", p.task_id),
        }
    }

    /// Report that a task is blocked.
    #[tool(
        description = "Specialist only. Report a task as blocked. Provide the task_id, reason, and your agent name."
    )]
    async fn block_task(&self, Parameters(p): Parameters<BlockTaskParams>) -> String {
        // Legacy hard block: release the claim and escalate.
        let agent_owned = self.resolve_claimed_agent(&p.task_id, p.agent.as_deref());
        let agent = agent_owned.as_str();
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
        if let Err(e) = self.block_task_legacy(&p.task_id, agent, &p.reason) {
            tracing::warn!(task_id = %p.task_id, "handle_specialist_blocked error: {e:#}");
        }
        format!("Task {} reported as blocked: {}", p.task_id, p.reason)
    }

    /// Ask lead for help while keeping the task claimed.
    #[tool(
        description = "Specialist only. Request help from the lead without releasing the claim. Provide task_id, message, and optionally your agent name."
    )]
    async fn request_help(&self, Parameters(p): Parameters<RequestHelpParams>) -> String {
        let agent = self.resolve_claimed_agent(&p.task_id, p.agent.as_deref());
        let owns_claim = self
            .orchestrator
            .registry
            .is_claimed_by(&p.task_id, &agent)
            .unwrap_or(false);
        if !owns_claim {
            return format!(
                "Cannot request help for task {}: not currently claimed by {}.",
                p.task_id, agent
            );
        }
        match self
            .orchestrator
            .handle_specialist_help_requested(&p.task_id, &agent, &p.message)
        {
            Ok(()) => format!("Help request sent for task {}.", p.task_id),
            Err(e) => format!("Error requesting help for task {}: {e}", p.task_id),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SharedTeamToolServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "ACP-family shared team tools. \
                 Lead agents use: create_task, start_execution, request_confirmation, post_update, get_task_status, assign_task. \
                 Lead agents may also use: accept_task, reopen_task. \
                 Specialist agents may also use: checkpoint_task, submit_task_result, request_help. \
                 Legacy specialist compatibility tools remain: complete_task, block_task. \
                 Canonical multi-backend semantics are defined in qai-runtime's tool_bridge contract."
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
            let server = axum::serve(listener, sse_router).with_graceful_shutdown(async move {
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
        srv.orchestrator
            .registry
            .try_claim("T001", "codex")
            .unwrap();

        let result = srv
            .complete_task(Parameters(CompleteTaskParams {
                task_id: "T001".to_string(),
                note: "done".to_string(),
                agent: Some("codex".to_string()),
            }))
            .await;
        assert_eq!(
            result, "Task T001 marked done.",
            "unexpected result: {result}"
        );

        let task = srv.orchestrator.registry.get_task("T001").unwrap().unwrap();
        assert!(
            matches!(task.status_parsed(), TaskStatus::Done),
            "expected Done, got {:?}",
            task.status_parsed()
        );
    }

    #[tokio::test]
    async fn test_submit_then_accept_transitions_to_accepted() {
        let (srv, _tmp) = make_server();
        srv.orchestrator
            .registry
            .create_task(CT {
                id: "T010".into(),
                title: "reviewable".into(),
                ..Default::default()
            })
            .unwrap();
        srv.orchestrator
            .registry
            .try_claim("T010", "codex")
            .unwrap();

        let submitted = srv
            .submit_task_result(Parameters(SubmitTaskResultParams {
                task_id: "T010".into(),
                summary: "ready for review".into(),
                agent: Some("codex".into()),
            }))
            .await;
        assert!(submitted.contains("submitted"), "result: {submitted}");

        let accepted = srv
            .accept_task(Parameters(AcceptTaskParams {
                task_id: "T010".into(),
                by: Some("claude".into()),
            }))
            .await;
        assert!(accepted.contains("accepted"), "result: {accepted}");

        let task = srv.orchestrator.registry.get_task("T010").unwrap().unwrap();
        assert!(matches!(task.status_parsed(), TaskStatus::Accepted { .. }));
    }

    #[tokio::test]
    async fn test_reopen_task_returns_task_to_pending() {
        let (srv, _tmp) = make_server();
        srv.orchestrator
            .registry
            .create_task(CT {
                id: "T011".into(),
                title: "reopenable".into(),
                ..Default::default()
            })
            .unwrap();
        srv.orchestrator
            .registry
            .try_claim("T011", "codex")
            .unwrap();
        srv.orchestrator
            .handle_specialist_submitted("T011", "codex", "ready")
            .unwrap();

        let result = srv
            .reopen_task(Parameters(ReopenTaskParams {
                task_id: "T011".into(),
                reason: "needs edge-case fix".into(),
                by: Some("leader".into()),
            }))
            .await;
        assert!(result.contains("reopened"), "result: {result}");

        let task = srv.orchestrator.registry.get_task("T011").unwrap().unwrap();
        assert!(matches!(task.status_parsed(), TaskStatus::Pending));
    }

    #[tokio::test]
    async fn test_request_help_does_not_release_claim() {
        let (srv, _tmp) = make_server();
        srv.orchestrator
            .registry
            .create_task(CT {
                id: "T012".into(),
                title: "stuck".into(),
                ..Default::default()
            })
            .unwrap();
        srv.orchestrator
            .registry
            .try_claim("T012", "codex")
            .unwrap();

        let result = srv
            .request_help(Parameters(RequestHelpParams {
                task_id: "T012".into(),
                message: "need schema guidance".into(),
                agent: Some("codex".into()),
            }))
            .await;
        assert!(result.contains("Help request sent"), "result: {result}");
        assert!(srv
            .orchestrator
            .registry
            .is_claimed_by("T012", "codex")
            .unwrap());
    }

    #[tokio::test]
    async fn test_checkpoint_does_not_change_task_state() {
        let (srv, _tmp) = make_server();
        srv.orchestrator
            .registry
            .create_task(CT {
                id: "T013".into(),
                title: "checkpoint".into(),
                ..Default::default()
            })
            .unwrap();
        srv.orchestrator
            .registry
            .try_claim("T013", "codex")
            .unwrap();

        let result = srv
            .checkpoint_task(Parameters(CheckpointTaskParams {
                task_id: "T013".into(),
                note: "schema drafted".into(),
                agent: Some("codex".into()),
            }))
            .await;
        assert!(result.contains("Checkpoint recorded"), "result: {result}");
        let task = srv.orchestrator.registry.get_task("T013").unwrap().unwrap();
        assert!(matches!(task.status_parsed(), TaskStatus::Claimed { .. }));
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
        srv.orchestrator
            .registry
            .try_claim("T002", "codex")
            .unwrap();

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
        assert!(matches!(
            srv.orchestrator.team_state(),
            TeamState::AwaitingConfirm
        ));
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
