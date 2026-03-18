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
    if cfg_path.exists() {
        let content = std::fs::read_to_string(&cfg_path).unwrap_or_default();
        match toml::from_str::<toml::Value>(&content) {
            Ok(_) => println!(
                "  {} ~/.clawbro/config.toml (valid TOML)",
                style("✓").green()
            ),
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

    // 4. Runtime directories
    println!("\n[4] Runtime directories");
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

    // 5. Gateway process
    println!("\n[5] Gateway process");
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
