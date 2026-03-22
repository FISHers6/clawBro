use anyhow::Result;
use clawbro::agent_sdk_internal::bridge::{AgentTurnRequest, ExecutionRole};
use clawbro::runtime::RuntimeEvent;
use std::io::{self, Read};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("clawbro-native-team-missing-completion-fixture: {err:#}");
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
        ExecutionRole::Leader => emit_complete("leader:noop")?,
        ExecutionRole::Specialist => {
            let reminder = session.context.task_reminder.as_deref().unwrap_or_default();
            let task_id = extract_task_id(reminder).unwrap_or_else(|| "T001".to_string());
            emit_complete(&format!("worker:text-only:{task_id}"))?;
        }
        ExecutionRole::Solo => emit_complete("solo:noop")?,
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
