//! qai-server library target.
//! Exposes modules and test helpers for integration tests.

pub mod config;
pub mod gateway;
pub mod state;

pub use config::GatewayConfig;
pub use state::AppState;

use anyhow::Result;
use qai_agent::{EngineConfig, SessionRegistry};
use qai_session::{SessionManager, SessionStorage};
use qai_skills::SkillLoader;
use std::net::SocketAddr;
use std::sync::Arc;

/// Start a gateway instance for testing.
/// The server runs until the tokio runtime shuts down (end of `#[tokio::test]`).
/// `agent_binary`: path to the ACP agent binary (e.g. `quickai-rust-agent`).
/// Returns the bound SocketAddr (port 0 = OS-assigned).
pub async fn start_test_gateway(agent_binary: &str) -> Result<SocketAddr> {
    let engine_config = EngineConfig::RustAgent {
        binary: Some(agent_binary.to_string()),
    };
    start_test_gateway_with_engine(engine_config).await
}

/// Start a gateway instance for testing with an explicit `EngineConfig`.
/// Returns the bound SocketAddr (port 0 = OS-assigned).
pub async fn start_test_gateway_with_engine(engine_config: EngineConfig) -> Result<SocketAddr> {
    let cfg = GatewayConfig::default();
    let storage = SessionStorage::new(cfg.session.dir.clone());
    let session_manager = Arc::new(SessionManager::new(storage));

    let skill_loader = SkillLoader::new(vec![cfg.skills.dir.clone()]);
    let skills = skill_loader.load_all();
    let system_injection = skill_loader.build_system_injection(&skills);

    // _event_rx dropped: new WS clients subscribe via state.event_tx.subscribe() instead
    let (registry, _event_rx) = SessionRegistry::new(
        engine_config,
        session_manager,
        system_injection,
        None, // no roster in test helpers
        None, // no memory system in test helpers
    );
    let event_tx = registry.global_sender();

    let state = AppState {
        registry,
        event_tx,
        cfg: Arc::new(cfg),
    };
    let addr = gateway::server::start(state, "127.0.0.1", 0).await?;
    Ok(addr)
}
