use anyhow::Result;
use std::sync::{Arc, Mutex};

use crate::{config::AgentConfig, runtime_bridge::QuickAiRuntimeBridge};
use crate::team::QuickAiTeamToolAugmentor;
use clawbro_agent_sdk::bridge::{AgentEvent, AgentTurnRequest};

pub async fn run_stdio_bridge() -> Result<()> {
    let session: AgentTurnRequest = serde_json::from_reader(std::io::stdin())?;
    match AgentConfig::from_env() {
        Ok(config) => {
            let bridge = QuickAiRuntimeBridge::new(config);
            let team_tools = QuickAiTeamToolAugmentor::from_env();
            let stdout = Arc::new(Mutex::new(std::io::BufWriter::new(std::io::stdout())));
            let delta_writer = Arc::clone(&stdout);
            tracing::info!(
                session = %session.session_ref,
                history_messages = session.context.history_messages.len(),
                history_lines = session.context.history_lines.len(),
                user_input = session.context.user_input.as_deref().unwrap_or_default(),
                "native runtime bridge received turn request"
            );
            let result = bridge
                .execute_with_augmentor(&session, move |delta| {
                    if let Ok(mut stdout) = delta_writer.lock() {
                        let _ = serde_json::to_writer(&mut *stdout, &delta);
                        let _ = std::io::Write::write_all(&mut *stdout, b"\n");
                        let _ = std::io::Write::flush(&mut *stdout);
                    }
                }, &team_tools)
                .await;

            match result {
                Ok(full_text) => {
                    let event = AgentEvent::TurnComplete { full_text };
                    let mut stdout = stdout.lock().expect("stdout lock");
                    serde_json::to_writer(&mut *stdout, &event)?;
                    std::io::Write::write_all(&mut *stdout, b"\n")?;
                    std::io::Write::flush(&mut *stdout)?;
                    Ok(())
                }
                Err(err) => {
                    let event = AgentEvent::TurnFailed {
                        error: err.to_string(),
                    };
                    let mut stdout = stdout.lock().expect("stdout lock");
                    serde_json::to_writer(&mut *stdout, &event)?;
                    std::io::Write::write_all(&mut *stdout, b"\n")?;
                    std::io::Write::flush(&mut *stdout)?;
                    Err(err)
                }
            }
        }
        Err(err) => {
            let mut stdout = std::io::BufWriter::new(std::io::stdout());
            let reply = format!("Echo: {}", session.prompt_text);
            serde_json::to_writer(
                &mut stdout,
                &AgentEvent::TextDelta {
                    text: reply.clone(),
                },
            )?;
            std::io::Write::write_all(&mut stdout, b"\n")?;
            serde_json::to_writer(
                &mut stdout,
                &AgentEvent::TurnComplete { full_text: reply },
            )?;
            std::io::Write::write_all(&mut stdout, b"\n")?;
            std::io::Write::flush(&mut stdout)?;
            tracing::warn!("runtime bridge running in echo fallback mode: {err}");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use clawbro_agent_sdk::bridge::{
        AgentTurnRequest, ApprovalMode, ExecutionRole, ExternalMcpServerSpec,
        ExternalMcpTransport, RuntimeContext, ToolSurfaceSpec,
    };

    #[test]
    fn runtime_bridge_protocol_uses_turn_request() {
        let session = AgentTurnRequest {
            participant_name: None,
            session_ref: "native:test".into(),
            role: ExecutionRole::Solo,
            workspace_dir: None,
            prompt_text: "hello".into(),
            tool_surface: ToolSurfaceSpec::default(),
            approval_mode: ApprovalMode::Manual,
            tool_bridge_url: None,
            external_mcp_servers: vec![ExternalMcpServerSpec {
                name: "filesystem".into(),
                transport: ExternalMcpTransport::Sse {
                    url: "http://127.0.0.1:3001/sse".into(),
                },
            }],
            provider_profile: None,
            context: RuntimeContext::default(),
        };
        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("\"prompt_text\":\"hello\""));
        assert!(json.contains("\"external_mcp_servers\""));
        assert!(json.contains("\"filesystem\""));
    }
}
