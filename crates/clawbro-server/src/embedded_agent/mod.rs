use crate::agent_sdk_internal::{
    bridge::{AgentTurnRequest, ApprovalMode},
    tools::{ConfiguredAgentBuilder, RuntimeToolAugmentor, ToolProgressTracker},
};
use rig::completion::CompletionModel;

pub mod acp_agent;
pub mod native_runtime;
pub mod schedule;
pub mod team;

pub fn install_rustls_default() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[derive(Debug, Clone)]
pub struct ChainedAugmentor<A, B> {
    first: A,
    second: B,
}

impl<A, B> ChainedAugmentor<A, B> {
    pub fn new(first: A, second: B) -> Self {
        Self { first, second }
    }
}

impl<A, B> RuntimeToolAugmentor for ChainedAugmentor<A, B>
where
    A: RuntimeToolAugmentor,
    B: RuntimeToolAugmentor,
{
    fn augment<M: CompletionModel>(
        &self,
        builder: ConfiguredAgentBuilder<M>,
        session: &AgentTurnRequest,
        tracker: Option<ToolProgressTracker>,
        approval_mode: ApprovalMode,
    ) -> ConfiguredAgentBuilder<M> {
        let builder = self
            .first
            .augment(builder, session, tracker.clone(), approval_mode);
        self.second
            .augment(builder, session, tracker, approval_mode)
    }
}
