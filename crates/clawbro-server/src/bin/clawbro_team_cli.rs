use anyhow::{anyhow, bail, Context, Result};
use clawbro_protocol::SessionKey;
use clawbro_runtime::{render_team_helper_success, TeamToolCall, TeamToolRequest, TeamToolResponse};
use serde_json::{Map, Value};
use std::env;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("clawbro-team-cli: {err:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse(env::args().skip(1))?;
    let call = cli.call.clone();
    let response = reqwest::Client::new()
        .post(&cli.url)
        .json(&TeamToolRequest {
            session_key: SessionKey::new(cli.session_channel, cli.session_scope),
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

#[derive(Debug)]
struct Cli {
    url: String,
    session_channel: String,
    session_scope: String,
    call: TeamToolCall,
}

impl Cli {
    fn parse<I>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = String>,
    {
        let mut args = args.into_iter();
        let mut url = None;
        let mut session_channel = None;
        let mut session_scope = None;
        let mut positionals = Vec::new();

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--url" => url = Some(expect_value(&mut args, "--url")?),
                "--session-channel" => {
                    session_channel = Some(expect_value(&mut args, "--session-channel")?)
                }
                "--session-scope" => {
                    session_scope = Some(expect_value(&mut args, "--session-scope")?)
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => positionals.push(other.to_string()),
            }
        }

        let url = url.ok_or_else(|| anyhow!("--url is required"))?;
        let session_channel =
            session_channel.ok_or_else(|| anyhow!("--session-channel is required"))?;
        let session_scope = session_scope.ok_or_else(|| anyhow!("--session-scope is required"))?;
        let call = parse_call(positionals)?;

        Ok(Self {
            url,
            session_channel,
            session_scope,
            call,
        })
    }
}

fn expect_value<I>(args: &mut I, flag: &str) -> Result<String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| anyhow!("{flag} requires a value"))
}

fn parse_call(mut args: Vec<String>) -> Result<TeamToolCall> {
    if args.is_empty() {
        bail!("missing team tool subcommand");
    }
    let subcommand = args.remove(0);
    match subcommand.as_str() {
        "create-task" => Ok(TeamToolCall::CreateTask {
            id: Some(take_flag(&mut args, "--id")?),
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

fn print_help() {
    println!(
        "clawbro-team-cli --url <endpoint> --session-channel <channel> --session-scope <scope> <subcommand> [flags]\n\
         subcommands: create-task, start-execution, request-confirmation, post-update, get-task-status,\n\
         assign-task, accept-task, reopen-task, checkpoint-task, submit-task-result, complete-task, block-task, request-help"
    );
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
                ("task_id".into(), Value::String(id.clone().unwrap_or_default())),
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
                ("response_message".into(), Value::String(response.message)),
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
            task_id,
            summary,
            result_markdown,
            ..
        } => render_team_helper_success(
            "submit_task_result",
            Map::from_iter([
                ("task_id".into(), Value::String(task_id.clone())),
                ("summary".into(), Value::String(summary.clone())),
                (
                    "result_markdown".into(),
                    result_markdown
                        .as_ref()
                        .map(|text| Value::String(text.clone()))
                        .unwrap_or(Value::Null),
                ),
                ("message".into(), Value::String(response.message)),
                (
                    "artifacts".into(),
                    response
                        .payload
                        .as_ref()
                        .and_then(|value| value.get("artifacts"))
                        .cloned()
                        .unwrap_or_else(|| Value::Array(vec![])),
                ),
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
        TeamToolCall::CompleteTask {
            task_id,
            note,
            result_markdown,
            ..
        } => render_team_helper_success(
            "complete_task",
            Map::from_iter([
                ("task_id".into(), Value::String(task_id.clone())),
                ("note".into(), Value::String(note.clone())),
                (
                    "result_markdown".into(),
                    result_markdown
                        .as_ref()
                        .map(|text| Value::String(text.clone()))
                        .unwrap_or(Value::Null),
                ),
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
                ("response_message".into(), Value::String(response.message)),
                ("payload".into(), response.payload.unwrap_or(Value::Null)),
            ]),
        ),
        _ => render_team_helper_success(
            helper_action_name(call),
            Map::from_iter([
                ("message".into(), Value::String(response.message)),
                ("payload".into(), response.payload.unwrap_or(Value::Null)),
            ]),
        ),
    }
}

fn helper_action_name(call: &TeamToolCall) -> &'static str {
    match call {
        TeamToolCall::CreateTask { .. } => "create_task",
        TeamToolCall::StartExecution => "start_execution",
        TeamToolCall::RequestConfirmation { .. } => "request_confirmation",
        TeamToolCall::PostUpdate { .. } => "post_update",
        TeamToolCall::GetTaskStatus => "get_task_status",
        TeamToolCall::AssignTask { .. } => "assign_task",
        TeamToolCall::CheckpointTask { .. } => "checkpoint_task",
        TeamToolCall::SubmitTaskResult { .. } => "submit_task_result",
        TeamToolCall::CompleteTask { .. } => "complete_task",
        TeamToolCall::AcceptTask { .. } => "accept_task",
        TeamToolCall::ReopenTask { .. } => "reopen_task",
        TeamToolCall::BlockTask { .. } => "block_task",
        TeamToolCall::RequestHelp { .. } => "request_help",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_submit_task_result() {
        let cli = Cli::parse(
            [
                "--url",
                "http://127.0.0.1:3000/runtime/team-tools?token=x",
                "--session-channel",
                "team",
                "--session-scope",
                "scope",
                "submit-task-result",
                "--task-id",
                "T1",
                "--summary",
                "done",
                "--agent",
                "openclaw",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();

        assert_eq!(cli.url, "http://127.0.0.1:3000/runtime/team-tools?token=x");
        assert_eq!(cli.session_channel, "team");
        assert_eq!(cli.session_scope, "scope");
        assert!(matches!(
            cli.call,
            TeamToolCall::SubmitTaskResult { task_id, summary, result_markdown, agent }
                if task_id == "T1"
                    && summary == "done"
                    && result_markdown.is_none()
                    && agent.as_deref() == Some("openclaw")
        ));
    }

    #[test]
    fn render_submit_task_result_as_structured_json() {
        let rendered = render_helper_output(
            &TeamToolCall::SubmitTaskResult {
                task_id: "T1".into(),
                summary: "done".into(),
                result_markdown: Some("# Result\n\nfull body".into()),
                agent: Some("openclaw".into()),
            },
            TeamToolResponse {
                ok: true,
                message: "submitted".into(),
                payload: Some(json!({
                    "artifacts": ["src/lib.rs"]
                })),
            },
        );

        assert_eq!(rendered["contract"], clawbro_runtime::TEAM_HELPER_CONTRACT);
        assert_eq!(rendered["version"], clawbro_runtime::TEAM_HELPER_VERSION);
        assert_eq!(rendered["ok"], true);
        assert_eq!(rendered["action"], "submit_task_result");
        assert_eq!(rendered["task_id"], "T1");
        assert_eq!(rendered["summary"], "done");
        assert_eq!(rendered["artifacts"], json!(["src/lib.rs"]));
    }

    #[test]
    fn render_checkpoint_task_as_structured_json() {
        let rendered = render_helper_output(
            &TeamToolCall::CheckpointTask {
                task_id: "T1".into(),
                note: "halfway".into(),
                agent: Some("openclaw".into()),
            },
            TeamToolResponse {
                ok: true,
                message: "checkpointed".into(),
                payload: None,
            },
        );

        assert_eq!(rendered["ok"], true);
        assert_eq!(rendered["action"], "checkpoint_task");
        assert_eq!(rendered["task_id"], "T1");
        assert_eq!(rendered["note"], "halfway");
    }

    #[test]
    fn render_request_help_as_structured_json() {
        let rendered = render_helper_output(
            &TeamToolCall::RequestHelp {
                task_id: "T1".into(),
                message: "need context".into(),
                agent: Some("openclaw".into()),
            },
            TeamToolResponse {
                ok: true,
                message: "help queued".into(),
                payload: None,
            },
        );

        assert_eq!(rendered["ok"], true);
        assert_eq!(rendered["action"], "request_help");
        assert_eq!(rendered["task_id"], "T1");
        assert_eq!(rendered["message"], "need context");
    }

    #[test]
    fn render_create_task_as_structured_json() {
        let rendered = render_helper_output(
            &TeamToolCall::CreateTask {
                id: Some("T1".into()),
                title: "Implement JWT".into(),
                assignee: Some("worker".into()),
                spec: None,
                deps: vec![],
                success_criteria: None,
            },
            TeamToolResponse {
                ok: true,
                message: "created".into(),
                payload: None,
            },
        );

        assert_eq!(rendered["ok"], true);
        assert_eq!(rendered["action"], "create_task");
        assert_eq!(rendered["task_id"], "T1");
        assert_eq!(rendered["title"], "Implement JWT");
        assert_eq!(rendered["assignee"], "worker");
    }

    #[test]
    fn render_accept_task_as_structured_json() {
        let rendered = render_helper_output(
            &TeamToolCall::AcceptTask {
                task_id: "T1".into(),
                by: Some("leader".into()),
            },
            TeamToolResponse {
                ok: true,
                message: "accepted".into(),
                payload: None,
            },
        );

        assert_eq!(rendered["ok"], true);
        assert_eq!(rendered["action"], "accept_task");
        assert_eq!(rendered["task_id"], "T1");
        assert_eq!(rendered["by"], "leader");
    }

    #[test]
    fn render_reopen_task_as_structured_json() {
        let rendered = render_helper_output(
            &TeamToolCall::ReopenTask {
                task_id: "T1".into(),
                reason: "tests missing".into(),
                by: Some("leader".into()),
            },
            TeamToolResponse {
                ok: true,
                message: "reopened".into(),
                payload: None,
            },
        );

        assert_eq!(rendered["ok"], true);
        assert_eq!(rendered["action"], "reopen_task");
        assert_eq!(rendered["task_id"], "T1");
        assert_eq!(rendered["reason"], "tests missing");
        assert_eq!(rendered["by"], "leader");
    }

    #[test]
    fn helper_action_name_normalizes_variants() {
        assert_eq!(
            helper_action_name(&TeamToolCall::BlockTask {
                task_id: "T1".into(),
                reason: "blocked".into(),
                agent: None,
            }),
            "block_task"
        );
        assert_eq!(
            helper_action_name(&TeamToolCall::StartExecution),
            "start_execution"
        );
    }
}
