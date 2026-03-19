use crate::config::GatewayConfig;
use crate::scheduler_runtime::resolve_scheduler_db_path;
use anyhow::Result;
use console::style;

pub async fn run() -> Result<()> {
    let cfg_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".clawbro")
        .join("config.toml");

    println!("{}", style("ClawBro — Status").bold().cyan());
    println!("{}", "─".repeat(40));

    if !cfg_path.exists() {
        println!(
            "{} config.toml not found — run: clawbro setup",
            style("⚠").yellow()
        );
        return Ok(());
    }

    let content = std::fs::read_to_string(&cfg_path)?;
    let val: toml::Value =
        toml::from_str(&content).unwrap_or(toml::Value::Table(toml::map::Map::new()));
    let parsed_cfg = GatewayConfig::from_toml_str(&content).ok();

    let port = val
        .get("gateway")
        .and_then(|g| g.get("port"))
        .and_then(|p| p.as_integer())
        .unwrap_or(0);
    println!(
        "  Port         {}",
        if port == 0 {
            "random".into()
        } else {
            port.to_string()
        }
    );

    let roster_n = val
        .get("agent_roster")
        .and_then(|r| r.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    println!(
        "  Mode         {}",
        if roster_n > 0 {
            format!("Multi-agent ({} agents)", roster_n)
        } else {
            "Solo".into()
        }
    );

    let backends: Vec<&str> = val
        .get("backend")
        .and_then(|b| b.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|b| b.get("id").and_then(|id| id.as_str()))
                .collect()
        })
        .unwrap_or_default();
    println!(
        "  Backends     {}",
        if backends.is_empty() {
            "(none configured)".into()
        } else {
            backends.join(", ")
        }
    );

    let profiles: Vec<&str> = val
        .get("provider_profile")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.get("id").and_then(|id| id.as_str()))
                .collect()
        })
        .unwrap_or_default();
    println!(
        "  Providers    {}",
        if profiles.is_empty() {
            "(none configured)".into()
        } else {
            profiles.join(", ")
        }
    );

    let lark = val.get("channels").and_then(|c| c.get("lark")).is_some();
    let dt = val
        .get("channels")
        .and_then(|c| c.get("dingtalk"))
        .is_some();
    let ch_str = match (lark, dt) {
        (true, true) => "Lark + DingTalk",
        (true, false) => "Lark",
        (false, true) => "DingTalk",
        _ => "WebSocket only",
    };
    println!("  Channel      {}", ch_str);

    let has_ws_token = val
        .get("auth")
        .and_then(|a| a.get("ws_token"))
        .and_then(|t| t.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    println!(
        "  WS Auth      {}",
        if has_ws_token {
            style("enabled (token set)").green().to_string()
        } else {
            "open mode (no token)".into()
        }
    );

    let has_key = std::env::var("ANTHROPIC_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .is_ok();
    println!(
        "  API Key      {}",
        if has_key {
            style("set").green().to_string()
        } else {
            style("not set (source ~/.clawbro/.env)")
                .yellow()
                .to_string()
        }
    );

    let port_file = dirs::home_dir()
        .unwrap_or_default()
        .join(".clawbro")
        .join("gateway.port");
    println!(
        "  Gateway      {}",
        if port_file.exists() {
            style("running").green().to_string()
        } else {
            style("not running").dim().to_string()
        }
    );

    if let Some(cfg) = parsed_cfg.as_ref() {
        let scheduler_db = resolve_scheduler_db_path(cfg);
        println!(
            "  Scheduler    {}",
            if cfg.scheduler.enabled {
                format!(
                    "enabled (poll={}s, db={})",
                    cfg.scheduler.poll_secs,
                    scheduler_db.display()
                )
            } else {
                format!("disabled (db={})", scheduler_db.display())
            }
        );
        println!(
            "  Scheduler DB {}",
            if scheduler_db.exists() {
                style("present").green().to_string()
            } else {
                style("missing").yellow().to_string()
            }
        );
    } else {
        println!("  Scheduler    config invalid");
    }

    println!("\nConfig: {}", cfg_path.display());
    Ok(())
}
