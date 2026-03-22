use crate::cli::config_store::load_graph;
use crate::cli::config_validate::{validate_graph, ValidationSeverity};
use crate::config::config_file_path;
use crate::scheduler_runtime::resolve_scheduler_db_path;
use anyhow::Result;
use console::style;

pub async fn run() -> Result<()> {
    println!("{}", style("ClawBro Doctor").bold().cyan());
    println!("{}", "─".repeat(40));
    let mut issues = 0usize;

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

    println!("\n[2] Config file");
    let cfg_path = config_file_path();
    if !cfg_path.exists() {
        println!(
            "  {} config.toml missing — run: clawbro setup",
            style("✗").red()
        );
        issues += 1;
    } else {
        println!("  {} {}", style("✓").green(), cfg_path.display());
    }

    if !cfg_path.exists() {
        println!(
            "\n{} Found {} issue(s)",
            style("Doctor complete.").bold(),
            issues
        );
        return Ok(());
    }

    let graph = load_graph()?;
    let cfg = graph.to_gateway_config();
    let report = validate_graph(&graph);

    println!("\n[3] API / credentials");
    let env_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".clawbro")
        .join(".env");
    println!(
        "  {} ~/.clawbro/.env {}",
        if env_path.exists() {
            style("✓").green()
        } else {
            style("–").yellow()
        },
        if env_path.exists() {
            "exists"
        } else {
            "missing"
        }
    );
    let wechat_path = crate::channels_internal::wechat::WeChatConfig::credentials_path();
    println!(
        "  {} WeChat credentials {}",
        if wechat_path.exists() {
            style("✓").green()
        } else {
            style("–").yellow()
        },
        wechat_path.display()
    );

    println!("\n[4] Channels");
    println!(
        "  {} WeChat {}",
        if graph
            .channels
            .wechat
            .as_ref()
            .is_some_and(|cfg| cfg.enabled)
        {
            style("✓").green()
        } else {
            style("–").yellow()
        },
        if let Some(wechat) = graph.channels.wechat.as_ref() {
            format!("enabled (presentation={:?})", wechat.presentation)
        } else {
            "not enabled".to_string()
        }
    );
    println!(
        "  {} Lark {}",
        if graph.channels.lark.as_ref().is_some_and(|cfg| cfg.enabled) {
            style("✓").green()
        } else {
            style("–").yellow()
        },
        if let Some(lark) = graph.channels.lark.as_ref() {
            format!("enabled (instances={})", lark.instances.len())
        } else {
            "not enabled".to_string()
        }
    );
    println!(
        "  {} DingTalk {}",
        if graph
            .channels
            .dingtalk
            .as_ref()
            .is_some_and(|cfg| cfg.enabled)
        {
            style("✓").green()
        } else {
            style("–").yellow()
        },
        if graph.channels.dingtalk.is_some() {
            "enabled".to_string()
        } else {
            "not enabled".to_string()
        }
    );

    println!("\n[5] Runtime directories");
    for sub in ["sessions", "shared", "skills"] {
        let p = dirs::home_dir()
            .unwrap_or_default()
            .join(".clawbro")
            .join(sub);
        if p.exists() {
            println!("  {} ~/.clawbro/{}", style("✓").green(), sub);
        } else {
            println!("  {} ~/.clawbro/{} missing", style("✗").red(), sub);
            issues += 1;
        }
    }

    println!("\n[6] Scheduler");
    let scheduler_db = resolve_scheduler_db_path(&cfg);
    println!(
        "  {} scheduler db {}",
        if scheduler_db.exists() {
            style("✓").green()
        } else {
            style("–").yellow()
        },
        scheduler_db.display()
    );

    println!("\n[7] Validation");
    if report.issues.is_empty() {
        println!("  {} no validation issues", style("✓").green());
    } else {
        for issue in &report.issues {
            match issue.severity {
                ValidationSeverity::Error => {
                    println!("  {} [{}] {}", style("✗").red(), issue.code, issue.message);
                    issues += 1;
                }
                ValidationSeverity::Warning => {
                    println!(
                        "  {} [{}] {}",
                        style("!").yellow(),
                        issue.code,
                        issue.message
                    );
                }
            }
        }
    }

    println!(
        "\n{} Found {} issue(s)",
        style("Doctor complete.").bold(),
        issues
    );
    Ok(())
}
