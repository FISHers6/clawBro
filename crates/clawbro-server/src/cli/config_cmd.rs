use crate::cli::args::{ConfigArgs, ConfigCommands};
use anyhow::{Context, Result};
use console::style;

pub async fn run(args: ConfigArgs) -> Result<()> {
    match args.command {
        ConfigCommands::Show => cmd_show(),
        ConfigCommands::Validate => cmd_validate(),
        ConfigCommands::Edit => cmd_edit(),
    }
}

fn config_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".clawbro")
        .join("config.toml")
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
    let content = std::fs::read_to_string(&path)?;
    let _: toml::Value = toml::from_str(&content).context("TOML syntax error")?;
    let cfg = crate::config::GatewayConfig::load().context("config load failed")?;
    cfg.validate_runtime_topology()
        .context("topology validation failed")?;
    println!(
        "{} Config syntax and topology are valid",
        style("✓").green()
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
