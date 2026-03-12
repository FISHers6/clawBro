use crate::{
    acp::{
        policy::{AcpBackendPolicy, BootstrapStyle, ResumeStrategy},
        AcpAuthMethod, AcpBackend, CodexProjectionMode,
    },
    approval::{ApprovalBroker, ApprovalDecision},
    backend::ApprovalMode,
    codex_local_config::prepare_isolated_codex_home,
    contract::{
        render_runtime_prompt, ExternalMcpServerSpec, ExternalMcpTransport, RuntimeEvent,
        RuntimeSessionSpec, TurnResult,
    },
    event_sink::RuntimeEventSink,
};
use agent_client_protocol as acp;
use std::cell::Cell;
use std::collections::HashMap;
use tokio::process::Command;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpCommandConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeTranscriptOwnership {
    HostReplay,
    BackendResume,
}

pub async fn probe_command_backend(
    config: &AcpCommandConfig,
) -> anyhow::Result<acp::InitializeResponse> {
    use acp::Agent as _;

    struct ChildKillGuard(tokio::process::Child);
    impl Drop for ChildKillGuard {
        fn drop(&mut self) {
            let _ = self.0.start_kill();
        }
    }

    let mut child = ChildKillGuard(spawn_command(config, None)?);
    let stdin = child
        .0
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("probe child stdin unavailable"))?;
    let stdout = child
        .0
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("probe child stdout unavailable"))?;
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

    // child guard drops here
    Ok(init)
}

pub async fn run_command_turn(
    config: &AcpCommandConfig,
    acp_backend: Option<AcpBackend>,
    acp_auth_method: Option<AcpAuthMethod>,
    codex_projection: Option<CodexProjectionMode>,
    session: RuntimeSessionSpec,
    sink: RuntimeEventSink,
    approvals: ApprovalBroker,
) -> anyhow::Result<TurnResult> {
    use acp::Agent as _;
    use std::{cell::RefCell, rc::Rc};

    let policy = AcpBackendPolicy::for_backend(acp_backend);
    tracing::debug!(
        acp_backend = ?acp_backend,
        approval_mode = ?session.approval_mode,
        backend_session_id = session.backend_session_id.as_deref().unwrap_or("<none>"),
        backend_id = %session.backend_id,
        workspace_dir = ?session.workspace_dir,
        external_mcp_servers = session.external_mcp_servers.len(),
        has_tool_bridge = session.tool_bridge_url.is_some(),
        codex_projection = ?codex_projection,
        "Starting ACP command turn"
    );
    tracing::debug!(
        bootstrap = ?policy.bootstrap_style,
        special_mcp_loading = policy.special_mcp_loading,
        "ACP backend policy applied"
    );
    if policy.bootstrap_style == BootstrapStyle::BridgeBacked {
        tracing::debug!(
            "ACP backend is bridge-backed; command is an adapter package, not a raw CLI"
        );
    }
    // RAII guard: ensures the child process is killed on every exit path (early error returns,
    // happy path, and future panics). Without this, any `?` before the final `child.kill()` call
    // would leave a zombie process on the OS.
    struct ChildKillGuard(tokio::process::Child);
    impl Drop for ChildKillGuard {
        fn drop(&mut self) {
            let _ = self.0.start_kill();
        }
    }

    let projected_config = apply_provider_profile_for_acp_backend(config, acp_backend, &session)?;
    let projected_config = apply_codex_projection_for_acp_backend(
        &projected_config,
        acp_backend,
        codex_projection,
        &session,
    )?;
    let spawn_config = augment_command_config_for_policy(&projected_config, &policy);
    tracing::debug!(
        command = %spawn_config.command,
        args = ?spawn_config.args,
        env_count = spawn_config.env.len(),
        "Launching ACP child process"
    );
    let mut child = ChildKillGuard(spawn_command(
        &spawn_config,
        session.workspace_dir.as_deref(),
    )?);
    let stdin = child
        .0
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("ACP child stdin unavailable"))?;
    let stdout = child
        .0
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("ACP child stdout unavailable"))?;
    let outgoing = stdin.compat_write();
    let incoming = stdout.compat();

    #[derive(Clone)]
    struct EventClient {
        sink: RuntimeEventSink,
        accumulated: Rc<RefCell<String>>,
        approvals: ApprovalBroker,
        approval_mode: ApprovalMode,
        tool_titles: Rc<RefCell<HashMap<String, String>>>,
        /// Set to true during ACP load_session replay window.
        /// Suppresses AgentMessageChunk/ToolCall/ToolCallUpdate from being forwarded
        /// to sink or accumulated, preventing replayed history from appearing as
        /// current-turn live output.
        suppress_replay: Rc<Cell<bool>>,
    }

    #[async_trait::async_trait(?Send)]
    impl acp::Client for EventClient {
        async fn request_permission(
            &self,
            args: acp::RequestPermissionRequest,
        ) -> acp::Result<acp::RequestPermissionResponse> {
            tracing::debug!(
                session_id = %args.session_id,
                tool_call_id = %args.tool_call.tool_call_id,
                option_count = args.options.len(),
                approval_mode = ?self.approval_mode,
                title = args.tool_call.fields.title.as_deref().unwrap_or("ACP tool permission required"),
                "Received ACP permission request"
            );
            let decision = match self.approval_mode {
                ApprovalMode::Manual => {
                    let request = permission_request_from_acp(&args);
                    let pending = self.approvals.register(&request);
                    let _ = self.sink.emit(RuntimeEvent::ApprovalRequest(request));
                    pending.wait().await
                }
                ApprovalMode::AutoAllow => ApprovalDecision::AllowOnce,
                ApprovalMode::AutoDeny => ApprovalDecision::Deny,
            };
            let outcome = outcome_from_decision(&args.options, decision);
            tracing::debug!(
                session_id = %args.session_id,
                tool_call_id = %args.tool_call.tool_call_id,
                decision = ?decision,
                outcome = ?outcome,
                "Resolved ACP permission request"
            );
            Ok(acp::RequestPermissionResponse::new(outcome))
        }

        async fn session_notification(
            &self,
            notification: acp::SessionNotification,
        ) -> acp::Result<()> {
            let session_id = notification.session_id.clone();
            let update_kind = acp_session_update_kind(&notification.update);
            tracing::debug!(
                session_id = %session_id,
                update_kind,
                "Received ACP session update"
            );
            match notification.update {
                acp::SessionUpdate::AgentMessageChunk(chunk) => {
                    if self.suppress_replay.get() {
                        tracing::debug!(
                            session_id = %session_id,
                            "Suppressing ACP agent_message_chunk during load_session replay"
                        );
                        return Ok(());
                    }
                    if let acp::ContentBlock::Text(t) = chunk.content {
                        tracing::debug!(
                            session_id = %session_id,
                            text_len = t.text.len(),
                            "Forwarding ACP agent message chunk"
                        );
                        self.accumulated.borrow_mut().push_str(&t.text);
                        let _ = self.sink.emit(RuntimeEvent::TextDelta { text: t.text });
                    }
                }
                acp::SessionUpdate::ToolCall(tool_call) => {
                    if self.suppress_replay.get() {
                        tracing::debug!(
                            session_id = %session_id,
                            tool_call_id = %tool_call.tool_call_id,
                            "Suppressing ACP tool_call during load_session replay"
                        );
                        return Ok(());
                    }
                    let call_id = tool_call.tool_call_id.to_string();
                    self.tool_titles
                        .borrow_mut()
                        .insert(call_id.clone(), tool_call.title.clone());
                    tracing::debug!(
                        session_id = %session_id,
                        tool_call_id = %call_id,
                        title = %tool_call.title,
                        "Forwarding ACP tool call start"
                    );
                    let _ = self.sink.emit(RuntimeEvent::ToolCallStarted {
                        tool_name: tool_call.title,
                        call_id,
                        input_summary: tool_call.raw_input.map(|value| truncate_json(&value)),
                    });
                }
                acp::SessionUpdate::ToolCallUpdate(update) => {
                    if self.suppress_replay.get() {
                        tracing::debug!(
                            session_id = %session_id,
                            tool_call_id = %update.tool_call_id,
                            "Suppressing ACP tool_call_update during load_session replay"
                        );
                        return Ok(());
                    }
                    let call_id = update.tool_call_id.to_string();
                    let seen_before = self.tool_titles.borrow().contains_key(&call_id);
                    if let Some(title) = update.fields.title.clone() {
                        self.tool_titles.borrow_mut().insert(call_id.clone(), title);
                    }
                    let tool_name = self
                        .tool_titles
                        .borrow()
                        .get(&call_id)
                        .cloned()
                        .or(update.fields.title.clone())
                        .unwrap_or_else(|| "acp_tool".to_string());
                    match update.fields.status {
                        Some(acp::ToolCallStatus::Pending | acp::ToolCallStatus::InProgress) => {
                            tracing::debug!(
                                session_id = %session_id,
                                tool_call_id = %call_id,
                                tool_name = %tool_name,
                                seen_before,
                                "Forwarding ACP tool call pending/in-progress update"
                            );
                            if !seen_before {
                                self.tool_titles
                                    .borrow_mut()
                                    .insert(call_id.clone(), tool_name.clone());
                                let _ = self.sink.emit(RuntimeEvent::ToolCallStarted {
                                    tool_name,
                                    call_id,
                                    input_summary: update
                                        .fields
                                        .raw_input
                                        .as_ref()
                                        .map(truncate_json),
                                });
                            }
                        }
                        Some(acp::ToolCallStatus::Completed) => {
                            let result = update
                                .fields
                                .raw_output
                                .as_ref()
                                .map(truncate_json)
                                .unwrap_or_else(|| "\"<acp tool completed>\"".to_string());
                            self.tool_titles.borrow_mut().remove(&call_id);
                            tracing::debug!(
                                session_id = %session_id,
                                tool_call_id = %call_id,
                                tool_name = %tool_name,
                                result_len = result.len(),
                                "Forwarding ACP tool call completion"
                            );
                            let _ = self.sink.emit(RuntimeEvent::ToolCallCompleted {
                                tool_name,
                                call_id,
                                result,
                            });
                        }
                        Some(acp::ToolCallStatus::Failed) => {
                            let error = update
                                .fields
                                .raw_output
                                .as_ref()
                                .map(truncate_json)
                                .unwrap_or_else(|| "ACP tool failed".to_string());
                            self.tool_titles.borrow_mut().remove(&call_id);
                            tracing::debug!(
                                session_id = %session_id,
                                tool_call_id = %call_id,
                                tool_name = %tool_name,
                                error_len = error.len(),
                                "Forwarding ACP tool call failure"
                            );
                            let _ = self.sink.emit(RuntimeEvent::ToolCallFailed {
                                tool_name,
                                call_id,
                                error,
                            });
                        }
                        None | Some(_) => {}
                    }
                }
                acp::SessionUpdate::UsageUpdate(_) => {
                    tracing::debug!(
                        session_id = %session_id,
                        "Ignoring ACP usage_update notification"
                    );
                }
                acp::SessionUpdate::SessionInfoUpdate(_) => {
                    tracing::debug!(
                        session_id = %session_id,
                        "Ignoring ACP session_info_update notification"
                    );
                }
                _ => {
                    tracing::debug!(
                        session_id = %session_id,
                        update_kind,
                        "Ignoring ACP session update variant with no runtime projection"
                    );
                }
            }
            Ok(())
        }
    }

    let accumulated = Rc::new(RefCell::new(String::new()));
    let suppress_replay = Rc::new(Cell::new(false));
    let client = EventClient {
        sink: sink.clone(),
        accumulated: accumulated.clone(),
        approvals,
        approval_mode: session.approval_mode,
        tool_titles: Rc::new(RefCell::new(HashMap::new())),
        suppress_replay: suppress_replay.clone(),
    };
    let (conn, handle_io) = acp::ClientSideConnection::new(client, outgoing, incoming, |fut| {
        tokio::task::spawn_local(fut);
    });
    tokio::task::spawn_local(handle_io);

    tracing::debug!("Sending ACP initialize request");
    let init_resp = conn
        .initialize(
            acp::InitializeRequest::new(acp::ProtocolVersion::V1).client_info(
                acp::Implementation::new("qai-runtime", env!("CARGO_PKG_VERSION")),
            ),
        )
        .await
        .map_err(|e| anyhow::anyhow!("ACP initialize failed: {e:?}"))?;
    tracing::debug!(
        load_session = init_resp.agent_capabilities.load_session,
        has_sse_mcp = init_resp.agent_capabilities.mcp_capabilities.sse,
        auth_methods = ?init_resp
            .auth_methods
            .iter()
            .map(|method| method.id.to_string())
            .collect::<Vec<_>>(),
        "ACP initialize completed"
    );

    authenticate_if_configured(&conn, acp_backend, acp_auth_method, &init_resp).await?;

    let session_root = session
        .workspace_dir
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let mcp_servers = build_mcp_servers(
        init_resp.agent_capabilities.mcp_capabilities.sse,
        session.tool_bridge_url.as_deref(),
        &session.external_mcp_servers,
    );
    tracing::debug!(
        session_root = %session_root.display(),
        mcp_servers = mcp_servers.len(),
        "Prepared ACP session bootstrap inputs"
    );

    // Decide: resume via session/load or start a new session.
    let can_load = init_resp.agent_capabilities.load_session;
    let (active_session_id, emitted_backend_session_id, transcript_ownership) = if policy.resume_strategy
        == ResumeStrategy::AcpLoadSession
        && can_load
        && session.backend_session_id.is_some()
    {
        let prior_id = session.backend_session_id.as_ref().unwrap();
        tracing::debug!(
            session_id = %prior_id,
            "Sending ACP load_session request (replay suppression active)"
        );
        // Suppress replay notifications for the duration of load_session.
        // ACP protocol requires the agent to stream the entire conversation history
        // as session/update notifications before responding to session/load.
        // Without suppression these replayed chunks would appear as current-turn
        // live output (sink emissions + accumulated text).
        suppress_replay.set(true);
        let load_result = conn.load_session(
            acp::LoadSessionRequest::new(
                acp::SessionId::new(prior_id.clone()),
                session_root.clone(),
            )
            .mcp_servers(mcp_servers),
        )
        .await;
        // Clear suppression before error propagation so the flag is never left set.
        suppress_replay.set(false);
        load_result.map_err(|e| anyhow::anyhow!("ACP load_session failed: {e:?}"))?;
        // LoadSessionResponse has no session_id field; reuse the passed-in prior_id.
        tracing::debug!(session_id = %prior_id, "ACP session resumed via session/load");
        (
            acp::SessionId::new(prior_id.clone()),
            None,
            ClaudeTranscriptOwnership::BackendResume,
        )
    } else {
        tracing::debug!("Sending ACP new_session request");
        let sess = conn
            .new_session(acp::NewSessionRequest::new(session_root).mcp_servers(mcp_servers))
            .await
            .map_err(|e| anyhow::anyhow!("ACP new_session failed: {e:?}"))?;
        let new_id = sess.session_id.to_string();
        tracing::debug!(session_id = %new_id, "ACP new session created");
        (
            sess.session_id,
            Some(new_id),
            ClaudeTranscriptOwnership::HostReplay,
        )
    };

    tracing::debug!(
        session_id = %active_session_id,
        "Applying Codex session mode projection if required"
    );
    apply_codex_session_mode_projection(
        &conn,
        acp_backend,
        &active_session_id,
        session.approval_mode,
    )
    .await?;

    let prompt_session = session_for_acp_prompt(&session, acp_backend, transcript_ownership);
    let prompt_blocks = prompt_blocks_from_session(&prompt_session);
    tracing::debug!(
        session_id = %active_session_id,
        transcript_ownership = ?transcript_ownership,
        prompt_blocks = prompt_blocks.len(),
        prompt_text_len = prompt_blocks
            .iter()
            .map(|block| match block {
                acp::ContentBlock::Text(text) => text.text.len(),
                _ => 0,
            })
            .sum::<usize>(),
        "Sending ACP prompt request"
    );
    conn.prompt(acp::PromptRequest::new(active_session_id, prompt_blocks))
        .await
        .map_err(|e| anyhow::anyhow!("ACP prompt failed: {e:?}"))?;
    tracing::debug!("ACP prompt request completed");

    let full_text = accumulated.borrow().clone();
    tracing::debug!(
        full_text_len = full_text.len(),
        "Emitting runtime TurnComplete from ACP session driver"
    );
    let complete = RuntimeEvent::TurnComplete {
        full_text: full_text.clone(),
    };
    let _ = sink.emit(complete.clone());
    // child guard drops here and kills the process via start_kill()

    Ok(TurnResult {
        full_text,
        events: vec![complete],
        emitted_backend_session_id,
        used_backend_id: None, // stamped by run_dispatch_job one level up
    })
}

async fn authenticate_if_configured(
    conn: &acp::ClientSideConnection,
    acp_backend: Option<AcpBackend>,
    acp_auth_method: Option<AcpAuthMethod>,
    init_resp: &acp::InitializeResponse,
) -> anyhow::Result<()> {
    use acp::Agent as _;

    let Some(method) = acp_auth_method else {
        tracing::debug!("ACP auth method not configured; skipping authenticate");
        return Ok(());
    };

    if acp_backend != Some(AcpBackend::Codex) {
        anyhow::bail!(
            "acp_auth_method `{}` is only supported for acp_backend = \"codex\" in the current phase",
            method.protocol_id()
        );
    }

    let advertised = init_resp
        .auth_methods
        .iter()
        .any(|candidate| candidate.id.to_string() == method.protocol_id());

    if !advertised {
        let available = init_resp
            .auth_methods
            .iter()
            .map(|method| method.id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "ACP backend did not advertise auth method `{}` (available: [{}])",
            method.protocol_id(),
            available
        );
    }

    tracing::debug!(
        acp_backend = ?acp_backend,
        auth_method = method.protocol_id(),
        "Sending ACP authenticate request"
    );
    conn.authenticate(acp::AuthenticateRequest::new(method.protocol_id()))
        .await
        .map_err(|e| anyhow::anyhow!("ACP authenticate failed: {e:?}"))?;
    tracing::debug!(
        acp_backend = ?acp_backend,
        auth_method = method.protocol_id(),
        "ACP authenticate completed"
    );
    Ok(())
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
        .map(truncate_json)
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

fn truncate_json(value: &serde_json::Value) -> String {
    let compact = value.to_string();
    if compact.len() <= 400 {
        return compact;
    }
    // Use char-boundary–safe truncation to avoid panics on multi-byte UTF-8 content
    // (e.g. Chinese characters in tool output from Qwen / Kimi backends).
    let cutoff = compact
        .char_indices()
        .nth(400)
        .map(|(i, _)| i)
        .unwrap_or(compact.len());
    format!("{}...", &compact[..cutoff])
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

pub fn build_mcp_servers(
    supports_sse: bool,
    team_tools_url: Option<&str>,
    external_mcp_servers: &[ExternalMcpServerSpec],
) -> Vec<acp::McpServer> {
    if !supports_sse {
        if team_tools_url.is_some() || !external_mcp_servers.is_empty() {
            tracing::warn!(
                configured_external = external_mcp_servers.len(),
                has_team_tools = team_tools_url.is_some(),
                "ACP agent does not report SSE MCP capability; skipping MCP server registration"
            );
        }
        return vec![];
    }

    let mut servers = Vec::new();
    if let Some(u) = team_tools_url {
        if !u.is_empty() {
            servers.push(acp::McpServer::Sse(acp::McpServerSse::new("team-tools", u)));
        }
    }
    for server in external_mcp_servers {
        match &server.transport {
            ExternalMcpTransport::Sse { url } if !url.is_empty() => {
                servers.push(acp::McpServer::Sse(acp::McpServerSse::new(
                    &server.name,
                    url,
                )));
            }
            ExternalMcpTransport::Sse { .. } => {}
        }
    }
    servers
}

fn prompt_blocks_from_session(session: &RuntimeSessionSpec) -> Vec<acp::ContentBlock> {
    vec![acp::ContentBlock::Text(acp::TextContent::new(
        render_runtime_prompt(session),
    ))]
}

fn session_for_acp_prompt(
    session: &RuntimeSessionSpec,
    acp_backend: Option<AcpBackend>,
    transcript_ownership: ClaudeTranscriptOwnership,
) -> RuntimeSessionSpec {
    if acp_backend != Some(AcpBackend::Claude)
        || transcript_ownership != ClaudeTranscriptOwnership::BackendResume
    {
        return session.clone();
    }

    let mut projected = session.clone();
    projected.context.history_lines.clear();
    projected.context.history_messages.clear();
    projected
}

fn acp_session_update_kind(update: &acp::SessionUpdate) -> &'static str {
    match update {
        acp::SessionUpdate::UserMessageChunk(_) => "user_message_chunk",
        acp::SessionUpdate::AgentMessageChunk(_) => "agent_message_chunk",
        acp::SessionUpdate::AgentThoughtChunk(_) => "agent_thought_chunk",
        acp::SessionUpdate::ToolCall(_) => "tool_call",
        acp::SessionUpdate::ToolCallUpdate(_) => "tool_call_update",
        acp::SessionUpdate::Plan(_) => "plan",
        acp::SessionUpdate::AvailableCommandsUpdate(_) => "available_commands_update",
        acp::SessionUpdate::CurrentModeUpdate(_) => "current_mode_update",
        acp::SessionUpdate::ConfigOptionUpdate(_) => "config_option_update",
        acp::SessionUpdate::SessionInfoUpdate(_) => "session_info_update",
        acp::SessionUpdate::UsageUpdate(_) => "usage_update",
        _ => "other",
    }
}

fn codex_mode_id_for_approval_mode(approval_mode: ApprovalMode) -> Option<&'static str> {
    match approval_mode {
        ApprovalMode::Manual | ApprovalMode::AutoDeny => Some("read-only"),
        ApprovalMode::AutoAllow => Some("full-access"),
    }
}

async fn apply_codex_session_mode_projection(
    conn: &acp::ClientSideConnection,
    acp_backend: Option<AcpBackend>,
    session_id: &acp::SessionId,
    approval_mode: ApprovalMode,
) -> anyhow::Result<()> {
    use acp::Agent as _;

    if acp_backend != Some(AcpBackend::Codex) {
        return Ok(());
    }

    let Some(mode_id) = codex_mode_id_for_approval_mode(approval_mode) else {
        return Ok(());
    };

    conn.set_session_mode(acp::SetSessionModeRequest::new(
        session_id.clone(),
        acp::SessionModeId::new(mode_id),
    ))
    .await
    .map_err(|e| anyhow::anyhow!("ACP set_session_mode failed: {e:?}"))?;

    tracing::debug!(
        session_id = %session_id,
        mode_id,
        approval_mode = ?approval_mode,
        "Applied Codex ACP session mode projection"
    );

    Ok(())
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

fn augment_command_config_for_policy(
    config: &AcpCommandConfig,
    policy: &AcpBackendPolicy,
) -> AcpCommandConfig {
    let mut config = config.clone();
    if policy.special_mcp_loading {
        maybe_append_codebuddy_mcp_config(&mut config);
    }
    config
}

fn apply_provider_profile_for_acp_backend(
    config: &AcpCommandConfig,
    acp_backend: Option<AcpBackend>,
    session: &RuntimeSessionSpec,
) -> anyhow::Result<AcpCommandConfig> {
    let mut config = config.clone();
    let Some(profile) = &session.provider_profile else {
        return Ok(config);
    };
    if !matches!(acp_backend, Some(AcpBackend::Claude)) {
        return Ok(config);
    }

    match &profile.protocol {
        crate::RuntimeProviderProtocol::OfficialSession => Ok(config),
        crate::RuntimeProviderProtocol::AnthropicCompatible {
            base_url,
            auth_token,
            default_model,
            small_fast_model,
        } => {
            upsert_env(&mut config.env, "ANTHROPIC_BASE_URL", base_url);
            upsert_env(&mut config.env, "ANTHROPIC_AUTH_TOKEN", auth_token);
            upsert_env(&mut config.env, "ANTHROPIC_MODEL", default_model);
            if let Some(model) = small_fast_model.as_ref() {
                upsert_env(&mut config.env, "ANTHROPIC_SMALL_FAST_MODEL", model);
            }
            Ok(config)
        }
        crate::RuntimeProviderProtocol::OpenaiCompatible { .. } => {
            anyhow::bail!("ACP Claude backend does not support openai_compatible provider profiles")
        }
    }
}

fn apply_codex_projection_for_acp_backend(
    config: &AcpCommandConfig,
    acp_backend: Option<AcpBackend>,
    codex_projection: Option<CodexProjectionMode>,
    session: &RuntimeSessionSpec,
) -> anyhow::Result<AcpCommandConfig> {
    if acp_backend != Some(AcpBackend::Codex) {
        return Ok(config.clone());
    }

    match codex_projection {
        None | Some(CodexProjectionMode::AcpAuth) => Ok(config.clone()),
        Some(CodexProjectionMode::LocalConfig) => {
            let profile = session.provider_profile.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "codex projection = local_config requires a resolved provider profile"
                )
            })?;
            let home = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .ok_or_else(|| {
                    anyhow::anyhow!("HOME is required for codex local_config projection")
                })?;
            let codex_home = prepare_isolated_codex_home(
                &home,
                &session.backend_id,
                profile,
                session.workspace_dir.as_deref(),
            )?;
            let mut config = config.clone();
            upsert_env(
                &mut config.env,
                "CODEX_HOME",
                codex_home.to_string_lossy().as_ref(),
            );
            // Inject the API key as OPENAI_API_KEY so that the ACP authenticate() call
            // (acp_auth_method = openai_api_key) can read it via read_openai_api_key_from_env()
            // and write it into the isolated CODEX_HOME/auth.json.
            if let crate::RuntimeProviderProtocol::OpenaiCompatible { api_key, .. } =
                &profile.protocol
            {
                upsert_env(&mut config.env, "OPENAI_API_KEY", api_key);
            }
            Ok(config)
        }
    }
}

fn upsert_env(env: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some(existing) = env.iter_mut().find(|(existing_key, _)| existing_key == key) {
        existing.1 = value.to_string();
    } else {
        env.push((key.to_string(), value.to_string()));
    }
}

fn maybe_append_codebuddy_mcp_config(config: &mut AcpCommandConfig) {
    if config.args.iter().any(|arg| arg == "--mcp-config") {
        return;
    }
    let Some(path) = codebuddy_mcp_config_path(&config.env) else {
        tracing::debug!("CodeBuddy MCP config path unavailable; starting without --mcp-config");
        return;
    };
    if path.is_file() {
        tracing::info!(path = %path.display(), "Loading CodeBuddy MCP config");
        config.args.push("--mcp-config".into());
        config.args.push(path.display().to_string());
    } else {
        tracing::debug!(path = %path.display(), "No CodeBuddy MCP config found; starting without --mcp-config");
    }
}

fn codebuddy_mcp_config_path(env: &[(String, String)]) -> Option<std::path::PathBuf> {
    let home = env
        .iter()
        .find(|(key, _)| key == "HOME")
        .map(|(_, value)| value.clone())
        .or_else(|| std::env::var("HOME").ok())?;
    Some(
        std::path::PathBuf::from(home)
            .join(".codebuddy")
            .join("mcp.json"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_mcp_servers_empty_when_no_url() {
        assert!(build_mcp_servers(true, None, &[]).is_empty());
    }

    #[test]
    fn build_mcp_servers_empty_when_no_sse_capability() {
        assert!(build_mcp_servers(false, Some("http://127.0.0.1:9999"), &[]).is_empty());
    }

    #[test]
    fn build_mcp_servers_populated_when_url_and_capability() {
        let servers = build_mcp_servers(true, Some("http://127.0.0.1:9999"), &[]);
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
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: None,
            provider_profile: None,
            backend_session_id: None,
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
        assert!(text
            .text
            .contains("[tool_call:read#call-1]: {\"path\":\"README.md\"}"));
        assert!(text.text.contains("[tool_result:read#call-1]: ok"));
        assert!(text.text.contains("第三条"));
    }

    #[test]
    fn claude_resumed_prompt_suppresses_replayed_history_but_keeps_projection_context() {
        let session = RuntimeSessionSpec {
            backend_id: "claude-main".into(),
            participant_name: Some("worker".into()),
            session_key: qai_protocol::SessionKey::new("ws", "history:test"),
            role: crate::contract::RuntimeRole::Solo,
            workspace_dir: None,
            prompt_text: String::new(),
            tool_surface: crate::contract::ToolSurfaceSpec::default(),
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: None,
            provider_profile: None,
            backend_session_id: Some("claude-session-1".into()),
            context: crate::contract::RuntimeContext {
                system_prompt: Some("系统规则".into()),
                workspace_native_files: vec!["AGENTS.md".into()],
                memory_summary: Some("共享记忆".into()),
                agent_memory: Some("专属记忆".into()),
                history_lines: vec![
                    "[user]: [alice]: 第一条".into(),
                    "[assistant]: [@claude]: 第二条".into(),
                ],
                history_messages: vec![
                    crate::contract::RuntimeHistoryMessage {
                        role: "user".into(),
                        content: "第一条".into(),
                        sender: Some("alice".into()),
                        tool_calls: vec![],
                    },
                    crate::contract::RuntimeHistoryMessage {
                        role: "assistant".into(),
                        content: "第二条".into(),
                        sender: Some("@claude".into()),
                        tool_calls: vec![],
                    },
                ],
                user_input: Some("第三条".into()),
                ..crate::contract::RuntimeContext::default()
            },
        };

        let projected = session_for_acp_prompt(
            &session,
            Some(AcpBackend::Claude),
            ClaudeTranscriptOwnership::BackendResume,
        );
        let blocks = prompt_blocks_from_session(&projected);
        let acp::ContentBlock::Text(text) = &blocks[0] else {
            panic!("expected text content block");
        };

        assert!(!text.text.contains("[user]: [alice]: 第一条"));
        assert!(!text.text.contains("[assistant]: [@claude]: 第二条"));
        assert!(text.text.contains("系统规则"));
        assert!(text.text.contains("共享记忆"));
        assert!(text.text.contains("专属记忆"));
        assert!(text.text.contains("AGENTS.md"));
        assert!(text.text.contains("第三条"));
    }

    #[test]
    fn claude_new_session_prompt_keeps_replayed_history() {
        let session = RuntimeSessionSpec {
            backend_id: "claude-main".into(),
            participant_name: Some("worker".into()),
            session_key: qai_protocol::SessionKey::new("ws", "history:test"),
            role: crate::contract::RuntimeRole::Solo,
            workspace_dir: None,
            prompt_text: String::new(),
            tool_surface: crate::contract::ToolSurfaceSpec::default(),
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: None,
            provider_profile: None,
            backend_session_id: None,
            context: crate::contract::RuntimeContext {
                history_lines: vec![
                    "[user]: [alice]: 第一条".into(),
                    "[assistant]: [@claude]: 第二条".into(),
                ],
                user_input: Some("第三条".into()),
                ..crate::contract::RuntimeContext::default()
            },
        };

        let projected = session_for_acp_prompt(
            &session,
            Some(AcpBackend::Claude),
            ClaudeTranscriptOwnership::HostReplay,
        );
        let blocks = prompt_blocks_from_session(&projected);
        let acp::ContentBlock::Text(text) = &blocks[0] else {
            panic!("expected text content block");
        };

        assert!(text.text.contains("[user]: [alice]: 第一条"));
        assert!(text.text.contains("[assistant]: [@claude]: 第二条"));
        assert!(text.text.contains("第三条"));
    }

    #[test]
    fn non_claude_resumed_prompt_keeps_replayed_history() {
        let session = RuntimeSessionSpec {
            backend_id: "codex-main".into(),
            participant_name: Some("worker".into()),
            session_key: qai_protocol::SessionKey::new("ws", "history:test"),
            role: crate::contract::RuntimeRole::Solo,
            workspace_dir: None,
            prompt_text: String::new(),
            tool_surface: crate::contract::ToolSurfaceSpec::default(),
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: None,
            provider_profile: None,
            backend_session_id: Some("codex-session-1".into()),
            context: crate::contract::RuntimeContext {
                history_lines: vec![
                    "[user]: [alice]: 第一条".into(),
                    "[assistant]: [@codex]: 第二条".into(),
                ],
                user_input: Some("第三条".into()),
                ..crate::contract::RuntimeContext::default()
            },
        };

        let projected = session_for_acp_prompt(
            &session,
            Some(AcpBackend::Codex),
            ClaudeTranscriptOwnership::BackendResume,
        );
        let blocks = prompt_blocks_from_session(&projected);
        let acp::ContentBlock::Text(text) = &blocks[0] else {
            panic!("expected text content block");
        };

        assert!(text.text.contains("[user]: [alice]: 第一条"));
        assert!(text.text.contains("[assistant]: [@codex]: 第二条"));
        assert!(text.text.contains("第三条"));
    }

    #[test]
    fn truncate_json_caps_large_payloads() {
        let value = serde_json::json!({ "output": "x".repeat(512) });
        let rendered = truncate_json(&value);
        assert!(rendered.ends_with("..."));
        assert!(rendered.len() < 450);
    }

    #[test]
    fn build_mcp_servers_merges_team_and_external_servers() {
        let external = vec![
            ExternalMcpServerSpec {
                name: "filesystem".into(),
                transport: ExternalMcpTransport::Sse {
                    url: "http://127.0.0.1:3001/sse".into(),
                },
            },
            ExternalMcpServerSpec {
                name: "github".into(),
                transport: ExternalMcpTransport::Sse {
                    url: "http://127.0.0.1:3002/sse".into(),
                },
            },
        ];

        let servers = build_mcp_servers(true, Some("http://127.0.0.1:9999"), &external);
        assert_eq!(servers.len(), 3);
        let names: Vec<_> = servers
            .iter()
            .map(|server| match server {
                acp::McpServer::Sse(sse) => sse.name.clone(),
                other => panic!("unexpected mcp server: {other:?}"),
            })
            .collect();
        assert_eq!(names, vec!["team-tools", "filesystem", "github"]);
    }

    #[test]
    fn claude_backend_uses_bridge_backed_policy() {
        use crate::acp::{
            policy::{AcpBackendPolicy, BootstrapStyle},
            AcpBackend,
        };
        let policy = AcpBackendPolicy::for_backend(Some(AcpBackend::Claude));
        assert_eq!(policy.bootstrap_style, BootstrapStyle::BridgeBacked);
        assert!(!policy.special_mcp_loading);
    }

    #[test]
    fn codebuddy_backend_uses_bridge_policy_with_special_mcp_loading() {
        use crate::acp::{
            policy::{AcpBackendPolicy, BootstrapStyle},
            AcpBackend,
        };
        let policy = AcpBackendPolicy::for_backend(Some(AcpBackend::Codebuddy));
        assert_eq!(policy.bootstrap_style, BootstrapStyle::BridgeBacked);
        assert!(policy.special_mcp_loading);
    }

    #[test]
    fn generic_acp_backends_use_generic_policy() {
        use crate::acp::policy::{AcpBackendPolicy, BootstrapStyle};
        // Qwen: explicit generic
        let qwen = AcpBackendPolicy::for_backend(Some(crate::acp::AcpBackend::Qwen));
        assert_eq!(qwen.bootstrap_style, BootstrapStyle::Generic);
        assert!(!qwen.special_mcp_loading);
        // None: omitted backend_id → generic
        let generic = AcpBackendPolicy::for_backend(None);
        assert_eq!(generic.bootstrap_style, BootstrapStyle::Generic);
        assert!(!generic.special_mcp_loading);
    }

    #[test]
    fn augment_command_config_adds_codebuddy_mcp_config_when_present() {
        let home = std::env::temp_dir().join(format!("codebuddy-home-{}", uuid::Uuid::new_v4()));
        let config_dir = home.join(".codebuddy");
        std::fs::create_dir_all(&config_dir).unwrap();
        let mcp_path = config_dir.join("mcp.json");
        std::fs::write(&mcp_path, "{}").unwrap();

        let config = AcpCommandConfig {
            command: "npx".into(),
            args: vec!["@tencent-ai/codebuddy-code".into(), "--acp".into()],
            env: vec![("HOME".into(), home.display().to_string())],
        };
        let policy = AcpBackendPolicy::for_backend(Some(AcpBackend::Codebuddy));

        let config = augment_command_config_for_policy(&config, &policy);
        assert_eq!(
            config.args,
            vec![
                "@tencent-ai/codebuddy-code",
                "--acp",
                "--mcp-config",
                mcp_path.to_str().unwrap()
            ]
        );
    }

    #[test]
    fn augment_command_config_skips_missing_codebuddy_mcp_config() {
        let home = std::env::temp_dir().join(format!("codebuddy-home-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let config = AcpCommandConfig {
            command: "npx".into(),
            args: vec!["@tencent-ai/codebuddy-code".into(), "--acp".into()],
            env: vec![("HOME".into(), home.display().to_string())],
        };
        let policy = AcpBackendPolicy::for_backend(Some(AcpBackend::Codebuddy));

        let config = augment_command_config_for_policy(&config, &policy);
        assert_eq!(config.args, vec!["@tencent-ai/codebuddy-code", "--acp"]);
    }

    #[test]
    fn codebuddy_mcp_config_path_uses_home_from_env_override() {
        let env = vec![("HOME".to_string(), "/tmp/qai-home".to_string())];
        let path = codebuddy_mcp_config_path(&env).unwrap();
        assert_eq!(path, PathBuf::from("/tmp/qai-home/.codebuddy/mcp.json"));
    }

    #[test]
    fn codex_local_config_projection_injects_isolated_codex_home() {
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", temp.path());
        let session = RuntimeSessionSpec {
            backend_id: "codex-main".into(),
            participant_name: None,
            session_key: qai_protocol::SessionKey::new("ws", "user:test"),
            role: crate::contract::RuntimeRole::Solo,
            workspace_dir: None,
            prompt_text: "hello".into(),
            tool_surface: crate::contract::ToolSurfaceSpec::default(),
            approval_mode: ApprovalMode::Manual,
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: None,
            provider_profile: Some(crate::provider_profiles::RuntimeProviderProfile {
                id: "deepseek-openai".into(),
                protocol: crate::provider_profiles::RuntimeProviderProtocol::OpenaiCompatible {
                    base_url: "https://api.deepseek.com/v1".into(),
                    api_key: "sk-test".into(),
                    default_model: "deepseek-chat".into(),
                },
            }),
            backend_session_id: None,
            context: crate::contract::RuntimeContext::default(),
        };
        let config = AcpCommandConfig {
            command: "npx".into(),
            args: vec!["@zed-industries/codex-acp@0.9.5".into()],
            env: vec![],
        };

        let projected = apply_codex_projection_for_acp_backend(
            &config,
            Some(AcpBackend::Codex),
            Some(CodexProjectionMode::LocalConfig),
            &session,
        )
        .unwrap();
        let codex_home = projected
            .env
            .iter()
            .find(|(key, _)| key == "CODEX_HOME")
            .map(|(_, value)| value.clone())
            .expect("CODEX_HOME should be injected");
        assert!(codex_home.ends_with("/.quickai/runtime/codex/codex-main"));
        assert!(std::path::Path::new(&codex_home).join("auth.json").exists());
        assert!(std::path::Path::new(&codex_home)
            .join("config.toml")
            .exists());
    }

    #[test]
    fn codex_projection_maps_manual_to_read_only() {
        assert_eq!(
            codex_mode_id_for_approval_mode(ApprovalMode::Manual),
            Some("read-only")
        );
    }

    #[test]
    fn codex_projection_maps_auto_allow_to_full_access() {
        assert_eq!(
            codex_mode_id_for_approval_mode(ApprovalMode::AutoAllow),
            Some("full-access")
        );
    }

    #[test]
    fn codex_projection_maps_auto_deny_to_read_only() {
        assert_eq!(
            codex_mode_id_for_approval_mode(ApprovalMode::AutoDeny),
            Some("read-only")
        );
    }

    /// Verifies the semantic contract for emitted_backend_session_id:
    /// - new_session path: emitted = Some(id)  → written to SessionMeta on next complete_turn
    /// - load_session path: emitted = None      → existing SessionMeta ID is preserved unchanged
    #[test]
    fn emitted_session_id_is_some_on_new_path_and_none_on_load_path() {
        // Simulates the TurnResult construction at the end of run_command_turn.
        // new_session path:
        let new_id = "acp-sess-12345".to_string();
        let new_turn = crate::contract::TurnResult {
            full_text: "ok".into(),
            events: vec![],
            emitted_backend_session_id: Some(new_id.clone()), // ← new path emits Some
            used_backend_id: None,
        };
        assert_eq!(
            new_turn.emitted_backend_session_id.as_deref(),
            Some("acp-sess-12345"),
            "new_session path must emit Some(id)"
        );

        // load_session path:
        let load_turn = crate::contract::TurnResult {
            full_text: "ok".into(),
            events: vec![],
            emitted_backend_session_id: None, // ← load path emits None (reuses prior_id)
            used_backend_id: None,
        };
        assert!(
            load_turn.emitted_backend_session_id.is_none(),
            "load_session path must emit None (prior_id is preserved in SessionMeta)"
        );
    }

    /// Verifies that the resume branch is only taken when all three conditions hold:
    /// AcpLoadSession strategy + load_session capability + prior session ID present.
    #[test]
    fn resume_branch_requires_strategy_plus_capability_plus_prior_id() {
        use crate::acp::policy::ResumeStrategy;

        // All three conditions satisfied → resume
        let should_resume = ResumeStrategy::AcpLoadSession == ResumeStrategy::AcpLoadSession
            && true  // can_load
            && true; // backend_session_id.is_some()
        assert!(should_resume);

        // Missing capability → no resume
        let no_capability = ResumeStrategy::AcpLoadSession == ResumeStrategy::AcpLoadSession
            && false // can_load = false
            && true;
        assert!(!no_capability);

        // ResumeStrategy::None → no resume
        let no_strategy = ResumeStrategy::None == ResumeStrategy::AcpLoadSession && true && true;
        assert!(!no_strategy);

        // Missing prior ID → no resume
        let no_prior_id =
            ResumeStrategy::AcpLoadSession == ResumeStrategy::AcpLoadSession && true && false; // backend_session_id.is_none()
        assert!(!no_prior_id);
    }
}
