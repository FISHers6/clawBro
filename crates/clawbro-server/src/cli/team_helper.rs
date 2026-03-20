use crate::cli::args::TeamHelperArgs;
use crate::protocol::{render_session_key_text, SessionKey};
use crate::runtime::{
    render_team_helper_failure, render_team_helper_success, TeamToolCall, TeamToolRequest,
    TeamToolResponse,
};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{Map, Value};
use tokio::process::Command;

pub async fn run(args: TeamHelperArgs) -> Result<()> {
    let call = parse_call(args.command)?;
    let session_key = SessionKey::new(args.session_channel, args.session_scope);
    let rendered = match call {
        HelperCall::Team(call) => {
            let url = args
                .url
                .as_ref()
                .context("--url is required for team helper subcommands")?;
            let response = reqwest::Client::new()
                .post(url)
                .json(&TeamToolRequest {
                    session_key: session_key.clone(),
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
            render_team_helper_output(&call, body)
        }
        HelperCall::Schedule { action, command } => {
            let command = normalize_schedule_helper_command(command, &action, &session_key);
            match invoke_schedule_command(&command).await {
                Ok(body) => render_schedule_helper_output(&action, body),
                Err(err) => render_team_helper_failure(&action, None, &err.to_string()),
            }
        }
    };
    println!("{}", serde_json::to_string(&rendered)?);
    Ok(())
}

#[derive(Clone)]
enum HelperCall {
    Team(TeamToolCall),
    Schedule {
        action: String,
        command: Vec<String>,
    },
}

fn parse_call(mut args: Vec<String>) -> Result<HelperCall> {
    if args.is_empty() {
        bail!("missing team tool subcommand");
    }
    let subcommand = args.remove(0);
    match subcommand.as_str() {
        "create-task" => Ok(HelperCall::Team(TeamToolCall::CreateTask {
            id: take_optional_flag(&mut args, "--id"),
            title: take_flag(&mut args, "--title")?,
            assignee: take_optional_flag(&mut args, "--assignee"),
            spec: take_optional_flag(&mut args, "--spec"),
            deps: take_csv_flag(&mut args, "--deps"),
            success_criteria: take_optional_flag(&mut args, "--success-criteria"),
        })),
        "start-execution" => Ok(HelperCall::Team(TeamToolCall::StartExecution)),
        "request-confirmation" => Ok(HelperCall::Team(TeamToolCall::RequestConfirmation {
            plan_summary: take_flag(&mut args, "--plan-summary")?,
        })),
        "post-update" => Ok(HelperCall::Team(TeamToolCall::PostUpdate {
            message: take_flag(&mut args, "--message")?,
        })),
        "get-task-status" => Ok(HelperCall::Team(TeamToolCall::GetTaskStatus)),
        "assign-task" => Ok(HelperCall::Team(TeamToolCall::AssignTask {
            task_id: take_flag(&mut args, "--task-id")?,
            new_assignee: take_flag(&mut args, "--assignee")?,
        })),
        "accept-task" => Ok(HelperCall::Team(TeamToolCall::AcceptTask {
            task_id: take_flag(&mut args, "--task-id")?,
            by: take_optional_flag(&mut args, "--by"),
        })),
        "reopen-task" => Ok(HelperCall::Team(TeamToolCall::ReopenTask {
            task_id: take_flag(&mut args, "--task-id")?,
            reason: take_flag(&mut args, "--reason")?,
            by: take_optional_flag(&mut args, "--by"),
        })),
        "checkpoint-task" => Ok(HelperCall::Team(TeamToolCall::CheckpointTask {
            task_id: take_flag(&mut args, "--task-id")?,
            note: take_flag(&mut args, "--note")?,
            agent: take_optional_flag(&mut args, "--agent"),
        })),
        "submit-task-result" => Ok(HelperCall::Team(TeamToolCall::SubmitTaskResult {
            task_id: take_flag(&mut args, "--task-id")?,
            summary: take_flag(&mut args, "--summary")?,
            result_markdown: take_optional_flag(&mut args, "--result-markdown"),
            agent: take_optional_flag(&mut args, "--agent"),
        })),
        "complete-task" => Ok(HelperCall::Team(TeamToolCall::CompleteTask {
            task_id: take_flag(&mut args, "--task-id")?,
            note: take_flag(&mut args, "--note")?,
            result_markdown: take_optional_flag(&mut args, "--result-markdown"),
            agent: take_optional_flag(&mut args, "--agent"),
        })),
        "block-task" => Ok(HelperCall::Team(TeamToolCall::BlockTask {
            task_id: take_flag(&mut args, "--task-id")?,
            reason: take_flag(&mut args, "--reason")?,
            agent: take_optional_flag(&mut args, "--agent"),
        })),
        "request-help" => Ok(HelperCall::Team(TeamToolCall::RequestHelp {
            task_id: take_flag(&mut args, "--task-id")?,
            message: take_flag(&mut args, "--message")?,
            agent: take_optional_flag(&mut args, "--agent"),
        })),
        "create-schedule" => {
            let (action, command) = parse_schedule_create(&mut args)?;
            if !command.iter().any(|arg| arg == "--prompt") {
                bail!("create-schedule requires --prompt");
            }
            Ok(HelperCall::Schedule { action, command })
        }
        "create-delay-reminder" => {
            let (action, command) = parse_split_schedule_create(
                &mut args,
                "create_delay_reminder",
                "add-delay",
                "--delay",
                "delivery-message",
                "--message",
            )?;
            Ok(HelperCall::Schedule { action, command })
        }
        "create-at-reminder" => {
            let (action, command) = parse_split_schedule_create(
                &mut args,
                "create_at_reminder",
                "add-at",
                "--run-at",
                "delivery-message",
                "--message",
            )?;
            Ok(HelperCall::Schedule { action, command })
        }
        "create-every-reminder" => {
            let (action, command) = parse_split_schedule_create(
                &mut args,
                "create_every_reminder",
                "add-every",
                "--every",
                "delivery-message",
                "--message",
            )?;
            Ok(HelperCall::Schedule { action, command })
        }
        "create-cron-reminder" => {
            let (action, command) = parse_split_schedule_create(
                &mut args,
                "create_cron_reminder",
                "add-cron",
                "--expr",
                "delivery-message",
                "--message",
            )?;
            Ok(HelperCall::Schedule { action, command })
        }
        "create-delay-agent-schedule" => {
            let (action, command) = parse_split_schedule_create(
                &mut args,
                "create_delay_agent_schedule",
                "add-delay",
                "--delay",
                "agent-turn",
                "--task-prompt",
            )?;
            Ok(HelperCall::Schedule { action, command })
        }
        "create-at-agent-schedule" => {
            let (action, command) = parse_split_schedule_create(
                &mut args,
                "create_at_agent_schedule",
                "add-at",
                "--run-at",
                "agent-turn",
                "--task-prompt",
            )?;
            Ok(HelperCall::Schedule { action, command })
        }
        "create-every-agent-schedule" => {
            let (action, command) = parse_split_schedule_create(
                &mut args,
                "create_every_agent_schedule",
                "add-every",
                "--every",
                "agent-turn",
                "--task-prompt",
            )?;
            Ok(HelperCall::Schedule { action, command })
        }
        "create-cron-agent-schedule" => {
            let (action, command) = parse_split_schedule_create(
                &mut args,
                "create_cron_agent_schedule",
                "add-cron",
                "--expr",
                "agent-turn",
                "--task-prompt",
            )?;
            Ok(HelperCall::Schedule { action, command })
        }
        "list-schedules" => Ok(HelperCall::Schedule {
            action: "list_schedules".into(),
            command: vec!["schedule".into(), "--json".into(), "list".into()],
        }),
        "list-current-session-schedules" => Ok(HelperCall::Schedule {
            action: "list_current_session_schedules".into(),
            command: vec![
                "schedule".into(),
                "--json".into(),
                "list".into(),
                "--current-session-key".into(),
            ],
        }),
        "pause-schedule" => Ok(HelperCall::Schedule {
            action: "pause_schedule".into(),
            command: vec![
                "schedule".into(),
                "--json".into(),
                "pause".into(),
                "--job-id".into(),
                take_flag(&mut args, "--job-id")?,
            ],
        }),
        "resume-schedule" => Ok(HelperCall::Schedule {
            action: "resume_schedule".into(),
            command: vec![
                "schedule".into(),
                "--json".into(),
                "resume".into(),
                "--job-id".into(),
                take_flag(&mut args, "--job-id")?,
            ],
        }),
        "delete-schedule" => Ok(HelperCall::Schedule {
            action: "delete_schedule".into(),
            command: vec![
                "schedule".into(),
                "--json".into(),
                "delete".into(),
                "--job-id".into(),
                take_flag(&mut args, "--job-id")?,
            ],
        }),
        "delete-schedule-by-name" => Ok(HelperCall::Schedule {
            action: "delete_schedule_by_name".into(),
            command: vec![
                "schedule".into(),
                "--json".into(),
                "delete".into(),
                "--name".into(),
                take_flag(&mut args, "--name")?,
                "--current-session-key".into(),
            ],
        }),
        "clear-current-session-schedules" => Ok(HelperCall::Schedule {
            action: "clear_current_session_schedules".into(),
            command: vec![
                "schedule".into(),
                "--json".into(),
                "delete-all".into(),
                "--current-session-key".into(),
            ],
        }),
        "run-schedule-now" => Ok(HelperCall::Schedule {
            action: "run_schedule_now".into(),
            command: vec![
                "schedule".into(),
                "--json".into(),
                "run-now".into(),
                "--job-id".into(),
                take_flag(&mut args, "--job-id")?,
            ],
        }),
        "schedule-history" => {
            let mut command = vec!["schedule".into(), "--json".into(), "history".into()];
            if let Some(job_id) = take_optional_flag(&mut args, "--job-id") {
                command.push("--job-id".into());
                command.push(job_id);
            }
            Ok(HelperCall::Schedule {
                action: "schedule_history".into(),
                command,
            })
        }
        other => bail!("unknown team tool subcommand: {other}"),
    }
}

fn parse_schedule_create(args: &mut Vec<String>) -> Result<(String, Vec<String>)> {
    let kind = take_optional_flag(args, "--kind")
        .or_else(|| infer_schedule_kind_from_flags(args))
        .ok_or_else(|| {
            anyhow!(
                "missing schedule kind: provide --kind or one of --expr/--run-at/--every/--delay"
            )
        })?;
    let mut command = vec!["schedule".into(), "--json".into()];
    let action = "create_schedule".to_string();
    match kind.as_str() {
        "cron" => {
            command.push("add-cron".into());
            command.push("--expr".into());
            command.push(take_flag(args, "--expr")?);
        }
        "at" => {
            command.push("add-at".into());
            command.push("--at".into());
            command.push(take_flag(args, "--run-at")?);
        }
        "every" => {
            command.push("add-every".into());
            command.push("--every".into());
            command.push(take_flag(args, "--every")?);
        }
        "delay" => {
            command.push("add-delay".into());
            command.push("--delay".into());
            command.push(take_flag(args, "--delay")?);
        }
        other => bail!("unsupported --kind '{}'", other),
    }
    command.push("--name".into());
    command.push(
        take_optional_flag(args, "--name")
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| synthesize_helper_schedule_name(&action)),
    );
    if let Some(target_kind) = take_optional_flag(args, "--target-kind") {
        command.push("--target-kind".into());
        command.push(target_kind);
    }
    if let Some(target_session_key) = take_optional_flag(args, "--target-session-key") {
        command.push("--session-key".into());
        command.push(target_session_key);
    }
    if let Some(prompt) = take_optional_flag(args, "--prompt") {
        command.push("--prompt".into());
        command.push(prompt);
    }
    if let Some(timezone) = take_optional_flag(args, "--timezone") {
        command.push("--timezone".into());
        command.push(timezone);
    }
    if let Some(agent) = take_optional_flag(args, "--agent") {
        command.push("--agent".into());
        command.push(agent);
    }
    if let Some(threshold) = take_optional_flag(args, "--idle-gt-seconds") {
        command.push("--idle-gt-seconds".into());
        command.push(threshold);
    }
    Ok((action, command))
}

fn parse_split_schedule_create(
    args: &mut Vec<String>,
    action: &str,
    schedule_subcommand: &str,
    schedule_flag: &str,
    target_kind: &str,
    prompt_flag: &str,
) -> Result<(String, Vec<String>)> {
    let mut command = vec![
        "schedule".into(),
        "--json".into(),
        schedule_subcommand.into(),
    ];
    command.push(schedule_cli_flag(schedule_flag));
    command.push(take_flag(args, schedule_flag)?);
    command.push("--name".into());
    command.push(
        take_optional_flag(args, "--name")
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| synthesize_helper_schedule_name(action)),
    );
    command.push("--target-kind".into());
    command.push(target_kind.into());
    if let Some(target_session_key) = take_optional_flag(args, "--target-session-key") {
        command.push("--session-key".into());
        command.push(target_session_key);
    }
    command.push("--prompt".into());
    command.push(take_flag(args, prompt_flag)?);
    if let Some(timezone) = take_optional_flag(args, "--timezone") {
        command.push("--timezone".into());
        command.push(timezone);
    }
    if let Some(agent) = take_optional_flag(args, "--agent") {
        command.push("--agent".into());
        command.push(agent);
    }
    if let Some(threshold) = take_optional_flag(args, "--idle-gt-seconds") {
        command.push("--idle-gt-seconds".into());
        command.push(threshold);
    }
    Ok((action.to_string(), command))
}

fn schedule_cli_flag(helper_flag: &str) -> String {
    match helper_flag {
        "--run-at" => "--at".to_string(),
        other => other.to_string(),
    }
}

fn schedule_default_current_session_key(session_key: &SessionKey) -> String {
    render_session_key_text(session_key)
}

fn normalize_schedule_helper_command(
    mut command: Vec<String>,
    action: &str,
    session_key: &SessionKey,
) -> Vec<String> {
    if action.starts_with("create_")
        && !command.iter().any(|arg| arg == "--session-key")
        && !command.iter().any(|arg| arg == "--current-session-key")
    {
        command.push("--current-session-key".into());
    }
    let mut normalized = Vec::with_capacity(command.len() + 2);
    let current = schedule_default_current_session_key(session_key);
    let mut index = 0usize;
    while index < command.len() {
        normalized.push(command[index].clone());
        if command[index] == "--current-session-key" {
            let next_is_value = command
                .get(index + 1)
                .is_some_and(|next| !next.starts_with("--"));
            if !next_is_value {
                normalized.push(current.clone());
            }
        }
        index += 1;
    }
    normalized
}

fn infer_schedule_kind_from_flags(args: &[String]) -> Option<String> {
    let mut inferred = Vec::new();
    if args.iter().any(|arg| arg == "--expr") {
        inferred.push("cron");
    }
    if args.iter().any(|arg| arg == "--run-at") {
        inferred.push("at");
    }
    if args.iter().any(|arg| arg == "--every") {
        inferred.push("every");
    }
    if args.iter().any(|arg| arg == "--delay") {
        inferred.push("delay");
    }
    match inferred.as_slice() {
        [only] => Some((*only).to_string()),
        _ => None,
    }
}

fn synthesize_helper_schedule_name(prefix: &str) -> String {
    let epoch_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    format!("{prefix}-{epoch_ms}")
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

fn render_team_helper_output(call: &TeamToolCall, response: TeamToolResponse) -> Value {
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
        TeamToolCall::BlockTask {
            task_id, reason, ..
        } => render_team_helper_success(
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

fn render_schedule_helper_output(action: &str, response: Value) -> Value {
    let message = response
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("schedule command completed")
        .to_string();
    let data = response.get("data").cloned().unwrap_or(Value::Null);
    render_team_helper_success(
        action,
        Map::from_iter([
            ("message".into(), Value::String(message)),
            ("result".into(), response),
            ("payload".into(), data),
        ]),
    )
}

async fn invoke_schedule_command(command: &[String]) -> Result<Value> {
    let exe = std::env::current_exe().context("failed to locate current clawbro binary")?;
    let output = Command::new(exe)
        .args(command)
        .output()
        .await
        .context("failed to invoke local clawbro schedule command")?;
    let stdout =
        String::from_utf8(output.stdout).context("schedule command stdout is not utf-8")?;
    let stderr = String::from_utf8(output.stderr).unwrap_or_default();
    let value: Value = serde_json::from_str(stdout.trim()).with_context(|| {
        format!(
            "schedule command did not return valid JSON; stdout='{}' stderr='{}'",
            stdout.trim(),
            stderr.trim()
        )
    })?;
    let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if output.status.success() && ok {
        return Ok(value);
    }
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("schedule command failed");
    bail!("{message}")
}
