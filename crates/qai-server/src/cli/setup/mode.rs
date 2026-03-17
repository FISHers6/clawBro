use crate::cli::{
    args::{ModeArg, SetupArgs},
    i18n::{Language, Messages},
};
use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Mode { Solo, Multi, Team }

#[derive(Debug, Clone)]
pub struct ModeConfig {
    pub mode: Mode,
    pub port: u16,
    pub workspace: Option<PathBuf>,
}

pub fn collect(args: &SetupArgs, lang: Language) -> Result<ModeConfig> {
    let m = Messages::for_lang(lang);
    let theme = ColorfulTheme::default();

    let mode = if let Some(a) = &args.mode {
        match a {
            ModeArg::Solo  => Mode::Solo,
            ModeArg::Multi => Mode::Multi,
            ModeArg::Team  => Mode::Team,
        }
    } else {
        let items = [m.mode_solo, m.mode_multi, m.mode_team];
        let idx = Select::with_theme(&theme)
            .with_prompt(m.select_mode)
            .items(&items)
            .default(0)
            .interact()?;
        match idx { 1 => Mode::Multi, 2 => Mode::Team, _ => Mode::Solo }
    };

    let port: u16 = if args.non_interactive {
        8080
    } else {
        let port_str: String = Input::with_theme(&theme)
            .with_prompt(m.enter_port)
            .default("8080".into())
            .interact_text()?;
        port_str.trim().parse().unwrap_or(8080)
    };

    let workspace = if args.non_interactive {
        None
    } else {
        let ws_str: String = Input::with_theme(&theme)
            .with_prompt(m.enter_workspace)
            .allow_empty(true)
            .interact_text()?;
        if ws_str.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(ws_str.trim()))
        }
    };

    Ok(ModeConfig { mode, port, workspace })
}
