use crate::agent_core::{
    ApprovalDecision as AgentApprovalDecision, ApprovalResolver, SessionRegistry,
};
use crate::channel_registry::ChannelRegistry;
use crate::channels_internal::dingtalk_webhook::DingTalkWebhookChannel;
use crate::config::GatewayConfig;
use crate::protocol::AgentEvent;
use crate::runtime::{
    ApprovalBroker, ApprovalDecision as RuntimeApprovalDecision, BackendRegistry,
};
use crate::scheduler::SchedulerService;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<SessionRegistry>,
    pub runtime_registry: Arc<BackendRegistry>,
    pub event_tx: broadcast::Sender<AgentEvent>,
    pub cfg: Arc<GatewayConfig>,
    pub channel_registry: Arc<ChannelRegistry>,
    pub dingtalk_webhook_channel: Option<Arc<DingTalkWebhookChannel>>,
    pub runtime_token: Arc<String>,
    pub approvals: ApprovalBroker,
    pub scheduler_service: Arc<SchedulerService>,
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
