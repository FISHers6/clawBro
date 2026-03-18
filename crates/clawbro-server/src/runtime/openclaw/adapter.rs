use crate::runtime::{
    adapter::{BackendAdapter, LaunchSpec},
    approval::ApprovalBroker,
    backend::{ApprovalMode, BackendFamily, CapabilityProfile},
    contract::{PermissionRequest, RuntimeSessionSpec, TurnResult},
    event_sink::RuntimeEventSink,
    openclaw::{
        client::{
            canonical_openclaw_session_key, GatewayInbound, OpenClawConnectConfig,
            OpenClawGatewayClient,
        },
        lead_bridge::{OpenClawLeadBridge, OpenClawLeadRunBinding},
        probe::{
            default_openclaw_capability_profile, upgraded_openclaw_lead_profile,
            upgraded_openclaw_team_profile,
        },
        team_bridge::{OpenClawRunBinding, OpenClawTeamBridge},
    },
    registry::BackendSpec,
};
use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct OpenClawBackendAdapter {
    approvals: ApprovalBroker,
}

impl OpenClawBackendAdapter {
    pub fn new(approvals: ApprovalBroker) -> Self {
        Self { approvals }
    }
}

#[async_trait::async_trait(?Send)]
impl BackendAdapter for OpenClawBackendAdapter {
    async fn probe(&self, spec: &BackendSpec) -> anyhow::Result<CapabilityProfile> {
        if spec.family != BackendFamily::OpenClawGateway {
            anyhow::bail!(
                "openclaw adapter requires OpenClawGateway family, got {:?}",
                spec.family
            );
        }
        match &spec.launch {
            LaunchSpec::GatewayWs {
                team_helper_command,
                lead_helper_mode,
                ..
            } => {
                if helper_command_is_ready(team_helper_command.as_deref()).await? {
                    if *lead_helper_mode {
                        Ok(upgraded_openclaw_lead_profile())
                    } else {
                        Ok(upgraded_openclaw_team_profile())
                    }
                } else {
                    Ok(default_openclaw_capability_profile())
                }
            }
            other => anyhow::bail!(
                "openclaw backend '{}' requires LaunchSpec::GatewayWs, got {other:?}",
                spec.backend_id
            ),
        }
    }

    async fn run_turn(
        &self,
        spec: &BackendSpec,
        session: RuntimeSessionSpec,
        sink: RuntimeEventSink,
    ) -> anyhow::Result<TurnResult> {
        if spec.family != BackendFamily::OpenClawGateway {
            anyhow::bail!(
                "openclaw adapter requires OpenClawGateway family, got {:?}",
                spec.family
            );
        }
        let LaunchSpec::GatewayWs {
            endpoint,
            token,
            password,
            role,
            scopes,
            agent_id,
            team_helper_command,
            team_helper_args,
            lead_helper_mode: _,
        } = &spec.launch
        else {
            anyhow::bail!(
                "openclaw backend '{}' requires LaunchSpec::GatewayWs, got {:?}",
                spec.backend_id,
                spec.launch
            );
        };

        let connect = OpenClawConnectConfig {
            endpoint: endpoint.clone(),
            token: token.clone(),
            password: password.clone(),
            role: role.clone().unwrap_or_else(|| "operator".into()),
            scopes: scopes.clone(),
        };
        let mut client = OpenClawGatewayClient::connect(&connect).await?;
        let helper = if session.tool_surface.team_tools {
            Some(resolve_team_helper_config(
                team_helper_command.as_deref(),
                team_helper_args,
                &session,
            )?)
        } else {
            None
        };
        if let Some(helper) = &helper {
            ensure_helper_allowlisted(&mut client, agent_id.as_deref(), helper.command.as_str())
                .await?;
        }
        let session_key = client
            .resolve_session_key(
                &canonical_openclaw_session_key(&session.session_key),
                agent_id.as_deref(),
            )
            .await?;
        let run_id = client
            .send_chat(
                &session_key,
                &augment_prompt_for_openclaw(&session, helper.as_ref()),
            )
            .await?;
        let specialist_bridge = helper.as_ref().and_then(|_| {
            (session.role == crate::runtime::contract::RuntimeRole::Specialist).then(|| {
                OpenClawTeamBridge::new(build_run_binding(
                    spec,
                    &session,
                    &session_key,
                    run_id.clone(),
                ))
            })
        });
        let lead_bridge = helper.as_ref().and_then(|_| {
            (session.role == crate::runtime::contract::RuntimeRole::Leader).then(|| {
                OpenClawLeadBridge::new(build_lead_run_binding(
                    spec,
                    &session,
                    &session_key,
                    run_id.clone(),
                ))
            })
        });

        let mut full_text = String::new();
        let turn_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            let inbound = tokio::time::timeout_at(
                turn_deadline,
                client.read_inbound(&session_key, run_id.as_deref(), &full_text),
            )
            .await
            .map_err(|_| anyhow::anyhow!("OpenClaw turn timed out after 300s"))??;
            match inbound {
                Some(GatewayInbound::Runtime(event)) => match event {
                    crate::runtime::contract::RuntimeEvent::TextDelta { ref text } => {
                        full_text.push_str(text);
                        sink.emit(event)?;
                    }
                    crate::runtime::contract::RuntimeEvent::ApprovalRequest(request) => {
                        let approval_id = request.id.clone();
                        let decision = self
                            .handle_approval_request(&sink, session.approval_mode, request)
                            .await?;
                        client
                            .resolve_exec_approval(&approval_id, &decision)
                            .await
                            .context("failed to resolve OpenClaw exec approval")?;
                    }
                    _ => sink.emit(event)?,
                },
                Some(GatewayInbound::HelperResult(result)) => {
                    if let Some(bridge) = specialist_bridge.as_ref() {
                        sink.emit(bridge.handle_helper_result(&result)?)?;
                    } else if let Some(bridge) = lead_bridge.as_ref() {
                        sink.emit(bridge.handle_helper_result(&result)?)?;
                    } else {
                        anyhow::bail!("received OpenClaw team helper result outside team mode");
                    }
                }
                Some(GatewayInbound::FinalText(text)) => {
                    sink.emit(crate::runtime::contract::RuntimeEvent::TurnComplete {
                        full_text: text.clone(),
                    })?;
                    return Ok(TurnResult {
                        full_text: text,
                        events: Vec::new(),
                        emitted_backend_session_id: None,
                        backend_resume_fingerprint: None,
                        used_backend_id: None,
                        resume_recovery: None,
                    });
                }
                Some(GatewayInbound::Started { .. }) | Some(GatewayInbound::Ack) => continue,
                None => continue,
            }
        }
    }
}

impl OpenClawBackendAdapter {
    async fn handle_approval_request(
        &self,
        sink: &RuntimeEventSink,
        approval_mode: ApprovalMode,
        request: PermissionRequest,
    ) -> anyhow::Result<String> {
        let decision = match approval_mode {
            ApprovalMode::Manual => {
                let pending = self.approvals.register(&request);
                sink.emit(crate::runtime::contract::RuntimeEvent::ApprovalRequest(request))?;
                pending.wait().await
            }
            ApprovalMode::AutoAllow => crate::runtime::approval::ApprovalDecision::AllowOnce,
            ApprovalMode::AutoDeny => crate::runtime::approval::ApprovalDecision::Deny,
        };
        Ok(decision.as_openclaw_str().to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TeamHelperConfig {
    command: String,
    args: Vec<String>,
    session_channel: String,
    session_scope: String,
    team_tool_url: String,
}

fn build_run_binding(
    spec: &BackendSpec,
    session: &RuntimeSessionSpec,
    openclaw_session_key: &str,
    run_id: Option<String>,
) -> OpenClawRunBinding {
    let team_id = session
        .session_key
        .scope
        .split(':')
        .next()
        .unwrap_or(&session.session_key.scope)
        .to_string();
    OpenClawRunBinding {
        backend_id: spec.backend_id.clone(),
        participant_name: session
            .participant_name
            .clone()
            .unwrap_or_else(|| spec.backend_id.clone()),
        team_id,
        task_id: String::new(),
        session_key: session.session_key.clone(),
        openclaw_session_key: openclaw_session_key.to_string(),
        run_id,
        helper_invocation_id: None,
    }
}

fn build_lead_run_binding(
    spec: &BackendSpec,
    session: &RuntimeSessionSpec,
    openclaw_session_key: &str,
    run_id: Option<String>,
) -> OpenClawLeadRunBinding {
    let team_id = session
        .session_key
        .scope
        .split(':')
        .next()
        .unwrap_or(&session.session_key.scope)
        .to_string();
    OpenClawLeadRunBinding {
        backend_id: spec.backend_id.clone(),
        participant_name: session
            .participant_name
            .clone()
            .unwrap_or_else(|| spec.backend_id.clone()),
        session_key: session.session_key.clone(),
        team_id,
        turn_id: None,
        openclaw_session_key: openclaw_session_key.to_string(),
        run_id,
        helper_invocation_id: None,
    }
}

async fn helper_command_is_ready(command: Option<&str>) -> anyhow::Result<bool> {
    let Some(command) = command.filter(|value| !value.trim().is_empty()) else {
        return Ok(false);
    };
    let path = match resolve_helper_path(command) {
        Ok(path) => path,
        Err(_) => return Ok(false),
    };
    match tokio::fs::metadata(path).await {
        Ok(meta) => Ok(meta.is_file() && file_is_executable(&meta)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

fn file_is_executable(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

fn resolve_team_helper_config(
    command: Option<&str>,
    args: &[String],
    session: &RuntimeSessionSpec,
) -> anyhow::Result<TeamHelperConfig> {
    let command = command
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("OpenClaw team mode requires launch.team_helper_command"))?;
    let path = resolve_helper_path(command)?;
    let team_tool_url = session
        .team_tool_url
        .clone()
        .ok_or_else(|| anyhow!("OpenClaw team mode requires session.team_tool_url"))?;
    Ok(TeamHelperConfig {
        command: path.display().to_string(),
        args: args.to_vec(),
        session_channel: session.session_key.channel.clone(),
        session_scope: session.session_key.scope.clone(),
        team_tool_url,
    })
}

fn resolve_helper_path(command: &str) -> anyhow::Result<std::path::PathBuf> {
    let path = Path::new(command);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    which::which(command).map_err(|_| anyhow!("OpenClaw team helper not found in PATH: {command}"))
}

async fn ensure_helper_allowlisted(
    client: &mut OpenClawGatewayClient,
    agent_id: Option<&str>,
    helper_command: &str,
) -> anyhow::Result<()> {
    let approvals = client
        .get_exec_approvals()
        .await
        .context("failed to load OpenClaw exec approvals")?;
    let base_hash = approvals.get("hash").and_then(Value::as_str);
    let mut file = approvals
        .get("file")
        .cloned()
        .unwrap_or_else(|| json!({ "version": 1 }));
    let agent_key = agent_id
        .filter(|id| !id.trim().is_empty())
        .unwrap_or("main");
    let allowlist = ensure_allowlist_array(&mut file, agent_key)?;

    let already_present = allowlist.iter().any(|entry| {
        entry
            .get("pattern")
            .and_then(Value::as_str)
            .map(|pattern| pattern == helper_command)
            .unwrap_or(false)
    });
    if already_present {
        return Ok(());
    }

    allowlist.push(json!({
        "pattern": helper_command,
        "lastUsedAt": 0,
    }));
    client
        .set_exec_approvals(file, base_hash)
        .await
        .context("failed to persist OpenClaw exec approvals allowlist")?;
    Ok(())
}

fn ensure_allowlist_array<'a>(
    file: &'a mut Value,
    agent_key: &str,
) -> anyhow::Result<&'a mut Vec<Value>> {
    let file_obj = file
        .as_object_mut()
        .ok_or_else(|| anyhow!("exec approvals file must be an object"))?;
    let agents = file_obj
        .entry("agents")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("exec approvals agents must be an object"))?;
    let agent = agents
        .entry(agent_key.to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("exec approvals agent entry must be an object"))?;
    agent
        .entry("allowlist")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| anyhow!("exec approvals allowlist must be an array"))
}

fn helper_prefix(helper: &TeamHelperConfig) -> String {
    let mut parts = vec![helper.command.clone()];
    parts.extend(helper.args.clone());
    parts.push("--url".into());
    parts.push(shell_quote(&helper.team_tool_url));
    parts.push("--session-channel".into());
    parts.push(shell_quote(&helper.session_channel));
    parts.push("--session-scope".into());
    parts.push(shell_quote(&helper.session_scope));
    parts.join(" ")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn augment_prompt_for_openclaw(
    session: &RuntimeSessionSpec,
    helper: Option<&TeamHelperConfig>,
) -> String {
    let Some(helper) = helper else {
        return crate::runtime::contract::render_runtime_prompt(session);
    };
    let role_notes = match session.role {
        crate::runtime::contract::RuntimeRole::Leader => [
            format!(
                "`{} create-task --id <task-id> --title <title> --assignee <agent>`",
                helper_prefix(helper)
            ),
            format!("`{} start-execution`", helper_prefix(helper)),
            format!("`{} get-task-status`", helper_prefix(helper)),
            format!(
                "`{} assign-task --task-id <task-id> --assignee <agent>`",
                helper_prefix(helper)
            ),
            format!(
                "`{} accept-task --task-id <task-id>`",
                helper_prefix(helper)
            ),
            format!(
                "`{} reopen-task --task-id <task-id> --reason <reason>`",
                helper_prefix(helper)
            ),
            format!(
                "`{} post-update --message <message>`",
                helper_prefix(helper)
            ),
        ]
        .join("\n"),
        crate::runtime::contract::RuntimeRole::Specialist => [
            format!(
                "`{} checkpoint-task --task-id <task-id> --note <note>`",
                helper_prefix(helper)
            ),
            format!(
                "`{} submit-task-result --task-id <task-id> --summary <summary>`",
                helper_prefix(helper)
            ),
            format!(
                "`{} request-help --task-id <task-id> --message <message>`",
                helper_prefix(helper)
            ),
            format!(
                "`{} block-task --task-id <task-id> --reason <reason>`",
                helper_prefix(helper)
            ),
        ]
        .join("\n"),
        crate::runtime::contract::RuntimeRole::Solo => String::new(),
    };

    if role_notes.is_empty() {
        return crate::runtime::contract::render_runtime_prompt(session);
    }

    format!(
        "{}\n\n<clawbro_team_contract>\nYou are running inside ClawBro Team mode.\nUse the following helper commands for team coordination instead of inventing your own protocol.\n{}\nIf a task is complete, submit results instead of only saying it in plain text.\n</clawbro_team_contract>",
        crate::runtime::contract::render_runtime_prompt(session),
        role_notes
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{
        contract::{RuntimeContext, RuntimeEvent, RuntimeRole, ToolSurfaceSpec},
        registry::BackendSpec,
    };
    use tempfile::NamedTempFile;

    fn spec() -> BackendSpec {
        BackendSpec {
            backend_id: "openclaw-main".into(),
            family: BackendFamily::OpenClawGateway,
            adapter_key: "openclaw".into(),
            launch: LaunchSpec::GatewayWs {
                endpoint: "ws://127.0.0.1:18789".into(),
                token: None,
                password: None,
                role: None,
                scopes: vec![],
                agent_id: None,
                team_helper_command: None,
                team_helper_args: vec![],
                lead_helper_mode: false,
            },
            approval_mode: Default::default(),
            external_mcp_servers: vec![],
            provider_profile: None,
            acp_backend: None,
            acp_auth_method: None,
            codex_projection: None,
        }
    }

    #[tokio::test]
    async fn openclaw_adapter_probe_reports_default_profile() {
        let adapter = OpenClawBackendAdapter::default();
        let profile = adapter.probe(&spec()).await.unwrap();

        assert!(profile.role_eligibility.solo);
        assert!(profile.role_eligibility.relay);
        assert!(!profile.role_eligibility.lead);
    }

    #[test]
    fn helper_result_is_normalized_into_canonical_tool_callback() {
        let bridge = OpenClawTeamBridge::new(OpenClawRunBinding {
            backend_id: "openclaw-main".into(),
            participant_name: "worker".into(),
            team_id: "team-test".into(),
            task_id: String::new(),
            session_key: crate::protocol::SessionKey::new("specialist", "team-test:openclaw-main"),
            openclaw_session_key: "specialist:team-test:openclaw-main".into(),
            run_id: Some("run-1".into()),
            helper_invocation_id: None,
        });

        let event = bridge
            .handle_helper_result(&crate::runtime::render_team_helper_success(
                "submit_task_result",
                serde_json::Map::from_iter([
                    ("task_id".into(), serde_json::Value::String("T1".into())),
                    ("summary".into(), serde_json::Value::String("done".into())),
                ]),
            ))
            .unwrap();
        assert!(matches!(
            event,
            RuntimeEvent::ToolCallback(crate::runtime::contract::TeamCallback::TaskSubmitted {
                ref task_id,
                ref summary,
                ref agent,
                ..
            }) if task_id == "T1" && summary == "done" && agent == "worker"
        ));
    }

    #[tokio::test]
    async fn openclaw_adapter_run_turn_rejects_connection_failure() {
        let adapter = OpenClawBackendAdapter::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let err = adapter
            .run_turn(
                &spec(),
                RuntimeSessionSpec {
                    backend_id: "openclaw-main".into(),
                    participant_name: None,
                    session_key: crate::protocol::SessionKey::new("ws", "openclaw:test"),
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
                },
                RuntimeEventSink::new(tx),
            )
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains("failed to connect")
                || err.to_string().contains("Connection refused")
        );
    }

    #[tokio::test]
    async fn openclaw_adapter_probe_upgrades_when_helper_is_configured() {
        let helper = NamedTempFile::new().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(helper.path()).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(helper.path(), perms).unwrap();
        }
        let adapter = OpenClawBackendAdapter::default();
        let mut spec = spec();
        spec.launch = LaunchSpec::GatewayWs {
            endpoint: "ws://127.0.0.1:18789".into(),
            token: None,
            password: None,
            role: None,
            scopes: vec![],
            agent_id: Some("main".into()),
            team_helper_command: Some(helper.path().display().to_string()),
            team_helper_args: vec!["--verbose".into()],
            lead_helper_mode: false,
        };

        let profile = adapter.probe(&spec).await.unwrap();
        assert!(profile.role_eligibility.specialist);
        assert!(!profile.role_eligibility.lead);
    }

    #[tokio::test]
    async fn openclaw_adapter_probe_upgrades_to_lead_when_lead_mode_enabled() {
        let helper = NamedTempFile::new().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(helper.path()).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(helper.path(), perms).unwrap();
        }

        let adapter = OpenClawBackendAdapter::default();
        let mut spec = spec();
        spec.launch = LaunchSpec::GatewayWs {
            endpoint: "ws://127.0.0.1:18789".into(),
            token: None,
            password: None,
            role: None,
            scopes: vec![],
            agent_id: Some("main".into()),
            team_helper_command: Some(helper.path().display().to_string()),
            team_helper_args: vec![],
            lead_helper_mode: true,
        };

        let profile = adapter.probe(&spec).await.unwrap();
        assert!(profile.role_eligibility.specialist);
        assert!(profile.role_eligibility.lead);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn openclaw_adapter_probe_does_not_upgrade_non_executable_helper() {
        use std::os::unix::fs::PermissionsExt;

        let helper = NamedTempFile::new().unwrap();
        let mut perms = std::fs::metadata(helper.path()).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(helper.path(), perms).unwrap();

        let adapter = OpenClawBackendAdapter::default();
        let mut spec = spec();
        spec.launch = LaunchSpec::GatewayWs {
            endpoint: "ws://127.0.0.1:18789".into(),
            token: None,
            password: None,
            role: None,
            scopes: vec![],
            agent_id: Some("main".into()),
            team_helper_command: Some(helper.path().display().to_string()),
            team_helper_args: vec![],
            lead_helper_mode: false,
        };

        let profile = adapter.probe(&spec).await.unwrap();
        assert!(!profile.role_eligibility.specialist);
        assert!(!profile.role_eligibility.lead);
    }

    #[test]
    fn openclaw_prompt_includes_helper_commands_for_specialists() {
        let helper = TeamHelperConfig {
            command: "/opt/clawbro-team-cli".into(),
            args: vec!["--trace".into()],
            session_channel: "specialist".into(),
            session_scope: "team:codex".into(),
            team_tool_url: "http://127.0.0.1:3000/runtime/team-tools?token=t".into(),
        };
        let session = RuntimeSessionSpec {
            backend_id: "openclaw-main".into(),
            participant_name: Some("worker".into()),
            session_key: crate::protocol::SessionKey::new("specialist", "team:codex"),
            role: RuntimeRole::Specialist,
            workspace_dir: None,
            prompt_text: "Implement task".into(),
            tool_surface: ToolSurfaceSpec {
                team_tools: true,
                allowed_team_tools: vec![],
                local_skills: false,
                external_mcp: false,
                backend_native_tools: true,
            },
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: Some(helper.team_tool_url.clone()),
            provider_profile: None,
            backend_session_id: None,
            context: RuntimeContext::default(),
        };

        let prompt = augment_prompt_for_openclaw(&session, Some(&helper));
        assert!(prompt.contains("submit-task-result"));
        assert!(prompt.contains("/opt/clawbro-team-cli"));
        assert!(prompt.contains("--session-channel 'specialist'"));
    }

    #[test]
    fn openclaw_prompt_keeps_rendered_recent_history_when_helper_is_enabled() {
        let helper = TeamHelperConfig {
            command: "/opt/clawbro-team-cli".into(),
            args: vec![],
            session_channel: "specialist".into(),
            session_scope: "team:codex".into(),
            team_tool_url: "http://127.0.0.1:3000/runtime/team-tools?token=t".into(),
        };
        let session = RuntimeSessionSpec {
            backend_id: "openclaw-main".into(),
            participant_name: Some("worker".into()),
            session_key: crate::protocol::SessionKey::new("specialist", "team:codex"),
            role: RuntimeRole::Specialist,
            workspace_dir: None,
            prompt_text: String::new(),
            tool_surface: ToolSurfaceSpec {
                team_tools: true,
                allowed_team_tools: vec![],
                local_skills: false,
                external_mcp: false,
                backend_native_tools: true,
            },
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: Some(helper.team_tool_url.clone()),
            provider_profile: None,
            backend_session_id: None,
            context: RuntimeContext {
                history_lines: vec![
                    "[user]: [alice]: 第一条".into(),
                    "[assistant]: [@codex]: 第二条".into(),
                    "[tool_call:read#call-1]: {\"path\":\"README.md\"}".into(),
                    "[tool_result:read#call-1]: ok".into(),
                ],
                user_input: Some("第三条".into()),
                ..RuntimeContext::default()
            },
        };

        let prompt = augment_prompt_for_openclaw(&session, Some(&helper));
        assert!(prompt.contains("[user]: [alice]: 第一条"));
        assert!(prompt.contains("[assistant]: [@codex]: 第二条"));
        assert!(prompt.contains("[tool_call:read#call-1]: {\"path\":\"README.md\"}"));
        assert!(prompt.contains("[tool_result:read#call-1]: ok"));
        assert!(prompt.contains("第三条"));
        assert!(prompt.contains("submit-task-result"));
    }

    #[test]
    fn openclaw_prompt_includes_helper_commands_for_leaders() {
        let helper = TeamHelperConfig {
            command: "/opt/clawbro-team-cli".into(),
            args: vec![],
            session_channel: "ws".into(),
            session_scope: "group:team".into(),
            team_tool_url: "http://127.0.0.1:3000/runtime/team-tools?token=t".into(),
        };
        let session = RuntimeSessionSpec {
            backend_id: "openclaw-main".into(),
            participant_name: Some("leader".into()),
            session_key: crate::protocol::SessionKey::new("ws", "group:team"),
            role: RuntimeRole::Leader,
            workspace_dir: None,
            prompt_text: "Plan the work".into(),
            tool_surface: ToolSurfaceSpec {
                team_tools: true,
                allowed_team_tools: vec![],
                local_skills: false,
                external_mcp: false,
                backend_native_tools: true,
            },
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: Some(helper.team_tool_url.clone()),
            provider_profile: None,
            backend_session_id: None,
            context: RuntimeContext::default(),
        };

        let prompt = augment_prompt_for_openclaw(&session, Some(&helper));
        assert!(prompt.contains("create-task"));
        assert!(prompt.contains("assign-task"));
        assert!(prompt.contains("start-execution"));
        assert!(prompt.contains("accept-task"));
        assert!(prompt.contains("reopen-task"));
        assert!(prompt.contains("post-update"));
    }
}
