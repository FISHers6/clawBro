use crate::cli::{
    args::{SetupArgs, SetupPresetArg},
    i18n::Language,
};
use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Select};

use super::{
    channel::{self, ChannelConfig},
    mode::{self, Mode, ModeConfig, TeamTarget},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupPreset {
    Custom,
    WechatSolo,
    WechatDmTeam,
    LarkGroupTeam,
}

impl SetupPreset {
    pub fn from_arg(arg: Option<SetupPresetArg>) -> Self {
        match arg {
            Some(SetupPresetArg::WechatSolo) => Self::WechatSolo,
            Some(SetupPresetArg::WechatDmTeam) => Self::WechatDmTeam,
            Some(SetupPresetArg::LarkGroupTeam) => Self::LarkGroupTeam,
            _ => Self::Custom,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Custom => "Custom",
            Self::WechatSolo => "WeChat Solo",
            Self::WechatDmTeam => "WeChat DM Team",
            Self::LarkGroupTeam => "Lark Group Team",
        }
    }
}

pub fn select(args: &SetupArgs) -> Result<SetupPreset> {
    if args.preset.is_some() {
        return Ok(SetupPreset::from_arg(args.preset));
    }
    if args.non_interactive {
        return Ok(SetupPreset::Custom);
    }

    let theme = ColorfulTheme::default();
    let idx = Select::with_theme(&theme)
        .with_prompt("Choose a setup preset")
        .items(&["Custom", "WeChat Solo", "WeChat DM Team", "Lark Group Team"])
        .default(0)
        .interact()?;
    Ok(match idx {
        1 => SetupPreset::WechatSolo,
        2 => SetupPreset::WechatDmTeam,
        3 => SetupPreset::LarkGroupTeam,
        _ => SetupPreset::Custom,
    })
}

pub fn collect_topology(
    preset: SetupPreset,
    args: &SetupArgs,
    lang: Language,
) -> Result<(ModeConfig, ChannelConfig)> {
    match preset {
        SetupPreset::Custom => {
            let mode_cfg = mode::collect(args, lang)?;
            let channel_cfg = if args.non_interactive {
                ChannelConfig::None
            } else {
                channel::collect(lang)?
            };
            Ok((mode_cfg, channel_cfg))
        }
        SetupPreset::WechatSolo => {
            let mode_cfg = mode::collect_with_defaults(args, lang, Mode::Solo, None, None, &[])?;
            let channel_cfg = if args.non_interactive {
                channel::default_wechat()
            } else {
                channel::collect_wechat(lang)?
            };
            Ok((mode_cfg, channel_cfg))
        }
        SetupPreset::WechatDmTeam => {
            let mode_cfg = mode::collect_with_defaults(
                args,
                lang,
                Mode::Team,
                Some(TeamTarget::DirectMessage),
                Some("lead"),
                &["specialist"],
            )?;
            let channel_cfg = if args.non_interactive {
                channel::default_wechat()
            } else {
                channel::collect_wechat(lang)?
            };
            Ok((mode_cfg, channel_cfg))
        }
        SetupPreset::LarkGroupTeam => {
            if args.non_interactive {
                anyhow::bail!(
                    "`--preset lark-group-team` requires interactive setup because Lark credentials must be entered"
                );
            }
            let mode_cfg = mode::collect_with_defaults(
                args,
                lang,
                Mode::Team,
                Some(TeamTarget::Group),
                Some("lead"),
                &["specialist"],
            )?;
            let channel_cfg = channel::collect_lark(lang)?;
            Ok((mode_cfg, channel_cfg))
        }
    }
}
