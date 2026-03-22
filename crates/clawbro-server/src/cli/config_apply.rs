use crate::cli::config_model::ConfigGraph;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn default_config_path() -> PathBuf {
    crate::config::config_file_path()
}

pub fn default_env_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".clawbro")
        .join(".env")
}

pub fn render_graph_to_toml(graph: &ConfigGraph) -> Result<String> {
    toml::to_string_pretty(&graph.to_gateway_config()).context("render config graph to toml")
}

pub fn backup_existing_file(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let backup = path.with_extension(format!("toml.bak.{ts}"));
    std::fs::copy(path, &backup)
        .with_context(|| format!("backup existing config {}", path.display()))?;
    Ok(Some(backup))
}

pub fn apply_graph_to_path(graph: &ConfigGraph, path: &Path) -> Result<Option<PathBuf>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create directory {}", parent.display()))?;
    }
    let backup = backup_existing_file(path)?;
    let content = render_graph_to_toml(graph)?;
    std::fs::write(path, content).with_context(|| format!("write {}", path.display()))?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_graph_to_toml_contains_sections() {
        let graph = ConfigGraph::default();
        let text = render_graph_to_toml(&graph).unwrap();
        assert!(text.contains("[gateway]"));
        assert!(text.contains("[scheduler]"));
    }

    #[test]
    fn apply_graph_to_path_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let graph = ConfigGraph::default();
        let backup = apply_graph_to_path(&graph, &path).unwrap();
        assert!(backup.is_none());
        assert!(path.exists());
    }

    #[test]
    fn backup_existing_file_creates_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[gateway]\nport = 1\n").unwrap();
        let backup = backup_existing_file(&path).unwrap();
        assert!(backup.as_ref().is_some_and(|p| p.exists()));
    }
}
