use crate::config::GatewayConfig;
use crate::scheduler_runtime::resolve_scheduler_db_path;
use anyhow::Result;
use console::style;

pub async fn run() -> Result<()> {
    println!("{}", style("ClawBro Doctor").bold().cyan());
    println!("{}", "─".repeat(40));
    let mut issues = 0usize;

    // 1. Binaries
    println!("\n[1] Binaries");
    for bin in ["clawbro"] {
        match which::which(bin) {
            Ok(p) => println!("  {} {} ({})", style("✓").green(), bin, p.display()),
            Err(_) => {
                println!(
                    "  {} {} not found — check PATH or rebuild",
                    style("✗").red(),
                    bin
                );
                issues += 1;
            }
        }
    }

    // 2. Config
    println!("\n[2] Config file");
    let cfg_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".clawbro")
        .join("config.toml");
    let mut parsed_cfg: Option<GatewayConfig> = None;
    if cfg_path.exists() {
        let content = std::fs::read_to_string(&cfg_path).unwrap_or_default();
        match toml::from_str::<toml::Value>(&content) {
            Ok(_) => {
                println!(
                    "  {} ~/.clawbro/config.toml (valid TOML)",
                    style("✓").green()
                );
                match toml::from_str::<GatewayConfig>(&content) {
                    Ok(cfg) => parsed_cfg = Some(cfg),
                    Err(e) => {
                        println!("  {} config.toml schema error: {e}", style("✗").red());
                        issues += 1;
                    }
                }
            }
            Err(e) => {
                println!("  {} config.toml syntax error: {e}", style("✗").red());
                issues += 1;
            }
        }
    } else {
        println!(
            "  {} config.toml missing — run: clawbro setup",
            style("✗").red()
        );
        issues += 1;
    }

    // 3. API Keys
    println!("\n[3] API Keys");
    let env_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".clawbro")
        .join(".env");
    if env_path.exists() {
        println!("  {} ~/.clawbro/.env exists", style("✓").green());
    } else {
        println!("  {} ~/.clawbro/.env missing", style("–").yellow());
    }
    for var in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"] {
        match std::env::var(var) {
            Ok(v) if !v.is_empty() => {
                println!("  {} {} set (current shell)", style("✓").green(), var)
            }
            _ => println!(
                "  {} {} not set (run: source ~/.clawbro/.env)",
                style("–").yellow(),
                var
            ),
        }
    }

    // 4. Channel configuration
    println!("\n[4] Channel configuration");
    if let Some(cfg) = &parsed_cfg {
        match cfg
            .channels
            .dingtalk_webhook
            .as_ref()
            .filter(|section| section.enabled)
        {
            Some(webhook) => {
                if webhook.secret_key.trim().is_empty() {
                    println!("  {} dingtalk_webhook.secret_key missing", style("✗").red());
                    issues += 1;
                } else {
                    println!(
                        "  {} dingtalk_webhook.secret_key configured",
                        style("✓").green()
                    );
                }
                if webhook.webhook_path.trim().is_empty() {
                    println!(
                        "  {} dingtalk_webhook.webhook_path missing",
                        style("✗").red()
                    );
                    issues += 1;
                } else {
                    println!(
                        "  {} dingtalk_webhook.webhook_path = {}",
                        style("✓").green(),
                        webhook.webhook_path
                    );
                }
                if webhook
                    .access_token
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                {
                    println!(
                        "  {} dingtalk_webhook.access_token configured (fallback enabled)",
                        style("✓").green()
                    );
                } else {
                    println!(
                        "  {} dingtalk_webhook.access_token missing (no robot/send fallback)",
                        style("–").yellow()
                    );
                }
            }
            None => {
                println!("  {} dingtalk_webhook not enabled", style("–").yellow());
            }
        }
    } else {
        println!(
            "  {} channel checks skipped (config invalid)",
            style("–").yellow()
        );
    }

    // 5. Runtime directories
    println!("\n[5] Runtime directories");
    for sub in ["sessions", "shared", "skills"] {
        let p = dirs::home_dir()
            .unwrap_or_default()
            .join(".clawbro")
            .join(sub);
        if p.exists() {
            println!("  {} ~/.clawbro/{}", style("✓").green(), sub);
        } else {
            println!(
                "  {} ~/.clawbro/{} missing (mkdir -p ~/.clawbro/{})",
                style("✗").red(),
                sub,
                sub
            );
            issues += 1;
        }
    }

    // 6. Gateway process
    println!("\n[6] Gateway process");
    let port_file = dirs::home_dir()
        .unwrap_or_default()
        .join(".clawbro")
        .join("gateway.port");
    if port_file.exists() {
        let port = std::fs::read_to_string(&port_file).unwrap_or_default();
        println!("  {} Running (port: {})", style("✓").green(), port.trim());
    } else {
        println!(
            "  {} Not running (gateway.port missing)",
            style("–").yellow()
        );
    }

    // 7. Scheduler
    println!("\n[7] Scheduler");
    if let Some(cfg) = &parsed_cfg {
        let db_path = resolve_scheduler_db_path(cfg);
        println!(
            "  {} scheduler {}",
            if cfg.scheduler.enabled {
                style("✓").green()
            } else {
                style("–").yellow()
            },
            if cfg.scheduler.enabled {
                format!(
                    "enabled (poll={}s, max_concurrent={})",
                    cfg.scheduler.poll_secs, cfg.scheduler.max_concurrent
                )
            } else {
                "disabled".to_string()
            }
        );
        if db_path.exists() {
            println!("  {} db path {}", style("✓").green(), db_path.display());
        } else {
            println!(
                "  {} db path {} (not created yet)",
                style("–").yellow(),
                db_path.display()
            );
        }
    } else {
        println!(
            "  {} scheduler checks skipped (config invalid)",
            style("–").yellow()
        );
    }

    // Summary
    println!("\n{}", "─".repeat(40));
    if issues == 0 {
        println!("{}", style("✓ All checks passed").bold().green());
    } else {
        println!(
            "{}",
            style(format!("{} issue(s) found — see above", issues))
                .bold()
                .yellow()
        );
    }
    Ok(())
}
