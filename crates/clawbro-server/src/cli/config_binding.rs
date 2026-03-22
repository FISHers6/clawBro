use crate::cli::args::{
    ConfigBindingAddChannelArgs, ConfigBindingAddChannelInstanceArgs, ConfigBindingAddDefaultArgs,
    ConfigBindingAddPeerArgs, ConfigBindingAddScopeArgs, ConfigBindingAddTeamArgs,
    ConfigBindingAddThreadArgs, ConfigBindingArgs, ConfigBindingCommands, ConfigBindingPeerKindArg,
};
use crate::cli::config_builders::build_channel_binding as build_channel_binding_value;
use crate::cli::config_keys::BindingKey;
use crate::cli::config_patch::ConfigPatch;
use crate::cli::config_report::persist_and_report;
use crate::cli::config_store::load_graph;
use crate::config::{BindingConfig, BindingPeerKindConfig};
use anyhow::{Context, Result};
use console::style;

pub async fn run(args: ConfigBindingArgs) -> Result<()> {
    match args.command {
        ConfigBindingCommands::List => cmd_list(),
        ConfigBindingCommands::Show { id } => cmd_show(&id),
        ConfigBindingCommands::Remove { id } => cmd_remove(&id),
        ConfigBindingCommands::AddThread(args) => cmd_add(build_thread_binding(args)),
        ConfigBindingCommands::AddScope(args) => cmd_add(build_scope_binding(args)),
        ConfigBindingCommands::AddPeer(args) => cmd_add(build_peer_binding(args)),
        ConfigBindingCommands::AddTeam(args) => cmd_add(build_team_binding(args)),
        ConfigBindingCommands::AddChannelInstance(args) => {
            cmd_add(build_channel_instance_binding(args))
        }
        ConfigBindingCommands::AddChannel(args) => cmd_add(build_channel_binding(args)),
        ConfigBindingCommands::AddDefault(args) => cmd_add(build_default_binding(args)),
    }
}

fn cmd_list() -> Result<()> {
    let graph = load_graph()?;
    if graph.bindings.is_empty() {
        println!("{}", style("No binding entries configured").yellow());
        return Ok(());
    }
    for (id, binding) in &graph.bindings {
        println!("{} {} -> {}", style("•").cyan(), id, binding.agent_name());
    }
    Ok(())
}

fn cmd_show(id: &str) -> Result<()> {
    let graph = load_graph()?;
    let binding = graph
        .bindings
        .get(id)
        .with_context(|| format!("binding `{id}` not found"))?;
    let text = toml::to_string_pretty(binding).context("render binding")?;
    println!("{text}");
    Ok(())
}

fn cmd_add(binding: BindingConfig) -> Result<()> {
    let mut graph = load_graph()?;
    let key = BindingKey::from_binding(&binding);
    ConfigPatch::UpsertBinding {
        key: key.clone(),
        value: binding,
    }
    .apply(&mut graph);
    persist_and_report(&graph, "binding", &key.to_string(), "saved")
}

fn cmd_remove(id: &str) -> Result<()> {
    let mut graph = load_graph()?;
    if !graph.bindings.contains_key(id) {
        anyhow::bail!("binding `{id}` not found");
    }
    ConfigPatch::RemoveBinding(BindingKey::new(id)).apply(&mut graph);
    persist_and_report(&graph, "binding", id, "removed")
}

fn build_thread_binding(args: ConfigBindingAddThreadArgs) -> BindingConfig {
    BindingConfig::Thread {
        agent: args.agent,
        scope: args.scope,
        thread_id: args.thread_id,
        channel: args.channel,
    }
}

fn build_scope_binding(args: ConfigBindingAddScopeArgs) -> BindingConfig {
    BindingConfig::Scope {
        agent: args.agent,
        scope: args.scope,
        channel: args.channel,
    }
}

fn build_peer_binding(args: ConfigBindingAddPeerArgs) -> BindingConfig {
    BindingConfig::Peer {
        agent: args.agent,
        peer_kind: match args.peer_kind {
            ConfigBindingPeerKindArg::User => BindingPeerKindConfig::User,
            ConfigBindingPeerKindArg::Group => BindingPeerKindConfig::Group,
        },
        peer_id: args.peer_id,
        channel: args.channel,
    }
}

fn build_team_binding(args: ConfigBindingAddTeamArgs) -> BindingConfig {
    BindingConfig::Team {
        agent: args.agent,
        team_id: args.team_id,
    }
}

fn build_channel_instance_binding(args: ConfigBindingAddChannelInstanceArgs) -> BindingConfig {
    BindingConfig::ChannelInstance {
        agent: args.agent,
        channel: args.channel,
        channel_instance: args.channel_instance,
    }
}

fn build_channel_binding(args: ConfigBindingAddChannelArgs) -> BindingConfig {
    build_channel_binding_value(args.agent, args.channel).1
}

fn build_default_binding(args: ConfigBindingAddDefaultArgs) -> BindingConfig {
    BindingConfig::Default { agent: args.agent }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_channel_binding_has_stable_key() {
        let binding = build_channel_binding(ConfigBindingAddChannelArgs {
            agent: "claw".to_string(),
            channel: "wechat".to_string(),
        });
        let key = BindingKey::from_binding(&binding);
        assert_eq!(key.to_string(), "channel:claw:wechat");
    }

    #[test]
    fn build_peer_binding_keeps_kind_and_id() {
        let binding = build_peer_binding(ConfigBindingAddPeerArgs {
            agent: "claw".to_string(),
            peer_kind: ConfigBindingPeerKindArg::Group,
            peer_id: "group-123".to_string(),
            channel: Some("wechat".to_string()),
        });
        match binding {
            BindingConfig::Peer {
                peer_kind, peer_id, ..
            } => {
                assert_eq!(peer_kind, BindingPeerKindConfig::Group);
                assert_eq!(peer_id, "group-123");
            }
            _ => panic!("expected peer binding"),
        }
    }
}
