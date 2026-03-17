use super::{
    auth_cfg::AuthConfig,
    channel::ChannelConfig,
    mode::{Mode, ModeConfig},
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

pub fn build_config_toml(input: &WriteInputs) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let qdir = home.join(".quickai");
    let mut s = String::new();

    // [gateway]
    s.push_str("[gateway]\n");
    s.push_str("host = \"127.0.0.1\"\n");
    s.push_str(&format!("port = {}\n", input.mode.port));
    let require_mention = !matches!(input.mode.mode, Mode::Solo);
    s.push_str(&format!("require_mention_in_groups = {}\n", require_mention));
    if let Some(ws) = &input.mode.workspace {
        s.push_str(&format!("default_workspace = {:?}\n", ws.to_string_lossy().as_ref()));
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
    s.push_str(&format!("protocol = {:?}\n", input.provider.kind.protocol_tag()));
    s.push_str(&format!("base_url = {:?}\n", input.provider.base_url));
    if !input.provider.kind.env_var().is_empty() {
        s.push_str(&format!("auth_token_env = {:?}\n", input.provider.kind.env_var()));
    }
    s.push_str(&format!("default_model = {:?}\n", input.provider.model));
    s.push('\n');

    // [[backend]]
    s.push_str("[[backend]]\n");
    s.push_str("id = \"native-main\"\n");
    s.push_str("family = \"quick_ai_native\"\n");
    s.push_str(&format!("provider_profile = {:?}\n", input.provider.profile_id));
    s.push('\n');
    s.push_str("[backend.launch]\n");
    s.push_str("type = \"bundled_command\"\n");
    s.push('\n');

    // [agent] or comments
    match input.mode.mode {
        Mode::Solo => {
            s.push_str("[agent]\n");
            s.push_str("backend_id = \"native-main\"\n");
            s.push('\n');
        }
        Mode::Multi | Mode::Team => {
            s.push_str("# Add [[agent_roster]] entries below to configure multiple agents\n");
            s.push_str("# Example:\n");
            s.push_str("# [[agent_roster]]\n");
            s.push_str("# name = \"claude\"\n");
            s.push_str("# mentions = [\"@claude\"]\n");
            s.push_str("# backend_id = \"native-main\"\n");
            s.push('\n');
        }
    }

    // [session]
    s.push_str("[session]\n");
    s.push_str(&format!("dir = {:?}\n", qdir.join("sessions").to_string_lossy().as_ref()));
    s.push('\n');

    // [memory]
    s.push_str("[memory]\n");
    s.push_str(&format!("shared_dir = {:?}\n", qdir.join("shared").to_string_lossy().as_ref()));
    s.push_str("distill_every_n = 20\n");
    s.push_str("distiller_binary = \"quickai-rust-agent\"\n");
    s.push('\n');

    // [skills]
    s.push_str("[skills]\n");
    s.push_str(&format!("dir = {:?}\n", qdir.join("skills").to_string_lossy().as_ref()));
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
        ChannelConfig::DingTalk(d) => {
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
            lines.push(format!("export LARK_VERIFICATION_TOKEN={}", l.verification_token));
        }
        ChannelConfig::DingTalk(d) => {
            lines.push(format!("export DINGTALK_APP_KEY={}", d.client_id));
            lines.push(format!("export DINGTALK_APP_SECRET={}", d.client_secret));
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
    dirs::home_dir().unwrap_or_default().join(".quickai").join("config.toml")
}

pub fn env_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".quickai").join(".env")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::setup::{
        auth_cfg::AuthConfig,
        channel::{ChannelConfig, DingTalkCfg, LarkCfg},
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
    fn solo() -> ModeConfig { ModeConfig { mode: Mode::Solo, port: 8080, workspace: None } }
    fn no_auth() -> AuthConfig { AuthConfig { ws_token: None } }

    #[test]
    fn toml_has_gateway() {
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(), mode: &solo(), auth: &no_auth(), channel: &ChannelConfig::None,
        });
        assert!(t.contains("[gateway]"), "missing [gateway]: {t}");
        assert!(t.contains("port = 8080"), "missing port: {t}");
    }

    #[test]
    fn toml_has_provider_profile() {
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(), mode: &solo(), auth: &no_auth(), channel: &ChannelConfig::None,
        });
        assert!(t.contains("[[provider_profile]]"), "missing [[provider_profile]]: {t}");
        assert!(t.contains("anthropic_compatible"), "missing protocol: {t}");
        assert!(t.contains("anthropic-main"), "missing profile id: {t}");
    }

    #[test]
    fn toml_auth_token_written() {
        let auth = AuthConfig { ws_token: Some("my-secret".into()) };
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(), mode: &solo(), auth: &auth, channel: &ChannelConfig::None,
        });
        assert!(t.contains("[auth]"), "missing [auth]: {t}");
        assert!(t.contains("my-secret"), "missing token: {t}");
    }

    #[test]
    fn toml_no_auth_section_when_no_token() {
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(), mode: &solo(), auth: &no_auth(), channel: &ChannelConfig::None,
        });
        assert!(!t.contains("[auth]"), "should not have [auth]: {t}");
    }

    #[test]
    fn toml_lark_channel() {
        let lark = ChannelConfig::Lark(LarkCfg {
            app_id: "cli_abc".into(),
            app_secret: "sec".into(),
            verification_token: "tok".into(),
            bot_name: Some("AI".into()),
        });
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(), mode: &solo(), auth: &no_auth(), channel: &lark,
        });
        assert!(t.contains("[channels.lark]"), "missing lark: {t}");
        assert!(t.contains("cli_abc"), "missing app_id: {t}");
    }

    #[test]
    fn toml_dingtalk_agent_id() {
        let dt = ChannelConfig::DingTalk(DingTalkCfg {
            client_id: "dingxxxx".into(),
            client_secret: "sec".into(),
            agent_id: Some(12345),
            bot_name: None,
        });
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(), mode: &solo(), auth: &no_auth(), channel: &dt,
        });
        assert!(t.contains("[channels.dingtalk]"), "missing dingtalk: {t}");
        assert!(t.contains("agent_id = 12345"), "missing agent_id: {t}");
    }

    #[test]
    fn env_anthropic_key() {
        let e = build_env_content(&anthropic(), &ChannelConfig::None);
        assert!(e.contains("ANTHROPIC_API_KEY=sk-ant-test"), "missing key: {e}");
    }

    #[test]
    fn env_lark_credentials() {
        let lark = ChannelConfig::Lark(LarkCfg {
            app_id: "cli_abc".into(),
            app_secret: "sec".into(),
            verification_token: "vtok".into(),
            bot_name: None,
        });
        let e = build_env_content(&anthropic(), &lark);
        assert!(e.contains("LARK_APP_ID=cli_abc"), "missing lark id: {e}");
        assert!(e.contains("LARK_VERIFICATION_TOKEN=vtok"), "missing token: {e}");
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
        let multi_mode = ModeConfig { mode: Mode::Multi, port: 8080, workspace: None };
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic(), mode: &multi_mode, auth: &no_auth(), channel: &ChannelConfig::None,
        });
        assert!(!t.contains("\n[agent]\n"), "multi mode should not have [agent] section: {t}");
        assert!(t.contains("agent_roster"), "should have roster comment: {t}");
    }
}
