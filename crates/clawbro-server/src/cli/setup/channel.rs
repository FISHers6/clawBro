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
    pub bot_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DingTalkCfg {
    pub receive_mode: DingTalkReceiveMode,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub agent_id: Option<u64>,
    pub bot_name: Option<String>,
    pub webhook_secret_key: Option<String>,
    pub webhook_access_token: Option<String>,
    pub webhook_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DingTalkReceiveMode {
    Stream,
    Webhook,
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
        bot_name,
    }))
}

fn collect_dingtalk(m: &Messages, theme: &ColorfulTheme) -> Result<ChannelConfig> {
    let mode_idx = Select::with_theme(theme)
        .with_prompt(m.select_dingtalk_mode)
        .items(&[m.dingtalk_mode_stream, m.dingtalk_mode_webhook])
        .default(0)
        .interact()?;
    match mode_idx {
        1 => collect_dingtalk_webhook(m, theme),
        _ => collect_dingtalk_stream(m, theme),
    }
}

fn collect_dingtalk_stream(m: &Messages, theme: &ColorfulTheme) -> Result<ChannelConfig> {
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
        receive_mode: DingTalkReceiveMode::Stream,
        client_id: Some(client_id.trim().to_string()),
        client_secret: Some(client_secret),
        agent_id,
        bot_name,
        webhook_secret_key: None,
        webhook_access_token: None,
        webhook_path: None,
    }))
}

fn collect_dingtalk_webhook(m: &Messages, theme: &ColorfulTheme) -> Result<ChannelConfig> {
    let webhook_secret_key = Password::with_theme(theme)
        .with_prompt(m.enter_dingtalk_webhook_secret_key)
        .interact()?;
    let webhook_access_token = Password::with_theme(theme)
        .with_prompt(m.enter_dingtalk_webhook_access_token)
        .allow_empty_password(true)
        .interact()?;
    let webhook_path: String = Input::with_theme(theme)
        .with_prompt(m.enter_dingtalk_webhook_path)
        .allow_empty(true)
        .interact_text()?;
    Ok(ChannelConfig::DingTalk(DingTalkCfg {
        receive_mode: DingTalkReceiveMode::Webhook,
        client_id: None,
        client_secret: None,
        agent_id: None,
        bot_name: None,
        webhook_secret_key: Some(webhook_secret_key),
        webhook_access_token: (!webhook_access_token.trim().is_empty())
            .then(|| webhook_access_token.trim().to_string()),
        webhook_path: (!webhook_path.trim().is_empty()).then(|| webhook_path.trim().to_string()),
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
            bot_name: Some("AI".into()),
        };
        assert_eq!(cfg.app_id, "cli_abc");
        assert_eq!(cfg.bot_name.as_deref(), Some("AI"));
    }

    #[test]
    fn dingtalk_optional_fields() {
        let cfg = DingTalkCfg {
            receive_mode: DingTalkReceiveMode::Stream,
            client_id: Some("dingxxxx".into()),
            client_secret: Some("sec".into()),
            agent_id: None,
            bot_name: None,
            webhook_secret_key: None,
            webhook_access_token: None,
            webhook_path: None,
        };
        assert!(cfg.agent_id.is_none());
    }
}
