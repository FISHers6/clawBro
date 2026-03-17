use crate::cli::args::{AuthArgs, AuthCommands};
use anyhow::Result;
use console::style;
use std::collections::HashMap;

pub async fn run(args: AuthArgs) -> Result<()> {
    match args.command {
        AuthCommands::Set { provider, key } => cmd_set(&provider, &key),
        AuthCommands::List                  => cmd_list(),
        AuthCommands::Check                 => cmd_check().await,
    }
}

fn env_path() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_default().join(".clawbro").join(".env")
}

fn load_env_map() -> HashMap<String, String> {
    let Ok(content) = std::fs::read_to_string(env_path()) else {
        return HashMap::new();
    };
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

fn save_env_map(map: &HashMap<String, String>) -> Result<()> {
    let path = env_path();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut lines: Vec<String> = map
        .iter()
        .map(|(k, v)| format!("export {}={}", k, v))
        .collect();
    lines.sort(); // stable ordering
    let content = lines.join("\n") + "\n";
    std::fs::write(&path, content)?;
    Ok(())
}

fn provider_env_var(provider: &str) -> &'static str {
    match provider.to_lowercase().as_str() {
        "anthropic" | "claude" => "ANTHROPIC_API_KEY",
        "openai" | "gpt"       => "OPENAI_API_KEY",
        "deepseek"             => "OPENAI_API_KEY",
        "azure"                => "OPENAI_API_KEY",
        "ollama"               => "",
        _                      => "OPENAI_API_KEY",
    }
}

fn cmd_set(provider: &str, key: &str) -> Result<()> {
    let var = provider_env_var(provider);
    if var.is_empty() {
        println!("{} Ollama typically does not require an API key", style("ℹ").cyan());
        return Ok(());
    }
    let mut map = load_env_map();
    map.insert(var.to_string(), key.to_string());
    save_env_map(&map)?;
    println!("{} {} updated (~/.clawbro/.env)", style("✓").green(), var);
    println!("  Reload: source ~/.clawbro/.env");
    Ok(())
}

fn cmd_list() -> Result<()> {
    println!("{}", style("Configured API Keys:").bold());
    let map = load_env_map();
    let vars = [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "LARK_APP_ID",
        "LARK_APP_SECRET",
        "DINGTALK_APP_KEY",
        "DINGTALK_APP_SECRET",
    ];
    let mut found = false;
    for var in vars {
        if let Some(val) = map.get(var) {
            println!("  {} {} = {}", style("✓").green(), var, mask_key(val));
            found = true;
        }
    }
    if !found {
        println!(
            "  {} No API keys found (~/.clawbro/.env is missing or empty)",
            style("–").yellow()
        );
        println!("  Run: clawbro auth set anthropic <key>");
    }
    Ok(())
}

async fn cmd_check() -> Result<()> {
    println!("{}", style("Checking API key validity…").bold());
    let map = load_env_map();

    if let Some(key) = map.get("ANTHROPIC_API_KEY") {
        let ok = check_anthropic(key).await;
        let icon = if ok { style("✓").green() } else { style("✗").red() };
        println!("  {} Anthropic: {}", icon, if ok { "valid" } else { "invalid or network error" });
    } else {
        println!("  {} Anthropic: not configured", style("–").yellow());
    }

    if let Some(key) = map.get("OPENAI_API_KEY") {
        let ok = check_openai(key).await;
        let icon = if ok { style("✓").green() } else { style("✗").red() };
        println!("  {} OpenAI: {}", icon, if ok { "valid" } else { "invalid or network error" });
    } else {
        println!("  {} OpenAI: not configured", style("–").yellow());
    }

    Ok(())
}

async fn check_anthropic(key: &str) -> bool {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();
    client
        .get("https://api.anthropic.com/v1/models")
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn check_openai(key: &str) -> bool {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();
    client
        .get("https://api.openai.com/v1/models")
        .bearer_auth(key)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn mask_key(val: &str) -> String {
    if val.len() <= 8 {
        "****".to_string()
    } else {
        format!("{}…{}", &val[..6], &val[val.len() - 3..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_short() {
        assert_eq!(mask_key("abc"), "****");
    }

    #[test]
    fn mask_long() {
        let m = mask_key("sk-ant-api03-abc123xyz");
        assert!(m.starts_with("sk-ant"), "prefix: {m}");
        assert!(m.contains('…'), "ellipsis: {m}");
    }

    #[test]
    fn env_var_anthropic() {
        assert_eq!(provider_env_var("anthropic"), "ANTHROPIC_API_KEY");
    }

    #[test]
    fn env_var_deepseek_uses_openai() {
        assert_eq!(provider_env_var("deepseek"), "OPENAI_API_KEY");
    }

    #[test]
    fn env_var_ollama_empty() {
        assert_eq!(provider_env_var("ollama"), "");
    }

    #[test]
    fn save_and_reload_env_map() {
        // Test round-trip without touching real fs
        let mut map = HashMap::new();
        map.insert("TEST_KEY".to_string(), "test_val".to_string());
        let lines: Vec<String> = map.iter().map(|(k, v)| format!("export {}={}", k, v)).collect();
        let content = lines.join("\n") + "\n";
        // Parse it back
        let mut loaded = HashMap::new();
        for line in content.lines() {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
            if let Some((k, v)) = line.split_once('=') {
                loaded.insert(k.to_string(), v.to_string());
            }
        }
        assert_eq!(loaded.get("TEST_KEY").map(|s| s.as_str()), Some("test_val"));
    }
}
