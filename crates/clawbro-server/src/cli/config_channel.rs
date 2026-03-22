use crate::cli::args::{
    ConfigChannelArg, ConfigChannelArgs, ConfigChannelCommands, ConfigChannelPresentationArgs,
    ConfigChannelSetupSoloArgs, ConfigChannelSetupTeamArgs, ConfigPresentationArg,
};
use crate::cli::config_builders::{build_channel_binding, build_team_scope, parse_public_updates};
use crate::cli::config_model::ConfigGraph;
use crate::cli::config_patch::ConfigPatch;
use crate::cli::config_report::persist_and_report;
use crate::cli::config_store::load_graph;
use crate::config::ProgressPresentationMode;
use anyhow::Result;
use console::style;

pub async fn run(args: ConfigChannelArgs) -> Result<()> {
    match args.command {
        ConfigChannelCommands::Show { channel } => cmd_show(channel),
        ConfigChannelCommands::Enable { channel } => cmd_set_enabled(channel, true),
        ConfigChannelCommands::Disable { channel } => cmd_set_enabled(channel, false),
        ConfigChannelCommands::Login { channel } => cmd_login(channel).await,
        ConfigChannelCommands::SetPresentation(args) => cmd_set_presentation(args),
        ConfigChannelCommands::SetupSolo(args) => cmd_setup_solo(args),
        ConfigChannelCommands::SetupTeam(args) => cmd_setup_team(args),
    }
}

fn cmd_show(channel: ConfigChannelArg) -> Result<()> {
    let graph = load_graph()?;
    let name = channel.as_str();
    println!("{}", style(format!("Channel: {name}")).bold().cyan());
    println!("{}", render_channel_summary(&graph, name));
    Ok(())
}

fn cmd_set_enabled(channel: ConfigChannelArg, enabled: bool) -> Result<()> {
    let mut graph = load_graph()?;
    ConfigPatch::SetChannelEnabled {
        channel: channel.as_str().to_string(),
        enabled,
    }
    .apply(&mut graph);
    let state = if enabled { "enabled" } else { "disabled" };
    persist_and_report(&graph, "channel", channel.as_str(), state)
}

async fn cmd_login(channel: ConfigChannelArg) -> Result<()> {
    match channel {
        ConfigChannelArg::Wechat => {
            let path = clawbro_channels::wechat_login().await?;
            println!(
                "{} WeChat credentials saved to {}",
                style("✓").green(),
                path.display()
            );
            Ok(())
        }
        _ => anyhow::bail!(
            "channel `{}` does not support login flow yet",
            channel.as_str()
        ),
    }
}

fn cmd_set_presentation(args: ConfigChannelPresentationArgs) -> Result<()> {
    let mut graph = load_graph()?;
    let presentation = parse_presentation(args.presentation);
    match args.channel {
        ConfigChannelArg::Wechat => {
            ConfigPatch::SetWeChatPresentation(presentation).apply(&mut graph);
        }
        _ => anyhow::bail!(
            "channel `{}` does not support CLI presentation update yet",
            args.channel.as_str()
        ),
    }
    persist_and_report(
        &graph,
        "channel",
        args.channel.as_str(),
        &format!("presentation set to {:?}", presentation),
    )
}

fn cmd_setup_solo(args: ConfigChannelSetupSoloArgs) -> Result<()> {
    let mut graph = load_graph()?;
    ConfigPatch::SetChannelEnabled {
        channel: args.channel.as_str().to_string(),
        enabled: true,
    }
    .apply(&mut graph);
    let (key, binding) = build_channel_binding(args.agent, args.channel.as_str().to_string());
    ConfigPatch::UpsertBinding {
        key: key.clone(),
        value: binding,
    }
    .apply(&mut graph);
    persist_and_report(
        &graph,
        "channel",
        args.channel.as_str(),
        &format!("configured for solo via binding `{}`", key),
    )
}

fn cmd_setup_team(args: ConfigChannelSetupTeamArgs) -> Result<()> {
    let mut graph = load_graph()?;
    ConfigPatch::SetChannelEnabled {
        channel: args.channel.as_str().to_string(),
        enabled: true,
    }
    .apply(&mut graph);

    let public_updates = parse_public_updates(&args.public_updates)?;
    let (key, value) = build_team_scope(
        args.channel.as_str().to_string(),
        args.scope,
        args.name,
        args.front_bot,
        args.specialists,
        args.max_parallel,
        public_updates,
    )?;
    ConfigPatch::UpsertTeamScope {
        key: key.clone(),
        value,
    }
    .apply(&mut graph);
    persist_and_report(
        &graph,
        "channel",
        args.channel.as_str(),
        &format!("configured for team scope `{}`", key),
    )
}

fn render_channel_summary(graph: &ConfigGraph, channel: &str) -> String {
    match channel {
        "wechat" => match graph.channels.wechat.as_ref() {
            Some(section) => format!(
                "enabled = {}\npresentation = {:?}",
                section.enabled, section.presentation
            ),
            None => "not configured".to_string(),
        },
        "lark" => match graph.channels.lark.as_ref() {
            Some(section) => format!(
                "enabled = {}\npresentation = {:?}\ninstances = {}\ndefault_instance = {}",
                section.enabled,
                section.presentation,
                section.instances.len(),
                section.default_instance.as_deref().unwrap_or("default")
            ),
            None => "not configured".to_string(),
        },
        "dingtalk" => match graph.channels.dingtalk.as_ref() {
            Some(section) => format!(
                "enabled = {}\npresentation = {:?}",
                section.enabled, section.presentation
            ),
            None => "not configured".to_string(),
        },
        "dingtalk_webhook" => match graph.channels.dingtalk_webhook.as_ref() {
            Some(section) => format!(
                "enabled = {}\npresentation = {:?}\nwebhook_path = {}\nsecret_key = {}",
                section.enabled,
                section.presentation,
                section.webhook_path,
                if section.secret_key.trim().is_empty() {
                    "<empty>"
                } else {
                    "<configured>"
                }
            ),
            None => "not configured".to_string(),
        },
        _ => "unknown channel".to_string(),
    }
}

fn parse_presentation(value: ConfigPresentationArg) -> ProgressPresentationMode {
    match value {
        ConfigPresentationArg::FinalOnly => ProgressPresentationMode::FinalOnly,
        ConfigPresentationArg::ProgressCompact => ProgressPresentationMode::ProgressCompact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::team::milestone_delivery::TeamPublicUpdatesMode;

    #[test]
    fn render_channel_summary_for_missing_wechat_is_human_readable() {
        let graph = ConfigGraph::default();
        let summary = render_channel_summary(&graph, "wechat");
        assert_eq!(summary, "not configured");
    }

    #[test]
    fn render_channel_summary_for_enabled_wechat_includes_presentation() {
        let mut graph = ConfigGraph::default();
        ConfigPatch::SetChannelEnabled {
            channel: "wechat".to_string(),
            enabled: true,
        }
        .apply(&mut graph);
        let summary = render_channel_summary(&graph, "wechat");
        assert!(summary.contains("enabled = true"));
        assert!(summary.contains("presentation"));
    }

    #[test]
    fn parse_presentation_maps_cli_values() {
        assert_eq!(
            parse_presentation(ConfigPresentationArg::FinalOnly),
            ProgressPresentationMode::FinalOnly
        );
        assert_eq!(
            parse_presentation(ConfigPresentationArg::ProgressCompact),
            ProgressPresentationMode::ProgressCompact
        );
    }

    #[test]
    fn parse_public_updates_supports_minimal() {
        assert_eq!(
            parse_public_updates("minimal").unwrap(),
            TeamPublicUpdatesMode::Minimal
        );
    }
}
