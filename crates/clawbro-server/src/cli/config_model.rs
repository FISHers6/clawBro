use crate::agent_core::roster::AgentEntry;
use crate::cli::config_keys::{
    AgentKey, BackendKey, BindingKey, DeliverySenderBindingKey, DeliveryTargetOverrideKey,
    ProviderKey, TeamScopeKey,
};
use crate::config::{
    AuthSection, BackendCatalogEntry, ChannelsSection, DeliverySenderBindingConfig,
    DeliveryTargetOverrideConfig, GatewayConfig, GatewaySection, GroupConfig, MemorySection,
    ProviderProfileConfig, SchedulerSection, SessionSection, SkillsSection, TeamScopeConfig,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct ConfigGraph {
    pub gateway: GatewaySection,
    pub agent: crate::config::AgentSection,
    pub auth: AuthSection,
    pub channels: ChannelsSection,
    pub skills: SkillsSection,
    pub session: SessionSection,
    pub memory: MemorySection,
    pub scheduler: SchedulerSection,
    pub providers: BTreeMap<String, ProviderProfileConfig>,
    pub backends: BTreeMap<String, BackendCatalogEntry>,
    pub agents: BTreeMap<String, AgentEntry>,
    pub groups: BTreeMap<String, GroupConfig>,
    pub team_scopes: BTreeMap<TeamScopeKey, TeamScopeConfig>,
    pub bindings: BTreeMap<String, crate::config::BindingConfig>,
    pub delivery_sender_bindings: BTreeMap<String, DeliverySenderBindingConfig>,
    pub delivery_target_overrides: BTreeMap<String, DeliveryTargetOverrideConfig>,
}

impl ConfigGraph {
    pub fn from_gateway_config(cfg: GatewayConfig) -> Self {
        let providers = cfg
            .provider_profiles
            .into_iter()
            .map(|provider| (ProviderKey::new(provider.id.clone()).to_string(), provider))
            .collect();
        let backends = cfg
            .backends
            .into_iter()
            .map(|backend| (BackendKey::new(backend.id.clone()).to_string(), backend))
            .collect();
        let agents = cfg
            .agent_roster
            .into_iter()
            .map(|agent| (AgentKey::new(agent.name.clone()).to_string(), agent))
            .collect();
        let groups = cfg
            .groups
            .into_iter()
            .map(|group| (group.scope.clone(), group))
            .collect();
        let team_scopes = cfg
            .team_scopes
            .into_iter()
            .map(|team_scope| {
                let channel = team_scope
                    .mode
                    .channel
                    .clone()
                    .unwrap_or_else(|| "*".to_string());
                (
                    TeamScopeKey::new(channel, team_scope.scope.clone()),
                    team_scope,
                )
            })
            .collect();
        let bindings = cfg
            .bindings
            .into_iter()
            .map(|binding| (BindingKey::from_binding(&binding).to_string(), binding))
            .collect();
        let delivery_sender_bindings = cfg
            .delivery_sender_bindings
            .into_iter()
            .map(|binding| {
                (
                    DeliverySenderBindingKey::from_binding(&binding).to_string(),
                    binding,
                )
            })
            .collect();
        let delivery_target_overrides = cfg
            .delivery_target_overrides
            .into_iter()
            .map(|binding| {
                (
                    DeliveryTargetOverrideKey::from_binding(&binding).to_string(),
                    binding,
                )
            })
            .collect();

        Self {
            gateway: cfg.gateway,
            agent: cfg.agent,
            auth: cfg.auth,
            channels: cfg.channels,
            skills: cfg.skills,
            session: cfg.session,
            memory: cfg.memory,
            scheduler: cfg.scheduler,
            providers,
            backends,
            agents,
            groups,
            team_scopes,
            bindings,
            delivery_sender_bindings,
            delivery_target_overrides,
        }
    }

    pub fn from_toml_str(content: &str) -> anyhow::Result<Self> {
        Ok(Self::from_gateway_config(GatewayConfig::from_toml_str(
            content,
        )?))
    }

    pub fn to_gateway_config(&self) -> GatewayConfig {
        let team_scopes = self
            .team_scopes
            .iter()
            .map(|(_, team_scope)| team_scope.clone())
            .collect();
        GatewayConfig {
            gateway: self.gateway.clone(),
            agent: self.agent.clone(),
            auth: self.auth.clone(),
            channels: self.channels.clone(),
            skills: self.skills.clone(),
            session: self.session.clone(),
            agent_roster: self.agents.values().cloned().collect(),
            backends: self.backends.values().cloned().collect(),
            provider_profiles: self.providers.values().cloned().collect(),
            memory: self.memory.clone(),
            scheduler: self.scheduler.clone(),
            groups: self.groups.values().cloned().collect(),
            team_scopes,
            bindings: self.bindings.values().cloned().collect(),
            delivery_sender_bindings: self.delivery_sender_bindings.values().cloned().collect(),
            delivery_target_overrides: self.delivery_target_overrides.values().cloned().collect(),
        }
    }

    pub fn provider_ids(&self) -> BTreeSet<String> {
        self.providers.keys().cloned().collect()
    }

    pub fn backend_ids(&self) -> BTreeSet<String> {
        self.backends.keys().cloned().collect()
    }

    pub fn agent_names(&self) -> BTreeSet<String> {
        self.agents.keys().cloned().collect()
    }
}

impl Default for ConfigGraph {
    fn default() -> Self {
        Self::from_gateway_config(GatewayConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_graph_collects_resources_by_stable_keys() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
[gateway]
port = 18080

[agent]
backend_id = "claude-main"

[[provider_profile]]
id = "deepseek-anthropic"
protocol = "anthropic_compatible"
base_url = "https://api.deepseek.com/anthropic"
auth_token_env = "DEEPSEEK_API_KEY"
default_model = "deepseek-chat"

[[backend]]
id = "claude-main"
family = "acp"
acp_backend = "claude"
provider_profile = "deepseek-anthropic"

[backend.launch]
type = "bundled_command"

[[agent_roster]]
name = "claude"
mentions = ["@claude"]
backend_id = "claude-main"

[[team_scope]]
scope = "user:abc"
name = "wechat-team"

[team_scope.mode]
interaction = "team"
channel = "wechat"
front_bot = "claude"

[team_scope.team]
roster = ["claude"]
"#,
        )
        .unwrap();

        let graph = ConfigGraph::from_gateway_config(cfg);
        assert!(graph.providers.contains_key("deepseek-anthropic"));
        assert!(graph.backends.contains_key("claude-main"));
        assert!(graph.agents.contains_key("claude"));
        assert!(graph
            .team_scopes
            .contains_key(&TeamScopeKey::new("wechat", "user:abc")));
    }

    #[test]
    fn config_graph_round_trips_gateway_config() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
[channels.wechat]
enabled = true
presentation = "progress_compact"

[[binding]]
kind = "channel"
agent = "claw"
channel = "wechat"
"#,
        )
        .unwrap();

        let graph = ConfigGraph::from_gateway_config(cfg.clone());
        let rendered = graph.to_gateway_config();
        assert_eq!(
            rendered.channels.wechat.as_ref().map(|c| c.enabled),
            cfg.channels.wechat.as_ref().map(|c| c.enabled)
        );
        assert_eq!(rendered.bindings, cfg.bindings);
    }
}
