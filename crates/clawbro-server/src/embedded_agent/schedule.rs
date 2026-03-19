use crate::agent_sdk_internal::{
    bridge::{AgentTurnRequest, ApprovalMode, ExecutionRole},
    tools::{ConfiguredAgentBuilder, EventedTool, RuntimeToolAugmentor, ToolProgressTracker},
};
use crate::protocol::ScheduleTool;
use rig::{
    completion::{CompletionModel, ToolDefinition},
    tool::{Tool, ToolError},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::process::Command;

#[derive(Debug, Clone)]
struct ScheduleToolClient {
    command: String,
    default_session_ref: String,
}

impl ScheduleToolClient {
    fn new(command: String, default_session_ref: String) -> Self {
        Self {
            command,
            default_session_ref,
        }
    }

    async fn invoke(
        &self,
        tool_name: &'static str,
        args: Vec<String>,
    ) -> Result<ScheduleToolOutput, ToolError> {
        let args = normalize_current_session_args(args, &self.default_session_ref);
        let output = Command::new(&self.command)
            .args(args)
            .output()
            .await
            .map_err(|e| {
                ToolError::ToolCallError(format!("failed to invoke clawbro schedule: {e}").into())
            })?;
        let stdout = String::from_utf8(output.stdout).map_err(|e| {
            ToolError::ToolCallError(format!("schedule stdout was not utf-8: {e}").into())
        })?;
        let stderr = String::from_utf8(output.stderr).unwrap_or_default();
        let body: Value = serde_json::from_str(stdout.trim()).map_err(|e| {
            tracing::warn!(
                tool = tool_name,
                stdout = stdout.trim(),
                stderr = stderr.trim(),
                error = %e,
                "schedule tool returned non-json output"
            );
            ToolError::ToolCallError(
                format!(
                    "schedule json decode failed: {e}; stdout='{}' stderr='{}'",
                    stdout.trim(),
                    stderr.trim()
                )
                .into(),
            )
        })?;
        let ok = body.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if !output.status.success() || !ok {
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("schedule command failed");
            tracing::warn!(
                tool = tool_name,
                status = output.status.code().unwrap_or(-1),
                stderr = stderr.trim(),
                body = %body,
                "schedule tool command failed"
            );
            return Err(ToolError::ToolCallError(message.to_string().into()));
        }
        let payload = body.get("data").cloned();
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("scheduler command completed")
            .to_string();

        Ok(ScheduleToolOutput { message, payload })
    }
}

fn normalize_current_session_args(
    args: Vec<String>,
    default_session_ref: &str,
) -> Vec<String> {
    let mut normalized = Vec::with_capacity(args.len() + 2);
    let mut index = 0usize;
    while index < args.len() {
        normalized.push(args[index].clone());
        if args[index] == "--current-session-key" {
            let next_is_value = args
                .get(index + 1)
                .is_some_and(|next| !next.starts_with("--"));
            if !next_is_value {
                normalized.push(default_session_ref.to_string());
            }
        }
        index += 1;
    }
    normalized
}

#[derive(Debug, Serialize)]
struct ScheduleToolOutput {
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    payload: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct ClawBroScheduleToolAugmentor {
    command: Option<String>,
}

impl ClawBroScheduleToolAugmentor {
    pub fn from_command(command: Option<String>) -> Self {
        Self { command }
    }

    pub fn from_env() -> Self {
        let command = std::env::var("CLAWBRO_SCHEDULE_COMMAND").ok().or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|path| path.display().to_string())
        });
        Self::from_command(command)
    }
}

impl RuntimeToolAugmentor for ClawBroScheduleToolAugmentor {
    fn augment<M: CompletionModel>(
        &self,
        builder: ConfiguredAgentBuilder<M>,
        session: &AgentTurnRequest,
        tracker: Option<ToolProgressTracker>,
        approval_mode: ApprovalMode,
    ) -> ConfiguredAgentBuilder<M> {
        if !session.tool_surface.schedule_tools {
            return builder;
        }
        let Some(command) = self.command.clone() else {
            return builder;
        };
        let client = ScheduleToolClient::new(command, session.session_ref.clone());
        match tracker {
            Some(tracker) => register_schedule_tools_with_progress(
                builder,
                session.role,
                &session.tool_surface.allowed_schedule_tools,
                client,
                tracker,
                approval_mode,
            ),
            None => register_schedule_tools(
                builder,
                session.role,
                &session.tool_surface.allowed_schedule_tools,
                client,
                approval_mode,
            ),
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreateScheduleArgs {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    target_kind: Option<String>,
    #[serde(default)]
    expr: Option<String>,
    #[serde(default)]
    run_at: Option<String>,
    #[serde(default)]
    every: Option<String>,
    #[serde(default)]
    delay: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    target_session_key: Option<String>,
    prompt: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    idle_gt_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct DelayReminderArgs {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    delay: Option<String>,
    #[serde(default)]
    target_session_key: Option<String>,
    message: String,
}

#[derive(Debug, Deserialize)]
struct AtReminderArgs {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    run_at: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    target_session_key: Option<String>,
    message: String,
}

#[derive(Debug, Deserialize)]
struct EveryReminderArgs {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    every: Option<String>,
    #[serde(default)]
    target_session_key: Option<String>,
    message: String,
}

#[derive(Debug, Deserialize)]
struct CronReminderArgs {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    expr: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    target_session_key: Option<String>,
    message: String,
}

#[derive(Debug, Deserialize)]
struct DelayAgentScheduleArgs {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    delay: Option<String>,
    #[serde(default)]
    target_session_key: Option<String>,
    task_prompt: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    idle_gt_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct AtAgentScheduleArgs {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    run_at: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    target_session_key: Option<String>,
    task_prompt: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    idle_gt_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct EveryAgentScheduleArgs {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    every: Option<String>,
    #[serde(default)]
    target_session_key: Option<String>,
    task_prompt: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    idle_gt_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct CronAgentScheduleArgs {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    expr: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    target_session_key: Option<String>,
    task_prompt: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    idle_gt_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct JobRefArgs {
    job_id: String,
}

#[derive(Debug, Deserialize)]
struct NameRefArgs {
    name: String,
}

#[derive(Debug, Deserialize)]
struct HistoryArgs {
    #[serde(default)]
    job_id: Option<String>,
}

macro_rules! define_schedule_tool {
    ($tool_name:ident, $const_name:literal, $args_ty:ty, $desc:expr, $schema:expr, $call_builder:expr) => {
        #[derive(Debug, Clone)]
        struct $tool_name {
            client: ScheduleToolClient,
        }

        impl $tool_name {
            fn new(client: ScheduleToolClient) -> Self {
                Self { client }
            }
        }

        impl Tool for $tool_name {
            const NAME: &'static str = $const_name;
            type Error = ToolError;
            type Args = $args_ty;
            type Output = ScheduleToolOutput;

            async fn definition(&self, _prompt: String) -> ToolDefinition {
                ToolDefinition {
                    name: Self::NAME.to_string(),
                    description: $desc.to_string(),
                    parameters: $schema,
                }
            }

            async fn call(&self, args: $args_ty) -> Result<ScheduleToolOutput, ToolError> {
                self.client.invoke(Self::NAME, ($call_builder)(args)?).await
            }
        }
    };
}

macro_rules! define_create_schedule_tool {
    ($tool_name:ident, $const_name:literal, $args_ty:ty, $desc:expr, $schema:expr, $builder:expr) => {
        #[derive(Debug, Clone)]
        struct $tool_name {
            client: ScheduleToolClient,
        }

        impl $tool_name {
            fn new(client: ScheduleToolClient) -> Self {
                Self { client }
            }
        }

        impl Tool for $tool_name {
            const NAME: &'static str = $const_name;
            type Error = ToolError;
            type Args = $args_ty;
            type Output = ScheduleToolOutput;

            async fn definition(&self, _prompt: String) -> ToolDefinition {
                ToolDefinition {
                    name: Self::NAME.to_string(),
                    description: $desc.to_string(),
                    parameters: $schema,
                }
            }

            async fn call(&self, args: $args_ty) -> Result<ScheduleToolOutput, ToolError> {
                let schedule_args = ($builder)(args);
                self.client
                    .invoke(
                        Self::NAME,
                        build_create_schedule_call(
                            Self::NAME,
                            schedule_args,
                            &self.client.default_session_ref,
                        )?,
                    )
                    .await
            }
        }
    };
}

define_create_schedule_tool!(
    CreateDelayReminderTool,
    "create_delay_reminder",
    DelayReminderArgs,
    "Create a one-shot reminder delivered directly after a delay.",
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "delay": {"type": "string", "description": "Required. Duration like 30s, 5m, 2h, 1d, 500ms."},
            "target_session_key": {"type": "string", "description": "Optional. Omit to remind in the current conversation."},
            "message": {"type": "string", "description": "The exact reminder text to deliver at trigger time."}
        },
        "required": ["delay", "message"]
    }),
    |args: DelayReminderArgs| CreateScheduleArgs {
        name: args.name,
        target_kind: Some("delivery-message".into()),
        expr: None,
        run_at: None,
        every: None,
        delay: args.delay,
        timezone: None,
        target_session_key: args.target_session_key,
        prompt: args.message,
        agent: None,
        idle_gt_seconds: None,
    }
);

define_create_schedule_tool!(
    CreateAtReminderTool,
    "create_at_reminder",
    AtReminderArgs,
    "Create a one-shot reminder delivered directly at an exact time.",
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "run_at": {"type": "string", "description": "Required. RFC3339 timestamp in UTC or with explicit offset."},
            "timezone": {"type": "string"},
            "target_session_key": {"type": "string", "description": "Optional. Omit to remind in the current conversation."},
            "message": {"type": "string", "description": "The exact reminder text to deliver at trigger time."}
        },
        "required": ["run_at", "message"]
    }),
    |args: AtReminderArgs| CreateScheduleArgs {
        name: args.name,
        target_kind: Some("delivery-message".into()),
        expr: None,
        run_at: args.run_at,
        every: None,
        delay: None,
        timezone: args.timezone,
        target_session_key: args.target_session_key,
        prompt: args.message,
        agent: None,
        idle_gt_seconds: None,
    }
);

define_create_schedule_tool!(
    CreateEveryReminderTool,
    "create_every_reminder",
    EveryReminderArgs,
    "Create a recurring reminder delivered directly at a fixed interval.",
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "every": {"type": "string", "description": "Required. Duration like 30s, 5m, 2h, 1d, 500ms."},
            "target_session_key": {"type": "string", "description": "Optional. Omit to remind in the current conversation."},
            "message": {"type": "string", "description": "The exact reminder text to deliver at each trigger."}
        },
        "required": ["every", "message"]
    }),
    |args: EveryReminderArgs| CreateScheduleArgs {
        name: args.name,
        target_kind: Some("delivery-message".into()),
        expr: None,
        run_at: None,
        every: args.every,
        delay: None,
        timezone: None,
        target_session_key: args.target_session_key,
        prompt: args.message,
        agent: None,
        idle_gt_seconds: None,
    }
);

define_create_schedule_tool!(
    CreateCronReminderTool,
    "create_cron_reminder",
    CronReminderArgs,
    "Create a cron-based recurring reminder delivered directly.",
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "expr": {"type": "string", "description": "Required. Cron expression."},
            "timezone": {"type": "string"},
            "target_session_key": {"type": "string", "description": "Optional. Omit to remind in the current conversation."},
            "message": {"type": "string", "description": "The exact reminder text to deliver at each trigger."}
        },
        "required": ["expr", "message"]
    }),
    |args: CronReminderArgs| CreateScheduleArgs {
        name: args.name,
        target_kind: Some("delivery-message".into()),
        expr: args.expr,
        run_at: None,
        every: None,
        delay: None,
        timezone: args.timezone,
        target_session_key: args.target_session_key,
        prompt: args.message,
        agent: None,
        idle_gt_seconds: None,
    }
);

define_create_schedule_tool!(
    CreateDelayAgentScheduleTool,
    "create_delay_agent_schedule",
    DelayAgentScheduleArgs,
    "Create a one-shot scheduled agent task after a delay.",
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "delay": {"type": "string", "description": "Required. Duration like 30s, 5m, 2h, 1d, 500ms."},
            "target_session_key": {"type": "string", "description": "Optional. Omit to send the result to the current conversation."},
            "task_prompt": {"type": "string", "description": "The work the agent should perform when the schedule triggers."},
            "agent": {"type": "string"},
            "idle_gt_seconds": {"type": "integer", "minimum": 1}
        },
        "required": ["delay", "task_prompt"]
    }),
    |args: DelayAgentScheduleArgs| CreateScheduleArgs {
        name: args.name,
        target_kind: Some("agent-turn".into()),
        expr: None,
        run_at: None,
        every: None,
        delay: args.delay,
        timezone: None,
        target_session_key: args.target_session_key,
        prompt: args.task_prompt,
        agent: args.agent,
        idle_gt_seconds: args.idle_gt_seconds,
    }
);

define_create_schedule_tool!(
    CreateAtAgentScheduleTool,
    "create_at_agent_schedule",
    AtAgentScheduleArgs,
    "Create a one-shot scheduled agent task at an exact time.",
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "run_at": {"type": "string", "description": "Required. RFC3339 timestamp in UTC or with explicit offset."},
            "timezone": {"type": "string"},
            "target_session_key": {"type": "string", "description": "Optional. Omit to send the result to the current conversation."},
            "task_prompt": {"type": "string", "description": "The work the agent should perform when the schedule triggers."},
            "agent": {"type": "string"},
            "idle_gt_seconds": {"type": "integer", "minimum": 1}
        },
        "required": ["run_at", "task_prompt"]
    }),
    |args: AtAgentScheduleArgs| CreateScheduleArgs {
        name: args.name,
        target_kind: Some("agent-turn".into()),
        expr: None,
        run_at: args.run_at,
        every: None,
        delay: None,
        timezone: args.timezone,
        target_session_key: args.target_session_key,
        prompt: args.task_prompt,
        agent: args.agent,
        idle_gt_seconds: args.idle_gt_seconds,
    }
);

define_create_schedule_tool!(
    CreateEveryAgentScheduleTool,
    "create_every_agent_schedule",
    EveryAgentScheduleArgs,
    "Create a recurring scheduled agent task at a fixed interval.",
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "every": {"type": "string", "description": "Required. Duration like 30s, 5m, 2h, 1d, 500ms."},
            "target_session_key": {"type": "string", "description": "Optional. Omit to send the result to the current conversation."},
            "task_prompt": {"type": "string", "description": "The work the agent should perform when the schedule triggers."},
            "agent": {"type": "string"},
            "idle_gt_seconds": {"type": "integer", "minimum": 1}
        },
        "required": ["every", "task_prompt"]
    }),
    |args: EveryAgentScheduleArgs| CreateScheduleArgs {
        name: args.name,
        target_kind: Some("agent-turn".into()),
        expr: None,
        run_at: None,
        every: args.every,
        delay: None,
        timezone: None,
        target_session_key: args.target_session_key,
        prompt: args.task_prompt,
        agent: args.agent,
        idle_gt_seconds: args.idle_gt_seconds,
    }
);

define_create_schedule_tool!(
    CreateCronAgentScheduleTool,
    "create_cron_agent_schedule",
    CronAgentScheduleArgs,
    "Create a cron-based recurring scheduled agent task.",
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "expr": {"type": "string", "description": "Required. Cron expression."},
            "timezone": {"type": "string"},
            "target_session_key": {"type": "string", "description": "Optional. Omit to send the result to the current conversation."},
            "task_prompt": {"type": "string", "description": "The work the agent should perform when the schedule triggers."},
            "agent": {"type": "string"},
            "idle_gt_seconds": {"type": "integer", "minimum": 1}
        },
        "required": ["expr", "task_prompt"]
    }),
    |args: CronAgentScheduleArgs| CreateScheduleArgs {
        name: args.name,
        target_kind: Some("agent-turn".into()),
        expr: args.expr,
        run_at: None,
        every: None,
        delay: None,
        timezone: args.timezone,
        target_session_key: args.target_session_key,
        prompt: args.task_prompt,
        agent: args.agent,
        idle_gt_seconds: args.idle_gt_seconds,
    }
);

define_schedule_tool!(
    ListSchedulesTool,
    "list_schedules",
    serde_json::Value,
    "List durable scheduler jobs visible to this session.",
    json!({"type": "object", "properties": {}}),
    |_args: serde_json::Value| list_schedules_call()
);

define_schedule_tool!(
    ListCurrentSessionSchedulesTool,
    "list_current_session_schedules",
    serde_json::Value,
    "List scheduler jobs for the current conversation only.",
    json!({"type": "object", "properties": {}}),
    |_args: serde_json::Value| list_current_session_schedules_call()
);

define_schedule_tool!(
    PauseScheduleTool,
    "pause_schedule",
    JobRefArgs,
    "Pause a scheduler job by id.",
    json!({
        "type": "object",
        "properties": {"job_id": {"type": "string"}},
        "required": ["job_id"]
    }),
    |args: JobRefArgs| pause_schedule_call(args)
);

define_schedule_tool!(
    ResumeScheduleTool,
    "resume_schedule",
    JobRefArgs,
    "Resume a paused scheduler job by id.",
    json!({
        "type": "object",
        "properties": {"job_id": {"type": "string"}},
        "required": ["job_id"]
    }),
    |args: JobRefArgs| resume_schedule_call(args)
);

define_schedule_tool!(
    DeleteScheduleTool,
    "delete_schedule",
    JobRefArgs,
    "Delete a scheduler job by id.",
    json!({
        "type": "object",
        "properties": {"job_id": {"type": "string"}},
        "required": ["job_id"]
    }),
    |args: JobRefArgs| delete_schedule_call(args)
);

define_schedule_tool!(
    DeleteScheduleByNameTool,
    "delete_schedule_by_name",
    NameRefArgs,
    "Delete scheduler jobs in the current conversation by exact name.",
    json!({
        "type": "object",
        "properties": {"name": {"type": "string"}},
        "required": ["name"]
    }),
    |args: NameRefArgs| delete_schedule_by_name_call(args)
);

define_schedule_tool!(
    ClearCurrentSessionSchedulesTool,
    "clear_current_session_schedules",
    serde_json::Value,
    "Delete all scheduler jobs for the current conversation.",
    json!({"type": "object", "properties": {}}),
    |_args: serde_json::Value| clear_current_session_schedules_call()
);

define_schedule_tool!(
    RunScheduleNowTool,
    "run_schedule_now",
    JobRefArgs,
    "Request immediate execution for a scheduler job by id.",
    json!({
        "type": "object",
        "properties": {"job_id": {"type": "string"}},
        "required": ["job_id"]
    }),
    |args: JobRefArgs| run_schedule_now_call(args)
);

define_schedule_tool!(
    ScheduleHistoryTool,
    "schedule_history",
    HistoryArgs,
    "Show scheduler run history, optionally filtered to one job id.",
    json!({
        "type": "object",
        "properties": {"job_id": {"type": "string"}}
    }),
    |args: HistoryArgs| schedule_history_call(args)
);

fn build_create_schedule_call(
    tool_name: &'static str,
    args: CreateScheduleArgs,
    default_session_ref: &str,
) -> Result<Vec<String>, ToolError> {
    let mut command = vec!["schedule".to_string(), "--json".to_string()];
    if let Some(expr) = args.expr {
            command.push("add-cron".into());
            command.push("--expr".into());
            command.push(expr);
    } else if let Some(run_at) = args.run_at {
            command.push("add-at".into());
            command.push("--at".into());
            command.push(run_at);
    } else if let Some(every) = args.every {
            command.push("add-every".into());
            command.push("--every".into());
            command.push(every);
    } else if let Some(delay) = args.delay {
            command.push("add-delay".into());
            command.push("--delay".into());
            command.push(delay);
    } else {
        return Err(ToolError::ToolCallError(
            "missing schedule time field for create schedule call".into(),
        ));
    }
    command.push("--name".into());
    command.push(resolve_job_name(args.name, tool_name, &args.prompt));
    if let Some(target_kind) = args.target_kind {
        command.push("--target-kind".into());
        command.push(target_kind);
    }
    if let Some(target_session_key) = args.target_session_key {
        command.push("--session-key".into());
        command.push(target_session_key);
    } else {
        command.push("--current-session-key".into());
        command.push(default_session_ref.to_string());
    }
    command.push("--prompt".into());
    command.push(args.prompt);
    if let Some(timezone) = args.timezone {
        command.push("--timezone".into());
        command.push(timezone);
    }
    if let Some(agent) = args.agent {
        command.push("--agent".into());
        command.push(agent);
    }
    if let Some(threshold) = args.idle_gt_seconds {
        command.push("--idle-gt-seconds".into());
        command.push(threshold.to_string());
    }
    Ok(command)
}

fn resolve_job_name(name: Option<String>, tool_name: &str, content: &str) -> String {
    if let Some(name) = name.filter(|v| !v.trim().is_empty()) {
        return name;
    }
    let prefix = match tool_name {
        "create_delay_reminder"
        | "create_at_reminder"
        | "create_every_reminder"
        | "create_cron_reminder" => "reminder",
        "create_delay_agent_schedule"
        | "create_at_agent_schedule"
        | "create_every_agent_schedule"
        | "create_cron_agent_schedule" => "agent-task",
        _ => "schedule",
    };
    let snippet: String = content
        .trim()
        .chars()
        .take(12)
        .collect::<String>()
        .replace(char::is_whitespace, "");
    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    if snippet.is_empty() {
        format!("{prefix}-{epoch_ms}")
    } else {
        format!("{prefix}-{snippet}-{epoch_ms}")
    }
}

fn list_schedules_call() -> Result<Vec<String>, ToolError> {
    Ok(vec!["schedule".into(), "--json".into(), "list".into()])
}

fn list_current_session_schedules_call() -> Result<Vec<String>, ToolError> {
    Ok(vec![
        "schedule".into(),
        "--json".into(),
        "list".into(),
        "--current-session-key".into(),
    ])
}

fn pause_schedule_call(args: JobRefArgs) -> Result<Vec<String>, ToolError> {
    Ok(vec![
        "schedule".into(),
        "--json".into(),
        "pause".into(),
        "--job-id".into(),
        args.job_id,
    ])
}

fn resume_schedule_call(args: JobRefArgs) -> Result<Vec<String>, ToolError> {
    Ok(vec![
        "schedule".into(),
        "--json".into(),
        "resume".into(),
        "--job-id".into(),
        args.job_id,
    ])
}

fn delete_schedule_call(args: JobRefArgs) -> Result<Vec<String>, ToolError> {
    Ok(vec![
        "schedule".into(),
        "--json".into(),
        "delete".into(),
        "--job-id".into(),
        args.job_id,
    ])
}

fn delete_schedule_by_name_call(args: NameRefArgs) -> Result<Vec<String>, ToolError> {
    Ok(vec![
        "schedule".into(),
        "--json".into(),
        "delete".into(),
        "--name".into(),
        args.name,
        "--current-session-key".into(),
    ])
}

fn clear_current_session_schedules_call() -> Result<Vec<String>, ToolError> {
    Ok(vec![
        "schedule".into(),
        "--json".into(),
        "delete-all".into(),
        "--current-session-key".into(),
    ])
}

fn run_schedule_now_call(args: JobRefArgs) -> Result<Vec<String>, ToolError> {
    Ok(vec![
        "schedule".into(),
        "--json".into(),
        "run-now".into(),
        "--job-id".into(),
        args.job_id,
    ])
}

fn schedule_history_call(args: HistoryArgs) -> Result<Vec<String>, ToolError> {
    let mut command = vec!["schedule".into(), "--json".into(), "history".into()];
    if let Some(job_id) = args.job_id {
        command.push("--job-id".into());
        command.push(job_id);
    }
    Ok(command)
}

fn register_schedule_tools<M: CompletionModel>(
    builder: ConfiguredAgentBuilder<M>,
    role: ExecutionRole,
    allowed_schedule_tools: &[ScheduleTool],
    client: ScheduleToolClient,
    approval_mode: ApprovalMode,
) -> ConfiguredAgentBuilder<M> {
    register_schedule_tools_with_progress(
        builder,
        role,
        allowed_schedule_tools,
        client,
        ToolProgressTracker::new(std::sync::Arc::new(|_| {})),
        approval_mode,
    )
}

fn register_schedule_tools_with_progress<M: CompletionModel>(
    builder: ConfiguredAgentBuilder<M>,
    role: ExecutionRole,
    allowed_schedule_tools: &[ScheduleTool],
    client: ScheduleToolClient,
    tracker: ToolProgressTracker,
    approval_mode: ApprovalMode,
) -> ConfiguredAgentBuilder<M> {
    let mut builder = builder;
    for tool in visible_schedule_tools_for_request(role, allowed_schedule_tools) {
        builder = match tool {
            ScheduleTool::CreateDelayReminder => builder.tool(EventedTool::new(
                CreateDelayReminderTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::CreateAtReminder => builder.tool(EventedTool::new(
                CreateAtReminderTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::CreateEveryReminder => builder.tool(EventedTool::new(
                CreateEveryReminderTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::CreateCronReminder => builder.tool(EventedTool::new(
                CreateCronReminderTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::CreateDelayAgentSchedule => builder.tool(EventedTool::new(
                CreateDelayAgentScheduleTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::CreateAtAgentSchedule => builder.tool(EventedTool::new(
                CreateAtAgentScheduleTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::CreateEveryAgentSchedule => builder.tool(EventedTool::new(
                CreateEveryAgentScheduleTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::CreateCronAgentSchedule => builder.tool(EventedTool::new(
                CreateCronAgentScheduleTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::ListSchedules => builder.tool(EventedTool::new(
                ListSchedulesTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::ListCurrentSessionSchedules => builder.tool(EventedTool::new(
                ListCurrentSessionSchedulesTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::PauseSchedule => builder.tool(EventedTool::new(
                PauseScheduleTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::ResumeSchedule => builder.tool(EventedTool::new(
                ResumeScheduleTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::DeleteSchedule => builder.tool(EventedTool::new(
                DeleteScheduleTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::DeleteScheduleByName => builder.tool(EventedTool::new(
                DeleteScheduleByNameTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::ClearCurrentSessionSchedules => builder.tool(EventedTool::new(
                ClearCurrentSessionSchedulesTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::RunScheduleNow => builder.tool(EventedTool::new(
                RunScheduleNowTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
            ScheduleTool::ScheduleHistory => builder.tool(EventedTool::new(
                ScheduleHistoryTool::new(client.clone()),
                Some(tracker.clone()),
                approval_mode,
            )),
        };
    }

    builder
}

fn visible_schedule_tools_for_request(
    role: ExecutionRole,
    allowed_schedule_tools: &[ScheduleTool],
) -> Vec<ScheduleTool> {
    let default_tools = match role {
        ExecutionRole::Solo | ExecutionRole::Leader => vec![
            ScheduleTool::CreateDelayReminder,
            ScheduleTool::CreateAtReminder,
            ScheduleTool::CreateEveryReminder,
            ScheduleTool::CreateCronReminder,
            ScheduleTool::CreateDelayAgentSchedule,
            ScheduleTool::CreateAtAgentSchedule,
            ScheduleTool::CreateEveryAgentSchedule,
            ScheduleTool::CreateCronAgentSchedule,
            ScheduleTool::ListSchedules,
            ScheduleTool::ListCurrentSessionSchedules,
            ScheduleTool::PauseSchedule,
            ScheduleTool::ResumeSchedule,
            ScheduleTool::DeleteSchedule,
            ScheduleTool::DeleteScheduleByName,
            ScheduleTool::ClearCurrentSessionSchedules,
            ScheduleTool::RunScheduleNow,
            ScheduleTool::ScheduleHistory,
        ],
        ExecutionRole::Specialist => vec![],
    };
    if allowed_schedule_tools.is_empty() {
        return default_tools;
    }
    default_tools
        .into_iter()
        .filter(|tool| allowed_schedule_tools.contains(tool))
        .collect()
}
