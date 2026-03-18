use anyhow::Result;

pub async fn run_runtime_bridge() -> Result<()> {
    crate::cli::env::sanitize_tracing_env();
    crate::embedded_agent::install_rustls_default();
    crate::embedded_agent::native_runtime::run_stdio_bridge().await
}

pub async fn run_acp_agent() -> Result<()> {
    crate::cli::env::sanitize_tracing_env();
    crate::embedded_agent::install_rustls_default();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    crate::embedded_agent::acp_agent::run_stdio_agent().await
}
