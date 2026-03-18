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

    // Load ~/.clawbro/.env and normalize common quoted values such as RUST_LOG='info'.
    crate::cli::env::load_user_dot_env();

    // Gateway config is still sourced through environment overrides so
    // `clawbro serve` and the internal service entrypoint share the same path.
    std::env::set_var("CLAWBRO_CONFIG", config_path.to_string_lossy().as_ref());
    if let Some(port) = args.port {
        std::env::set_var("CLAWBRO_PORT", port.to_string());
    }

    crate::run_gateway_process().await
}
