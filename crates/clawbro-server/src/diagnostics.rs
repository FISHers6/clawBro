use crate::agent_core::team::orchestrator::{
    TeamArtifactHealthSummary, TeamRoutingStats, TeamRuntimeSummary, TeamState, TeamTaskCounts,
};
use crate::agent_core::team::session::{ChannelSendRecord, LeaderUpdateRecord};
use crate::runtime::{
    provider_profiles::ConfiguredProviderProtocol, AcpBackend, BackendFamily, CapabilityProfile,
};
use crate::{config, state::AppState};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsStatus {
    Ok,
    Degraded,
    Unavailable,
}

impl DiagnosticsStatus {
    pub fn ok(self) -> bool {
        !matches!(self, Self::Unavailable)
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticScope {
    Backend,
    Team,
    Channel,
    Topology,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticFinding {
    pub severity: DiagnosticSeverity,
    pub scope: DiagnosticScope,
    pub subject: String,
    pub message: String,
    pub suggested_action: String,
}

/// Support category for ACP backends, for diagnostic reporting.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcpSupportCategory {
    /// Validated in ClawBro with a dedicated bridge adapter package.
    SupportedWithBridge,
    /// Generic ACP CLI path — expected to work if the tool speaks ACP over stdio.
    GenericAcpCli,
}

impl AcpSupportCategory {
    pub fn for_backend(backend: Option<AcpBackend>) -> Option<Self> {
        match backend {
            None => None, // generic, no explicit identity
            Some(AcpBackend::Claude) | Some(AcpBackend::Codex) | Some(AcpBackend::Codebuddy) => {
                Some(Self::SupportedWithBridge)
            }
            Some(AcpBackend::Qwen)
            | Some(AcpBackend::Iflow)
            | Some(AcpBackend::Goose)
            | Some(AcpBackend::Kimi)
            | Some(AcpBackend::Opencode)
            | Some(AcpBackend::Qoder)
            | Some(AcpBackend::Vibe)
            | Some(AcpBackend::Custom) => Some(Self::GenericAcpCli),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BackendDiagnostic {
    pub backend_id: String,
    pub family: BackendFamily,
    pub adapter_key: String,
    pub registered: bool,
    pub adapter_registered: bool,
    pub probed: bool,
    pub healthy: bool,
    pub error: Option<String>,
    pub capability_profile: Option<CapabilityProfile>,
    /// ACP backend identity, if declared. Only present when `family == Acp`.
    pub acp_backend: Option<AcpBackend>,
    /// ACP support category, derived from backend identity. `None` for non-ACP or generic backends.
    pub acp_support_category: Option<AcpSupportCategory>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TeamDiagnostic {
    pub team_id: String,
    pub state: TeamState,
    pub lead_agent_name: Option<String>,
    pub latest_leader_update: Option<LeaderUpdateRecord>,
    pub latest_channel_send: Option<ChannelSendRecord>,
    pub tool_surface_ready: bool,
    pub mcp_port: Option<u16>,
    pub task_counts: TeamTaskCounts,
    pub artifact_health: TeamArtifactHealthSummary,
    pub routing_stats: TeamRoutingStats,
    pub healthy: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelDiagnostic {
    pub channel: String,
    pub configured: bool,
    pub enabled: bool,
    pub routing_present: bool,
    pub credential_state: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BindingSummary {
    pub kind: String,
    pub agent: String,
    pub channel: Option<String>,
    pub scope: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopologyDiagnostic {
    pub default_backend_id: Option<String>,
    pub backend_catalog: Vec<String>,
    pub roster_agents: Vec<String>,
    pub bindings: Vec<BindingSummary>,
    pub group_scopes: Vec<String>,
    pub team_groups: Vec<String>,
    pub configured_channels: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusReport {
    pub ok: bool,
    pub state: DiagnosticsStatus,
    pub backend_count: usize,
    pub pending_approvals: usize,
    pub backends: Vec<BackendDiagnostic>,
    pub teams: Vec<TeamDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    pub ok: bool,
    pub state: DiagnosticsStatus,
    pub backend_count: usize,
    pub unhealthy_backends: usize,
    pub active_teams: usize,
    pub unhealthy_teams: usize,
    pub pending_approvals: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub state: DiagnosticsStatus,
    pub findings: Vec<DiagnosticFinding>,
}

pub async fn collect_backend_diagnostics(state: &AppState) -> Vec<BackendDiagnostic> {
    let backend_specs = state.runtime_registry.all_backend_specs().await;
    let mut backends = Vec::with_capacity(backend_specs.len());

    for spec in backend_specs {
        let adapter_registered = state.runtime_registry.has_adapter(&spec.adapter_key).await;
        let capability_profile = state
            .runtime_registry
            .cached_capability_profile(&spec.backend_id);
        let probed = capability_profile.is_some();
        let mut notes = Vec::new();
        let error = if !adapter_registered {
            Some(format!("no adapter registered for '{}'", spec.adapter_key))
        } else if !probed {
            notes.push("capability profile not yet probed".to_string());
            None
        } else {
            None
        };

        let acp_backend = spec.acp_backend;
        let acp_support_category = AcpSupportCategory::for_backend(acp_backend);
        if spec.family == BackendFamily::ClawBroNative {
            if let Some(profile) = &spec.provider_profile {
                if let ConfiguredProviderProtocol::AnthropicCompatible { base_url, .. } =
                    &profile.protocol
                {
                    if base_url.contains("deepseek.com/anthropic") {
                        notes.push(
                            "native + DeepSeek anthropic_compatible is vendor-dependent; prefer openai_compatible for validated operation".to_string(),
                        );
                    }
                }
            }
        }
        backends.push(BackendDiagnostic {
            backend_id: spec.backend_id,
            family: spec.family,
            adapter_key: spec.adapter_key,
            registered: true,
            adapter_registered,
            probed,
            healthy: adapter_registered && probed,
            error,
            capability_profile,
            acp_backend,
            acp_support_category,
            notes,
        });
    }

    backends
}

pub fn collect_team_diagnostics(state: &AppState) -> Vec<TeamDiagnostic> {
    let mut teams: Vec<_> = state
        .registry
        .team_summaries()
        .into_iter()
        .map(team_diagnostic_from_summary)
        .collect();
    teams.sort_by(|a, b| a.team_id.cmp(&b.team_id));
    teams
}

pub fn collect_channel_diagnostics(state: &AppState) -> Vec<ChannelDiagnostic> {
    let mut channels = inferred_channels(state.cfg.as_ref());
    if channels.is_empty() {
        return Vec::new();
    }

    channels
        .drain(..)
        .map(|channel| {
            let configured = match channel.as_str() {
                "lark" => state.cfg.channels.lark.is_some(),
                "dingtalk" => state.cfg.channels.dingtalk.is_some(),
                "dingtalk_webhook" => state.cfg.channels.dingtalk_webhook.is_some(),
                "ws" => true,
                _ => false,
            };
            let enabled = match channel.as_str() {
                "lark" => state
                    .cfg
                    .channels
                    .lark
                    .as_ref()
                    .is_some_and(|cfg| cfg.enabled),
                "dingtalk" => state
                    .cfg
                    .channels
                    .dingtalk
                    .as_ref()
                    .is_some_and(|cfg| cfg.enabled),
                "dingtalk_webhook" => state
                    .cfg
                    .channels
                    .dingtalk_webhook
                    .as_ref()
                    .is_some_and(|cfg| cfg.enabled),
                "ws" => true,
                _ => false,
            };
            let routing_present = channel_has_routing(state.cfg.as_ref(), &channel);
            let credential_state = match channel.as_str() {
                "ws" => "not_applicable".to_string(),
                _ if enabled => "unknown".to_string(),
                _ => "disabled".to_string(),
            };
            let mut notes = Vec::new();
            if enabled && channel != "ws" && !routing_present {
                notes.push("channel is enabled but has no group or binding wiring".to_string());
            }

            ChannelDiagnostic {
                channel,
                configured,
                enabled,
                routing_present,
                credential_state,
                notes,
            }
        })
        .collect()
}

pub fn collect_topology_diagnostic(state: &AppState) -> TopologyDiagnostic {
    let backend_catalog = state
        .cfg
        .backends
        .iter()
        .map(|entry| entry.id.clone())
        .collect();
    let roster_agents = state
        .cfg
        .agent_roster
        .iter()
        .map(|entry| entry.name.clone())
        .collect();
    let bindings = state.cfg.bindings.iter().map(binding_summary).collect();
    let group_scopes = state
        .cfg
        .groups
        .iter()
        .map(|group| group.scope.clone())
        .collect();
    let team_groups = state
        .cfg
        .groups
        .iter()
        .filter(|group| !group.team.roster.is_empty())
        .map(|group| group.scope.clone())
        .collect();

    TopologyDiagnostic {
        default_backend_id: state.cfg.resolved_default_backend_id(),
        backend_catalog,
        roster_agents,
        bindings,
        group_scopes,
        team_groups,
        configured_channels: inferred_channels(state.cfg.as_ref()),
    }
}

pub async fn collect_status_report(state: &AppState) -> StatusReport {
    let backends = collect_backend_diagnostics(state).await;
    let teams = collect_team_diagnostics(state);
    let status = overall_status(&doctor_findings(
        &backends,
        &teams,
        &collect_channel_diagnostics(state),
    ));

    StatusReport {
        ok: status.ok(),
        state: status,
        backend_count: backends.len(),
        pending_approvals: state.approvals.pending_count(),
        backends,
        teams,
    }
}

pub async fn collect_health_report(state: &AppState) -> HealthReport {
    let status = collect_status_report(state).await;
    let unhealthy_backends = status
        .backends
        .iter()
        .filter(|backend| !backend.healthy)
        .count();
    let unhealthy_teams = status.teams.iter().filter(|team| !team.healthy).count();

    HealthReport {
        ok: status.ok,
        state: status.state,
        backend_count: status.backend_count,
        unhealthy_backends,
        active_teams: status.teams.len(),
        unhealthy_teams,
        pending_approvals: status.pending_approvals,
    }
}

pub async fn collect_doctor_report(state: &AppState) -> DoctorReport {
    let backends = collect_backend_diagnostics(state).await;
    let teams = collect_team_diagnostics(state);
    let channels = collect_channel_diagnostics(state);
    let findings = doctor_findings(&backends, &teams, &channels);
    let state = overall_status(&findings);

    DoctorReport {
        ok: state.ok(),
        state,
        findings,
    }
}

fn team_diagnostic_from_summary(summary: TeamRuntimeSummary) -> TeamDiagnostic {
    let mut notes = Vec::new();
    if matches!(summary.state, TeamState::Running) && !summary.tool_surface_ready {
        notes.push("team is running but tool surface is not ready".to_string());
    }
    if !summary.artifact_health.root_present {
        notes.push("team artifact root is missing".to_string());
    }
    let expects_full_team_surface = matches!(summary.state, TeamState::Running | TeamState::Done);
    if expects_full_team_surface && !summary.artifact_health.team_md_present {
        notes.push("TEAM.md missing".to_string());
    }
    if expects_full_team_surface && !summary.artifact_health.context_md_present {
        notes.push("CONTEXT.md missing".to_string());
    }
    if expects_full_team_surface
        && summary.task_counts.total > 0
        && !summary.artifact_health.tasks_md_present
    {
        notes.push("TASKS.md missing".to_string());
    }
    if summary.task_counts.total > 0 && !summary.artifact_health.task_artifacts_present {
        notes.push("tasks/ artifact directory missing".to_string());
    }
    if summary.routing_stats.pending_count > 0 {
        notes.push(format!(
            "{} pending completion routing event(s) waiting for delivery",
            summary.routing_stats.pending_count
        ));
    }
    if summary.routing_stats.fallback_redirected > 0 {
        notes.push(format!(
            "{} completion routing event(s) required fallback delivery",
            summary.routing_stats.fallback_redirected
        ));
    }
    if summary.routing_stats.missing_delivery_target > 0 {
        notes.push(format!(
            "{} completion routing event(s) have no live delivery target and remain pending",
            summary.routing_stats.missing_delivery_target
        ));
    }
    if summary.routing_stats.delivery_dedupe_ledger_size > 0 {
        notes.push(format!(
            "delivery dedupe ledger contains {} persisted key(s)",
            summary.routing_stats.delivery_dedupe_ledger_size
        ));
    }
    if summary.routing_stats.delivery_dedupe_hits > 0 {
        notes.push(format!(
            "{} duplicate user-visible milestone delivery attempt(s) were suppressed",
            summary.routing_stats.delivery_dedupe_hits
        ));
    }

    let healthy = notes.is_empty();

    TeamDiagnostic {
        team_id: summary.team_id,
        state: summary.state,
        lead_agent_name: summary.lead_agent_name,
        latest_leader_update: summary.latest_leader_update,
        latest_channel_send: summary.latest_channel_send,
        tool_surface_ready: summary.tool_surface_ready,
        mcp_port: summary.mcp_port,
        task_counts: summary.task_counts,
        artifact_health: summary.artifact_health,
        routing_stats: summary.routing_stats,
        healthy,
        notes,
    }
}

fn doctor_findings(
    backends: &[BackendDiagnostic],
    teams: &[TeamDiagnostic],
    channels: &[ChannelDiagnostic],
) -> Vec<DiagnosticFinding> {
    let mut findings = Vec::new();

    for backend in backends {
        if !backend.adapter_registered {
            findings.push(DiagnosticFinding {
                severity: DiagnosticSeverity::Error,
                scope: DiagnosticScope::Backend,
                subject: backend.backend_id.clone(),
                message: backend
                    .error
                    .clone()
                    .unwrap_or_else(|| "backend adapter is missing".to_string()),
                suggested_action: "register the adapter for this backend family or fix adapter_key"
                    .to_string(),
            });
        } else if !backend.probed {
            findings.push(DiagnosticFinding {
                severity: DiagnosticSeverity::Warn,
                scope: DiagnosticScope::Backend,
                subject: backend.backend_id.clone(),
                message: "backend has not been probed yet".to_string(),
                suggested_action: "trigger a backend probe or wait for the first runtime use"
                    .to_string(),
            });
        }
    }

    for team in teams {
        if matches!(team.state, TeamState::Running) && !team.tool_surface_ready {
            findings.push(DiagnosticFinding {
                severity: DiagnosticSeverity::Error,
                scope: DiagnosticScope::Team,
                subject: team.team_id.clone(),
                message: "team is running but tool surface is not ready".to_string(),
                suggested_action: "verify team runtime wiring and MCP/tool endpoint startup"
                    .to_string(),
            });
        }
        if !team.artifact_health.root_present {
            findings.push(DiagnosticFinding {
                severity: DiagnosticSeverity::Error,
                scope: DiagnosticScope::Team,
                subject: team.team_id.clone(),
                message: "team artifact root is missing".to_string(),
                suggested_action:
                    "check team session directory creation and filesystem permissions".to_string(),
            });
        } else if !team.artifact_health.team_md_present
            || !team.artifact_health.context_md_present
            || (team.task_counts.total > 0 && !team.artifact_health.task_artifacts_present)
        {
            findings.push(DiagnosticFinding {
                severity: DiagnosticSeverity::Warn,
                scope: DiagnosticScope::Team,
                subject: team.team_id.clone(),
                message: "team artifact set is incomplete".to_string(),
                suggested_action:
                    "inspect team session files and regenerate TEAM/CONTEXT/task artifacts"
                        .to_string(),
            });
        }
        if team.routing_stats.pending_count > 0 {
            findings.push(DiagnosticFinding {
                severity: DiagnosticSeverity::Warn,
                scope: DiagnosticScope::Team,
                subject: team.team_id.clone(),
                message: format!(
                    "team has {} pending completion routing event(s)",
                    team.routing_stats.pending_count
                ),
                suggested_action:
                    "restore requester delivery path or inspect pending-completions.jsonl"
                        .to_string(),
            });
        }
    }

    for channel in channels {
        if channel.channel != "ws" && channel.enabled && !channel.routing_present {
            findings.push(DiagnosticFinding {
                severity: DiagnosticSeverity::Warn,
                scope: DiagnosticScope::Channel,
                subject: channel.channel.clone(),
                message: "channel is enabled but no routing or group wiring references it"
                    .to_string(),
                suggested_action: "add group or binding wiring for this channel, or disable it"
                    .to_string(),
            });
        }
    }

    findings
}

fn overall_status(findings: &[DiagnosticFinding]) -> DiagnosticsStatus {
    if findings
        .iter()
        .any(|finding| matches!(finding.severity, DiagnosticSeverity::Error))
    {
        DiagnosticsStatus::Unavailable
    } else if findings
        .iter()
        .any(|finding| matches!(finding.severity, DiagnosticSeverity::Warn))
    {
        DiagnosticsStatus::Degraded
    } else {
        DiagnosticsStatus::Ok
    }
}

fn inferred_channels(cfg: &config::GatewayConfig) -> Vec<String> {
    let mut channels = BTreeSet::new();
    if cfg.channels.lark.is_some() {
        channels.insert("lark".to_string());
    }
    if cfg.channels.dingtalk.is_some() {
        channels.insert("dingtalk".to_string());
    }
    if cfg.channels.dingtalk_webhook.is_some() {
        channels.insert("dingtalk_webhook".to_string());
    }
    channels.insert("ws".to_string());

    for group in &cfg.groups {
        if let Some(channel) = group.mode.channel.as_deref() {
            channels.insert(channel.to_string());
        }
        if let Some(channel) = scope_channel(&group.scope) {
            channels.insert(channel.to_string());
        }
    }
    for binding in &cfg.bindings {
        match binding {
            config::BindingConfig::Thread { channel, .. }
            | config::BindingConfig::Scope { channel, .. }
            | config::BindingConfig::Peer { channel, .. } => {
                if let Some(channel) = channel.as_deref() {
                    channels.insert(channel.to_string());
                }
            }
            config::BindingConfig::ChannelInstance { channel, .. }
            | config::BindingConfig::Channel { channel, .. } => {
                channels.insert(channel.clone());
            }
            config::BindingConfig::Team { .. } | config::BindingConfig::Default { .. } => {}
        }
    }

    channels.into_iter().collect()
}

fn channel_has_routing(cfg: &config::GatewayConfig, channel: &str) -> bool {
    if channel == "ws" {
        return true;
    }
    cfg.groups.iter().any(|group| {
        group.mode.channel.as_deref() == Some(channel)
            || scope_channel(&group.scope) == Some(channel)
    }) || cfg.bindings.iter().any(|binding| match binding {
        config::BindingConfig::Thread {
            channel: binding_channel,
            ..
        }
        | config::BindingConfig::Scope {
            channel: binding_channel,
            ..
        }
        | config::BindingConfig::Peer {
            channel: binding_channel,
            ..
        } => binding_channel.as_deref() == Some(channel),
        config::BindingConfig::ChannelInstance {
            channel: binding_channel,
            ..
        }
        | config::BindingConfig::Channel {
            channel: binding_channel,
            ..
        } => binding_channel == channel,
        config::BindingConfig::Team { .. } | config::BindingConfig::Default { .. } => false,
    })
}

fn scope_channel(scope: &str) -> Option<&str> {
    let mut parts = scope.split(':');
    let _kind = parts.next()?;
    parts.next()
}

fn binding_summary(binding: &config::BindingConfig) -> BindingSummary {
    match binding {
        config::BindingConfig::Thread {
            agent,
            scope,
            thread_id,
            channel,
        } => BindingSummary {
            kind: "thread".to_string(),
            agent: agent.clone(),
            channel: channel.clone(),
            scope: Some(scope.clone()),
            target: Some(thread_id.clone()),
        },
        config::BindingConfig::Scope {
            agent,
            scope,
            channel,
        } => BindingSummary {
            kind: "scope".to_string(),
            agent: agent.clone(),
            channel: channel.clone(),
            scope: Some(scope.clone()),
            target: None,
        },
        config::BindingConfig::Peer {
            agent,
            peer_kind,
            peer_id,
            channel,
        } => BindingSummary {
            kind: format!("peer:{peer_kind:?}").to_lowercase(),
            agent: agent.clone(),
            channel: channel.clone(),
            scope: None,
            target: Some(peer_id.clone()),
        },
        config::BindingConfig::Team { agent, team_id } => BindingSummary {
            kind: "team".to_string(),
            agent: agent.clone(),
            channel: None,
            scope: None,
            target: Some(team_id.clone()),
        },
        config::BindingConfig::ChannelInstance {
            agent,
            channel,
            channel_instance,
        } => BindingSummary {
            kind: "channel_instance".to_string(),
            agent: agent.clone(),
            channel: Some(channel.clone()),
            scope: None,
            target: Some(channel_instance.clone()),
        },
        config::BindingConfig::Channel { agent, channel } => BindingSummary {
            kind: "channel".to_string(),
            agent: agent.clone(),
            channel: Some(channel.clone()),
            scope: None,
            target: None,
        },
        config::BindingConfig::Default { agent } => BindingSummary {
            kind: "default".to_string(),
            agent: agent.clone(),
            channel: None,
            scope: None,
            target: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::{
        team::{
            heartbeat::DispatchFn,
            orchestrator::TeamOrchestrator,
            registry::TaskRegistry,
            session::{ChannelSendSourceKind, ChannelSendStatus, LeaderUpdateKind, TeamSession},
        },
        SessionRegistry, TurnDeliverySource,
    };
    use crate::runtime::{ApprovalBroker, BackendRegistry, BackendSpec, LaunchSpec};
    use crate::session::{SessionManager, SessionStorage};
    use crate::{config, state::AppState};
    use std::sync::Arc;
    use tempfile::tempdir;

    async fn diagnostics_state(register_adapter: bool) -> AppState {
        let cfg = config::GatewayConfig {
            backends: vec![config::BackendCatalogEntry {
                id: "native-main".to_string(),
                family: config::BackendFamilyConfig::ClawBroNative,
                adapter_key: Some("native".to_string()),
                acp_backend: None,
                acp_auth_method: None,
                codex: None,
                provider_profile: None,
                approval: Default::default(),
                external_mcp_servers: vec![],
                launch: config::BackendLaunchConfig::BundledCommand,
            }],
            channels: config::ChannelsSection {
                lark: Some(config::LarkSection {
                    enabled: true,
                    presentation: config::ProgressPresentationMode::FinalOnly,
                    trigger_policy: None,
                    default_instance: None,
                    instances: vec![],
                }),
                dingtalk: Some(config::DingTalkSection {
                    enabled: false,
                    presentation: config::ProgressPresentationMode::FinalOnly,
                }),
                dingtalk_webhook: None,
            },
            groups: vec![config::GroupConfig {
                scope: "group:lark:abc".to_string(),
                name: None,
                mode: config::GroupModeConfig {
                    channel: Some("lark".to_string()),
                    ..Default::default()
                },
                team: Default::default(),
            }],
            ..config::GatewayConfig::default()
        };
        let storage = SessionStorage::new(
            std::env::temp_dir().join(format!("diagnostics-state-{}", uuid::Uuid::new_v4())),
        );
        let session_manager = Arc::new(SessionManager::new(storage));
        let (registry, _rx) = SessionRegistry::new(
            None,
            session_manager,
            String::new(),
            None,
            None,
            None,
            None,
            vec![],
        );
        let runtime_registry = Arc::new(BackendRegistry::new());
        if register_adapter {
            runtime_registry
                .register_adapter(
                    "native",
                    Arc::new(crate::runtime::ClawBroNativeBackendAdapter),
                )
                .await;
        }
        runtime_registry
            .register_backend(BackendSpec {
                backend_id: "native-main".into(),
                family: BackendFamily::ClawBroNative,
                adapter_key: "native".into(),
                launch: LaunchSpec::BundledCommand,
                approval_mode: Default::default(),
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
            })
            .await;

        AppState {
            registry,
            runtime_registry,
            event_tx: tokio::sync::broadcast::channel(8).0,
            cfg: Arc::new(cfg),
            channel_registry: Arc::new(crate::channel_registry::ChannelRegistry::new()),
            dingtalk_webhook_channel: None,
            runtime_token: Arc::new("diagnostics-token".to_string()),
            approvals: ApprovalBroker::default(),
            scheduler_service: crate::scheduler_runtime::build_test_scheduler_service(),
        }
    }

    #[test]
    fn acp_support_category_bridge_backed_for_claude_codex_codebuddy() {
        use super::AcpSupportCategory;
        use crate::runtime::AcpBackend;
        assert_eq!(
            AcpSupportCategory::for_backend(Some(AcpBackend::Claude)),
            Some(AcpSupportCategory::SupportedWithBridge)
        );
        assert_eq!(
            AcpSupportCategory::for_backend(Some(AcpBackend::Codex)),
            Some(AcpSupportCategory::SupportedWithBridge)
        );
        assert_eq!(
            AcpSupportCategory::for_backend(Some(AcpBackend::Codebuddy)),
            Some(AcpSupportCategory::SupportedWithBridge)
        );
    }

    #[test]
    fn acp_support_category_generic_for_qwen_goose_etc() {
        use super::AcpSupportCategory;
        use crate::runtime::AcpBackend;
        assert_eq!(
            AcpSupportCategory::for_backend(Some(AcpBackend::Qwen)),
            Some(AcpSupportCategory::GenericAcpCli)
        );
        assert_eq!(
            AcpSupportCategory::for_backend(Some(AcpBackend::Goose)),
            Some(AcpSupportCategory::GenericAcpCli)
        );
        assert_eq!(
            AcpSupportCategory::for_backend(Some(AcpBackend::Iflow)),
            Some(AcpSupportCategory::GenericAcpCli)
        );
    }

    #[test]
    fn acp_support_category_none_when_identity_omitted() {
        use super::AcpSupportCategory;
        // Omitted acp_backend = generic ACP, no category label
        assert_eq!(AcpSupportCategory::for_backend(None), None);
    }

    #[tokio::test]
    async fn doctor_report_marks_missing_backend_adapter_as_error() {
        let state = diagnostics_state(false).await;
        let report = collect_doctor_report(&state).await;
        assert_eq!(report.state, DiagnosticsStatus::Unavailable);
        assert!(report.findings.iter().any(|finding| matches!(
            finding.scope,
            DiagnosticScope::Backend
        ) && matches!(
            finding.severity,
            DiagnosticSeverity::Error
        )));
    }

    #[tokio::test]
    async fn status_report_marks_unprobed_backend_as_degraded() {
        let state = diagnostics_state(true).await;
        let report = collect_status_report(&state).await;
        assert_eq!(report.state, DiagnosticsStatus::Degraded);
        assert!(!report.backends[0].healthy);
        assert!(!report.backends[0].probed);
    }

    #[tokio::test]
    async fn collect_team_diagnostics_planning_team_only_requires_root() {
        let state = diagnostics_state(true).await;
        let tmp = tempdir().unwrap();
        let session = Arc::new(TeamSession::from_dir("team-diag", tmp.path().to_path_buf()));
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        state
            .registry
            .register_team_orchestrator("team-diag".to_string(), Arc::clone(&orch));

        let teams = collect_team_diagnostics(&state);
        assert_eq!(teams.len(), 1);
        assert!(teams[0].healthy);
        assert!(teams[0].notes.is_empty());
    }

    #[tokio::test]
    async fn collect_team_diagnostics_running_team_reports_missing_required_files() {
        let state = diagnostics_state(true).await;
        let tmp = tempdir().unwrap();
        let session = Arc::new(TeamSession::from_dir(
            "team-running",
            tmp.path().to_path_buf(),
        ));
        session.write_team_md("manifest").unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        registry
            .create_task(crate::agent_core::team::registry::CreateTask {
                id: "T001".into(),
                title: "Do work".into(),
                ..Default::default()
            })
            .unwrap();
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        let _ = orch.mcp_server_port.set(32123);
        *orch.team_state_inner.lock().unwrap() = TeamState::Running;
        state
            .registry
            .register_team_orchestrator("team-running".to_string(), Arc::clone(&orch));

        let teams = collect_team_diagnostics(&state);
        assert_eq!(teams.len(), 1);
        assert!(!teams[0].healthy);
        assert!(teams[0]
            .notes
            .iter()
            .any(|note| note.contains("CONTEXT.md")));
        assert!(teams[0].notes.iter().any(|note| note.contains("TASKS.md")));
    }

    #[tokio::test]
    async fn collect_team_diagnostics_includes_pending_routing_stats() {
        let state = diagnostics_state(true).await;
        let tmp = tempdir().unwrap();
        let session = Arc::new(TeamSession::from_dir(
            "team-routing",
            tmp.path().to_path_buf(),
        ));
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            registry,
            Arc::clone(&session),
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        session
            .append_pending_completion(&crate::agent_core::team::completion_routing::TeamRoutingEnvelope {
                run_id: "run-1".into(),
                parent_run_id: None,
                requester_session_key: Some(crate::protocol::SessionKey::new("ws", "group:routing")),
                fallback_session_keys: vec![],
                team_id: "team-routing".into(),
                delivery_status:
                    crate::agent_core::team::completion_routing::RoutingDeliveryStatus::PersistedPending,
                event: crate::agent_core::team::completion_routing::TeamRoutingEvent::failed(
                    "T001", "pending",
                ),
                delivery_source: None,
            })
            .unwrap();
        session
            .mark_delivery_dedupe("group:routing", "all_tasks_done")
            .unwrap();
        session
            .record_delivery_dedupe_hit("group:routing", "all_tasks_done")
            .unwrap();
        state
            .registry
            .register_team_orchestrator("team-routing".to_string(), Arc::clone(&orch));

        let teams = collect_team_diagnostics(&state);
        assert_eq!(teams.len(), 1);
        assert_eq!(teams[0].routing_stats.pending_count, 1);
        assert_eq!(teams[0].routing_stats.delivery_dedupe_ledger_size, 1);
        assert_eq!(teams[0].routing_stats.delivery_dedupe_hits, 1);
        assert!(teams[0]
            .notes
            .iter()
            .any(|note| note.contains("pending completion routing event")));
        assert!(teams[0]
            .notes
            .iter()
            .any(|note| note.contains("delivery dedupe ledger contains 1 persisted key")));
        assert!(teams[0]
            .notes
            .iter()
            .any(|note| note.contains("duplicate user-visible milestone delivery attempt")));
    }

    #[tokio::test]
    async fn collect_team_diagnostics_surfaces_latest_delivery_records() {
        let state = diagnostics_state(true).await;
        let tmp = tempdir().unwrap();
        let session = Arc::new(TeamSession::from_dir(
            "team-observability",
            tmp.path().to_path_buf(),
        ));
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            registry,
            Arc::clone(&session),
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        let lead_key = crate::protocol::SessionKey::with_instance("lark", "alpha", "group:diag");
        let lead_source = TurnDeliverySource::from_session_key(&lead_key)
            .with_reply_context(Some("om_1".into()), Some("th_1".into()));
        session
            .record_leader_update(
                Some(&lead_key),
                Some(&lead_source),
                "codex-alpha",
                LeaderUpdateKind::PostUpdate,
                "working",
                Some("T001"),
            )
            .unwrap();
        session
            .record_channel_send(
                "lark",
                Some("beta"),
                Some("gamma"),
                "group:target",
                Some(&lead_key),
                Some(&lead_source),
                Some("om_2"),
                Some("th_2"),
                ChannelSendSourceKind::Milestone,
                "codex-beta",
                Some("T002"),
                Some("dedupe-1"),
                "finished",
                ChannelSendStatus::Sent,
                None,
            )
            .unwrap();
        state
            .registry
            .register_team_orchestrator("team-observability".to_string(), Arc::clone(&orch));

        let teams = collect_team_diagnostics(&state);
        assert_eq!(teams.len(), 1);
        let latest_leader = teams[0]
            .latest_leader_update
            .as_ref()
            .expect("leader update");
        assert_eq!(
            latest_leader.lead_session_channel_instance.as_deref(),
            Some("alpha")
        );
        assert_eq!(latest_leader.lead_reply_to.as_deref(), Some("om_1"));

        let latest_send = teams[0].latest_channel_send.as_ref().expect("channel send");
        assert_eq!(latest_send.sender_channel_instance.as_deref(), Some("beta"));
        assert_eq!(
            latest_send.target_channel_instance.as_deref(),
            Some("gamma")
        );
        assert_eq!(latest_send.target_scope, "group:target");
        assert_eq!(latest_send.reply_to.as_deref(), Some("om_2"));
        assert_eq!(latest_send.thread_ts.as_deref(), Some("th_2"));
    }
}
