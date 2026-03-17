use clawbro_protocol::SessionKey;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnExecutionContext {
    pub delivery_source: Option<TurnDeliverySource>,
}

// TeamRoutingEnvelope persists this type into team ledgers such as
// pending-completions.jsonl / routing-events.jsonl, so it must remain
// serializable even though it originates from turn-local runtime context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnDeliverySource {
    pub channel: String,
    pub channel_instance: Option<String>,
    pub scope: String,
    pub reply_to: Option<String>,
    pub thread_ts: Option<String>,
}

impl TurnDeliverySource {
    pub fn from_session_key(session_key: &SessionKey) -> Self {
        Self {
            channel: session_key.channel.clone(),
            channel_instance: session_key.channel_instance.clone(),
            scope: session_key.scope.clone(),
            reply_to: None,
            thread_ts: None,
        }
    }

    pub fn with_reply_context(
        mut self,
        reply_to: Option<String>,
        thread_ts: Option<String>,
    ) -> Self {
        self.reply_to = reply_to;
        self.thread_ts = thread_ts;
        self
    }

    pub fn session_key(&self) -> SessionKey {
        SessionKey {
            channel: self.channel.clone(),
            channel_instance: self.channel_instance.clone(),
            scope: self.scope.clone(),
        }
    }
}
