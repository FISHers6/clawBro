use crate::cli::args::{ConfigBackendAddArgs, ConfigBackendArgs, ConfigBackendCommands};
use crate::cli::config_keys::BackendKey;
use crate::cli::config_patch::ConfigPatch;
use crate::cli::config_report::persist_and_report;
use crate::cli::config_store::load_graph;
use crate::config::{
    AcpBackendConfig, BackendApprovalConfig, BackendCatalogEntry, BackendFamilyConfig,
    BackendLaunchConfig,
};
use anyhow::{Context, Result};
use console::style;
use std::collections::BTreeMap;

pub async fn run(args: ConfigBackendArgs) -> Result<()> {
    match args.command {
        ConfigBackendCommands::List => cmd_list(),
        ConfigBackendCommands::Show { id } => cmd_show(&id),
        ConfigBackendCommands::Remove { id } => cmd_remove(&id),
        ConfigBackendCommands::Add(args) => cmd_add(build_backend(args)?),
    }
}

fn cmd_list() -> Result<()> {
    let graph = load_graph()?;
    if graph.backends.is_empty() {
        println!("{}", style("No backend entries configured").yellow());
        return Ok(());
    }
    for backend in graph.backends.values() {
        println!("{} {}", style("•").cyan(), backend.id);
    }
    Ok(())
}

fn cmd_show(id: &str) -> Result<()> {
    let graph = load_graph()?;
    let backend = graph
        .backends
        .get(id)
        .with_context(|| format!("backend `{id}` not found"))?;
    let text = toml::to_string_pretty(backend).context("render backend")?;
    println!("{text}");
    Ok(())
}

fn cmd_add(backend: BackendCatalogEntry) -> Result<()> {
    let mut graph = load_graph()?;
    ConfigPatch::UpsertBackend(backend.clone()).apply(&mut graph);
    persist_and_report(&graph, "backend", &backend.id, "saved")
}

fn cmd_remove(id: &str) -> Result<()> {
    let mut graph = load_graph()?;
    if !graph.backends.contains_key(id) {
        anyhow::bail!("backend `{id}` not found");
    }
    ConfigPatch::RemoveBackend(BackendKey::new(id)).apply(&mut graph);
    persist_and_report(&graph, "backend", id, "removed")
}

fn build_backend(args: ConfigBackendAddArgs) -> Result<BackendCatalogEntry> {
    let family = parse_backend_family(&args.family)?;
    let acp_backend = match args.acp_backend.as_deref() {
        Some(value) => Some(parse_acp_backend(value)?),
        None => None,
    };
    let launch = parse_launch(&args.launch, args.command, args.args, args.env)?;
    Ok(BackendCatalogEntry {
        id: args.id,
        family,
        adapter_key: None,
        acp_backend,
        acp_auth_method: None,
        codex: None,
        provider_profile: args.provider,
        approval: BackendApprovalConfig::default(),
        external_mcp_servers: vec![],
        launch,
    })
}

fn parse_backend_family(value: &str) -> Result<BackendFamilyConfig> {
    match value.trim().to_lowercase().as_str() {
        "acp" => Ok(BackendFamilyConfig::Acp),
        "openclaw" | "openclaw_gateway" => Ok(BackendFamilyConfig::OpenClawGateway),
        "native" | "clawbro_native" => Ok(BackendFamilyConfig::ClawBroNative),
        _ => anyhow::bail!("unsupported backend family `{value}`"),
    }
}

fn parse_acp_backend(value: &str) -> Result<AcpBackendConfig> {
    match value.trim().to_lowercase().as_str() {
        "claude" => Ok(AcpBackendConfig::Claude),
        "codex" => Ok(AcpBackendConfig::Codex),
        "codebuddy" => Ok(AcpBackendConfig::Codebuddy),
        "qwen" => Ok(AcpBackendConfig::Qwen),
        "iflow" => Ok(AcpBackendConfig::Iflow),
        "goose" => Ok(AcpBackendConfig::Goose),
        "kimi" => Ok(AcpBackendConfig::Kimi),
        "opencode" => Ok(AcpBackendConfig::Opencode),
        "qoder" => Ok(AcpBackendConfig::Qoder),
        "vibe" => Ok(AcpBackendConfig::Vibe),
        "gemini" => Ok(AcpBackendConfig::Gemini),
        "custom" => Ok(AcpBackendConfig::Custom),
        _ => anyhow::bail!("unsupported acp backend `{value}`"),
    }
}

fn parse_launch(
    launch: &str,
    command: Option<String>,
    args: Vec<String>,
    env: Vec<String>,
) -> Result<BackendLaunchConfig> {
    match launch.trim().to_lowercase().as_str() {
        "bundled" => Ok(BackendLaunchConfig::BundledCommand),
        "external" => {
            let command = command.context("external launch requires --command")?;
            let env = parse_env_pairs(env)?;
            Ok(BackendLaunchConfig::ExternalCommand { command, args, env })
        }
        _ => anyhow::bail!("unsupported launch type `{launch}`"),
    }
}

fn parse_env_pairs(values: Vec<String>) -> Result<BTreeMap<String, String>> {
    let mut env_map = BTreeMap::new();
    for pair in values {
        let (key, value) = pair
            .split_once('=')
            .with_context(|| format!("invalid --env `{pair}`, expected KEY=VALUE"))?;
        env_map.insert(key.to_string(), value.to_string());
    }
    Ok(env_map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_backend_supports_external_launch() {
        let backend = build_backend(ConfigBackendAddArgs {
            id: "claude-main".to_string(),
            family: "acp".to_string(),
            acp_backend: Some("claude".to_string()),
            provider: Some("anthropic-main".to_string()),
            launch: "external".to_string(),
            command: Some("npx".to_string()),
            args: vec!["--yes".to_string()],
            env: vec!["FOO=bar".to_string()],
        })
        .unwrap();

        match backend.launch {
            BackendLaunchConfig::ExternalCommand { command, env, .. } => {
                assert_eq!(command, "npx");
                assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
            }
            _ => panic!("expected external command launch"),
        }
    }
}
