use crate::cli::{
    args::SetupArgs,
    i18n::{Language, Messages},
};
use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Input};

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub ws_token: Option<String>,
}

pub fn collect(args: &SetupArgs, lang: Language) -> Result<AuthConfig> {
    let m = Messages::for_lang(lang);
    let theme = ColorfulTheme::default();

    let ws_token = if let Some(t) = &args.ws_token {
        if t.is_empty() {
            None
        } else {
            Some(t.clone())
        }
    } else if args.non_interactive {
        None
    } else {
        println!("  {}", m.enter_ws_token_hint);
        let entered: String = Input::with_theme(&theme)
            .with_prompt(m.enter_ws_token)
            .allow_empty(true)
            .interact_text()?;
        if entered.trim().is_empty() {
            None
        } else {
            Some(entered.trim().to_string())
        }
    };

    Ok(AuthConfig { ws_token })
}
