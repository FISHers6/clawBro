use anyhow::Result;
use qai_runtime::{RuntimeEvent, RuntimeRole, RuntimeSessionSpec};
use std::io::{self, Read};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("qai-native-team-missing-completion-fixture: {err:#}");
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
        RuntimeRole::Leader => emit_complete("leader:noop")?,
        RuntimeRole::Specialist => {
            let reminder = session.context.task_reminder.as_deref().unwrap_or_default();
            let task_id = extract_task_id(reminder).unwrap_or_else(|| "T001".to_string());
            emit_complete(&format!("worker:text-only:{task_id}"))?;
        }
        RuntimeRole::Solo => emit_complete("solo:noop")?,
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
