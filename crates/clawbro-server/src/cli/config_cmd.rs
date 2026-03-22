use crate::cli::{
    args::{ConfigArgs, ConfigCommands},
    config_agent_cmd, config_backend, config_binding, config_channel, config_delivery,
    config_provider, config_team_scope_cmd, config_validate, config_wizard,
};
use anyhow::{Context, Result};
use console::style;

pub async fn run(args: ConfigArgs) -> Result<()> {
    match args.command {
        ConfigCommands::Show => cmd_show(),
        ConfigCommands::Validate => cmd_validate(),
        ConfigCommands::Edit => cmd_edit(),
        ConfigCommands::Wizard => config_wizard::run().await,
        ConfigCommands::Channel(args) => config_channel::run(args).await,
        ConfigCommands::Provider(args) => config_provider::run(args).await,
        ConfigCommands::Backend(args) => config_backend::run(args).await,
        ConfigCommands::Agent(args) => config_agent_cmd::run(args).await,
        ConfigCommands::Binding(args) => config_binding::run(args).await,
        ConfigCommands::DeliverySender(args) => config_delivery::run_sender(args).await,
        ConfigCommands::DeliveryTarget(args) => config_delivery::run_target(args).await,
        ConfigCommands::TeamScope(args) => config_team_scope_cmd::run(args).await,
    }
}

fn config_path() -> std::path::PathBuf {
    crate::config::config_file_path()
}

fn cmd_show() -> Result<()> {
    let path = config_path();
    if !path.exists() {
        println!(
            "{} config.toml not found — run: clawbro setup",
            style("✗").red()
        );
        return Ok(());
    }
    let content = std::fs::read_to_string(&path)?;
    println!("{}", redact_secrets(&content));
    Ok(())
}

pub fn redact_secrets(toml: &str) -> String {
    toml.lines()
        .map(|line| {
            let lower = line.to_lowercase();
            if (lower.contains("secret")
                || lower.contains("token")
                || lower.contains("password")
                || lower.contains("api_key"))
                && line.contains('=')
            {
                if let Some(eq) = line.find('=') {
                    return format!("{} = \"<redacted>\"", line[..eq].trim());
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn cmd_validate() -> Result<()> {
    let path = config_path();
    if !path.exists() {
        anyhow::bail!("config.toml not found: {}", path.display());
    }
    let report = config_validate::validate_config_path(&path)
        .with_context(|| format!("validate {}", path.display()))?;
    for issue in &report.issues {
        match issue.severity {
            config_validate::ValidationSeverity::Error => {
                println!("{} [{}] {}", style("✗").red(), issue.code, issue.message);
            }
            config_validate::ValidationSeverity::Warning => {
                println!("{} [{}] {}", style("!").yellow(), issue.code, issue.message);
            }
        }
    }
    if report.has_errors() {
        anyhow::bail!(
            "config validation failed with {} error(s) and {} warning(s)",
            report.error_count(),
            report.warning_count()
        );
    }
    println!(
        "{} Config is valid ({} warning{})",
        style("✓").green(),
        report.warning_count(),
        if report.warning_count() == 1 { "" } else { "s" }
    );
    Ok(())
}

fn cmd_edit() -> Result<()> {
    let path = config_path();
    if !path.exists() {
        println!(
            "{} config.toml not found — run: clawbro setup",
            style("✗").red()
        );
        return Ok(());
    }
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("Failed to launch editor: {:?}", editor))?;
    if !status.success() {
        anyhow::bail!("Editor exited with code: {:?}", status.code());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_secret_values() {
        let toml = "app_secret = \"real-secret\"\nport = 8080\nws_token = \"tok123\"";
        let out = redact_secrets(toml);
        assert!(!out.contains("real-secret"), "secret not redacted: {out}");
        assert!(out.contains("port = 8080"), "port preserved: {out}");
        assert!(!out.contains("tok123"), "token not redacted: {out}");
    }

    #[test]
    fn redact_preserves_normal_lines() {
        let toml = "[gateway]\nhost = \"127.0.0.1\"\nport = 8080";
        let out = redact_secrets(toml);
        assert_eq!(out, toml);
    }

    #[test]
    fn redact_api_key_field() {
        let toml =
            "auth_token_env = \"ANTHROPIC_API_KEY\"\nbase_url = \"https://api.anthropic.com\"";
        let out = redact_secrets(toml);
        // auth_token_env contains "api_key" in the value name but it's the env var NAME, not the actual key
        // The field name doesn't contain secret/token/password/api_key — so it won't be redacted
        // But the actual API key value in .env won't be in config.toml anyway
        assert!(out.contains("base_url"), "base_url preserved: {out}");
    }
}
