use crate::agent_sdk_internal::{
    bridge::{AgentTurnRequest, ApprovalMode, ExecutionRole},
    tools::{ConfiguredAgentBuilder, EventedTool, RuntimeToolAugmentor, ToolProgressTracker},
};
use crate::protocol::{
    parse_session_key_text, TeamTool, TeamToolCall, TeamToolRequest, TeamToolResponse,
};
use crate::runtime::RuntimeRole;
use crate::team_contract::projection::local_tools::project_local_team_tools;
use rig::{
    completion::{CompletionModel, ToolDefinition},
    tool::{Tool, ToolError},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone)]
struct TeamToolClient {
    endpoint: String,
    session_ref: String,
    client: reqwest::Client,
}

impl TeamToolClient {
    fn new(endpoint: String, session_ref: String) -> Self {
        Self {
            endpoint,
            session_ref,
            client: reqwest::Client::new(),
        }
    }

    async fn invoke(&self, call: TeamToolCall) -> Result<TeamToolOutput, ToolError> {
        let session_key = parse_session_key_text(&self.session_ref).map_err(|e| {
            ToolError::ToolCallError(format!("invalid team session key: {e}").into())
        })?;
        let response = self
            .client
            .post(&self.endpoint)
            .json(&TeamToolRequest { session_key, call })
            .send()
            .await
            .map_err(|e| {
                ToolError::ToolCallError(format!("team tool request failed: {e}").into())
            })?;

        let status = response.status();
        let body: TeamToolResponse = response.json().await.map_err(|e| {
            ToolError::ToolCallError(format!("team tool decode failed: {e}").into())
        })?;

        if !status.is_success() || !body.ok {
            return Err(ToolError::ToolCallError(body.message.into()));
        }

        Ok(TeamToolOutput {
            message: body.message,
        })
    }
}

#[derive(Debug, Serialize)]
struct TeamToolOutput {
    message: String,
}

#[derive(Debug, Clone, Default)]
pub struct ClawBroTeamToolAugmentor {
    endpoint: Option<String>,
}

impl ClawBroTeamToolAugmentor {
    pub fn from_endpoint(endpoint: Option<String>) -> Self {
        Self { endpoint }
    }

    pub fn from_env() -> Self {
        Self::from_endpoint(std::env::var("CLAWBRO_TEAM_TOOL_URL").ok())
    }
}

impl RuntimeToolAugmentor for ClawBroTeamToolAugmentor {
    fn augment<M: CompletionModel>(
        &self,
        builder: ConfiguredAgentBuilder<M>,
        session: &AgentTurnRequest,
        tracker: Option<ToolProgressTracker>,
        approval_mode: ApprovalMode,
    ) -> ConfiguredAgentBuilder<M> {
        let Some(endpoint) = self.endpoint.clone() else {
            return builder;
        };
        let client = TeamToolClient::new(endpoint, session.session_ref.clone());

        // Solo agents bypass the team_tools gate — they still get social tools
        // (list_agents + send_message) as long as an endpoint is configured.
        // tool_surface.team_tools is false for Solo by default (bridge.rs:466).
        if !session.tool_surface.team_tools {
            if session.role == ExecutionRole::Solo {
                return inject_social_tools_only(builder, &client, tracker, approval_mode);
            }
            return builder;
        }

        match tracker {
            Some(tracker) => register_team_tools_with_progress(
                builder,
                session.role,
                &session.tool_surface.allowed_team_tools,
                client,
                tracker,
                approval_mode,
            ),
            None => register_team_tools(
                builder,
                session.role,
                &session.tool_surface.allowed_team_tools,
                client,
                approval_mode,
            ),
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreateTaskArgs {
    #[serde(default)]
    id: Option<String>,
    title: String,
    #[serde(default)]
    assignee: Option<String>,
    #[serde(default)]
    spec: Option<String>,
    #[serde(default)]
    deps: Vec<String>,
    #[serde(default)]
    success_criteria: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RequestConfirmationArgs {
    plan_summary: String,
}

#[derive(Debug, Deserialize)]
struct PostUpdateArgs {
    message: String,
}

#[derive(Debug, Deserialize)]
struct AssignTaskArgs {
    task_id: String,
    new_assignee: String,
}

#[derive(Debug, Deserialize)]
struct CheckpointTaskArgs {
    task_id: String,
    note: String,
}

#[derive(Debug, Deserialize)]
struct SubmitTaskResultArgs {
    task_id: String,
    summary: String,
    result_markdown: String,
}

#[derive(Debug, Deserialize)]
struct AcceptTaskArgs {
    task_id: String,
    #[serde(default)]
    by: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReopenTaskArgs {
    task_id: String,
    reason: String,
    #[serde(default)]
    by: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BlockTaskArgs {
    task_id: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct RequestHelpArgs {
    task_id: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct SendMessageArgs {
    target: String,
    message: String,
    #[serde(default)]
    scope: Option<String>,
}

macro_rules! define_team_tool {
    ($tool_name:ident, $const_name:literal, $args_ty:ty, $desc:expr, $schema:expr, $call_builder:expr) => {
        #[derive(Debug, Clone)]
        struct $tool_name {
            client: TeamToolClient,
        }

        impl $tool_name {
            fn new(client: TeamToolClient) -> Self {
                Self { client }
            }
        }

        impl Tool for $tool_name {
            const NAME: &'static str = $const_name;
            type Error = ToolError;
            type Args = $args_ty;
            type Output = TeamToolOutput;

            async fn definition(&self, _prompt: String) -> ToolDefinition {
                ToolDefinition {
                    name: Self::NAME.to_string(),
                    description: $desc.to_string(),
                    parameters: $schema,
                }
            }

            async fn call(&self, args: $args_ty) -> Result<TeamToolOutput, ToolError> {
                self.client.invoke(($call_builder)(args)).await
            }
        }
    };
}

define_team_tool!(
    CreateTaskTool,
    "create_task",
    CreateTaskArgs,
    "Lead only. Register a new task in the team graph. Provide title and optionally id, assignee, spec, deps, success_criteria.",
    json!({
        "type": "object",
        "properties": {
            "id": {"type": "string", "description": "Optional task ID like T001. If omitted, the system allocates the next Txxx identifier."},
            "title": {"type": "string", "description": "Short human-readable task title."},
            "assignee": {"type": "string", "description": "Optional specialist to assign, such as codex-beta."},
            "spec": {"type": "string", "description": "Optional detailed task spec."},
            "deps": {"type": "array", "items": {"type": "string"}, "description": "Optional dependency task IDs."},
            "success_criteria": {"type": "string", "description": "Optional success criteria for acceptance."}
        },
        "required": ["title"]
    }),
    |args: CreateTaskArgs| TeamToolCall::CreateTask {
        id: args.id,
        title: args.title,
        assignee: args.assignee,
        spec: args.spec,
        deps: args.deps,
        success_criteria: args.success_criteria,
    }
);
define_team_tool!(
    StartExecutionTool,
    "start_execution",
    serde_json::Value,
    "Lead only. Start team execution after planning is complete.",
    json!({"type": "object", "properties": {}}),
    |_args: serde_json::Value| TeamToolCall::StartExecution
);
define_team_tool!(
    RequestConfirmationTool,
    "request_confirmation",
    RequestConfirmationArgs,
    "Lead only. Ask the user to confirm the plan before execution.",
    json!({
        "type": "object",
        "properties": {
            "plan_summary": {"type": "string", "description": "Summary of the plan to present for confirmation."}
        },
        "required": ["plan_summary"]
    }),
    |args: RequestConfirmationArgs| TeamToolCall::RequestConfirmation {
        plan_summary: args.plan_summary,
    }
);
define_team_tool!(
    PostUpdateTool,
    "post_update",
    PostUpdateArgs,
    "Lead or specialist. Post a progress update to the user-visible team timeline.",
    json!({
        "type": "object",
        "properties": {
            "message": {"type": "string", "description": "Short progress update for the user."}
        },
        "required": ["message"]
    }),
    |args: PostUpdateArgs| TeamToolCall::PostUpdate {
        message: args.message,
    }
);
define_team_tool!(
    GetTaskStatusTool,
    "get_task_status",
    serde_json::Value,
    "Lead or specialist. Fetch the current task graph status.",
    json!({"type": "object", "properties": {}}),
    |_args: serde_json::Value| TeamToolCall::GetTaskStatus
);
define_team_tool!(
    AssignTaskTool,
    "assign_task",
    AssignTaskArgs,
    "Lead only. Reassign a task to another specialist.",
    json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string"},
            "new_assignee": {"type": "string"}
        },
        "required": ["task_id", "new_assignee"]
    }),
    |args: AssignTaskArgs| TeamToolCall::AssignTask {
        task_id: args.task_id,
        new_assignee: args.new_assignee,
    }
);
define_team_tool!(
    CheckpointTaskTool,
    "checkpoint_task",
    CheckpointTaskArgs,
    "Specialist. Record an in-progress note for a task.",
    json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string"},
            "note": {"type": "string"}
        },
        "required": ["task_id", "note"]
    }),
    |args: CheckpointTaskArgs| TeamToolCall::CheckpointTask {
        task_id: args.task_id,
        note: args.note,
        agent: None,
    }
);
define_team_tool!(
    SubmitTaskResultTool,
    "submit_task_result",
    SubmitTaskResultArgs,
    "Specialist. Submit task results for lead acceptance. `result_markdown` must contain the actual final deliverable body, not a wrapper or artifact summary.",
    json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string"},
            "summary": {"type": "string"},
            "result_markdown": {"type": "string", "description": "Full final deliverable body for tasks/Txxx/result.md. Do not send only metadata, artifact paths, or delivery notes."}
        },
        "required": ["task_id", "summary", "result_markdown"]
    }),
    |args: SubmitTaskResultArgs| TeamToolCall::SubmitTaskResult {
        task_id: args.task_id,
        summary: args.summary,
        result_markdown: Some(args.result_markdown),
        agent: None,
    }
);
define_team_tool!(
    AcceptTaskTool,
    "accept_task",
    AcceptTaskArgs,
    "Lead only. Accept a submitted task.",
    json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string"},
            "by": {"type": "string"}
        },
        "required": ["task_id"]
    }),
    |args: AcceptTaskArgs| TeamToolCall::AcceptTask {
        task_id: args.task_id,
        by: args.by,
    }
);
define_team_tool!(
    ReopenTaskTool,
    "reopen_task",
    ReopenTaskArgs,
    "Lead only. Reopen a previously submitted or completed task.",
    json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string"},
            "reason": {"type": "string"},
            "by": {"type": "string"}
        },
        "required": ["task_id", "reason"]
    }),
    |args: ReopenTaskArgs| TeamToolCall::ReopenTask {
        task_id: args.task_id,
        reason: args.reason,
        by: args.by,
    }
);
define_team_tool!(
    BlockTaskTool,
    "block_task",
    BlockTaskArgs,
    "Specialist. Mark a task as blocked and explain why.",
    json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string"},
            "reason": {"type": "string"}
        },
        "required": ["task_id", "reason"]
    }),
    |args: BlockTaskArgs| TeamToolCall::BlockTask {
        task_id: args.task_id,
        reason: args.reason,
        agent: None,
    }
);
define_team_tool!(
    RequestHelpTool,
    "request_help",
    RequestHelpArgs,
    "Specialist. Ask the lead for clarification or assistance on a task.",
    json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string"},
            "message": {"type": "string"}
        },
        "required": ["task_id", "message"]
    }),
    |args: RequestHelpArgs| TeamToolCall::RequestHelp {
        task_id: args.task_id,
        message: args.message,
        agent: None,
    }
);

define_team_tool!(
    ListAgentsTool,
    "list_agents",
    serde_json::Value,
    "All roles. List all agents available in the team roster. Returns their names, mentions, and backend IDs. Use before send_message to verify the target agent name.",
    json!({"type": "object", "properties": {}}),
    |_args: serde_json::Value| TeamToolCall::ListAgents
);

define_team_tool!(
    SendMessageTool,
    "send_message",
    SendMessageArgs,
    "All roles. Send a message to another agent by name (e.g. 'coder') or to 'user' (the human operator). Use list_agents first to find valid agent names. V1: agent responses are delivered via WebSocket only.",
    json!({
        "type": "object",
        "properties": {
            "target": {
                "type": "string",
                "description": "Agent name from the roster (e.g. 'coder', 'reviewer') or 'user' to reach the human operator."
            },
            "message": {
                "type": "string",
                "description": "Message body to send."
            },
            "scope": {
                "type": "string",
                "description": "Optional session scope override. Leave empty to use the current session scope."
            }
        },
        "required": ["target", "message"]
    }),
    |args: SendMessageArgs| TeamToolCall::SendMessage {
        target: args.target,
        message: args.message,
        scope: args.scope,
    }
);

fn register_team_tools<M: CompletionModel>(
    builder: ConfiguredAgentBuilder<M>,
    role: ExecutionRole,
    allowed_team_tools: &[TeamTool],
    client: TeamToolClient,
    approval_mode: ApprovalMode,
) -> ConfiguredAgentBuilder<M> {
    register_team_tools_with_progress(
        builder,
        role,
        allowed_team_tools,
        client,
        ToolProgressTracker::new(std::sync::Arc::new(|_| {})),
        approval_mode,
    )
}

fn register_team_tools_with_progress<M: CompletionModel>(
    builder: ConfiguredAgentBuilder<M>,
    role: ExecutionRole,
    allowed_team_tools: &[TeamTool],
    client: TeamToolClient,
    tracker: ToolProgressTracker,
    approval_mode: ApprovalMode,
) -> ConfiguredAgentBuilder<M> {
    let mut builder = builder;
    let visible_tools = visible_team_tools_for_request(role, allowed_team_tools);
    match role {
        ExecutionRole::Leader => {
            for tool in visible_tools {
                builder = add_leader_team_tool(builder, tool, &client, &tracker, approval_mode);
            }
        }
        ExecutionRole::Specialist => {
            for tool in visible_tools {
                builder = add_specialist_team_tool(builder, tool, &client, &tracker, approval_mode);
            }
        }
        ExecutionRole::Solo => {
            for tool in visible_tools {
                builder = add_social_team_tool(builder, tool, &client, &tracker, approval_mode);
            }
        }
    }

    builder
}

fn visible_team_tools_for_request(
    role: ExecutionRole,
    allowed_team_tools: &[TeamTool],
) -> Vec<TeamTool> {
    let runtime_role = match role {
        ExecutionRole::Leader => RuntimeRole::Leader,
        ExecutionRole::Specialist => RuntimeRole::Specialist,
        ExecutionRole::Solo => RuntimeRole::Solo,
    };
    project_local_team_tools(runtime_role, allowed_team_tools)
}

fn add_leader_team_tool<M: CompletionModel>(
    builder: ConfiguredAgentBuilder<M>,
    tool: TeamTool,
    client: &TeamToolClient,
    tracker: &ToolProgressTracker,
    approval_mode: ApprovalMode,
) -> ConfiguredAgentBuilder<M> {
    match tool {
        TeamTool::CreateTask => builder.tool(EventedTool::new(
            CreateTaskTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::StartExecution => builder.tool(EventedTool::new(
            StartExecutionTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::RequestConfirmation => builder.tool(EventedTool::new(
            RequestConfirmationTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::PostUpdate => builder.tool(EventedTool::new(
            PostUpdateTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::GetTaskStatus => builder.tool(EventedTool::new(
            GetTaskStatusTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::AssignTask => builder.tool(EventedTool::new(
            AssignTaskTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::AcceptTask => builder.tool(EventedTool::new(
            AcceptTaskTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::ReopenTask => builder.tool(EventedTool::new(
            ReopenTaskTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::ListAgents => builder.tool(EventedTool::new(
            ListAgentsTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::SendMessage => builder.tool(EventedTool::new(
            SendMessageTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        _ => builder,
    }
}

fn add_specialist_team_tool<M: CompletionModel>(
    builder: ConfiguredAgentBuilder<M>,
    tool: TeamTool,
    client: &TeamToolClient,
    tracker: &ToolProgressTracker,
    approval_mode: ApprovalMode,
) -> ConfiguredAgentBuilder<M> {
    match tool {
        TeamTool::PostUpdate => builder.tool(EventedTool::new(
            PostUpdateTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::GetTaskStatus => builder.tool(EventedTool::new(
            GetTaskStatusTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::CheckpointTask => builder.tool(EventedTool::new(
            CheckpointTaskTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::SubmitTaskResult => builder.tool(EventedTool::new(
            SubmitTaskResultTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::BlockTask => builder.tool(EventedTool::new(
            BlockTaskTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::RequestHelp => builder.tool(EventedTool::new(
            RequestHelpTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::ListAgents => builder.tool(EventedTool::new(
            ListAgentsTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::SendMessage => builder.tool(EventedTool::new(
            SendMessageTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        _ => builder,
    }
}

fn inject_social_tools_only<M: CompletionModel>(
    mut builder: ConfiguredAgentBuilder<M>,
    client: &TeamToolClient,
    tracker: Option<ToolProgressTracker>,
    approval_mode: ApprovalMode,
) -> ConfiguredAgentBuilder<M> {
    let tracker = tracker.unwrap_or_else(|| ToolProgressTracker::new(std::sync::Arc::new(|_| {})));
    builder = builder.tool(EventedTool::new(
        ListAgentsTool::new(client.clone()),
        Some(tracker.clone()),
        approval_mode,
    ));
    builder = builder.tool(EventedTool::new(
        SendMessageTool::new(client.clone()),
        Some(tracker),
        approval_mode,
    ));
    builder
}

fn add_social_team_tool<M: CompletionModel>(
    builder: ConfiguredAgentBuilder<M>,
    tool: TeamTool,
    client: &TeamToolClient,
    tracker: &ToolProgressTracker,
    approval_mode: ApprovalMode,
) -> ConfiguredAgentBuilder<M> {
    match tool {
        TeamTool::ListAgents => builder.tool(EventedTool::new(
            ListAgentsTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::SendMessage => builder.tool(EventedTool::new(
            SendMessageTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        _ => builder,
    }
}
