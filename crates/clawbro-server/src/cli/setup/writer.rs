use super::{
    auth_cfg::AuthConfig,
    channel::{ChannelConfig, DingTalkCfg, DingTalkReceiveMode, LarkCfg, WeChatCfg},
    mode::{Mode, ModeConfig, TeamTarget},
    provider::{ProviderConfig, ProviderKind},
};
use crate::agent_core::roster::AgentEntry;
use crate::cli::config_apply::{
    apply_graph_to_path, default_config_path, default_env_path, render_graph_to_toml,
};
use crate::cli::config_keys::TeamScopeKey;
use crate::cli::config_model::ConfigGraph;
use crate::cli::config_validate::validate_graph_static;
use crate::config::{
    BackendCatalogEntry, BackendFamilyConfig, BackendLaunchConfig, DingTalkSection,
    DingTalkWebhookSection, GroupConfig, GroupModeConfig, GroupTeamConfig, InteractionMode,
    LarkInstanceConfig, LarkSection, ProgressPresentationMode, ProviderProfileConfig,
    ProviderProfileProtocolConfig, TeamScopeConfig, WeChatSection,
};
use anyhow::{Context, Result};
use std::path::PathBuf;

pub struct WriteInputs<'a> {
    pub provider: &'a ProviderConfig,
    pub mode: &'a ModeConfig,
    pub auth: &'a AuthConfig,
    pub channel: &'a ChannelConfig,
}

fn configured_team_channel(channel: &ChannelConfig) -> &'static str {
    match channel {
        ChannelConfig::WeChat(_) => "wechat",
        ChannelConfig::Lark(_) => "lark",
        ChannelConfig::DingTalk(d) => match d.receive_mode {
            DingTalkReceiveMode::Stream => "dingtalk",
            DingTalkReceiveMode::Webhook => "dingtalk_webhook",
        },
        ChannelConfig::None => "ws",
    }
}

fn provider_profile_auth_env(provider: &ProviderConfig) -> String {
    match provider.kind {
        ProviderKind::Anthropic => provider.kind.env_var().to_string(),
        _ => {
            let env = provider.kind.env_var().trim();
            if env.is_empty() {
                "OPENAI_API_KEY".to_string()
            } else {
                env.to_string()
            }
        }
    }
}

fn build_provider_profile(provider: &ProviderConfig) -> ProviderProfileConfig {
    let protocol = match provider.kind {
        ProviderKind::Anthropic => ProviderProfileProtocolConfig::AnthropicCompatible {
            base_url: provider.base_url.clone(),
            auth_token_env: provider_profile_auth_env(provider),
            default_model: provider.model.clone(),
            small_fast_model: None,
        },
        _ => ProviderProfileProtocolConfig::OpenaiCompatible {
            base_url: provider.base_url.clone(),
            auth_token_env: provider_profile_auth_env(provider),
            default_model: provider.model.clone(),
        },
    };

    ProviderProfileConfig {
        id: provider.profile_id.clone(),
        protocol,
    }
}

fn build_default_backend(provider: &ProviderConfig) -> BackendCatalogEntry {
    BackendCatalogEntry {
        id: "native-main".to_string(),
        family: BackendFamilyConfig::ClawBroNative,
        adapter_key: None,
        acp_backend: None,
        acp_auth_method: None,
        codex: None,
        provider_profile: Some(provider.profile_id.clone()),
        approval: Default::default(),
        external_mcp_servers: vec![],
        launch: BackendLaunchConfig::BundledCommand,
    }
}

fn make_agent_entry(name: String) -> AgentEntry {
    AgentEntry {
        mentions: vec![format!("@{name}")],
        name,
        backend_id: "native-main".to_string(),
        persona_dir: None,
        workspace_dir: None,
        extra_skills_dirs: vec![],
    }
}

fn insert_channel_config(graph: &mut ConfigGraph, channel: &ChannelConfig) {
    match channel {
        ChannelConfig::WeChat(WeChatCfg { presentation, .. }) => {
            graph.channels.wechat = Some(WeChatSection {
                enabled: true,
                presentation: *presentation,
            });
        }
        ChannelConfig::Lark(LarkCfg {
            app_id,
            app_secret,
            bot_name,
        }) => {
            graph.channels.lark = Some(LarkSection {
                enabled: true,
                presentation: ProgressPresentationMode::default(),
                trigger_policy: None,
                default_instance: Some("default".to_string()),
                instances: vec![LarkInstanceConfig {
                    id: "default".to_string(),
                    app_id: app_id.clone(),
                    app_secret: app_secret.clone(),
                    bot_name: bot_name.clone(),
                }],
            });
        }
        ChannelConfig::DingTalk(DingTalkCfg { receive_mode, .. }) => match receive_mode {
            DingTalkReceiveMode::Stream => {
                graph.channels.dingtalk = Some(DingTalkSection {
                    enabled: true,
                    presentation: ProgressPresentationMode::default(),
                });
            }
            DingTalkReceiveMode::Webhook => {
                graph.channels.dingtalk_webhook = Some(DingTalkWebhookSection {
                    enabled: true,
                    secret_key: channel.webhook_secret_key().unwrap_or_default().to_string(),
                    webhook_path: channel
                        .webhook_path()
                        .unwrap_or("/channels/dingtalk/webhook")
                        .to_string(),
                    access_token: channel.webhook_access_token().map(str::to_string),
                    presentation: ProgressPresentationMode::default(),
                });
            }
        },
        ChannelConfig::None => {}
    }
}

pub fn build_config_graph(input: &WriteInputs) -> ConfigGraph {
    let mut graph = ConfigGraph::default();
    graph.gateway.port = input.mode.port;
    graph.gateway.require_mention_in_groups = !matches!(input.mode.mode, Mode::Solo);
    graph.gateway.default_workspace = input.mode.workspace.clone();
    graph.auth.ws_token = input.auth.ws_token.clone();

    let provider = build_provider_profile(input.provider);
    graph.providers.insert(provider.id.clone(), provider);

    let backend = build_default_backend(input.provider);
    graph.backends.insert(backend.id.clone(), backend);

    insert_channel_config(&mut graph, input.channel);

    match input.mode.mode {
        Mode::Solo => {
            graph.agent.backend_id = "native-main".to_string();
        }
        Mode::Multi => {
            // Keep setup output valid while still allowing the user to extend into a roster later.
            graph.agent.backend_id = "native-main".to_string();
        }
        Mode::Team => {
            let front_bot = input
                .mode
                .front_bot
                .as_deref()
                .unwrap_or("lead")
                .to_string();
            let specialists = if input.mode.specialists.is_empty() {
                vec!["specialist".to_string()]
            } else {
                input.mode.specialists.clone()
            };
            graph
                .agents
                .insert(front_bot.clone(), make_agent_entry(front_bot.clone()));
            for specialist in &specialists {
                graph
                    .agents
                    .insert(specialist.clone(), make_agent_entry(specialist.clone()));
            }

            let mode = GroupModeConfig {
                interaction: InteractionMode::Team,
                auto_promote: false,
                front_bot: Some(front_bot),
                channel: Some(configured_team_channel(input.channel).to_string()),
            };
            let team = GroupTeamConfig {
                roster: specialists,
                ..Default::default()
            };

            match input.mode.team_target.unwrap_or(TeamTarget::DirectMessage) {
                TeamTarget::DirectMessage => {
                    let scope = input
                        .mode
                        .team_scope
                        .clone()
                        .unwrap_or_else(|| "user:default".to_string());
                    let key = TeamScopeKey::new(configured_team_channel(input.channel), &scope);
                    graph.team_scopes.insert(
                        key,
                        TeamScopeConfig {
                            scope,
                            name: input.mode.team_name.clone(),
                            mode,
                            team,
                        },
                    );
                }
                TeamTarget::Group => {
                    let scope = input
                        .mode
                        .team_scope
                        .clone()
                        .unwrap_or_else(|| "group:default".to_string());
                    graph.groups.insert(
                        scope.clone(),
                        GroupConfig {
                            scope,
                            name: input.mode.team_name.clone(),
                            mode,
                            team,
                        },
                    );
                }
            }
        }
    }

    graph
}

pub fn build_config_toml(input: &WriteInputs) -> Result<String> {
    render_graph_to_toml(&build_config_graph(input))
}

pub fn write_config(input: &WriteInputs) -> Result<Option<PathBuf>> {
    let graph = build_config_graph(input);
    let report = validate_graph_static(&graph);
    if report.has_errors() {
        let summary = report
            .issues
            .iter()
            .map(|issue| issue.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(summary);
    }
    apply_graph_to_path(&graph, &config_path())
}

pub fn build_env_content(provider: &ProviderConfig, channel: &ChannelConfig) -> String {
    let mut lines = Vec::<String>::new();
    let env_var = provider.kind.env_var();
    if !env_var.is_empty() && !provider.api_key.is_empty() {
        lines.push(format!("export {}={}", env_var, provider.api_key));
    }
    match channel {
        ChannelConfig::WeChat(_) => {}
        ChannelConfig::Lark(l) => {
            lines.push(format!("export LARK_APP_ID={}", l.app_id));
            lines.push(format!("export LARK_APP_SECRET={}", l.app_secret));
        }
        ChannelConfig::DingTalk(d) => {
            if d.receive_mode == DingTalkReceiveMode::Stream {
                if let Some(client_id) = d.client_id.as_deref().filter(|value| !value.is_empty()) {
                    lines.push(format!("export DINGTALK_APP_KEY={client_id}"));
                }
                if let Some(client_secret) =
                    d.client_secret.as_deref().filter(|value| !value.is_empty())
                {
                    lines.push(format!("export DINGTALK_APP_SECRET={client_secret}"));
                }
            }
        }
        ChannelConfig::None => {}
    }
    if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n") + "\n"
    }
}

pub fn write_env(provider: &ProviderConfig, channel: &ChannelConfig) -> Result<()> {
    let content = build_env_content(provider, channel);
    if content.is_empty() {
        return Ok(());
    }
    let path = env_path();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(&path, content).context("write .env")?;
    Ok(())
}

pub fn config_path() -> PathBuf {
    default_config_path()
}

pub fn env_path() -> PathBuf {
    default_env_path()
}

trait DingTalkCfgExt {
    fn webhook_secret_key(&self) -> Option<&str>;
    fn webhook_access_token(&self) -> Option<&str>;
    fn webhook_path(&self) -> Option<&str>;
}

impl DingTalkCfgExt for ChannelConfig {
    fn webhook_secret_key(&self) -> Option<&str> {
        match self {
            ChannelConfig::DingTalk(cfg) => cfg.webhook_secret_key.as_deref(),
            _ => None,
        }
    }

    fn webhook_access_token(&self) -> Option<&str> {
        match self {
            ChannelConfig::DingTalk(cfg) => cfg.webhook_access_token.as_deref(),
            _ => None,
        }
    }

    fn webhook_path(&self) -> Option<&str> {
        match self {
            ChannelConfig::DingTalk(cfg) => cfg.webhook_path.as_deref(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::config_validate::validate_graph_static;
    use crate::cli::setup::{auth_cfg::AuthConfig, mode::ModeConfig};

    fn anthropic() -> ProviderConfig {
        ProviderConfig {
            kind: ProviderKind::Anthropic,
            api_key: "sk-ant-test".into(),
            base_url: "https://api.anthropic.com".into(),
            model: "claude-sonnet-4-6".into(),
            profile_id: "anthropic-main".into(),
        }
    }

    fn solo() -> ModeConfig {
        ModeConfig {
            mode: Mode::Solo,
            team_target: None,
            front_bot: None,
            specialists: Vec::new(),
            team_scope: None,
            team_name: None,
            port: 8080,
            workspace: None,
        }
    }

    fn no_auth() -> AuthConfig {
        AuthConfig { ws_token: None }
    }

    fn render(input: &WriteInputs) -> String {
        build_config_toml(input).unwrap()
    }

    #[test]
    fn solo_setup_graph_is_valid() {
        let graph = build_config_graph(&WriteInputs {
            provider: &anthropic(),
            mode: &solo(),
            auth: &no_auth(),
            channel: &ChannelConfig::None,
        });
        let report = validate_graph_static(&graph);
        assert!(!report.has_errors(), "{report:#?}");
        assert_eq!(graph.agent.backend_id, "native-main");
    }

    #[test]
    fn team_dm_setup_graph_is_valid_and_uses_exact_scope() {
        let graph = build_config_graph(&WriteInputs {
            provider: &anthropic(),
            mode: &ModeConfig {
                mode: Mode::Team,
                team_target: Some(TeamTarget::DirectMessage),
                front_bot: Some("planner".into()),
                specialists: vec!["coder".into(), "reviewer".into()],
                team_scope: Some("user:ou_demo".into()),
                team_name: Some("my-team".into()),
                port: 8080,
                workspace: None,
            },
            auth: &no_auth(),
            channel: &ChannelConfig::WeChat(WeChatCfg {
                presentation: ProgressPresentationMode::ProgressCompact,
                login_now: false,
            }),
        });
        let report = validate_graph_static(&graph);
        assert!(!report.has_errors(), "{report:#?}");
        assert!(graph.agents.contains_key("planner"));
        assert!(graph.agents.contains_key("coder"));
        assert!(graph
            .team_scopes
            .contains_key(&TeamScopeKey::new("wechat", "user:ou_demo")));
    }

    #[test]
    fn team_group_setup_graph_is_valid_and_uses_group_scope() {
        let graph = build_config_graph(&WriteInputs {
            provider: &anthropic(),
            mode: &ModeConfig {
                mode: Mode::Team,
                team_target: Some(TeamTarget::Group),
                front_bot: Some("captain".into()),
                specialists: vec!["analyst".into()],
                team_scope: Some("group:lark:chat-123".into()),
                team_name: Some("research-room".into()),
                port: 8080,
                workspace: None,
            },
            auth: &no_auth(),
            channel: &ChannelConfig::Lark(LarkCfg {
                app_id: "cli_abc".into(),
                app_secret: "sec".into(),
                bot_name: Some("AI".into()),
            }),
        });
        let report = validate_graph_static(&graph);
        assert!(!report.has_errors(), "{report:#?}");
        assert_eq!(
            graph
                .groups
                .get("group:lark:chat-123")
                .and_then(|group| group.mode.channel.as_deref()),
            Some("lark")
        );
    }

    #[test]
    fn rendered_toml_contains_enabled_wechat_channel() {
        let text = render(&WriteInputs {
            provider: &anthropic(),
            mode: &solo(),
            auth: &no_auth(),
            channel: &ChannelConfig::WeChat(WeChatCfg {
                presentation: ProgressPresentationMode::ProgressCompact,
                login_now: false,
            }),
        });
        assert!(text.contains("[channels.wechat]"));
        assert!(text.contains("presentation = \"progress_compact\""));
    }

    #[test]
    fn rendered_toml_contains_lark_instance() {
        let text = render(&WriteInputs {
            provider: &anthropic(),
            mode: &solo(),
            auth: &no_auth(),
            channel: &ChannelConfig::Lark(LarkCfg {
                app_id: "cli_abc".into(),
                app_secret: "sec".into(),
                bot_name: Some("AI".into()),
            }),
        });
        assert!(text.contains("[channels.lark]"));
        assert!(text.contains("id = \"default\""));
        assert!(text.contains("app_id = \"cli_abc\""));
    }

    #[test]
    fn rendered_toml_contains_dingtalk_webhook_channel() {
        let text = render(&WriteInputs {
            provider: &anthropic(),
            mode: &solo(),
            auth: &no_auth(),
            channel: &ChannelConfig::DingTalk(DingTalkCfg {
                receive_mode: DingTalkReceiveMode::Webhook,
                client_id: None,
                client_secret: None,
                agent_id: None,
                bot_name: None,
                webhook_secret_key: Some("SEC-test".into()),
                webhook_access_token: Some("dt-token".into()),
                webhook_path: Some("/dingtalk-channel/message".into()),
            }),
        });
        assert!(text.contains("[channels.dingtalk_webhook]"));
        assert!(text.contains("secret_key = \"SEC-test\""));
        assert!(text.contains("webhook_path = \"/dingtalk-channel/message\""));
        assert!(text.contains("access_token = \"dt-token\""));
    }

    #[test]
    fn multi_mode_keeps_valid_default_backend() {
        let graph = build_config_graph(&WriteInputs {
            provider: &anthropic(),
            mode: &ModeConfig {
                mode: Mode::Multi,
                team_target: None,
                front_bot: None,
                specialists: vec![],
                team_scope: None,
                team_name: None,
                port: 8080,
                workspace: None,
            },
            auth: &no_auth(),
            channel: &ChannelConfig::None,
        });
        let report = validate_graph_static(&graph);
        assert!(!report.has_errors(), "{report:#?}");
        assert_eq!(graph.agent.backend_id, "native-main");
        assert!(graph.agents.is_empty());
    }

    #[test]
    fn env_content_writes_expected_exports() {
        let text = build_env_content(
            &anthropic(),
            &ChannelConfig::Lark(LarkCfg {
                app_id: "cli_abc".into(),
                app_secret: "sec".into(),
                bot_name: None,
            }),
        );
        assert!(text.contains("ANTHROPIC_API_KEY=sk-ant-test"));
        assert!(text.contains("LARK_APP_ID=cli_abc"));
        assert!(text.contains("LARK_APP_SECRET=sec"));
    }

    #[test]
    fn ollama_env_content_stays_empty() {
        let ollama = ProviderConfig {
            kind: ProviderKind::Ollama,
            api_key: String::new(),
            base_url: "http://localhost:11434".into(),
            model: "llama3".into(),
            profile_id: "ollama-main".into(),
        };
        assert!(build_env_content(&ollama, &ChannelConfig::None).is_empty());
    }
}
