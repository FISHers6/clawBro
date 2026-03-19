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
    ApprovalRequest {
        session_id: Uuid,
        session_key: crate::protocol::SessionKey,
        approval_id: String,
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at_ms: Option<u64>,
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
    ToolCallFailed {
        session_id: Uuid,
        tool_name: String,
        call_id: String,
        error: String,
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
            Self::ApprovalRequest { session_id, .. } => *session_id,
            Self::ToolCallStart { session_id, .. } => *session_id,
            Self::ToolCallResult { session_id, .. } => *session_id,
            Self::ToolCallFailed { session_id, .. } => *session_id,
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

    #[test]
    fn test_approval_request_round_trip() {
        let event = AgentEvent::ApprovalRequest {
            session_id: Uuid::nil(),
            session_key: crate::protocol::SessionKey::new("ws", "approval"),
            approval_id: "approval-1".into(),
            prompt: "Allow `git status`?".into(),
            command: Some("git status".into()),
            cwd: Some("/tmp".into()),
            host: Some("gateway".into()),
            agent_id: Some("main".into()),
            expires_at_ms: Some(123),
        };

        let json = serde_json::to_string(&event).unwrap();
        let decoded: AgentEvent = serde_json::from_str(&json).unwrap();
        match decoded {
            AgentEvent::ApprovalRequest {
                approval_id,
                session_key,
                command,
                expires_at_ms,
                ..
            } => {
                assert_eq!(approval_id, "approval-1");
                assert_eq!(
                    session_key,
                    crate::protocol::SessionKey::new("ws", "approval")
                );
                assert_eq!(command.as_deref(), Some("git status"));
                assert_eq!(expires_at_ms, Some(123));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
