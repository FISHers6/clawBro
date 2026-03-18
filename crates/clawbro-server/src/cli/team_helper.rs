use crate::cli::args::TeamHelperArgs;
use crate::protocol::SessionKey;
use crate::runtime::{
    render_team_helper_success, TeamToolCall, TeamToolRequest, TeamToolResponse,
};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{Map, Value};

pub async fn run(args: TeamHelperArgs) -> Result<()> {
    let call = parse_call(args.command)?;
    let response = reqwest::Client::new()
        .post(&args.url)
        .json(&TeamToolRequest {
            session_key: SessionKey::new(args.session_channel, args.session_scope),
            call: call.clone(),
        })
        .send()
        .await
        .context("failed to invoke team tool endpoint")?;

    let status = response.status();
    let body: TeamToolResponse = response
        .json()
        .await
        .context("failed to decode team tool response")?;

    if !status.is_success() || !body.ok {
        bail!(
            "team tool call failed (status {}): {}",
            status.as_u16(),
            body.message
        );
    }

    let rendered = render_helper_output(&call, body);
    println!("{}", serde_json::to_string(&rendered)?);
    Ok(())
}

fn parse_call(mut args: Vec<String>) -> Result<TeamToolCall> {
    if args.is_empty() {
        bail!("missing team tool subcommand");
    }
    let subcommand = args.remove(0);
    match subcommand.as_str() {
        "create-task" => Ok(TeamToolCall::CreateTask {
            id: take_optional_flag(&mut args, "--id"),
            title: take_flag(&mut args, "--title")?,
            assignee: take_optional_flag(&mut args, "--assignee"),
            spec: take_optional_flag(&mut args, "--spec"),
            deps: take_csv_flag(&mut args, "--deps"),
            success_criteria: take_optional_flag(&mut args, "--success-criteria"),
        }),
        "start-execution" => Ok(TeamToolCall::StartExecution),
        "request-confirmation" => Ok(TeamToolCall::RequestConfirmation {
            plan_summary: take_flag(&mut args, "--plan-summary")?,
        }),
        "post-update" => Ok(TeamToolCall::PostUpdate {
            message: take_flag(&mut args, "--message")?,
        }),
        "get-task-status" => Ok(TeamToolCall::GetTaskStatus),
        "assign-task" => Ok(TeamToolCall::AssignTask {
            task_id: take_flag(&mut args, "--task-id")?,
            new_assignee: take_flag(&mut args, "--assignee")?,
        }),
        "accept-task" => Ok(TeamToolCall::AcceptTask {
            task_id: take_flag(&mut args, "--task-id")?,
            by: take_optional_flag(&mut args, "--by"),
        }),
        "reopen-task" => Ok(TeamToolCall::ReopenTask {
            task_id: take_flag(&mut args, "--task-id")?,
            reason: take_flag(&mut args, "--reason")?,
            by: take_optional_flag(&mut args, "--by"),
        }),
        "checkpoint-task" => Ok(TeamToolCall::CheckpointTask {
            task_id: take_flag(&mut args, "--task-id")?,
            note: take_flag(&mut args, "--note")?,
            agent: take_optional_flag(&mut args, "--agent"),
        }),
        "submit-task-result" => Ok(TeamToolCall::SubmitTaskResult {
            task_id: take_flag(&mut args, "--task-id")?,
            summary: take_flag(&mut args, "--summary")?,
            result_markdown: take_optional_flag(&mut args, "--result-markdown"),
            agent: take_optional_flag(&mut args, "--agent"),
        }),
        "complete-task" => Ok(TeamToolCall::CompleteTask {
            task_id: take_flag(&mut args, "--task-id")?,
            note: take_flag(&mut args, "--note")?,
            result_markdown: take_optional_flag(&mut args, "--result-markdown"),
            agent: take_optional_flag(&mut args, "--agent"),
        }),
        "block-task" => Ok(TeamToolCall::BlockTask {
            task_id: take_flag(&mut args, "--task-id")?,
            reason: take_flag(&mut args, "--reason")?,
            agent: take_optional_flag(&mut args, "--agent"),
        }),
        "request-help" => Ok(TeamToolCall::RequestHelp {
            task_id: take_flag(&mut args, "--task-id")?,
            message: take_flag(&mut args, "--message")?,
            agent: take_optional_flag(&mut args, "--agent"),
        }),
        other => bail!("unknown team tool subcommand: {other}"),
    }
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> Result<String> {
    take_optional_flag(args, flag).ok_or_else(|| anyhow!("{flag} is required"))
}

fn take_optional_flag(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == flag)?;
    args.remove(index);
    if index >= args.len() {
        return None;
    }
    Some(args.remove(index))
}

fn take_csv_flag(args: &mut Vec<String>, flag: &str) -> Vec<String> {
    take_optional_flag(args, flag)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn render_helper_output(call: &TeamToolCall, response: TeamToolResponse) -> Value {
    match call {
        TeamToolCall::CreateTask {
            id,
            title,
            assignee,
            ..
        } => render_team_helper_success(
            "create_task",
            Map::from_iter([
                (
                    "task_id".into(),
                    Value::String(id.clone().unwrap_or_default()),
                ),
                ("title".into(), Value::String(title.clone())),
                (
                    "assignee".into(),
                    assignee
                        .as_ref()
                        .map(|v| Value::String(v.clone()))
                        .unwrap_or(Value::Null),
                ),
                ("message".into(), Value::String(response.message)),
                ("payload".into(), response.payload.unwrap_or(Value::Null)),
            ]),
        ),
        TeamToolCall::AssignTask {
            task_id,
            new_assignee,
        } => render_team_helper_success(
            "assign_task",
            Map::from_iter([
                ("task_id".into(), Value::String(task_id.clone())),
                ("assignee".into(), Value::String(new_assignee.clone())),
                ("message".into(), Value::String(response.message)),
                ("payload".into(), response.payload.unwrap_or(Value::Null)),
            ]),
        ),
        TeamToolCall::StartExecution => render_team_helper_success(
            "start_execution",
            Map::from_iter([
                ("message".into(), Value::String(response.message)),
                ("payload".into(), response.payload.unwrap_or(Value::Null)),
            ]),
        ),
        TeamToolCall::AcceptTask { task_id, by } => render_team_helper_success(
            "accept_task",
            Map::from_iter([
                ("task_id".into(), Value::String(task_id.clone())),
                (
                    "by".into(),
                    by.as_ref()
                        .map(|v| Value::String(v.clone()))
                        .unwrap_or(Value::Null),
                ),
                ("message".into(), Value::String(response.message)),
                ("payload".into(), response.payload.unwrap_or(Value::Null)),
            ]),
        ),
        TeamToolCall::ReopenTask {
            task_id,
            reason,
            by,
        } => render_team_helper_success(
            "reopen_task",
            Map::from_iter([
                ("task_id".into(), Value::String(task_id.clone())),
                ("reason".into(), Value::String(reason.clone())),
                (
                    "by".into(),
                    by.as_ref()
                        .map(|v| Value::String(v.clone()))
                        .unwrap_or(Value::Null),
                ),
                ("message".into(), Value::String(response.message)),
                ("payload".into(), response.payload.unwrap_or(Value::Null)),
            ]),
        ),
        TeamToolCall::PostUpdate { message } => render_team_helper_success(
            "post_update",
            Map::from_iter([
                ("message".into(), Value::String(message.clone())),
                ("payload".into(), response.payload.unwrap_or(Value::Null)),
            ]),
        ),
        TeamToolCall::CheckpointTask { task_id, note, .. } => render_team_helper_success(
            "checkpoint_task",
            Map::from_iter([
                ("task_id".into(), Value::String(task_id.clone())),
                ("note".into(), Value::String(note.clone())),
                ("message".into(), Value::String(response.message)),
                ("payload".into(), response.payload.unwrap_or(Value::Null)),
            ]),
        ),
        TeamToolCall::SubmitTaskResult {
            task_id, summary, ..
        } => render_team_helper_success(
            "submit_task_result",
            Map::from_iter([
                ("task_id".into(), Value::String(task_id.clone())),
                ("summary".into(), Value::String(summary.clone())),
                ("message".into(), Value::String(response.message)),
                ("payload".into(), response.payload.unwrap_or(Value::Null)),
            ]),
        ),
        TeamToolCall::CompleteTask { task_id, note, .. } => render_team_helper_success(
            "complete_task",
            Map::from_iter([
                ("task_id".into(), Value::String(task_id.clone())),
                ("note".into(), Value::String(note.clone())),
                ("message".into(), Value::String(response.message)),
                ("payload".into(), response.payload.unwrap_or(Value::Null)),
            ]),
        ),
        TeamToolCall::BlockTask { task_id, reason, .. } => render_team_helper_success(
            "block_task",
            Map::from_iter([
                ("task_id".into(), Value::String(task_id.clone())),
                ("reason".into(), Value::String(reason.clone())),
                ("message".into(), Value::String(response.message)),
                ("payload".into(), response.payload.unwrap_or(Value::Null)),
            ]),
        ),
        TeamToolCall::RequestHelp {
            task_id, message, ..
        } => render_team_helper_success(
            "request_help",
            Map::from_iter([
                ("task_id".into(), Value::String(task_id.clone())),
                ("message".into(), Value::String(message.clone())),
                ("payload".into(), response.payload.unwrap_or(Value::Null)),
            ]),
        ),
        _ => render_team_helper_success(
            "team_tool",
            Map::from_iter([
                ("message".into(), Value::String(response.message)),
                ("payload".into(), response.payload.unwrap_or(Value::Null)),
            ]),
        ),
    }
}
