use crate::agent_core::team::milestone_delivery::TeamPublicUpdatesMode;
use crate::cli::config_keys::{BindingKey, TeamScopeKey};
use crate::config::{
    BindingConfig, GroupModeConfig, GroupTeamConfig, InteractionMode, TeamScopeConfig,
};
use anyhow::Result;

pub fn parse_public_updates(value: &str) -> Result<TeamPublicUpdatesMode> {
    match value.trim().to_lowercase().as_str() {
        "minimal" => Ok(TeamPublicUpdatesMode::Minimal),
        "normal" => Ok(TeamPublicUpdatesMode::Normal),
        "verbose" => Ok(TeamPublicUpdatesMode::Verbose),
        _ => anyhow::bail!("unsupported public_updates `{value}`"),
    }
}

pub fn build_channel_binding(agent: String, channel: String) -> (BindingKey, BindingConfig) {
    let binding = BindingConfig::Channel { agent, channel };
    let key = BindingKey::from_binding(&binding);
    (key, binding)
}

pub fn build_team_scope(
    channel: String,
    scope: String,
    name: Option<String>,
    front_bot: String,
    specialists: Vec<String>,
    max_parallel: usize,
    public_updates: TeamPublicUpdatesMode,
) -> Result<(TeamScopeKey, TeamScopeConfig)> {
    if channel == "wechat" && !scope.starts_with("user:") {
        anyhow::bail!("wechat team scope must use `user:*` DM scope");
    }

    let key = TeamScopeKey::new(channel.clone(), scope.clone());
    let value = TeamScopeConfig {
        scope,
        name,
        mode: GroupModeConfig {
            interaction: InteractionMode::Team,
            auto_promote: false,
            front_bot: Some(front_bot),
            channel: Some(channel),
        },
        team: GroupTeamConfig {
            roster: specialists,
            public_updates,
            max_parallel,
        },
    };
    Ok((key, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_channel_binding_returns_stable_key() {
        let (key, binding) = build_channel_binding("claw".to_string(), "wechat".to_string());
        assert_eq!(key.to_string(), "channel:claw:wechat");
        match binding {
            BindingConfig::Channel { agent, channel } => {
                assert_eq!(agent, "claw");
                assert_eq!(channel, "wechat");
            }
            _ => panic!("expected channel binding"),
        }
    }

    #[test]
    fn build_team_scope_rejects_wechat_group_style_scope() {
        let err = build_team_scope(
            "wechat".to_string(),
            "group:abc".to_string(),
            None,
            "claude".to_string(),
            vec!["claw".to_string()],
            1,
            TeamPublicUpdatesMode::Minimal,
        )
        .unwrap_err();
        assert!(err.to_string().contains("wechat team scope"));
    }

    #[test]
    fn parse_public_updates_supports_verbose() {
        assert_eq!(
            parse_public_updates("verbose").unwrap(),
            TeamPublicUpdatesMode::Verbose
        );
    }
}
