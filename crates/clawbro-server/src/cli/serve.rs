use crate::cli::args::ServeArgs;
use anyhow::Result;

pub async fn run(args: ServeArgs) -> Result<()> {
    let config_path = args.config.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".clawbro")
            .join("config.toml")
    });

    if !config_path.exists() {
        anyhow::bail!(
            "Config file not found: {}\nRun: clawbro setup",
            config_path.display()
        );
    }

    // Load .env into current process environment (child inherits it after exec)
    load_dot_env();

    // Gateway config is still sourced through environment overrides so
    // `clawbro serve` and the internal service entrypoint share the same path.
    std::env::set_var("CLAWBRO_CONFIG", config_path.to_string_lossy().as_ref());
    if let Some(port) = args.port {
        std::env::set_var("CLAWBRO_PORT", port.to_string());
    }

    crate::run_gateway_process().await
}

fn load_dot_env() {
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join(".clawbro")
        .join(".env");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=') {
            if std::env::var(k.trim()).is_err() {
                std::env::set_var(k.trim(), v.trim());
            }
        }
    }
}
