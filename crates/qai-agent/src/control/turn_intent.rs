use qai_protocol::InboundMsg;
use qai_runtime::contract::{TurnIntent, TurnMode};

pub(crate) fn build_turn_intent(
    inbound: &InboundMsg,
    mode: TurnMode,
    leader_candidate: Option<&str>,
    target_backend: Option<&str>,
) -> TurnIntent {
    TurnIntent {
        session_key: inbound.session_key.clone(),
        mode,
        leader_candidate: leader_candidate.map(str::to_string),
        target_backend: target_backend.map(str::to_string),
        user_text: inbound.content.as_text().unwrap_or("").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qai_protocol::{MsgContent, MsgSource, SessionKey};

    #[test]
    fn team_human_message_without_mention_targets_leader_candidate() {
        let inbound = InboundMsg {
            id: "1".to_string(),
            session_key: SessionKey::new("lark", "group:test"),
            content: MsgContent::text("need a team"),
            sender: "user".to_string(),
            channel: "lark".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: MsgSource::Human,
        };

        let intent = build_turn_intent(&inbound, TurnMode::Team, Some("claude"), Some("claude"));
        assert_eq!(intent.mode, TurnMode::Team);
        assert_eq!(intent.leader_candidate.as_deref(), Some("claude"));
        assert_eq!(intent.target_backend.as_deref(), Some("claude"));
    }
}
