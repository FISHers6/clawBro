use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Agent 执行过程中产生的事件（用于 WebSocket 流式推送）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentEvent {
    TextDelta {
        session_id: Uuid,
        delta: String,
    },
    ToolCallStart {
        session_id: Uuid,
        tool_name: String,
        call_id: String,
    },
    ToolCallResult {
        session_id: Uuid,
        call_id: String,
        result: String,
    },
    Thinking {
        session_id: Uuid,
    },
    TurnComplete {
        session_id: Uuid,
        full_text: String,
        #[serde(default)]
        sender: Option<String>,
    },
    Error {
        session_id: Uuid,
        message: String,
    },
}

impl AgentEvent {
    pub fn session_id(&self) -> Uuid {
        match self {
            Self::TextDelta { session_id, .. } => *session_id,
            Self::ToolCallStart { session_id, .. } => *session_id,
            Self::ToolCallResult { session_id, .. } => *session_id,
            Self::Thinking { session_id } => *session_id,
            Self::TurnComplete { session_id, .. } => *session_id,
            Self::Error { session_id, .. } => *session_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_turn_complete_sender_default_none() {
        // Old-format JSON (no sender field) must deserialize with sender=None (backward compat)
        let json = r#"{"type":"TurnComplete","session_id":"00000000-0000-0000-0000-000000000001","full_text":"hello"}"#;
        let event: AgentEvent = serde_json::from_str(json).unwrap();
        if let AgentEvent::TurnComplete { sender, .. } = event {
            assert!(
                sender.is_none(),
                "legacy JSON should deserialize with sender=None"
            );
        } else {
            panic!("wrong variant");
        }
    }
}
