use crate::cli::args::{ConfigTeamScopeAddArgs, ConfigTeamScopeArgs, ConfigTeamScopeCommands};
use crate::cli::config_builders::{
    build_team_scope as build_team_scope_value, parse_public_updates,
};
use crate::cli::config_keys::TeamScopeKey;
use crate::cli::config_patch::ConfigPatch;
use crate::cli::config_report::persist_and_report;
use crate::cli::config_store::load_graph;
use crate::config::TeamScopeConfig;
use anyhow::{Context, Result};
use console::style;

pub async fn run(args: ConfigTeamScopeArgs) -> Result<()> {
    match args.command {
        ConfigTeamScopeCommands::List => cmd_list(),
        ConfigTeamScopeCommands::Show { channel, scope } => cmd_show(&channel, &scope),
        ConfigTeamScopeCommands::Remove { channel, scope } => cmd_remove(&channel, &scope),
        ConfigTeamScopeCommands::Add(args) => cmd_add(build_team_scope_from_args(args)?),
    }
}

fn cmd_list() -> Result<()> {
    let graph = load_graph()?;
    if graph.team_scopes.is_empty() {
        println!("{}", style("No team_scope entries configured").yellow());
        return Ok(());
    }
    for key in graph.team_scopes.keys() {
        println!("{} {}", style("•").cyan(), key);
    }
    Ok(())
}

fn cmd_show(channel: &str, scope: &str) -> Result<()> {
    let graph = load_graph()?;
    let key = TeamScopeKey::new(channel, scope);
    let team_scope = graph
        .team_scopes
        .get(&key)
        .with_context(|| format!("team_scope `{}` not found", key))?;
    let text = toml::to_string_pretty(team_scope).context("render team_scope")?;
    println!("{text}");
    Ok(())
}

fn cmd_add((key, team_scope): (TeamScopeKey, TeamScopeConfig)) -> Result<()> {
    let mut graph = load_graph()?;
    ConfigPatch::UpsertTeamScope {
        key: key.clone(),
        value: team_scope,
    }
    .apply(&mut graph);
    persist_and_report(&graph, "team_scope", &key.to_string(), "saved")
}

fn cmd_remove(channel: &str, scope: &str) -> Result<()> {
    let mut graph = load_graph()?;
    let key = TeamScopeKey::new(channel, scope);
    if !graph.team_scopes.contains_key(&key) {
        anyhow::bail!("team_scope `{}` not found", key);
    }
    ConfigPatch::RemoveTeamScope(key.clone()).apply(&mut graph);
    persist_and_report(&graph, "team_scope", &key.to_string(), "removed")
}

fn build_team_scope_from_args(
    args: ConfigTeamScopeAddArgs,
) -> Result<(TeamScopeKey, TeamScopeConfig)> {
    let updates = parse_public_updates(&args.public_updates)?;
    build_team_scope_value(
        args.channel,
        args.scope,
        args.name,
        args.front_bot,
        args.specialists,
        args.max_parallel,
        updates,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_team_scope_keeps_channel_and_scope() {
        let (key, team_scope) = build_team_scope_from_args(ConfigTeamScopeAddArgs {
            channel: "wechat".to_string(),
            scope: "user:o9cq@im.wechat".to_string(),
            name: Some("demo".to_string()),
            front_bot: "claude".to_string(),
            specialists: vec!["claw".to_string()],
            max_parallel: 1,
            public_updates: "minimal".to_string(),
        })
        .unwrap();

        assert_eq!(key.to_string(), "wechat:user:o9cq@im.wechat");
        assert_eq!(team_scope.mode.channel.as_deref(), Some("wechat"));
    }
}
