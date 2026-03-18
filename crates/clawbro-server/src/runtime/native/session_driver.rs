use crate::runtime::{
    contract::{RuntimeEvent, RuntimeSessionSpec, TurnResult},
    event_sink::RuntimeEventSink,
};
use crate::agent_sdk_internal::bridge::AgentEvent;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCommandConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub async fn run_command_turn(
    config: &NativeCommandConfig,
    session: RuntimeSessionSpec,
    sink: RuntimeEventSink,
) -> anyhow::Result<TurnResult> {
    tracing::info!(
        backend_id = %session.backend_id,
        session = ?session.session_key,
        history_messages = session.context.history_messages.len(),
        history_lines = session.context.history_lines.len(),
        user_input = session.context.user_input.as_deref().unwrap_or_default(),
        "spawning native runtime turn"
    );
    let mut child = spawn_command(
        config,
        session.workspace_dir.as_deref(),
        session.team_tool_url.as_deref(),
    )?;
    let mut stdin = child.stdin.take().expect("stdin available");
    let stdout = child.stdout.take().expect("stdout available");
    let payload = serde_json::to_vec(&session.to_agent_turn_request())?;
    stdin.write_all(&payload).await?;
    stdin.shutdown().await?;
    // Native runtime-bridge processes read the full JSON payload from stdin until EOF.
    // `shutdown()` flushes the pipe, but the child may still block waiting for EOF until
    // we drop the write handle on our side.
    drop(stdin);

    let mut lines = BufReader::new(stdout).lines();
    let mut events = Vec::new();
    let mut full_text = String::new();
    let mut turn_failed_error: Option<String> = None;

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let agent_event: AgentEvent = serde_json::from_str(&line)?;
        let event = runtime_event_from_agent_event(agent_event);
        match &event {
            RuntimeEvent::TextDelta { .. }
            | RuntimeEvent::ToolCallStarted { .. }
            | RuntimeEvent::ToolCallCompleted { .. }
            | RuntimeEvent::ToolCallFailed { .. }
            | RuntimeEvent::ApprovalRequest(_)
            | RuntimeEvent::ToolCallback(_) => {
                sink.emit(event.clone())?;
            }
            RuntimeEvent::TurnComplete { full_text: text } => {
                full_text = text.clone();
                sink.emit(event.clone())?;
            }
            RuntimeEvent::TurnFailed { .. } => {
                if let RuntimeEvent::TurnFailed { error } = &event {
                    turn_failed_error = Some(error.clone());
                }
                sink.emit(event.clone())?;
            }
        }
        events.push(event);
    }

    let status = child.wait().await?;
    if !status.success() {
        if turn_failed_error.is_some() {
            return Ok(TurnResult {
                full_text,
                events,
                emitted_backend_session_id: None,
                backend_resume_fingerprint: None,
                used_backend_id: None,
                resume_recovery: None,
            });
        }
        anyhow::bail!("native backend process exited with status {status}");
    }

    if full_text.is_empty() {
        if let Some(RuntimeEvent::TurnComplete { full_text: text }) = events
            .iter()
            .find(|e| matches!(e, RuntimeEvent::TurnComplete { .. }))
        {
            full_text = text.clone();
        }
    }

    Ok(TurnResult {
        full_text,
        events,
        emitted_backend_session_id: None,
        backend_resume_fingerprint: None,
        used_backend_id: None,
        resume_recovery: None,
    })
}

fn runtime_event_from_agent_event(event: AgentEvent) -> RuntimeEvent {
    match event {
        AgentEvent::TextDelta { text } => RuntimeEvent::TextDelta { text },
        AgentEvent::ToolCallStarted {
            tool_name,
            call_id,
            input_summary,
        } => RuntimeEvent::ToolCallStarted {
            tool_name,
            call_id,
            input_summary,
        },
        AgentEvent::ToolCallCompleted {
            tool_name,
            call_id,
            result,
        } => RuntimeEvent::ToolCallCompleted {
            tool_name,
            call_id,
            result,
        },
        AgentEvent::ToolCallFailed {
            tool_name,
            call_id,
            error,
        } => RuntimeEvent::ToolCallFailed {
            tool_name,
            call_id,
            error,
        },
        AgentEvent::TurnComplete { full_text } => RuntimeEvent::TurnComplete { full_text },
        AgentEvent::TurnFailed { error } => RuntimeEvent::TurnFailed { error },
    }
}

fn spawn_command(
    config: &NativeCommandConfig,
    workspace_dir: Option<&std::path::Path>,
    team_tool_url: Option<&str>,
) -> anyhow::Result<tokio::process::Child> {
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args)
        .envs(config.env.iter().cloned())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());

    if let Some(url) = team_tool_url {
        cmd.env("CLAWBRO_TEAM_TOOL_URL", url);
    }

    if let Some(ws) = workspace_dir {
        if ws.exists() {
            cmd.current_dir(ws);
        } else {
            tracing::warn!(path = %ws.display(), "Workspace directory does not exist; running in default directory");
        }
    }

    Ok(cmd.spawn()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{RuntimeContext, RuntimeRole, ToolSurfaceSpec};
    use tokio::sync::mpsc;

    #[test]
    fn native_command_config_carries_command_env_and_args() {
        let cfg = NativeCommandConfig {
            command: "clawbro-rust-agent".into(),
            args: vec!["--runtime-bridge".into()],
            env: vec![("OPENAI_API_KEY".into(), "sk-test".into())],
        };
        assert_eq!(cfg.command, "clawbro-rust-agent");
        assert_eq!(cfg.args, vec!["--runtime-bridge"]);
        assert_eq!(cfg.env.len(), 1);
    }

    #[test]
    fn turn_request_serializes_for_native_bridge() {
        let session = RuntimeSessionSpec {
            backend_id: "native".into(),
            participant_name: None,
            session_key: crate::protocol::SessionKey::new("ws", "native:test"),
            role: RuntimeRole::Solo,
            workspace_dir: None,
            prompt_text: "hello".into(),
            tool_surface: ToolSurfaceSpec::default(),
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: None,
            provider_profile: None,
            backend_session_id: None,
            context: RuntimeContext::default(),
        };
        let json = serde_json::to_string(&session.to_turn_request()).unwrap();
        assert!(json.contains("\"prompt_text\":\"hello\""));
        assert!(!json.contains("team_tool_url"));
    }

    fn sample_runtime_session() -> RuntimeSessionSpec {
        RuntimeSessionSpec {
            backend_id: "native".into(),
            participant_name: None,
            session_key: crate::protocol::SessionKey::new("ws", "native:test"),
            role: RuntimeRole::Solo,
            workspace_dir: None,
            prompt_text: "hello".into(),
            tool_surface: ToolSurfaceSpec::default(),
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: None,
            provider_profile: None,
            backend_session_id: None,
            context: RuntimeContext::default(),
        }
    }

    #[tokio::test]
    async fn nonzero_exit_preserves_turn_failed_event() {
        let cfg = NativeCommandConfig {
            command: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf '%s\\n' '{\"TurnFailed\":{\"error\":\"boom\"}}'; exit 1".into(),
            ],
            env: vec![],
        };
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink = RuntimeEventSink::new(tx);

        let result = run_command_turn(&cfg, sample_runtime_session(), sink)
            .await
            .expect("turn_failed event should be returned instead of generic exit error");

        assert!(result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::TurnFailed { error } if error == "boom")));
    }
}
