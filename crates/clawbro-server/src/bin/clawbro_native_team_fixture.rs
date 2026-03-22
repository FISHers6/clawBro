use anyhow::Result;
use clawbro::agent_sdk_internal::bridge::{AgentTurnRequest, ExecutionRole};
use clawbro::runtime::{RuntimeEvent, TeamToolCall, TeamToolRequest, TeamToolResponse};
use std::io::{self, Read};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("clawbro-native-team-fixture: {err:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    if input.trim().is_empty() {
        return Ok(());
    }
    let session: AgentTurnRequest = serde_json::from_str(&input)?;

    match session.role {
        ExecutionRole::Leader => run_leader(&session).await?,
        ExecutionRole::Specialist => run_specialist(&session).await?,
        ExecutionRole::Solo => emit_complete("solo:noop")?,
    }

    Ok(())
}

async fn run_leader(session: &AgentTurnRequest) -> Result<()> {
    let user_input = session.context.user_input.as_deref().unwrap_or_default();
    let team_url = std::env::var("CLAWBRO_TEAM_TOOL_URL")
        .map_err(|_| anyhow::anyhow!("missing CLAWBRO_TEAM_TOOL_URL for leader turn"))?;
    let session_key = clawbro::protocol::parse_session_key_text(&session.session_ref)
        .map_err(|err| anyhow::anyhow!("invalid session_ref: {err}"))?;

    if user_input.contains("请求协助") {
        let task_id = extract_task_id(user_input).unwrap_or_else(|| "T001".to_string());
        emit_complete(&format!("leader:help:{task_id}"))?;
        return Ok(());
    }

    if user_input.contains("已更新检查点") {
        let task_id = extract_task_id(user_input).unwrap_or_else(|| "T001".to_string());
        emit_complete(&format!("leader:checkpoint:{task_id}"))?;
        return Ok(());
    }

    if user_input.contains("已提交待验收") {
        let task_id = extract_task_id(user_input).unwrap_or_else(|| "T001".to_string());
        invoke_team_tool(
            &team_url,
            &session_key,
            TeamToolCall::AcceptTask {
                task_id: task_id.clone(),
                by: Some("leader".to_string()),
            },
        )
        .await?;
        emit_complete(&format!("leader:accepted:{task_id}"))?;
        return Ok(());
    }

    if user_input.contains("未调用任何 canonical team tool") {
        let task_id = extract_task_id(user_input).unwrap_or_else(|| "T001".to_string());
        emit_complete(&format!("leader:missing:{task_id}"))?;
        return Ok(());
    }

    if user_input.contains("所有任务已完成") || user_input.contains("已验收") {
        emit_complete("leader:done")?;
        return Ok(());
    }

    invoke_team_tool(
        &team_url,
        &session_key,
        TeamToolCall::CreateTask {
            id: Some("T001".to_string()),
            title: "fixture task".to_string(),
            assignee: Some("worker".to_string()),
            spec: Some("Implement the fixture task".to_string()),
            deps: vec![],
            success_criteria: Some("Submit task result".to_string()),
        },
    )
    .await?;
    invoke_team_tool(&team_url, &session_key, TeamToolCall::StartExecution).await?;
    emit_complete("leader:planned:T001")?;
    Ok(())
}

async fn run_specialist(session: &AgentTurnRequest) -> Result<()> {
    let team_url = std::env::var("CLAWBRO_TEAM_TOOL_URL")
        .map_err(|_| anyhow::anyhow!("missing CLAWBRO_TEAM_TOOL_URL for specialist turn"))?;
    let session_key = clawbro::protocol::parse_session_key_text(&session.session_ref)
        .map_err(|err| anyhow::anyhow!("invalid session_ref: {err}"))?;
    let reminder = session.context.task_reminder.as_deref().unwrap_or_default();
    let task_id = extract_task_id(reminder).unwrap_or_else(|| "T001".to_string());
    invoke_team_tool(
        &team_url,
        &session_key,
        TeamToolCall::SubmitTaskResult {
            task_id: task_id.clone(),
            summary: "worker fixture result".to_string(),
            result_markdown: Some(
                "# Worker Result\n\nImplemented the fixture task and prepared the final deliverable body for lead review."
                    .to_string(),
            ),
            agent: Some("worker".to_string()),
        },
    )
    .await?;
    emit_complete(&format!("worker:submitted:{task_id}"))?;
    Ok(())
}

async fn invoke_team_tool(
    url: &str,
    session_key: &clawbro::protocol::SessionKey,
    call: TeamToolCall,
) -> Result<()> {
    let response = reqwest::Client::new()
        .post(url)
        .json(&TeamToolRequest {
            session_key: session_key.clone(),
            call,
        })
        .send()
        .await?;
    let status = response.status();
    let body: TeamToolResponse = response.json().await?;
    if !status.is_success() || !body.ok {
        anyhow::bail!("team tool failed: {}", body.message);
    }
    Ok(())
}

fn emit_complete(text: &str) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string(&RuntimeEvent::TextDelta {
            text: text.to_string()
        })?
    );
    println!(
        "{}",
        serde_json::to_string(&RuntimeEvent::TurnComplete {
            full_text: text.to_string()
        })?
    );
    Ok(())
}

fn extract_task_id(text: &str) -> Option<String> {
    for token in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-') {
        if token.starts_with('T')
            && token.len() > 1
            && token[1..].chars().all(|c| c.is_ascii_digit())
        {
            return Some(token.to_string());
        }
    }
    None
}
