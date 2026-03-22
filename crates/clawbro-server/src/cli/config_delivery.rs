use crate::cli::args::{
    ConfigDeliveryPurposeArg, ConfigDeliverySenderAddArgs, ConfigDeliverySenderArgs,
    ConfigDeliverySenderCommands, ConfigDeliveryTargetAddArgs, ConfigDeliveryTargetArgs,
    ConfigDeliveryTargetCommands,
};
use crate::cli::config_keys::{DeliverySenderBindingKey, DeliveryTargetOverrideKey};
use crate::cli::config_patch::ConfigPatch;
use crate::cli::config_report::persist_and_report;
use crate::cli::config_store::load_graph;
use crate::config::{
    DeliveryPurposeConfig, DeliverySenderBindingConfig, DeliveryTargetOverrideConfig,
};
use anyhow::{Context, Result};
use console::style;

pub async fn run_sender(args: ConfigDeliverySenderArgs) -> Result<()> {
    match args.command {
        ConfigDeliverySenderCommands::List => cmd_list_sender(),
        ConfigDeliverySenderCommands::Show { id } => cmd_show_sender(&id),
        ConfigDeliverySenderCommands::Remove { id } => cmd_remove_sender(&id),
        ConfigDeliverySenderCommands::Add(args) => cmd_add_sender(build_sender(args)),
    }
}

pub async fn run_target(args: ConfigDeliveryTargetArgs) -> Result<()> {
    match args.command {
        ConfigDeliveryTargetCommands::List => cmd_list_target(),
        ConfigDeliveryTargetCommands::Show { id } => cmd_show_target(&id),
        ConfigDeliveryTargetCommands::Remove { id } => cmd_remove_target(&id),
        ConfigDeliveryTargetCommands::Add(args) => cmd_add_target(build_target(args)),
    }
}

fn cmd_list_sender() -> Result<()> {
    let graph = load_graph()?;
    if graph.delivery_sender_bindings.is_empty() {
        println!(
            "{}",
            style("No delivery_sender_binding entries configured").yellow()
        );
        return Ok(());
    }
    for (id, binding) in &graph.delivery_sender_bindings {
        println!(
            "{} {} -> {}",
            style("•").cyan(),
            id,
            binding.channel_instance
        );
    }
    Ok(())
}

fn cmd_show_sender(id: &str) -> Result<()> {
    let graph = load_graph()?;
    let binding = graph
        .delivery_sender_bindings
        .get(id)
        .with_context(|| format!("delivery_sender_binding `{id}` not found"))?;
    let text = toml::to_string_pretty(binding).context("render delivery_sender_binding")?;
    println!("{text}");
    Ok(())
}

fn cmd_add_sender(binding: DeliverySenderBindingConfig) -> Result<()> {
    let mut graph = load_graph()?;
    let key = DeliverySenderBindingKey::from_binding(&binding);
    ConfigPatch::UpsertDeliverySenderBinding {
        key: key.clone(),
        value: binding,
    }
    .apply(&mut graph);
    persist_and_report(&graph, "delivery_sender_binding", &key.to_string(), "saved")
}

fn cmd_remove_sender(id: &str) -> Result<()> {
    let mut graph = load_graph()?;
    if !graph.delivery_sender_bindings.contains_key(id) {
        anyhow::bail!("delivery_sender_binding `{id}` not found");
    }
    ConfigPatch::RemoveDeliverySenderBinding(DeliverySenderBindingKey::new(id)).apply(&mut graph);
    persist_and_report(&graph, "delivery_sender_binding", id, "removed")
}

fn cmd_list_target() -> Result<()> {
    let graph = load_graph()?;
    if graph.delivery_target_overrides.is_empty() {
        println!(
            "{}",
            style("No delivery_target_override entries configured").yellow()
        );
        return Ok(());
    }
    for (id, override_cfg) in &graph.delivery_target_overrides {
        println!("{} {} -> {}", style("•").cyan(), id, override_cfg.scope);
    }
    Ok(())
}

fn cmd_show_target(id: &str) -> Result<()> {
    let graph = load_graph()?;
    let override_cfg = graph
        .delivery_target_overrides
        .get(id)
        .with_context(|| format!("delivery_target_override `{id}` not found"))?;
    let text = toml::to_string_pretty(override_cfg).context("render delivery_target_override")?;
    println!("{text}");
    Ok(())
}

fn cmd_add_target(override_cfg: DeliveryTargetOverrideConfig) -> Result<()> {
    let mut graph = load_graph()?;
    let key = DeliveryTargetOverrideKey::from_binding(&override_cfg);
    ConfigPatch::UpsertDeliveryTargetOverride {
        key: key.clone(),
        value: override_cfg,
    }
    .apply(&mut graph);
    persist_and_report(
        &graph,
        "delivery_target_override",
        &key.to_string(),
        "saved",
    )
}

fn cmd_remove_target(id: &str) -> Result<()> {
    let mut graph = load_graph()?;
    if !graph.delivery_target_overrides.contains_key(id) {
        anyhow::bail!("delivery_target_override `{id}` not found");
    }
    ConfigPatch::RemoveDeliveryTargetOverride(DeliveryTargetOverrideKey::new(id)).apply(&mut graph);
    persist_and_report(&graph, "delivery_target_override", id, "removed")
}

fn build_sender(args: ConfigDeliverySenderAddArgs) -> DeliverySenderBindingConfig {
    DeliverySenderBindingConfig {
        purpose: parse_purpose(args.purpose),
        agent: args.agent,
        channel: args.channel,
        channel_instance: args.channel_instance,
    }
}

fn build_target(args: ConfigDeliveryTargetAddArgs) -> DeliveryTargetOverrideConfig {
    DeliveryTargetOverrideConfig {
        purpose: parse_purpose(args.purpose),
        agent: args.agent,
        channel: args.channel,
        channel_instance: args.channel_instance,
        scope: args.scope,
        reply_to: args.reply_to,
        thread_ts: args.thread_ts,
    }
}

fn parse_purpose(value: ConfigDeliveryPurposeArg) -> DeliveryPurposeConfig {
    match value {
        ConfigDeliveryPurposeArg::LeadFinal => DeliveryPurposeConfig::LeadFinal,
        ConfigDeliveryPurposeArg::LeadMessage => DeliveryPurposeConfig::LeadMessage,
        ConfigDeliveryPurposeArg::Milestone => DeliveryPurposeConfig::Milestone,
        ConfigDeliveryPurposeArg::Approval => DeliveryPurposeConfig::Approval,
        ConfigDeliveryPurposeArg::BotMention => DeliveryPurposeConfig::BotMention,
        ConfigDeliveryPurposeArg::Cron => DeliveryPurposeConfig::Cron,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_sender_keeps_channel_instance() {
        let binding = build_sender(ConfigDeliverySenderAddArgs {
            purpose: ConfigDeliveryPurposeArg::Milestone,
            agent: Some("claw".to_string()),
            channel: Some("wechat".to_string()),
            channel_instance: "default".to_string(),
        });
        assert_eq!(binding.channel_instance, "default");
        assert_eq!(binding.purpose, DeliveryPurposeConfig::Milestone);
    }

    #[test]
    fn build_target_keeps_scope_and_reply_to() {
        let override_cfg = build_target(ConfigDeliveryTargetAddArgs {
            purpose: ConfigDeliveryPurposeArg::LeadFinal,
            agent: Some("claw".to_string()),
            channel: Some("wechat".to_string()),
            channel_instance: Some("default".to_string()),
            scope: "user:abc".to_string(),
            reply_to: Some("msg-id".to_string()),
            thread_ts: None,
        });
        assert_eq!(override_cfg.scope, "user:abc");
        assert_eq!(override_cfg.reply_to.as_deref(), Some("msg-id"));
    }
}
