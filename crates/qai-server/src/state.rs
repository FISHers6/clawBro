use crate::config::GatewayConfig;
use async_trait::async_trait;
use qai_agent::{ApprovalDecision as AgentApprovalDecision, ApprovalResolver, SessionRegistry};
use qai_protocol::AgentEvent;
use qai_runtime::{ApprovalBroker, ApprovalDecision as RuntimeApprovalDecision, BackendRegistry};
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<SessionRegistry>,
    pub runtime_registry: Arc<BackendRegistry>,
    pub event_tx: broadcast::Sender<AgentEvent>,
    pub cfg: Arc<GatewayConfig>,
    pub runtime_token: Arc<String>,
    pub approvals: ApprovalBroker,
}

#[derive(Clone)]
pub struct BrokerApprovalResolver {
    approvals: ApprovalBroker,
}

impl BrokerApprovalResolver {
    pub fn new(approvals: ApprovalBroker) -> Self {
        Self { approvals }
    }
}

#[async_trait]
impl ApprovalResolver for BrokerApprovalResolver {
    async fn resolve(
        &self,
        approval_id: &str,
        decision: AgentApprovalDecision,
    ) -> anyhow::Result<bool> {
        let runtime_decision = match decision {
            AgentApprovalDecision::AllowOnce => RuntimeApprovalDecision::AllowOnce,
            AgentApprovalDecision::AllowAlways => RuntimeApprovalDecision::AllowAlways,
            AgentApprovalDecision::Deny => RuntimeApprovalDecision::Deny,
        };
        Ok(self.approvals.resolve(approval_id, runtime_decision))
    }
}
