use crate::{
    approval::{ApprovalBroker, ApprovalDecision},
    contract::{render_runtime_prompt, RuntimeEvent, RuntimeSessionSpec, TurnResult},
    event_sink::RuntimeEventSink,
};
use agent_client_protocol as acp;
use tokio::process::Command;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpCommandConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub async fn probe_command_backend(
    config: &AcpCommandConfig,
) -> anyhow::Result<acp::InitializeResponse> {
    use acp::Agent as _;

    let mut child = spawn_command(config, None)?;
    let stdin = child.stdin.take().expect("stdin available");
    let stdout = child.stdout.take().expect("stdout available");
    let outgoing = stdin.compat_write();
    let incoming = stdout.compat();

    struct ProbeClient;

    #[async_trait::async_trait(?Send)]
    impl acp::Client for ProbeClient {
        async fn request_permission(
            &self,
            args: acp::RequestPermissionRequest,
        ) -> acp::Result<acp::RequestPermissionResponse> {
            let outcome = args
                .options
                .iter()
                .find(|o| {
                    matches!(
                        o.kind,
                        acp::PermissionOptionKind::AllowOnce
                            | acp::PermissionOptionKind::AllowAlways
                    )
                })
                .map(|o| {
                    acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                        o.option_id.clone(),
                    ))
                })
                .unwrap_or(acp::RequestPermissionOutcome::Cancelled);
            Ok(acp::RequestPermissionResponse::new(outcome))
        }

        async fn session_notification(
            &self,
            _notification: acp::SessionNotification,
        ) -> acp::Result<()> {
            Ok(())
        }
    }

    let (conn, handle_io) =
        acp::ClientSideConnection::new(ProbeClient, outgoing, incoming, |fut| {
            tokio::task::spawn_local(fut);
        });
    tokio::task::spawn_local(handle_io);

    let init = conn
        .initialize(
            acp::InitializeRequest::new(acp::ProtocolVersion::V1).client_info(
                acp::Implementation::new("qai-runtime", env!("CARGO_PKG_VERSION")),
            ),
        )
        .await
        .map_err(|e| anyhow::anyhow!("ACP initialize failed: {e:?}"))?;

    child.kill().await.ok();
    Ok(init)
}

pub async fn run_command_turn(
    config: &AcpCommandConfig,
    session: RuntimeSessionSpec,
    sink: RuntimeEventSink,
    approvals: ApprovalBroker,
) -> anyhow::Result<TurnResult> {
    use acp::Agent as _;
    use std::{cell::RefCell, rc::Rc};

    let mut child = spawn_command(config, session.workspace_dir.as_deref())?;
    let stdin = child.stdin.take().expect("stdin available");
    let stdout = child.stdout.take().expect("stdout available");
    let outgoing = stdin.compat_write();
    let incoming = stdout.compat();

    #[derive(Clone)]
    struct EventClient {
        sink: RuntimeEventSink,
        accumulated: Rc<RefCell<String>>,
        approvals: ApprovalBroker,
    }

    #[async_trait::async_trait(?Send)]
    impl acp::Client for EventClient {
        async fn request_permission(
            &self,
            args: acp::RequestPermissionRequest,
        ) -> acp::Result<acp::RequestPermissionResponse> {
            let request = permission_request_from_acp(&args);
            let pending = self.approvals.register(&request);
            let _ = self.sink.emit(RuntimeEvent::ApprovalRequest(request));
            let decision = pending.wait().await;
            let outcome = outcome_from_decision(&args.options, decision);
            Ok(acp::RequestPermissionResponse::new(outcome))
        }

        async fn session_notification(
            &self,
            notification: acp::SessionNotification,
        ) -> acp::Result<()> {
            if let acp::SessionUpdate::AgentMessageChunk(chunk) = notification.update {
                if let acp::ContentBlock::Text(t) = chunk.content {
                    self.accumulated.borrow_mut().push_str(&t.text);
                    let _ = self.sink.emit(RuntimeEvent::TextDelta { text: t.text });
                }
            }
            Ok(())
        }
    }

    let accumulated = Rc::new(RefCell::new(String::new()));
    let client = EventClient {
        sink: sink.clone(),
        accumulated: accumulated.clone(),
        approvals,
    };
    let (conn, handle_io) = acp::ClientSideConnection::new(client, outgoing, incoming, |fut| {
        tokio::task::spawn_local(fut);
    });
    tokio::task::spawn_local(handle_io);

    let init_resp = conn
        .initialize(
            acp::InitializeRequest::new(acp::ProtocolVersion::V1).client_info(
                acp::Implementation::new("qai-runtime", env!("CARGO_PKG_VERSION")),
            ),
        )
        .await
        .map_err(|e| anyhow::anyhow!("ACP initialize failed: {e:?}"))?;

    let session_root = session
        .workspace_dir
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let mcp_servers = build_mcp_servers(
        init_resp.agent_capabilities.mcp_capabilities.sse,
        session.tool_bridge_url.as_deref(),
    );

    let sess = conn
        .new_session(acp::NewSessionRequest::new(session_root).mcp_servers(mcp_servers))
        .await
        .map_err(|e| anyhow::anyhow!("ACP new_session failed: {e:?}"))?;

    let prompt_result = conn
        .prompt(acp::PromptRequest::new(
            sess.session_id,
            prompt_blocks_from_session(&session),
        ))
        .await
        .map_err(|e| anyhow::anyhow!("ACP prompt failed: {e:?}"));

    prompt_result?;
    let full_text = accumulated.borrow().clone();
    let complete = RuntimeEvent::TurnComplete {
        full_text: full_text.clone(),
    };
    let _ = sink.emit(complete.clone());

    child.kill().await.ok();

    Ok(TurnResult {
        full_text,
        events: vec![complete],
    })
}

fn permission_request_from_acp(args: &acp::RequestPermissionRequest) -> crate::PermissionRequest {
    let title = args
        .tool_call
        .fields
        .title
        .clone()
        .unwrap_or_else(|| "ACP tool permission required".into());
    let raw_input = args
        .tool_call
        .fields
        .raw_input
        .as_ref()
        .map(|value| {
            let compact = value.to_string();
            if compact.len() > 400 {
                format!("{}...", &compact[..400])
            } else {
                compact
            }
        })
        .unwrap_or_default();
    let prompt = if raw_input.is_empty() {
        title.clone()
    } else {
        format!("{title} | input={raw_input}")
    };
    crate::PermissionRequest {
        id: args.tool_call.tool_call_id.to_string(),
        prompt,
        command: args.tool_call.fields.title.clone(),
        cwd: None,
        host: Some("acp".into()),
        agent_id: None,
        expires_at_ms: None,
    }
}

fn outcome_from_decision(
    options: &[acp::PermissionOption],
    decision: ApprovalDecision,
) -> acp::RequestPermissionOutcome {
    let preferred_kinds: &[acp::PermissionOptionKind] = match decision {
        ApprovalDecision::AllowOnce => &[acp::PermissionOptionKind::AllowOnce],
        ApprovalDecision::AllowAlways => &[
            acp::PermissionOptionKind::AllowAlways,
            acp::PermissionOptionKind::AllowOnce,
        ],
        ApprovalDecision::Deny => &[
            acp::PermissionOptionKind::RejectOnce,
            acp::PermissionOptionKind::RejectAlways,
        ],
    };

    options
        .iter()
        .find(|option| preferred_kinds.contains(&option.kind))
        .map(|option| {
            acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                option.option_id.clone(),
            ))
        })
        .unwrap_or(acp::RequestPermissionOutcome::Cancelled)
}

pub fn build_mcp_servers(supports_sse: bool, url: Option<&str>) -> Vec<acp::McpServer> {
    if supports_sse {
        if let Some(u) = url {
            if !u.is_empty() {
                return vec![acp::McpServer::Sse(acp::McpServerSse::new("team-tools", u))];
            }
        }
    }
    vec![]
}

fn prompt_blocks_from_session(session: &RuntimeSessionSpec) -> Vec<acp::ContentBlock> {
    vec![acp::ContentBlock::Text(acp::TextContent::new(
        render_runtime_prompt(session),
    ))]
}

fn spawn_command(
    config: &AcpCommandConfig,
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

    #[test]
    fn build_mcp_servers_empty_when_no_url() {
        assert!(build_mcp_servers(true, None).is_empty());
    }

    #[test]
    fn build_mcp_servers_empty_when_no_sse_capability() {
        assert!(build_mcp_servers(false, Some("http://127.0.0.1:9999")).is_empty());
    }

    #[test]
    fn build_mcp_servers_populated_when_url_and_capability() {
        let servers = build_mcp_servers(true, Some("http://127.0.0.1:9999"));
        assert_eq!(servers.len(), 1);
        match &servers[0] {
            acp::McpServer::Sse(sse) => {
                assert_eq!(sse.name, "team-tools");
                assert_eq!(sse.url, "http://127.0.0.1:9999");
            }
            other => panic!("unexpected mcp server: {other:?}"),
        }
    }

    #[test]
    fn acp_prompt_blocks_preserve_rendered_recent_history() {
        let session = RuntimeSessionSpec {
            backend_id: "acp-main".into(),
            participant_name: Some("worker".into()),
            session_key: qai_protocol::SessionKey::new("ws", "history:test"),
            role: crate::contract::RuntimeRole::Solo,
            workspace_dir: None,
            prompt_text: String::new(),
            tool_surface: crate::contract::ToolSurfaceSpec::default(),
            tool_bridge_url: None,
            team_tool_url: None,
            context: crate::contract::RuntimeContext {
                history_lines: vec![
                    "[user]: [alice]: 第一条".into(),
                    "[assistant]: [@codex]: 第二条".into(),
                    "[tool_call:read#call-1]: {\"path\":\"README.md\"}".into(),
                    "[tool_result:read#call-1]: ok".into(),
                ],
                user_input: Some("第三条".into()),
                ..crate::contract::RuntimeContext::default()
            },
        };

        let blocks = prompt_blocks_from_session(&session);
        assert_eq!(blocks.len(), 1);
        let acp::ContentBlock::Text(text) = &blocks[0] else {
            panic!("expected text content block");
        };
        assert!(text.text.contains("[user]: [alice]: 第一条"));
        assert!(text.text.contains("[assistant]: [@codex]: 第二条"));
        assert!(text.text.contains("[tool_call:read#call-1]: {\"path\":\"README.md\"}"));
        assert!(text.text.contains("[tool_result:read#call-1]: ok"));
        assert!(text.text.contains("第三条"));
    }
}
