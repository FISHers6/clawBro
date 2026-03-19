use super::{
    auth_cfg::AuthConfig,
    channel::{ChannelConfig, DingTalkReceiveMode},
    mode::{Mode, ModeConfig, TeamTarget},
    provider::ProviderConfig,
};
use anyhow::{Context, Result};
use std::path::PathBuf;

pub struct WriteInputs<'a> {
    pub provider: &'a ProviderConfig,
    pub mode: &'a ModeConfig,
    pub auth: &'a AuthConfig,
    pub channel: &'a ChannelConfig,
}

fn configured_team_channel(channel: &ChannelConfig) -> Option<&'static str> {
    match channel {
        ChannelConfig::Lark(_) => Some("lark"),
        ChannelConfig::DingTalk(d) => match d.receive_mode {
            DingTalkReceiveMode::Stream => Some("dingtalk"),
            DingTalkReceiveMode::Webhook => Some("dingtalk_webhook"),
        },
        ChannelConfig::None => None,
    }
}

pub fn build_config_toml(input: &WriteInputs) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let qdir = home.join(".clawbro");
    let front_bot = input.mode.front_bot.as_deref().unwrap_or("lead");
    let team_scope = input.mode.team_scope.as_deref();
    let team_name = input.mode.team_name.as_deref();
    let specialists = if input.mode.specialists.is_empty() {
        vec!["specialist".to_string()]
    } else {
        input.mode.specialists.clone()
    };
    let mut s = String::new();

    // [gateway]
    s.push_str("[gateway]\n");
    s.push_str("host = \"127.0.0.1\"\n");
    s.push_str(&format!("port = {}\n", input.mode.port));
    let require_mention = !matches!(input.mode.mode, Mode::Solo);
    s.push_str(&format!(
        "require_mention_in_groups = {}\n",
        require_mention
    ));
    if let Some(ws) = &input.mode.workspace {
        s.push_str(&format!(
            "default_workspace = {:?}\n",
            ws.to_string_lossy().as_ref()
        ));
    }
    s.push('\n');

    // [auth]
    if let Some(tok) = &input.auth.ws_token {
        s.push_str("[auth]\n");
        s.push_str(&format!("ws_token = {:?}\n", tok));
        s.push('\n');
    }

    // [[provider_profile]]
    s.push_str("[[provider_profile]]\n");
    s.push_str(&format!("id = {:?}\n", input.provider.profile_id));
    s.push_str(&format!(
        "protocol = {:?}\n",
        input.provider.kind.protocol_tag()
    ));
    s.push_str(&format!("base_url = {:?}\n", input.provider.base_url));
    if !input.provider.kind.env_var().is_empty() {
        s.push_str(&format!(
            "auth_token_env = {:?}\n",
            input.provider.kind.env_var()
        ));
    }
    s.push_str(&format!("default_model = {:?}\n", input.provider.model));
    s.push('\n');

    // [[backend]]
    s.push_str("[[backend]]\n");
    s.push_str("id = \"native-main\"\n");
    s.push_str("family = \"claw_bro_native\"\n");
    s.push_str(&format!(
        "provider_profile = {:?}\n",
        input.provider.profile_id
    ));
    s.push('\n');
    s.push_str("[backend.launch]\n");
    s.push_str("type = \"bundled_command\"\n");
    s.push('\n');

    // [agent], team skeleton, or multi-agent comments
    match input.mode.mode {
        Mode::Solo => {
            s.push_str("[agent]\n");
            s.push_str("backend_id = \"native-main\"\n");
            s.push('\n');
        }
        Mode::Multi => {
            s.push_str("# Add [[agent_roster]] entries below to configure multiple agents\n");
            s.push_str("# Example:\n");
            s.push_str("# [[agent_roster]]\n");
            s.push_str("# name = \"claude\"\n");
            s.push_str("# mentions = [\"@claude\"]\n");
            s.push_str("# backend_id = \"native-main\"\n");
            s.push('\n');
        }
        Mode::Team => {
            let team_channel = configured_team_channel(input.channel);
            s.push_str("[[agent_roster]]\n");
            s.push_str(&format!("name = {:?}\n", front_bot));
            s.push_str(&format!("mentions = [{:?}]\n", format!("@{front_bot}")));
            s.push_str("backend_id = \"native-main\"\n\n");

            for specialist in &specialists {
                s.push_str("[[agent_roster]]\n");
                s.push_str(&format!("name = {:?}\n", specialist));
                s.push_str(&format!("mentions = [{:?}]\n", format!("@{specialist}")));
                s.push_str("backend_id = \"native-main\"\n\n");
            }

            let specialist_roster = specialists
                .iter()
                .map(|specialist| format!("{specialist:?}"))
                .collect::<Vec<_>>()
                .join(", ");

            match input.mode.team_target.unwrap_or(TeamTarget::DirectMessage) {
                TeamTarget::DirectMessage => {
                    s.push_str("[[team_scope]]\n");
                    s.push_str(&format!(
                        "scope = {:?}\n",
                        team_scope.unwrap_or("user:default")
                    ));
                    s.push_str(&format!("name = {:?}\n\n", team_name.unwrap_or("my-team")));
                    s.push_str("[team_scope.mode]\n");
                    s.push_str("interaction = \"team\"\n");
                    s.push_str(&format!("front_bot = {:?}\n", front_bot));
                    if let Some(channel) = team_channel {
                        s.push_str(&format!("channel = {:?}\n", channel));
                    }
                    s.push('\n');
                    s.push_str("[team_scope.team]\n");
                    s.push_str(&format!("roster = [{}]\n", specialist_roster));
                    s.push_str("max_parallel = 1\n");
                    s.push('\n');
                }
                TeamTarget::Group => {
                    s.push_str("[[group]]\n");
                    let default_scope = match team_channel {
                        Some("lark") => "group:lark:default",
                        Some("dingtalk") => "group:dingtalk:default",
                        _ => "group:default",
                    };
                    s.push_str(&format!(
                        "scope = {:?}\n",
                        team_scope.unwrap_or(default_scope)
                    ));
                    if let Some(name) = team_name {
                        s.push_str(&format!("name = {:?}\n", name));
                    }
                    s.push('\n');
                    s.push_str("[group.mode]\n");
                    s.push_str("interaction = \"team\"\n");
                    s.push_str(&format!("front_bot = {:?}\n", front_bot));
                    if let Some(channel) = team_channel {
                        s.push_str(&format!("channel = {:?}\n", channel));
                    }
                    s.push('\n');
                    s.push_str("[group.team]\n");
                    s.push_str(&format!("roster = [{}]\n", specialist_roster));
                    s.push_str("max_parallel = 1\n");
                    s.push('\n');
                }
            }
        }
    }

    // [session]
    s.push_str("[session]\n");
    s.push_str(&format!(
        "dir = {:?}\n",
        qdir.join("sessions").to_string_lossy().as_ref()
    ));
    s.push('\n');

    // [memory]
    s.push_str("[memory]\n");
    s.push_str(&format!(
        "shared_dir = {:?}\n",
        qdir.join("shared").to_string_lossy().as_ref()
    ));
    s.push_str("distill_every_n = 20\n");
    s.push_str("distiller_binary = \"clawbro\"\n");
    s.push('\n');

    // [skills]
    s.push_str("# Built-in core skills such as `scheduler` are injected automatically.\n");
    s.push_str("# Use this directory only for extra user/project skills.\n");
    s.push_str("[skills]\n");
    s.push_str(&format!(
        "dir = {:?}\n",
        qdir.join("skills").to_string_lossy().as_ref()
    ));
    s.push('\n');

    // [scheduler]
    s.push_str("[scheduler]\n");
    s.push_str("enabled = true\n");
    s.push_str("poll_secs = 15\n");
    s.push_str("max_concurrent = 4\n");
    s.push_str("max_fetch_per_tick = 64\n");
    s.push_str("default_timezone = \"UTC\"\n");
    s.push('\n');

    // channels
    match input.channel {
        ChannelConfig::Lark(l) => {
            s.push_str("[channels.lark]\n");
            s.push_str("enabled = true\n");
            s.push('\n');
            s.push_str("[[channels.lark.instances]]\n");
            s.push_str("id = \"default\"\n");
            s.push_str(&format!("app_id = {:?}\n", l.app_id));
            s.push_str(&format!("app_secret = {:?}\n", l.app_secret));
            if let Some(bn) = &l.bot_name {
                s.push_str(&format!("bot_name = {:?}\n", bn));
            }
            s.push('\n');
        }
        ChannelConfig::DingTalk(d) => match d.receive_mode {
            DingTalkReceiveMode::Stream => {
                s.push_str("[channels.dingtalk]\n");
                s.push_str("enabled = true\n");
                if let Some(aid) = d.agent_id {
                    s.push_str(&format!("agent_id = {}\n", aid));
                }
                if let Some(bn) = &d.bot_name {
                    s.push_str(&format!("bot_name = {:?}\n", bn));
                }
                s.push('\n');
            }
            DingTalkReceiveMode::Webhook => {
                s.push_str("[channels.dingtalk_webhook]\n");
                s.push_str("enabled = true\n");
                s.push_str(&format!(
                    "secret_key = {:?}\n",
                    d.webhook_secret_key.as_deref().unwrap_or_default()
                ));
                s.push_str(&format!(
                    "webhook_path = {:?}\n",
                    d.webhook_path
                        .as_deref()
                        .unwrap_or("/channels/dingtalk/webhook")
                ));
                if let Some(access_token) = d
                    .webhook_access_token
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                {
                    s.push_str(&format!("access_token = {:?}\n", access_token));
                }
                s.push('\n');
            }
        },
        ChannelConfig::None => {}
    }

    s
}

pub fn build_env_content(provider: &ProviderConfig, channel: &ChannelConfig) -> String {
    let mut lines = Vec::<String>::new();
    let env_var = provider.kind.env_var();
    if !env_var.is_empty() && !provider.api_key.is_empty() {
        lines.push(format!("export {}={}", env_var, provider.api_key));
    }
    match channel {
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

pub fn write_config(input: &WriteInputs) -> Result<Option<PathBuf>> {
    let path = config_path();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let backup = if path.exists() {
        let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
        let bak = path.with_extension(format!("toml.bak.{ts}"));
        std::fs::copy(&path, &bak).context("backup failed")?;
        Some(bak)
    } else {
        None
    };
    std::fs::write(&path, build_config_toml(input)).context("write config.toml")?;
    Ok(backup)
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
    dirs::home_dir()
        .unwrap_or_default()
        .join(".clawbro")
        .join("config.toml")
}

pub fn env_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".clawbro")
        .join(".env")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::setup::{
        auth_cfg::AuthConfig,
        channel::{ChannelConfig, DingTalkCfg, DingTalkReceiveMode, LarkCfg},
        mode::{Mode, ModeConfig},
        provider::{ProviderConfig, ProviderKind},
    };

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

    #[test]
    fn toml_has_gateway() {
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(),
            mode: &solo(),
            auth: &no_auth(),
            channel: &ChannelConfig::None,
        });
        assert!(t.contains("[gateway]"), "missing [gateway]: {t}");
        assert!(t.contains("port = 8080"), "missing port: {t}");
    }

    #[test]
    fn toml_has_provider_profile() {
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(),
            mode: &solo(),
            auth: &no_auth(),
            channel: &ChannelConfig::None,
        });
        assert!(
            t.contains("[[provider_profile]]"),
            "missing [[provider_profile]]: {t}"
        );
        assert!(t.contains("anthropic_compatible"), "missing protocol: {t}");
        assert!(t.contains("anthropic-main"), "missing profile id: {t}");
    }

    #[test]
    fn toml_auth_token_written() {
        let auth = AuthConfig {
            ws_token: Some("my-secret".into()),
        };
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(),
            mode: &solo(),
            auth: &auth,
            channel: &ChannelConfig::None,
        });
        assert!(t.contains("[auth]"), "missing [auth]: {t}");
        assert!(t.contains("my-secret"), "missing token: {t}");
    }

    #[test]
    fn toml_no_auth_section_when_no_token() {
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(),
            mode: &solo(),
            auth: &no_auth(),
            channel: &ChannelConfig::None,
        });
        assert!(!t.contains("[auth]"), "should not have [auth]: {t}");
    }

    #[test]
    fn toml_lark_channel() {
        let lark = ChannelConfig::Lark(LarkCfg {
            app_id: "cli_abc".into(),
            app_secret: "sec".into(),
            bot_name: Some("AI".into()),
        });
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(),
            mode: &solo(),
            auth: &no_auth(),
            channel: &lark,
        });
        assert!(t.contains("[channels.lark]"), "missing lark: {t}");
        assert!(t.contains("cli_abc"), "missing app_id: {t}");
    }

    #[test]
    fn toml_dingtalk_agent_id() {
        let dt = ChannelConfig::DingTalk(DingTalkCfg {
            receive_mode: DingTalkReceiveMode::Stream,
            client_id: Some("dingxxxx".into()),
            client_secret: Some("sec".into()),
            agent_id: Some(12345),
            bot_name: None,
            webhook_secret_key: None,
            webhook_access_token: None,
            webhook_path: None,
        });
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(),
            mode: &solo(),
            auth: &no_auth(),
            channel: &dt,
        });
        assert!(t.contains("[channels.dingtalk]"), "missing dingtalk: {t}");
        assert!(t.contains("agent_id = 12345"), "missing agent_id: {t}");
    }

    #[test]
    fn toml_dingtalk_webhook_channel() {
        let dt = ChannelConfig::DingTalk(DingTalkCfg {
            receive_mode: DingTalkReceiveMode::Webhook,
            client_id: None,
            client_secret: None,
            agent_id: None,
            bot_name: None,
            webhook_secret_key: Some("SEC-test".into()),
            webhook_access_token: Some("dt-token".into()),
            webhook_path: Some("/dingtalk-channel/message".into()),
        });
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(),
            mode: &solo(),
            auth: &no_auth(),
            channel: &dt,
        });
        assert!(
            t.contains("[channels.dingtalk_webhook]"),
            "missing webhook: {t}"
        );
        assert!(
            t.contains("secret_key = \"SEC-test\""),
            "missing secret_key: {t}"
        );
        assert!(
            t.contains("webhook_path = \"/dingtalk-channel/message\""),
            "missing webhook_path: {t}"
        );
        assert!(
            t.contains("access_token = \"dt-token\""),
            "missing access_token: {t}"
        );
    }

    #[test]
    fn env_anthropic_key() {
        let e = build_env_content(&anthropic(), &ChannelConfig::None);
        assert!(
            e.contains("ANTHROPIC_API_KEY=sk-ant-test"),
            "missing key: {e}"
        );
    }

    #[test]
    fn env_lark_credentials() {
        let lark = ChannelConfig::Lark(LarkCfg {
            app_id: "cli_abc".into(),
            app_secret: "sec".into(),
            bot_name: None,
        });
        let e = build_env_content(&anthropic(), &lark);
        assert!(e.contains("LARK_APP_ID=cli_abc"), "missing lark id: {e}");
        assert!(e.contains("LARK_APP_SECRET=sec"), "missing secret: {e}");
        assert!(
            !e.contains("LARK_VERIFICATION_TOKEN"),
            "should not write verification token: {e}"
        );
    }

    #[test]
    fn env_dingtalk_webhook_writes_no_stream_credentials() {
        let dt = ChannelConfig::DingTalk(DingTalkCfg {
            receive_mode: DingTalkReceiveMode::Webhook,
            client_id: None,
            client_secret: None,
            agent_id: None,
            bot_name: None,
            webhook_secret_key: Some("SEC-test".into()),
            webhook_access_token: Some("dt-token".into()),
            webhook_path: None,
        });
        let e = build_env_content(&anthropic(), &dt);
        assert!(
            !e.contains("DINGTALK_APP_KEY"),
            "webhook should not write stream env: {e}"
        );
        assert!(
            !e.contains("DINGTALK_APP_SECRET"),
            "webhook should not write stream secret env: {e}"
        );
    }

    #[test]
    fn env_ollama_no_export() {
        let ollama = ProviderConfig {
            kind: ProviderKind::Ollama,
            api_key: String::new(),
            base_url: "http://localhost:11434".into(),
            model: "llama3".into(),
            profile_id: "ollama-main".into(),
        };
        let e = build_env_content(&ollama, &ChannelConfig::None);
        assert!(e.is_empty(), "ollama should produce empty env: {:?}", e);
    }

    #[test]
    fn multi_mode_has_no_agent_section() {
        let multi_mode = ModeConfig {
            mode: Mode::Multi,
            team_target: None,
            front_bot: None,
            specialists: Vec::new(),
            team_scope: None,
            team_name: None,
            port: 8080,
            workspace: None,
        };
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(),
            mode: &multi_mode,
            auth: &no_auth(),
            channel: &ChannelConfig::None,
        });
        assert!(
            !t.contains("\n[agent]\n"),
            "multi mode should not have [agent] section: {t}"
        );
        assert!(
            t.contains("agent_roster"),
            "should have roster comment: {t}"
        );
    }

    #[test]
    fn team_dm_mode_writes_valid_team_scope_skeleton() {
        let team_mode = ModeConfig {
            mode: Mode::Team,
            team_target: Some(TeamTarget::DirectMessage),
            front_bot: Some("planner".into()),
            specialists: vec!["coder".into(), "reviewer".into()],
            team_scope: Some("user:ou_demo".into()),
            team_name: Some("my-team".into()),
            port: 8080,
            workspace: None,
        };
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(),
            mode: &team_mode,
            auth: &no_auth(),
            channel: &ChannelConfig::None,
        });
        assert!(t.contains("[[agent_roster]]"), "missing roster: {t}");
        assert!(t.contains("name = \"planner\""), "missing lead: {t}");
        assert!(t.contains("name = \"coder\""), "missing coder: {t}");
        assert!(t.contains("name = \"reviewer\""), "missing reviewer: {t}");
        assert!(t.contains("[[team_scope]]"), "missing team_scope: {t}");
        assert!(
            t.contains("scope = \"user:ou_demo\""),
            "missing team scope: {t}"
        );
        assert!(t.contains("name = \"my-team\""), "missing team name: {t}");
        assert!(
            t.contains("interaction = \"team\""),
            "missing team interaction: {t}"
        );
        assert!(
            t.contains("front_bot = \"planner\""),
            "missing front bot: {t}"
        );
        assert!(
            t.contains("roster = [\"coder\", \"reviewer\"]"),
            "missing specialist roster: {t}"
        );
    }

    #[test]
    fn team_group_mode_writes_valid_group_skeleton() {
        let team_mode = ModeConfig {
            mode: Mode::Team,
            team_target: Some(TeamTarget::Group),
            front_bot: Some("captain".into()),
            specialists: vec!["analyst".into()],
            team_scope: Some("group:lark:chat-123".into()),
            team_name: Some("research-room".into()),
            port: 8080,
            workspace: None,
        };
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(),
            mode: &team_mode,
            auth: &no_auth(),
            channel: &ChannelConfig::Lark(LarkCfg {
                app_id: "cli_abc".into(),
                app_secret: "sec".into(),
                bot_name: Some("AI".into()),
            }),
        });
        assert!(t.contains("[[group]]"), "missing group: {t}");
        assert!(
            t.contains("scope = \"group:lark:chat-123\""),
            "missing group scope: {t}"
        );
        assert!(
            t.contains("name = \"research-room\""),
            "missing group name: {t}"
        );
        assert!(t.contains("[group.mode]"), "missing group.mode: {t}");
        assert!(
            t.contains("interaction = \"team\""),
            "missing group interaction: {t}"
        );
        assert!(
            t.contains("front_bot = \"captain\""),
            "missing front bot: {t}"
        );
        assert!(t.contains("channel = \"lark\""), "missing channel: {t}");
        assert!(t.contains("[group.team]"), "missing group.team: {t}");
        assert!(
            t.contains("roster = [\"analyst\"]"),
            "missing analyst roster: {t}"
        );
    }

    #[test]
    fn team_group_mode_writes_dingtalk_webhook_channel_name() {
        let team_mode = ModeConfig {
            mode: Mode::Team,
            team_target: Some(TeamTarget::Group),
            front_bot: Some("captain".into()),
            specialists: vec!["analyst".into()],
            team_scope: Some("group:dingtalk:conversation-123".into()),
            team_name: Some("ops-room".into()),
            port: 8080,
            workspace: None,
        };
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(),
            mode: &team_mode,
            auth: &no_auth(),
            channel: &ChannelConfig::DingTalk(DingTalkCfg {
                receive_mode: DingTalkReceiveMode::Webhook,
                client_id: None,
                client_secret: None,
                agent_id: None,
                bot_name: None,
                webhook_secret_key: Some("SEC-test".into()),
                webhook_access_token: Some("dt-token".into()),
                webhook_path: Some("/channels/dingtalk/webhook".into()),
            }),
        });
        assert!(t.contains("[[group]]"), "missing group: {t}");
        assert!(
            t.contains("scope = \"group:dingtalk:conversation-123\""),
            "missing group scope: {t}"
        );
        assert!(
            t.contains("channel = \"dingtalk_webhook\""),
            "missing dingtalk_webhook channel name: {t}"
        );
    }

    #[test]
    fn toml_has_scheduler_section_for_new_users() {
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(),
            mode: &solo(),
            auth: &no_auth(),
            channel: &ChannelConfig::None,
        });
        assert!(t.contains("[scheduler]"), "missing [scheduler]: {t}");
        assert!(
            t.contains("enabled = true"),
            "missing scheduler enabled: {t}"
        );
        assert!(
            t.contains("default_timezone = \"UTC\""),
            "missing scheduler timezone default: {t}"
        );
    }

    #[test]
    fn toml_skills_section_explains_builtin_scheduler_skill() {
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(),
            mode: &solo(),
            auth: &no_auth(),
            channel: &ChannelConfig::None,
        });
        assert!(
            t.contains("Built-in core skills such as `scheduler` are injected automatically."),
            "missing builtin scheduler skill guidance: {t}"
        );
    }
}
