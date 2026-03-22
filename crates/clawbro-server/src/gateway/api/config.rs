use crate::agent_core::roster::AgentEntry;
use crate::config::{
    AgentSection, BindingConfig, DeliverySenderBindingConfig, DeliveryTargetOverrideConfig,
    DingTalkSection, DingTalkWebhookSection, GatewayConfig, GroupConfig, LarkInstanceConfig,
    LarkSection, ProviderProfileConfig, TeamScopeConfig, TeamScopeSpec,
};
use crate::state::AppState;
use axum::{extract::State, Json};
use serde::Serialize;

use super::backends::{backend_config_view, BackendApiView};

#[derive(Debug, Clone, Serialize)]
pub struct EffectiveConfigView {
    pub default_backend_id: Option<String>,
    pub roster_agents: Vec<String>,
    pub team_scopes: Vec<TeamScopeView>,
    pub delivery_sender_bindings: Vec<DeliverySenderBindingConfig>,
    pub channels: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TeamScopeView {
    pub scope: String,
    pub name: Option<String>,
    pub channel: Option<String>,
    pub front_bot: Option<String>,
    pub roster: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigSpecView {
    pub gateway: GatewaySpecView,
    pub agent: AgentSection,
    pub auth: AuthSpecView,
    pub channels: ChannelsSpecView,
    pub skills: SkillsSpecView,
    pub session: SessionSpecView,
    pub memory: MemorySpecView,
    pub scheduler: SchedulerSpecView,
    pub agent_roster: Vec<AgentEntrySpecView>,
    pub backends: Vec<BackendApiView>,
    pub provider_profiles: Vec<ProviderProfileConfig>,
    pub groups: Vec<GroupConfig>,
    pub team_scopes: Vec<TeamScopeConfig>,
    pub bindings: Vec<BindingConfig>,
    pub delivery_sender_bindings: Vec<DeliverySenderBindingConfig>,
    pub delivery_target_overrides: Vec<DeliveryTargetOverrideConfig>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthSpecView {
    pub ws_token_configured: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GatewaySpecView {
    pub host: String,
    pub port: u16,
    pub require_mention_in_groups: bool,
    pub default_workspace_configured: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillsSpecView {
    pub dir_configured: bool,
    pub global_dir_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSpecView {
    pub dir_configured: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemorySpecView {
    pub distill_every_n: u64,
    pub distiller_binary: String,
    pub shared_dir_configured: bool,
    pub shared_memory_max_words: usize,
    pub agent_memory_max_words: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SchedulerSpecView {
    pub enabled: bool,
    pub poll_secs: u64,
    pub max_concurrent: usize,
    pub max_fetch_per_tick: usize,
    pub default_timezone: String,
    pub db_path_configured: bool,
    pub lease_secs: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentEntrySpecView {
    pub name: String,
    pub mentions: Vec<String>,
    pub backend_id: String,
    pub persona_dir_configured: bool,
    pub workspace_dir_configured: bool,
    pub extra_skills_dir_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelsSpecView {
    pub dingtalk: Option<DingTalkSection>,
    pub dingtalk_webhook: Option<DingTalkWebhookSpecView>,
    pub lark: Option<LarkSpecView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DingTalkWebhookSpecView {
    pub enabled: bool,
    pub webhook_path: String,
    pub access_token_configured: bool,
    pub secret_key_configured: bool,
    pub presentation: crate::config::ProgressPresentationMode,
}

#[derive(Debug, Clone, Serialize)]
pub struct LarkSpecView {
    pub enabled: bool,
    pub presentation: crate::config::ProgressPresentationMode,
    pub trigger_policy: Option<crate::config::LarkTriggerPolicyConfig>,
    pub default_instance: Option<String>,
    pub instances: Vec<LarkInstanceSpecView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LarkInstanceSpecView {
    pub id: String,
    pub app_id: String,
    pub bot_name: Option<String>,
    pub app_secret_configured: bool,
}

pub async fn get_effective_config(State(state): State<AppState>) -> Json<EffectiveConfigView> {
    Json(build_effective_config(state.cfg.as_ref()))
}

pub async fn get_config_spec(State(state): State<AppState>) -> Json<ConfigSpecView> {
    Json(build_config_spec(state.cfg.as_ref(), &state).await)
}

fn build_effective_config(cfg: &GatewayConfig) -> EffectiveConfigView {
    let mut channels = Vec::new();
    if cfg.channels.lark.is_some() {
        channels.push("lark".to_string());
    }
    if cfg.channels.dingtalk.is_some() {
        channels.push("dingtalk".to_string());
    }
    if cfg.channels.dingtalk_webhook.is_some() {
        channels.push("dingtalk_webhook".to_string());
    }

    EffectiveConfigView {
        default_backend_id: cfg.resolved_default_backend_id(),
        roster_agents: cfg
            .agent_roster
            .iter()
            .map(|agent| agent.name.clone())
            .collect(),
        team_scopes: cfg
            .normalized_team_scopes()
            .into_iter()
            .map(team_scope_view)
            .collect(),
        delivery_sender_bindings: cfg.delivery_sender_bindings.clone(),
        channels,
    }
}

async fn build_config_spec(cfg: &GatewayConfig, state: &AppState) -> ConfigSpecView {
    let diagnostics = crate::diagnostics::collect_backend_diagnostics(state).await;
    let diagnostic_map: std::collections::HashMap<_, _> = diagnostics
        .iter()
        .map(|entry| (entry.backend_id.as_str(), entry))
        .collect();
    let backend_specs = state.runtime_registry.all_backend_specs().await;
    let spec_map: std::collections::HashMap<_, _> = backend_specs
        .iter()
        .map(|entry| (entry.backend_id.as_str(), entry))
        .collect();

    ConfigSpecView {
        gateway: redact_gateway(cfg),
        agent: cfg.agent.clone(),
        auth: AuthSpecView {
            ws_token_configured: cfg
                .auth
                .ws_token
                .as_ref()
                .is_some_and(|token| !token.is_empty()),
        },
        channels: ChannelsSpecView {
            // DingTalkSection only contains `enabled: bool` and `presentation` mode —
            // no credentials or secrets. Safe to clone directly without redaction.
            dingtalk: cfg.channels.dingtalk.clone(),
            dingtalk_webhook: cfg
                .channels
                .dingtalk_webhook
                .as_ref()
                .map(redact_dingtalk_webhook),
            lark: cfg.channels.lark.as_ref().map(redact_lark),
        },
        skills: redact_skills(cfg),
        session: redact_session(cfg),
        memory: redact_memory(cfg),
        scheduler: redact_scheduler(cfg),
        agent_roster: cfg.agent_roster.iter().map(redact_agent_entry).collect(),
        backends: cfg
            .backends
            .iter()
            .map(|backend| {
                backend_config_view(
                    cfg,
                    backend,
                    spec_map.get(backend.id.as_str()).copied(),
                    diagnostic_map.get(backend.id.as_str()).copied(),
                )
            })
            .collect(),
        provider_profiles: cfg.provider_profiles.clone(),
        groups: cfg.groups.clone(),
        team_scopes: cfg.team_scopes.clone(),
        bindings: cfg.bindings.clone(),
        delivery_sender_bindings: cfg.delivery_sender_bindings.clone(),
        delivery_target_overrides: cfg.delivery_target_overrides.clone(),
    }
}

fn redact_gateway(cfg: &GatewayConfig) -> GatewaySpecView {
    GatewaySpecView {
        host: cfg.gateway.host.clone(),
        port: cfg.gateway.port,
        require_mention_in_groups: cfg.gateway.require_mention_in_groups,
        default_workspace_configured: cfg.gateway.default_workspace.is_some(),
    }
}

fn redact_skills(cfg: &GatewayConfig) -> SkillsSpecView {
    SkillsSpecView {
        dir_configured: !cfg.skills.dir.as_os_str().is_empty(),
        global_dir_count: cfg.skills.global_dirs.len(),
    }
}

fn redact_session(cfg: &GatewayConfig) -> SessionSpecView {
    SessionSpecView {
        dir_configured: !cfg.session.dir.as_os_str().is_empty(),
    }
}

fn redact_memory(cfg: &GatewayConfig) -> MemorySpecView {
    MemorySpecView {
        distill_every_n: cfg.memory.distill_every_n,
        distiller_binary: cfg.memory.distiller_binary.clone(),
        shared_dir_configured: !cfg.memory.shared_dir.as_os_str().is_empty(),
        shared_memory_max_words: cfg.memory.shared_memory_max_words,
        agent_memory_max_words: cfg.memory.agent_memory_max_words,
    }
}

fn redact_scheduler(cfg: &GatewayConfig) -> SchedulerSpecView {
    SchedulerSpecView {
        enabled: cfg.scheduler.enabled,
        poll_secs: cfg.scheduler.poll_secs,
        max_concurrent: cfg.scheduler.max_concurrent,
        max_fetch_per_tick: cfg.scheduler.max_fetch_per_tick,
        default_timezone: cfg.scheduler.default_timezone.clone(),
        db_path_configured: cfg.scheduler.db_path.is_some(),
        lease_secs: cfg.scheduler.lease_secs,
    }
}

fn team_scope_view(scope: TeamScopeSpec) -> TeamScopeView {
    TeamScopeView {
        scope: scope.scope,
        name: scope.name,
        channel: scope.mode.channel,
        front_bot: scope.mode.front_bot,
        roster: scope.team.roster,
    }
}

fn redact_dingtalk_webhook(cfg: &DingTalkWebhookSection) -> DingTalkWebhookSpecView {
    DingTalkWebhookSpecView {
        enabled: cfg.enabled,
        webhook_path: cfg.webhook_path.clone(),
        access_token_configured: cfg
            .access_token
            .as_ref()
            .is_some_and(|token| !token.is_empty()),
        secret_key_configured: !cfg.secret_key.is_empty(),
        presentation: cfg.presentation,
    }
}

fn redact_lark(cfg: &LarkSection) -> LarkSpecView {
    LarkSpecView {
        enabled: cfg.enabled,
        presentation: cfg.presentation,
        trigger_policy: cfg.trigger_policy,
        default_instance: cfg.default_instance.clone(),
        instances: cfg.instances.iter().map(redact_lark_instance).collect(),
    }
}

fn redact_lark_instance(instance: &LarkInstanceConfig) -> LarkInstanceSpecView {
    LarkInstanceSpecView {
        id: instance.id.clone(),
        app_id: instance.app_id.clone(),
        bot_name: instance.bot_name.clone(),
        app_secret_configured: !instance.app_secret.is_empty(),
    }
}

fn redact_agent_entry(entry: &AgentEntry) -> AgentEntrySpecView {
    AgentEntrySpecView {
        name: entry.name.clone(),
        mentions: entry.mentions.clone(),
        backend_id: entry.backend_id.clone(),
        persona_dir_configured: entry.persona_dir.is_some(),
        workspace_dir_configured: entry.workspace_dir.is_some(),
        extra_skills_dir_count: entry.extra_skills_dirs.len(),
    }
}
