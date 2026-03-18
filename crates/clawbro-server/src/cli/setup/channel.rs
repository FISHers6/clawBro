use crate::cli::i18n::{Language, Messages};
use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Input, Password, Select};

#[derive(Debug, Clone)]
pub enum ChannelConfig {
    None,
    Lark(LarkCfg),
    DingTalk(DingTalkCfg),
}

#[derive(Debug, Clone)]
pub struct LarkCfg {
    pub app_id: String,
    pub app_secret: String,
    pub verification_token: String,
    pub bot_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DingTalkCfg {
    pub client_id: String,
    pub client_secret: String,
    pub agent_id: Option<u64>,
    pub bot_name: Option<String>,
}

pub fn collect(lang: Language) -> Result<ChannelConfig> {
    let m = Messages::for_lang(lang);
    let theme = ColorfulTheme::default();
    let items = [m.channel_none, m.channel_lark, m.channel_dingtalk];
    let idx = Select::with_theme(&theme)
        .with_prompt(m.select_channel)
        .items(&items)
        .default(0)
        .interact()?;
    match idx {
        1 => collect_lark(m, &theme),
        2 => collect_dingtalk(m, &theme),
        _ => Ok(ChannelConfig::None),
    }
}

fn collect_lark(m: &Messages, theme: &ColorfulTheme) -> Result<ChannelConfig> {
    let app_id: String = Input::with_theme(theme)
        .with_prompt(m.enter_lark_app_id)
        .interact_text()?;
    let app_secret = Password::with_theme(theme)
        .with_prompt(m.enter_lark_app_secret)
        .interact()?;
    let verification_token: String = Input::with_theme(theme)
        .with_prompt(m.enter_lark_verify_token)
        .interact_text()?;
    let bot_raw: String = Input::with_theme(theme)
        .with_prompt(m.enter_lark_bot_name)
        .allow_empty(true)
        .interact_text()?;
    let bot_name = if bot_raw.trim().is_empty() {
        None
    } else {
        Some(bot_raw.trim().to_string())
    };
    Ok(ChannelConfig::Lark(LarkCfg {
        app_id: app_id.trim().to_string(),
        app_secret,
        verification_token: verification_token.trim().to_string(),
        bot_name,
    }))
}

fn collect_dingtalk(m: &Messages, theme: &ColorfulTheme) -> Result<ChannelConfig> {
    let client_id: String = Input::with_theme(theme)
        .with_prompt(m.enter_dingtalk_client_id)
        .interact_text()?;
    let client_secret = Password::with_theme(theme)
        .with_prompt(m.enter_dingtalk_client_secret)
        .interact()?;
    let agent_raw: String = Input::with_theme(theme)
        .with_prompt(m.enter_dingtalk_agent_id)
        .allow_empty(true)
        .interact_text()?;
    let agent_id = agent_raw.trim().parse::<u64>().ok();
    let bot_raw: String = Input::with_theme(theme)
        .with_prompt(m.enter_dingtalk_bot_name)
        .allow_empty(true)
        .interact_text()?;
    let bot_name = if bot_raw.trim().is_empty() {
        None
    } else {
        Some(bot_raw.trim().to_string())
    };
    Ok(ChannelConfig::DingTalk(DingTalkCfg {
        client_id: client_id.trim().to_string(),
        client_secret,
        agent_id,
        bot_name,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lark_cfg_fields() {
        let cfg = LarkCfg {
            app_id: "cli_abc".into(),
            app_secret: "sec".into(),
            verification_token: "tok".into(),
            bot_name: Some("AI".into()),
        };
        assert_eq!(cfg.app_id, "cli_abc");
        assert_eq!(cfg.bot_name.as_deref(), Some("AI"));
    }

    #[test]
    fn dingtalk_optional_fields() {
        let cfg = DingTalkCfg {
            client_id: "dingxxxx".into(),
            client_secret: "sec".into(),
            agent_id: None,
            bot_name: None,
        };
        assert!(cfg.agent_id.is_none());
    }
}
