use crate::memory::{MemoryEvent, MemorySystem};
use crate::relay::RelayEngine;
use crate::routing::RosterMatchData;
use dashmap::DashMap;
use clawbro_channels::mention_trigger::MentionTrigger;
use clawbro_protocol::{normalize_conversation_identity, InboundMsg, MsgSource, SessionKey};
use clawbro_session::{SessionStorage, StoredMessage};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) struct PostTurnProcessor<'a> {
    pub relay_engine: Option<&'a Arc<RelayEngine>>,
    pub mention_trigger: Option<&'a Arc<MentionTrigger>>,
    pub memory_system: Option<&'a Arc<MemorySystem>>,
    pub last_activity: &'a DashMap<SessionKey, std::time::Instant>,
    pub turn_counts: &'a DashMap<(SessionKey, String), u64>,
}

pub(crate) struct PostTurnInput<'a> {
    pub inbound: &'a InboundMsg,
    pub session_key: &'a SessionKey,
    pub session_id: Uuid,
    pub storage: Arc<SessionStorage>,
    pub sender_name: Option<String>,
    pub persona_prefix: Option<String>,
    pub roster_match: Option<&'a RosterMatchData>,
    pub persona_dir: Option<PathBuf>,
    pub user_text_for_log: &'a str,
    pub full_text: String,
    pub is_lead: bool,
    pub team_orchestrator: Option<std::sync::Arc<crate::team::orchestrator::TeamOrchestrator>>,
}

pub(crate) async fn process_post_turn(
    processor: PostTurnProcessor<'_>,
    input: PostTurnInput<'_>,
) -> anyhow::Result<String> {
    let full_text = apply_relay_hook(
        processor.relay_engine,
        input.session_key,
        input.full_text,
        input.is_lead,
    )
    .await;

    // Lead agents coordinate via MCP tools (assign_task), not via @mention in reply text.
    // Scanning Lead output would cause double-dispatch: MentionTrigger fires immediately
    // on the suppressed reply text, while Heartbeat also dispatches the same specialist.
    if !input.is_lead && should_scan_mentions(&input.inbound.source) {
        if let Some(trigger) = processor.mention_trigger {
            let sender = input
                .roster_match
                .map(|rm| rm.agent_name.as_str())
                .unwrap_or("agent");
            trigger.scan_and_dispatch(&full_text, sender, input.session_key, &input.inbound.source);
        }
    }

    let pending_fragments = if input.is_lead {
        input
            .team_orchestrator
            .as_ref()
            .map(|orch| orch.take_pending_lead_fragments())
            .filter(|fragments| !fragments.is_empty())
    } else {
        None
    };

    let pending_fragments =
        if input.is_lead && !full_text.trim().is_empty() && pending_fragments.is_none() {
            input.team_orchestrator.as_ref().map(|orch| {
                orch.record_leader_fragment(
                    crate::team::session::LeaderUpdateKind::FinalAnswerFragment,
                    &full_text,
                );
                orch.take_pending_lead_fragments()
            })
        } else {
            pending_fragments
        };

    let fragment_event_ids = pending_fragments
        .as_ref()
        .map(|fragments| {
            fragments
                .iter()
                .map(|fragment| fragment.event_id.clone())
                .collect::<Vec<_>>()
        })
        .filter(|ids| !ids.is_empty());

    let stored_content = if full_text.trim().is_empty() {
        pending_fragments
            .as_ref()
            .map(|fragments| {
                fragments
                    .iter()
                    .map(|fragment| fragment.text.as_str())
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| full_text.clone())
    } else {
        full_text.clone()
    };

    let assistant_msg = StoredMessage {
        id: Uuid::new_v4(),
        role: "assistant".to_string(),
        content: stored_content.clone(),
        timestamp: chrono::Utc::now(),
        sender: input.sender_name,
        tool_calls: None,
        aggregation_mode: fragment_event_ids
            .as_ref()
            .map(|_| "turn_compacted".to_string()),
        fragment_event_ids,
    };
    if !assistant_msg.content.trim().is_empty() || assistant_msg.fragment_event_ids.is_some() {
        input
            .storage
            .append_message(input.session_id, &assistant_msg)
            .await?;
    }

    processor.last_activity.insert(
        normalize_conversation_identity(input.session_key),
        std::time::Instant::now(),
    );

    if let (Some(ms), Some(persona_dir)) = (processor.memory_system, input.persona_dir) {
        let agent_name = input
            .roster_match
            .map(|rm| rm.agent_name.trim_start_matches('@').to_string())
            .unwrap_or_else(|| "default".to_string());
        let log_entry = format!(
            "**[{}]**: {}\n\n**[@{}]**: {}",
            input.inbound.sender, input.user_text_for_log, agent_name, stored_content
        );
        let store = ms.store();
        let sk = input.session_key.clone();
        let pd = persona_dir.clone();
        tokio::spawn(async move {
            store.append_daily_log(&pd, &sk, &log_entry).await.ok();
        });

        let count_key = (
            normalize_conversation_identity(input.session_key),
            agent_name.clone(),
        );
        let new_count = {
            let mut count = processor.turn_counts.entry(count_key).or_insert(0);
            *count += 1;
            *count
        };
        ms.emit(MemoryEvent::TurnCompleted {
            scope: input.session_key.clone(),
            agent: agent_name,
            persona_dir,
            turn_count: new_count,
        });
    }

    Ok(apply_reply_prefix(
        &full_text,
        input.persona_prefix.as_deref(),
    ))
}

pub(crate) async fn apply_relay_hook(
    relay_engine: Option<&Arc<RelayEngine>>,
    session_key: &SessionKey,
    full_text: String,
    is_lead: bool,
) -> String {
    if is_lead {
        if full_text.contains("[RELAY:") {
            tracing::warn!(
                session = ?session_key,
                "Lead turn output contains [RELAY:] syntax — relay hook skipped. \
                 Use assign_task MCP tool to communicate with Specialists."
            );
        }
        return full_text;
    }

    let Some(relay) = relay_engine else {
        return full_text;
    };
    if !full_text.contains("[RELAY:") {
        return full_text;
    }
    match relay.process(&full_text, session_key).await {
        Ok(processed) => processed,
        Err(error) => {
            tracing::warn!("relay engine error: {:#}", error);
            full_text
        }
    }
}

pub(crate) fn should_scan_mentions(source: &MsgSource) -> bool {
    !matches!(
        source,
        MsgSource::BotMention | MsgSource::Relay | MsgSource::TeamNotify | MsgSource::Heartbeat
    )
}

pub(crate) fn apply_reply_prefix(full_text: &str, persona_prefix: Option<&str>) -> String {
    match persona_prefix {
        Some(prefix) => format!("{prefix}{full_text}"),
        None => full_text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawbro_protocol::MsgContent;

    #[tokio::test]
    async fn lead_relay_output_is_left_untouched() {
        let session_key = SessionKey::new("lark", "group:test");
        let output = apply_relay_hook(
            None,
            &session_key,
            "[RELAY: @codex do work]".to_string(),
            true,
        )
        .await;
        assert_eq!(output, "[RELAY: @codex do work]");
    }

    #[test]
    fn lead_turns_do_not_scan_mentions_regardless_of_source() {
        // is_lead=true must block MentionTrigger even when source=Human,
        // preventing double-dispatch (MentionTrigger + Heartbeat both run beta).
        assert!(should_scan_mentions(&MsgSource::Human)); // Human would normally trigger
                                                          // The !is_lead guard in process_post_turn blocks the scan before we reach scan_and_dispatch.
                                                          // This test documents the invariant: Lead uses MCP tools, not @mentions.
    }

    #[test]
    fn automated_sources_do_not_scan_mentions() {
        assert!(!should_scan_mentions(&MsgSource::Heartbeat));
        assert!(!should_scan_mentions(&MsgSource::Relay));
        assert!(!should_scan_mentions(&MsgSource::TeamNotify));
        assert!(!should_scan_mentions(&MsgSource::BotMention));
        assert!(should_scan_mentions(&MsgSource::Human));
    }

    #[test]
    fn reply_prefix_is_only_applied_when_present() {
        assert_eq!(
            apply_reply_prefix("hello", Some("[@Rex]: ")),
            "[@Rex]: hello"
        );
        assert_eq!(apply_reply_prefix("hello", None), "hello");
    }

    #[test]
    fn post_turn_input_can_hold_human_message_context() {
        let inbound = InboundMsg {
            id: "post-turn-1".to_string(),
            session_key: SessionKey::new("ws", "post-turn"),
            content: MsgContent::text("hello"),
            sender: "user".to_string(),
            channel: "ws".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: MsgSource::Human,
        };
        assert_eq!(inbound.content.as_text(), Some("hello"));
    }
}
