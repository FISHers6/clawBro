use crate::cli::args::{
    ConfigProviderAnthropicAddArgs, ConfigProviderArgs, ConfigProviderCommands,
    ConfigProviderOpenaiAddArgs,
};
use crate::cli::config_keys::ProviderKey;
use crate::cli::config_patch::ConfigPatch;
use crate::cli::config_report::persist_and_report;
use crate::cli::config_store::load_graph;
use crate::config::{ProviderProfileConfig, ProviderProfileProtocolConfig};
use anyhow::{Context, Result};
use console::style;

pub async fn run(args: ConfigProviderArgs) -> Result<()> {
    match args.command {
        ConfigProviderCommands::List => cmd_list(),
        ConfigProviderCommands::Show { id } => cmd_show(&id),
        ConfigProviderCommands::Remove { id } => cmd_remove(&id),
        ConfigProviderCommands::AddOfficialSession { id } => cmd_add(ProviderProfileConfig {
            id,
            protocol: ProviderProfileProtocolConfig::OfficialSession,
        }),
        ConfigProviderCommands::AddAnthropicCompatible(args) => {
            cmd_add(build_anthropic_provider(args))
        }
        ConfigProviderCommands::AddOpenaiCompatible(args) => cmd_add(build_openai_provider(args)),
    }
}

fn cmd_list() -> Result<()> {
    let graph = load_graph()?;
    if graph.providers.is_empty() {
        println!(
            "{}",
            style("No provider_profile entries configured").yellow()
        );
        return Ok(());
    }
    for provider in graph.providers.values() {
        println!("{} {}", style("•").cyan(), provider.id);
    }
    Ok(())
}

fn cmd_show(id: &str) -> Result<()> {
    let graph = load_graph()?;
    let provider = graph
        .providers
        .get(id)
        .with_context(|| format!("provider_profile `{id}` not found"))?;
    let text = toml::to_string_pretty(provider).context("render provider_profile")?;
    println!("{text}");
    Ok(())
}

fn cmd_add(provider: ProviderProfileConfig) -> Result<()> {
    let mut graph = load_graph()?;
    ConfigPatch::UpsertProvider(provider.clone()).apply(&mut graph);
    persist_and_report(&graph, "provider_profile", &provider.id, "saved")
}

fn cmd_remove(id: &str) -> Result<()> {
    let mut graph = load_graph()?;
    if !graph.providers.contains_key(id) {
        anyhow::bail!("provider_profile `{id}` not found");
    }
    ConfigPatch::RemoveProvider(ProviderKey::new(id)).apply(&mut graph);
    persist_and_report(&graph, "provider_profile", id, "removed")
}

fn build_anthropic_provider(args: ConfigProviderAnthropicAddArgs) -> ProviderProfileConfig {
    ProviderProfileConfig {
        id: args.id,
        protocol: ProviderProfileProtocolConfig::AnthropicCompatible {
            base_url: args.base_url,
            auth_token_env: args.auth_env,
            default_model: args.default_model,
            small_fast_model: args.small_fast_model,
        },
    }
}

fn build_openai_provider(args: ConfigProviderOpenaiAddArgs) -> ProviderProfileConfig {
    ProviderProfileConfig {
        id: args.id,
        protocol: ProviderProfileProtocolConfig::OpenaiCompatible {
            base_url: args.base_url,
            auth_token_env: args.auth_env,
            default_model: args.default_model,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_openai_provider_keeps_fields() {
        let provider = build_openai_provider(ConfigProviderOpenaiAddArgs {
            id: "openai-main".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            auth_env: "OPENAI_API_KEY".to_string(),
            default_model: "gpt-5".to_string(),
        });
        match provider.protocol {
            ProviderProfileProtocolConfig::OpenaiCompatible {
                base_url,
                auth_token_env,
                default_model,
            } => {
                assert_eq!(base_url, "https://api.openai.com/v1");
                assert_eq!(auth_token_env, "OPENAI_API_KEY");
                assert_eq!(default_model, "gpt-5");
            }
            _ => panic!("expected openai_compatible"),
        }
    }
}
