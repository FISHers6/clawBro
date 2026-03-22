use crate::cli::config_model::ConfigGraph;
use crate::cli::config_store::persist_graph;
use anyhow::Result;
use console::style;

pub fn persist_and_report(graph: &ConfigGraph, noun: &str, id: &str, action: &str) -> Result<()> {
    let (path, backup) = persist_graph(graph)?;
    println!("{} {} `{}` {}", style("✓").green(), noun, id, action);
    println!("  config: {}", path.display());
    if let Some(backup) = backup {
        println!("  backup: {}", backup.display());
    }
    Ok(())
}
