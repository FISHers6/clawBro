use crate::{
    contract::{RuntimeEvent, RuntimeSessionSpec, TurnResult},
    event_sink::RuntimeEventSink,
};
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
    let mut child = spawn_command(config, session.workspace_dir.as_deref())?;
    let mut stdin = child.stdin.take().expect("stdin available");
    let stdout = child.stdout.take().expect("stdout available");
    let payload = serde_json::to_vec(&session)?;
    stdin.write_all(&payload).await?;
    stdin.shutdown().await?;
    // Native runtime-bridge processes read the full JSON payload from stdin until EOF.
    // `shutdown()` flushes the pipe, but the child may still block waiting for EOF until
    // we drop the write handle on our side.
    drop(stdin);

    let mut lines = BufReader::new(stdout).lines();
    let mut events = Vec::new();
    let mut full_text = String::new();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let event: RuntimeEvent = serde_json::from_str(&line)?;
        match &event {
            RuntimeEvent::TextDelta { .. }
            | RuntimeEvent::ApprovalRequest(_)
            | RuntimeEvent::ToolCallback(_) => {
                sink.emit(event.clone())?;
            }
            RuntimeEvent::TurnComplete { full_text: text } => {
                full_text = text.clone();
                sink.emit(event.clone())?;
            }
            RuntimeEvent::TurnFailed { .. } => {
                sink.emit(event.clone())?;
            }
        }
        events.push(event);
    }

    let status = child.wait().await?;
    if !status.success() {
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

    Ok(TurnResult { full_text, events })
}

fn spawn_command(
    config: &NativeCommandConfig,
    workspace_dir: Option<&std::path::Path>,
) -> anyhow::Result<tokio::process::Child> {
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args)
        .envs(config.env.iter().cloned())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());

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
    use crate::{RuntimeContext, RuntimeRole, ToolSurfaceSpec};

    #[test]
    fn native_command_config_carries_command_env_and_args() {
        let cfg = NativeCommandConfig {
            command: "quickai-rust-agent".into(),
            args: vec!["--runtime-bridge".into()],
            env: vec![("OPENAI_API_KEY".into(), "sk-test".into())],
        };
        assert_eq!(cfg.command, "quickai-rust-agent");
        assert_eq!(cfg.args, vec!["--runtime-bridge"]);
        assert_eq!(cfg.env.len(), 1);
    }

    #[test]
    fn runtime_session_spec_serializes_for_native_bridge() {
        let session = RuntimeSessionSpec {
            backend_id: "native".into(),
            participant_name: None,
            session_key: qai_protocol::SessionKey::new("ws", "native:test"),
            role: RuntimeRole::Solo,
            workspace_dir: None,
            prompt_text: "hello".into(),
            tool_surface: ToolSurfaceSpec::default(),
            tool_bridge_url: None,
            team_tool_url: None,
            context: RuntimeContext::default(),
        };
        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("\"backend_id\":\"native\""));
        assert!(json.contains("\"prompt_text\":\"hello\""));
    }
}
