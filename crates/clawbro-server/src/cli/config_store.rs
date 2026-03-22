use crate::cli::config_apply::apply_graph_to_path;
use crate::cli::config_model::ConfigGraph;
use crate::cli::config_validate::{validate_graph, ValidationReport, ValidationSeverity};
use crate::config::config_file_path;
use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn load_graph() -> Result<ConfigGraph> {
    let path = config_file_path();
    if !path.exists() {
        return Ok(ConfigGraph::default());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("read config {}", path.display()))?;
    ConfigGraph::from_toml_str(&content)
        .with_context(|| format!("parse config graph from {}", path.display()))
}

pub fn persist_graph(graph: &ConfigGraph) -> Result<(PathBuf, Option<PathBuf>)> {
    let report = validate_graph(graph);
    if report.has_errors() {
        anyhow::bail!(format_validation_report(&report));
    }
    let path = config_file_path();
    let backup = apply_graph_to_path(graph, &path)?;
    Ok((path, backup))
}

pub fn format_validation_report(report: &ValidationReport) -> String {
    report
        .issues
        .iter()
        .map(|issue| match issue.severity {
            ValidationSeverity::Error => format!("error[{}]: {}", issue.code, issue.message),
            ValidationSeverity::Warning => format!("warning[{}]: {}", issue.code, issue.message),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
