//! quickai-rust-agent — ACP Server binary
//! Communicates over stdio using the Agent Client Protocol.
//!
//! Usage:
//!   ANTHROPIC_API_KEY=sk-... quickai-rust-agent
//!   RUST_LOG=info ANTHROPIC_API_KEY=sk-... quickai-rust-agent
//!
//! Without an API key, runs as an echo stub (validates ACP protocol handshake).
//! With an API key, uses rig-core to generate real LLM responses.

use acp::Client as _;
use agent_client_protocol as acp;
use quickai_rust_agent::{agent, config, native_runtime};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // Install ring as the default rustls CryptoProvider.
    // reqwest 0.12 + rustls 0.23 requires this for HTTPS calls.
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr) // logs → stderr, ACP frames → stdout
        .init();

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--runtime-bridge") {
        return native_runtime::run_stdio_bridge().await;
    }

    let agent_config = config::AgentConfig::from_env().ok();

    if agent_config.is_some() {
        tracing::info!("Starting quickai-rust-agent with rig-core LLM engine");
    } else {
        tracing::info!("Starting quickai-rust-agent in echo stub mode (no API key set)");
    }

    let outgoing = tokio::io::stdout().compat_write();
    let incoming = tokio::io::stdin().compat();

    // ACP is !Send — must run inside LocalSet
    let local_set = tokio::task::LocalSet::new();
    local_set
        .run_until(async move {
            let (notif_tx, mut notif_rx) = tokio::sync::mpsc::unbounded_channel();

            let (conn, handle_io) = acp::AgentSideConnection::new(
                agent::QuickAiAgent::new(notif_tx, agent_config),
                outgoing,
                incoming,
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );

            // Background task: forward session_notifications to client
            tokio::task::spawn_local(async move {
                while let Some((notification, reply_tx)) = notif_rx.recv().await {
                    if let Err(e) = conn.session_notification(notification).await {
                        tracing::error!("session_notification failed: {e}");
                        break;
                    }
                    reply_tx.send(()).ok();
                }
            });

            handle_io.await?;
            Ok(())
        })
        .await
}
