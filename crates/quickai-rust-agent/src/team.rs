use qai_protocol::{parse_session_key_text, TeamToolCall, TeamToolRequest, TeamToolResponse};
use quickai_agent_sdk::{
    bridge::{AgentTurnRequest, ApprovalMode, ExecutionRole},
    tools::{EventedTool, RuntimeToolAugmentor, ToolProgressTracker},
};
use rig::{
    agent::AgentBuilder,
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
pub struct QuickAiTeamToolAugmentor {
    endpoint: Option<String>,
}

impl QuickAiTeamToolAugmentor {
    pub fn from_endpoint(endpoint: Option<String>) -> Self {
        Self { endpoint }
    }

    pub fn from_env() -> Self {
        Self::from_endpoint(std::env::var("QUICKAI_TEAM_TOOL_URL").ok())
    }
}

impl RuntimeToolAugmentor for QuickAiTeamToolAugmentor {
    fn augment<M: CompletionModel>(
        &self,
        builder: AgentBuilder<M>,
        session: &AgentTurnRequest,
        tracker: Option<ToolProgressTracker>,
        approval_mode: ApprovalMode,
    ) -> AgentBuilder<M> {
        if !session.tool_surface.team_tools {
            return builder;
        }
        let Some(endpoint) = self.endpoint.clone() else {
            return builder;
        };
        let client = TeamToolClient::new(endpoint, session.session_ref.clone());
        match tracker {
            Some(tracker) => register_team_tools_with_progress(
                builder,
                session.role,
                client,
                tracker,
                approval_mode,
            ),
            None => register_team_tools(builder, session.role, client, approval_mode),
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
    "Lead only. Publish a user-visible progress update.",
    json!({
        "type": "object",
        "properties": {
            "message": {"type": "string", "description": "Progress update message visible to the user."}
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
    "Lead only. Get current task status as JSON.",
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
    "Specialist only. Report a checkpoint without changing task state.",
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
    "Specialist only. Submit results for lead review.",
    json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string"},
            "summary": {"type": "string"}
        },
        "required": ["task_id", "summary"]
    }),
    |args: SubmitTaskResultArgs| TeamToolCall::SubmitTaskResult {
        task_id: args.task_id,
        summary: args.summary,
        result_markdown: None,
        agent: None,
    }
);
define_team_tool!(
    AcceptTaskTool,
    "accept_task",
    AcceptTaskArgs,
    "Lead only. Accept a submitted task result.",
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
    "Lead only. Reopen a task with a reason.",
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
    "Specialist only. Report a blocking reason and release the task.",
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
    "Specialist only. Ask the lead for help while keeping the task claimed.",
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

fn register_team_tools<M: CompletionModel>(
    mut builder: AgentBuilder<M>,
    role: ExecutionRole,
    client: TeamToolClient,
    approval_mode: ApprovalMode,
) -> AgentBuilder<M> {
    match role {
        ExecutionRole::Solo => builder,
        ExecutionRole::Leader => {
            builder = builder
                .tool(EventedTool::new(
                    CreateTaskTool::new(client.clone()),
                    None,
                    approval_mode,
                ))
                .tool(EventedTool::new(
                    StartExecutionTool::new(client.clone()),
                    None,
                    approval_mode,
                ))
                .tool(EventedTool::new(
                    RequestConfirmationTool::new(client.clone()),
                    None,
                    approval_mode,
                ))
                .tool(EventedTool::new(
                    PostUpdateTool::new(client.clone()),
                    None,
                    approval_mode,
                ))
                .tool(EventedTool::new(
                    GetTaskStatusTool::new(client.clone()),
                    None,
                    approval_mode,
                ))
                .tool(EventedTool::new(
                    AssignTaskTool::new(client.clone()),
                    None,
                    approval_mode,
                ))
                .tool(EventedTool::new(
                    AcceptTaskTool::new(client.clone()),
                    None,
                    approval_mode,
                ))
                .tool(EventedTool::new(
                    ReopenTaskTool::new(client),
                    None,
                    approval_mode,
                ));
            builder
        }
        ExecutionRole::Specialist => builder
            .tool(EventedTool::new(
                CheckpointTaskTool::new(client.clone()),
                None,
                approval_mode,
            ))
            .tool(EventedTool::new(
                SubmitTaskResultTool::new(client.clone()),
                None,
                approval_mode,
            ))
            .tool(EventedTool::new(
                BlockTaskTool::new(client.clone()),
                None,
                approval_mode,
            ))
            .tool(EventedTool::new(
                RequestHelpTool::new(client),
                None,
                approval_mode,
            )),
    }
}

fn register_team_tools_with_progress<M: CompletionModel>(
    mut builder: AgentBuilder<M>,
    role: ExecutionRole,
    client: TeamToolClient,
    tracker: ToolProgressTracker,
    approval_mode: ApprovalMode,
) -> AgentBuilder<M> {
    match role {
        ExecutionRole::Solo => builder,
        ExecutionRole::Leader => {
            builder = builder
                .tool(EventedTool::new(
                    CreateTaskTool::new(client.clone()),
                    Some(tracker.clone()),
                    approval_mode,
                ))
                .tool(EventedTool::new(
                    StartExecutionTool::new(client.clone()),
                    Some(tracker.clone()),
                    approval_mode,
                ))
                .tool(EventedTool::new(
                    RequestConfirmationTool::new(client.clone()),
                    Some(tracker.clone()),
                    approval_mode,
                ))
                .tool(EventedTool::new(
                    PostUpdateTool::new(client.clone()),
                    Some(tracker.clone()),
                    approval_mode,
                ))
                .tool(EventedTool::new(
                    GetTaskStatusTool::new(client.clone()),
                    Some(tracker.clone()),
                    approval_mode,
                ))
                .tool(EventedTool::new(
                    AssignTaskTool::new(client.clone()),
                    Some(tracker.clone()),
                    approval_mode,
                ))
                .tool(EventedTool::new(
                    AcceptTaskTool::new(client.clone()),
                    Some(tracker.clone()),
                    approval_mode,
                ))
                .tool(EventedTool::new(
                    ReopenTaskTool::new(client),
                    Some(tracker),
                    approval_mode,
                ));
            builder
        }
        ExecutionRole::Specialist => builder
            .tool(EventedTool::new(
                CheckpointTaskTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            ))
            .tool(EventedTool::new(
                SubmitTaskResultTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            ))
            .tool(EventedTool::new(
                BlockTaskTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            ))
            .tool(EventedTool::new(
                RequestHelpTool::new(client),
                Some(tracker),
                approval_mode,
            )),
    }
}
