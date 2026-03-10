use crate::{config, state::AppState};
use qai_agent::team::orchestrator::{
    TeamArtifactHealthSummary, TeamRuntimeSummary, TeamState, TeamTaskCounts,
};
use qai_runtime::{BackendFamily, CapabilityProfile};
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
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TeamDiagnostic {
    pub team_id: String,
    pub state: TeamState,
    pub lead_agent_name: Option<String>,
    pub tool_surface_ready: bool,
    pub mcp_port: Option<u16>,
    pub task_counts: TeamTaskCounts,
    pub artifact_health: TeamArtifactHealthSummary,
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
                "ws" => true,
                _ => false,
            };
            let enabled = match channel.as_str() {
                "lark" => state.cfg.channels.lark.as_ref().is_some_and(|cfg| cfg.enabled),
                "dingtalk" => state.cfg.channels.dingtalk.as_ref().is_some_and(|cfg| cfg.enabled),
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
    let group_scopes = state.cfg.groups.iter().map(|group| group.scope.clone()).collect();
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
    let status = overall_status(&doctor_findings(&backends, &teams, &collect_channel_diagnostics(state)));

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
    let unhealthy_backends = status.backends.iter().filter(|backend| !backend.healthy).count();
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
    if expects_full_team_surface && summary.task_counts.total > 0 && !summary.artifact_health.tasks_md_present
    {
        notes.push("TASKS.md missing".to_string());
    }
    if summary.task_counts.total > 0 && !summary.artifact_health.task_artifacts_present {
        notes.push("tasks/ artifact directory missing".to_string());
    }

    let healthy = notes.is_empty();

    TeamDiagnostic {
        team_id: summary.team_id,
        state: summary.state,
        lead_agent_name: summary.lead_agent_name,
        tool_surface_ready: summary.tool_surface_ready,
        mcp_port: summary.mcp_port,
        task_counts: summary.task_counts,
        artifact_health: summary.artifact_health,
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
                suggested_action: "register the adapter for this backend family or fix adapter_key".to_string(),
            });
        } else if !backend.probed {
            findings.push(DiagnosticFinding {
                severity: DiagnosticSeverity::Warn,
                scope: DiagnosticScope::Backend,
                subject: backend.backend_id.clone(),
                message: "backend has not been probed yet".to_string(),
                suggested_action: "trigger a backend probe or wait for the first runtime use".to_string(),
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
                suggested_action: "verify team runtime wiring and MCP/tool endpoint startup".to_string(),
            });
        }
        if !team.artifact_health.root_present {
            findings.push(DiagnosticFinding {
                severity: DiagnosticSeverity::Error,
                scope: DiagnosticScope::Team,
                subject: team.team_id.clone(),
                message: "team artifact root is missing".to_string(),
                suggested_action: "check team session directory creation and filesystem permissions".to_string(),
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
                suggested_action: "inspect team session files and regenerate TEAM/CONTEXT/task artifacts".to_string(),
            });
        }
    }

    for channel in channels {
        if channel.channel != "ws" && channel.enabled && !channel.routing_present {
            findings.push(DiagnosticFinding {
                severity: DiagnosticSeverity::Warn,
                scope: DiagnosticScope::Channel,
                subject: channel.channel.clone(),
                message: "channel is enabled but no routing or group wiring references it".to_string(),
                suggested_action: "add group or binding wiring for this channel, or disable it".to_string(),
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
            config::BindingConfig::Channel { channel, .. } => {
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
        group.mode.channel.as_deref() == Some(channel) || scope_channel(&group.scope) == Some(channel)
    }) || cfg.bindings.iter().any(|binding| match binding {
        config::BindingConfig::Thread { channel: binding_channel, .. }
        | config::BindingConfig::Scope { channel: binding_channel, .. }
        | config::BindingConfig::Peer { channel: binding_channel, .. } => {
            binding_channel.as_deref() == Some(channel)
        }
        config::BindingConfig::Channel { channel: binding_channel, .. } => binding_channel == channel,
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
    use crate::{config, state::AppState};
    use qai_agent::{
        team::{
            heartbeat::DispatchFn, orchestrator::TeamOrchestrator, registry::TaskRegistry,
            session::TeamSession,
        },
        SessionRegistry,
    };
    use qai_runtime::{ApprovalBroker, BackendRegistry, BackendSpec, LaunchSpec};
    use qai_session::{SessionManager, SessionStorage};
    use std::sync::Arc;
    use tempfile::tempdir;

    async fn diagnostics_state(register_adapter: bool) -> AppState {
        let cfg = config::GatewayConfig {
            backends: vec![config::BackendCatalogEntry {
                id: "native-main".to_string(),
                family: config::BackendFamilyConfig::QuickAiNative,
                adapter_key: Some("native".to_string()),
                launch: config::BackendLaunchConfig::Embedded,
            }],
            channels: config::ChannelsSection {
                lark: Some(config::LarkSection {
                    enabled: true,
                    presentation: config::ProgressPresentationMode::FinalOnly,
                }),
                dingtalk: Some(config::DingTalkSection {
                    enabled: false,
                    presentation: config::ProgressPresentationMode::FinalOnly,
                }),
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
                .register_adapter("native", Arc::new(qai_runtime::QuickAiNativeBackendAdapter))
                .await;
        }
        runtime_registry
            .register_backend(BackendSpec {
                backend_id: "native-main".into(),
                family: BackendFamily::QuickAiNative,
                adapter_key: "native".into(),
                launch: LaunchSpec::Embedded,
            })
            .await;

        AppState {
            registry,
            runtime_registry,
            event_tx: tokio::sync::broadcast::channel(8).0,
            cfg: Arc::new(cfg),
            runtime_token: Arc::new("diagnostics-token".to_string()),
            approvals: ApprovalBroker::default(),
        }
    }

    #[tokio::test]
    async fn doctor_report_marks_missing_backend_adapter_as_error() {
        let state = diagnostics_state(false).await;
        let report = collect_doctor_report(&state).await;
        assert_eq!(report.state, DiagnosticsStatus::Unavailable);
        assert!(report
            .findings
            .iter()
            .any(|finding| matches!(finding.scope, DiagnosticScope::Backend)
                && matches!(finding.severity, DiagnosticSeverity::Error)));
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
        let session = Arc::new(TeamSession::from_dir("team-running", tmp.path().to_path_buf()));
        session.write_team_md("manifest").unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        registry
            .create_task(qai_agent::team::registry::CreateTask {
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
        assert!(teams[0]
            .notes
            .iter()
            .any(|note| note.contains("TASKS.md")));
    }
}
