use crate::config::GatewayConfig;
use qai_agent::SessionRegistry;
use qai_protocol::AgentEvent;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<SessionRegistry>,
    pub event_tx: broadcast::Sender<AgentEvent>,
    pub cfg: Arc<GatewayConfig>,
}
