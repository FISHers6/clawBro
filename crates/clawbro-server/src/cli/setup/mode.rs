use crate::cli::{
    args::{ModeArg, SetupArgs, TeamTargetArg},
    i18n::{Language, Messages},
};
use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use std::collections::HashSet;
use std::path::PathBuf;

use super::channel::ChannelConfig;

#[derive(Debug, Clone)]
pub enum Mode {
    Solo,
    Multi,
    Team,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamTarget {
    DirectMessage,
    Group,
}

#[derive(Debug, Clone)]
pub struct ModeConfig {
    pub mode: Mode,
    pub team_target: Option<TeamTarget>,
    pub front_bot: Option<String>,
    pub specialists: Vec<String>,
    pub team_scope: Option<String>,
    pub team_name: Option<String>,
    pub port: u16,
    pub workspace: Option<PathBuf>,
}

fn normalize_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn collect_specialists(
    args: &SetupArgs,
    m: &Messages,
    theme: &ColorfulTheme,
    front_bot: &str,
) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut specialists = Vec::new();

    let mut push_candidate = |candidate: String| {
        if candidate == front_bot {
            println!("{}", m.specialist_name_conflict);
            return false;
        }
        if !seen.insert(candidate.clone()) {
            println!("{}", m.specialist_name_duplicate);
            return false;
        }
        specialists.push(candidate);
        true
    };

    if !args.specialist.is_empty() {
        for raw in &args.specialist {
            if let Some(candidate) = normalize_name(raw) {
                let _ = push_candidate(candidate);
            }
        }
    } else if args.non_interactive {
        let _ = push_candidate("specialist".to_string());
    } else {
        loop {
            let entered: String = Input::with_theme(theme)
                .with_prompt(m.enter_specialist_name)
                .allow_empty(true)
                .interact_text()?;
            let Some(candidate) = normalize_name(&entered) else {
                break;
            };
            let _ = push_candidate(candidate);
        }
    }

    if specialists.is_empty() {
        println!("{}", m.specialist_name_required);
        specialists.push("specialist".to_string());
    }

    Ok(specialists)
}

pub fn collect(args: &SetupArgs, lang: Language) -> Result<ModeConfig> {
    let m = Messages::for_lang(lang);
    let theme = ColorfulTheme::default();

    let mode = if let Some(a) = &args.mode {
        match a {
            ModeArg::Solo => Mode::Solo,
            ModeArg::Multi => Mode::Multi,
            ModeArg::Team => Mode::Team,
        }
    } else {
        let items = [m.mode_solo, m.mode_multi, m.mode_team];
        let idx = Select::with_theme(&theme)
            .with_prompt(m.select_mode)
            .items(&items)
            .default(0)
            .interact()?;
        match idx {
            1 => Mode::Multi,
            2 => Mode::Team,
            _ => Mode::Solo,
        }
    };

    let team_target = if matches!(mode, Mode::Team) {
        if let Some(target) = &args.team_target {
            Some(match target {
                TeamTargetArg::DirectMessage => TeamTarget::DirectMessage,
                TeamTargetArg::Group => TeamTarget::Group,
            })
        } else if args.non_interactive {
            Some(TeamTarget::DirectMessage)
        } else {
            let items = ["Direct Message team", "Group team"];
            let idx = Select::with_theme(&theme)
                .with_prompt("Choose Team target")
                .items(&items)
                .default(0)
                .interact()?;
            Some(match idx {
                1 => TeamTarget::Group,
                _ => TeamTarget::DirectMessage,
            })
        }
    } else {
        None
    };

    let front_bot = if matches!(mode, Mode::Team) {
        if let Some(name) = &args.front_bot {
            Some(name.trim().to_string())
        } else if args.non_interactive {
            Some("lead".to_string())
        } else {
            let entered: String = Input::with_theme(&theme)
                .with_prompt(m.enter_front_bot_name)
                .default("lead".into())
                .interact_text()?;
            let normalized = entered.trim();
            if normalized.is_empty() {
                Some("lead".to_string())
            } else {
                Some(normalized.to_string())
            }
        }
    } else {
        None
    };

    let specialists = if matches!(mode, Mode::Team) {
        collect_specialists(args, m, &theme, front_bot.as_deref().unwrap_or("lead"))?
    } else {
        Vec::new()
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

    Ok(ModeConfig {
        mode,
        team_target,
        front_bot,
        specialists,
        team_scope: None,
        team_name: None,
        port,
        workspace,
    })
}

fn team_scope_defaults(
    target: TeamTarget,
    channel: &ChannelConfig,
) -> (&'static str, &'static str) {
    match target {
        TeamTarget::DirectMessage => match channel {
            ChannelConfig::Lark(_) => ("user:ou_your_user_id", "my-team"),
            ChannelConfig::DingTalk(_) => ("user:ding_your_user_id", "my-team"),
            ChannelConfig::None => ("user:default", "my-team"),
        },
        TeamTarget::Group => match channel {
            ChannelConfig::Lark(_) => ("group:lark:chat-123", "group-team"),
            ChannelConfig::DingTalk(_) => ("group:dingtalk:conversation-123", "group-team"),
            ChannelConfig::None => ("group:default", "group-team"),
        },
    }
}

pub fn collect_team_scope_details(
    args: &SetupArgs,
    mode: &mut ModeConfig,
    channel: &ChannelConfig,
    lang: Language,
) -> Result<()> {
    if !matches!(mode.mode, Mode::Team) {
        return Ok(());
    }

    let target = mode.team_target.unwrap_or(TeamTarget::DirectMessage);
    let (default_scope, default_name) = team_scope_defaults(target, channel);

    if args.non_interactive {
        mode.team_scope = Some(
            args.team_scope
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(default_scope)
                .to_string(),
        );
        mode.team_name = Some(
            args.team_name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(default_name)
                .to_string(),
        );
        return Ok(());
    }

    let m = Messages::for_lang(lang);
    let theme = ColorfulTheme::default();

    let scope_input: String = Input::with_theme(&theme)
        .with_prompt(m.enter_team_scope)
        .default(
            args.team_scope
                .clone()
                .unwrap_or_else(|| default_scope.to_string()),
        )
        .interact_text()?;
    let normalized_scope = scope_input.trim();
    mode.team_scope = Some(if normalized_scope.is_empty() {
        default_scope.to_string()
    } else {
        normalized_scope.to_string()
    });

    let name_input: String = Input::with_theme(&theme)
        .with_prompt(m.enter_team_name)
        .default(
            args.team_name
                .clone()
                .unwrap_or_else(|| default_name.to_string()),
        )
        .interact_text()?;
    let normalized_name = name_input.trim();
    mode.team_name = Some(if normalized_name.is_empty() {
        default_name.to_string()
    } else {
        normalized_name.to_string()
    });

    Ok(())
}
