use crate::agent_core::roster::AgentEntry;
use crate::cli::args::{ConfigAgentAddArgs, ConfigAgentArgs, ConfigAgentCommands};
use crate::cli::config_keys::AgentKey;
use crate::cli::config_patch::ConfigPatch;
use crate::cli::config_report::persist_and_report;
use crate::cli::config_store::load_graph;
use anyhow::{Context, Result};
use console::style;

pub async fn run(args: ConfigAgentArgs) -> Result<()> {
    match args.command {
        ConfigAgentCommands::List => cmd_list(),
        ConfigAgentCommands::Show { name } => cmd_show(&name),
        ConfigAgentCommands::Remove { name } => cmd_remove(&name),
        ConfigAgentCommands::Add(args) => cmd_add(build_agent(args)),
    }
}

fn cmd_list() -> Result<()> {
    let graph = load_graph()?;
    if graph.agents.is_empty() {
        println!("{}", style("No agent_roster entries configured").yellow());
        return Ok(());
    }
    for agent in graph.agents.values() {
        println!("{} {}", style("•").cyan(), agent.name);
    }
    Ok(())
}

fn cmd_show(name: &str) -> Result<()> {
    let graph = load_graph()?;
    let agent = graph
        .agents
        .get(name)
        .with_context(|| format!("agent `{name}` not found"))?;
    let text = toml::to_string_pretty(agent).context("render agent")?;
    println!("{text}");
    Ok(())
}

fn cmd_add(agent: AgentEntry) -> Result<()> {
    let mut graph = load_graph()?;
    ConfigPatch::UpsertAgent(agent.clone()).apply(&mut graph);
    persist_and_report(&graph, "agent", &agent.name, "saved")
}

fn cmd_remove(name: &str) -> Result<()> {
    let mut graph = load_graph()?;
    if !graph.agents.contains_key(name) {
        anyhow::bail!("agent `{name}` not found");
    }
    ConfigPatch::RemoveAgent(AgentKey::new(name)).apply(&mut graph);
    persist_and_report(&graph, "agent", name, "removed")
}

fn build_agent(args: ConfigAgentAddArgs) -> AgentEntry {
    let mentions = if args.mentions.is_empty() {
        vec![format!("@{}", args.name)]
    } else {
        args.mentions
    };
    AgentEntry {
        name: args.name,
        mentions,
        backend_id: args.backend,
        persona_dir: args.persona_dir,
        workspace_dir: args.workspace_dir,
        extra_skills_dirs: args.extra_skills_dirs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_agent_defaults_mention_from_name() {
        let agent = build_agent(ConfigAgentAddArgs {
            name: "claw".to_string(),
            mentions: vec![],
            backend: "codex-main".to_string(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        });
        assert_eq!(agent.mentions, vec!["@claw".to_string()]);
    }
}
