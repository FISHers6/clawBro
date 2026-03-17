use crate::cli::args::ServeArgs;
use anyhow::{Context, Result};

pub async fn run(args: ServeArgs) -> Result<()> {
    let config_path = args.config.unwrap_or_else(|| {
        dirs::home_dir().unwrap_or_default().join(".quickai").join("config.toml")
    });

    if !config_path.exists() {
        anyhow::bail!(
            "Config file not found: {}\nRun: quickai setup",
            config_path.display()
        );
    }

    // Load .env into current process environment (child inherits it after exec)
    load_dot_env();

    // Find the gateway binary
    let gateway_bin = which::which("quickai-gateway")
        .context("quickai-gateway not found — ensure it's installed and in PATH")?;

    // Pass config path via env var
    std::env::set_var("QUICKAI_CONFIG", config_path.to_string_lossy().as_ref());
    if let Some(port) = args.port {
        std::env::set_var("QUICKAI_PORT", port.to_string());
    }

    // Unix: exec-replace current process (no zombie, clean signals)
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&gateway_bin).exec();
        return Err(anyhow::anyhow!("exec failed: {err}"));
    }

    // Windows: spawn and wait
    #[cfg(not(unix))]
    {
        let status = std::process::Command::new(&gateway_bin)
            .status()
            .context("Failed to start quickai-gateway")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn load_dot_env() {
    let path = dirs::home_dir().unwrap_or_default().join(".quickai").join(".env");
    let Ok(content) = std::fs::read_to_string(&path) else { return };
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() { continue; }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=') {
            if std::env::var(k.trim()).is_err() {
                std::env::set_var(k.trim(), v.trim());
            }
        }
    }
}
