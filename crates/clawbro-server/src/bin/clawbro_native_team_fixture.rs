use anyhow::Result;
use clawbro::runtime::{
    RuntimeEvent, RuntimeRole, RuntimeSessionSpec, TeamToolCall, TeamToolRequest, TeamToolResponse,
};
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
    let session: RuntimeSessionSpec = serde_json::from_str(&input)?;

    match session.role {
        RuntimeRole::Leader => run_leader(&session).await?,
        RuntimeRole::Specialist => run_specialist(&session).await?,
        RuntimeRole::Solo => emit_complete("solo:noop")?,
    }

    Ok(())
}

async fn run_leader(session: &RuntimeSessionSpec) -> Result<()> {
    let user_input = session.context.user_input.as_deref().unwrap_or_default();
    let team_url = session
        .team_tool_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("missing team_tool_url for leader turn"))?;

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
            team_url,
            &session.session_key,
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
        team_url,
        &session.session_key,
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
    invoke_team_tool(team_url, &session.session_key, TeamToolCall::StartExecution).await?;
    emit_complete("leader:planned:T001")?;
    Ok(())
}

async fn run_specialist(session: &RuntimeSessionSpec) -> Result<()> {
    let team_url = session
        .team_tool_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("missing team_tool_url for specialist turn"))?;
    let reminder = session.context.task_reminder.as_deref().unwrap_or_default();
    let task_id = extract_task_id(reminder).unwrap_or_else(|| "T001".to_string());
    invoke_team_tool(
        team_url,
        &session.session_key,
        TeamToolCall::SubmitTaskResult {
            task_id: task_id.clone(),
            summary: "worker fixture result".to_string(),
            result_markdown: None,
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
        if token.starts_with('T') && token.len() > 1 {
            return Some(token.to_string());
        }
    }
    None
}
