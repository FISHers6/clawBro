use crate::config::{GatewayConfig, GroupModeConfig, InteractionMode};
use crate::protocol::SessionKey;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ApiSessionQuery {
    pub channel: String,
    pub scope: String,
    #[serde(default)]
    pub channel_instance: Option<String>,
}

impl ApiSessionQuery {
    pub fn to_session_key(&self) -> SessionKey {
        SessionKey {
            channel: self.channel.clone(),
            scope: self.scope.clone(),
            channel_instance: self.channel_instance.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ApiIdentityRef {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ApiErrorBody {
    pub error: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ApiListResponse<T> {
    pub items: Vec<T>,
}

pub fn derive_agent_identities(cfg: &GatewayConfig, agent_name: &str) -> Vec<String> {
    let mut identities = BTreeSet::new();
    for group in &cfg.groups {
        collect_identities_from_mode(agent_name, &group.mode, &group.team.roster, &mut identities);
    }
    for team_scope in &cfg.team_scopes {
        collect_identities_from_mode(
            agent_name,
            &team_scope.mode,
            &team_scope.team.roster,
            &mut identities,
        );
    }
    if identities.is_empty() {
        identities.insert("standalone_bot".to_string());
    }
    identities.into_iter().collect()
}

fn collect_identities_from_mode(
    agent_name: &str,
    mode: &GroupModeConfig,
    roster: &[String],
    identities: &mut BTreeSet<String>,
) {
    if mode
        .front_bot
        .as_ref()
        .is_some_and(|front_bot| front_bot == agent_name)
    {
        identities.insert("front_bot".to_string());
    }
    if matches!(mode.interaction, InteractionMode::Team)
        && roster.iter().any(|member| member == agent_name)
    {
        identities.insert("roster_member".to_string());
    }
}
