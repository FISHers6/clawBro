use crate::channels_internal::ws_virtual::WsVirtualChannel;
use crate::config::ProgressPresentationMode;
use crate::protocol::{
    normalize_runtime_session_identity, AgentEvent, InboundMsg, SessionKey, WsTopic,
};
use crate::runtime::ApprovalDecision;
use crate::state::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc;

fn extract_bearer(header: &str) -> Option<&str> {
    let token = header.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// WS 客户端可发送的消息类型
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum WsClientMsg {
    /// 订阅指定 session 的事件
    Subscribe { session_key: SessionKey },
    /// 取消订阅
    Unsubscribe { session_key: SessionKey },
    /// 订阅 dashboard 主题事件
    SubscribeTopic { topic: WsTopic },
    /// 取消 dashboard 主题事件订阅
    UnsubscribeTopic { topic: WsTopic },
    /// 向 agent 发送消息（现有功能，InboundMsg 格式）
    ResolveApproval {
        approval_id: String,
        decision: String,
    },
    /// 向 agent 发送消息（现有功能，InboundMsg 格式）
    #[serde(untagged)]
    Message(InboundMsg),
}

pub async fn ws_upgrade(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // Auth check: if ws_token is configured and non-empty, validate Bearer token
    let required = state.cfg.auth.ws_token.as_deref().filter(|t| !t.is_empty());

    if let Some(expected) = required {
        let provided = headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| extract_bearer(s));

        match provided {
            Some(token) if token == expected => {}
            _ => return StatusCode::UNAUTHORIZED.into_response(),
        }
    }

    ws.on_upgrade(move |socket| handle_socket(socket, state))
        .into_response()
}

/// Subscribe this WS connection to `session_key` events.
///
/// Each subscription gets its own dedicated channel (`sub_tx`/`sub_rx`).  The
/// registry delivers events into `sub_tx`; a forwarder task relays them to the
/// connection-wide `private_tx`.  Dropping `sub_tx` (on `Unsubscribe` or
/// connection close) closes the forwarding channel — the registry's next
/// `retain(!tx.is_closed())` pass removes the dead entry automatically.
fn ensure_subscription(
    state: &AppState,
    private_tx: &mpsc::UnboundedSender<AgentEvent>,
    local_subscriptions: &mut HashMap<SessionKey, mpsc::UnboundedSender<AgentEvent>>,
    session_key: &SessionKey,
) {
    let normalized = normalize_runtime_session_identity(session_key);
    if local_subscriptions.contains_key(&normalized) {
        return;
    }
    let (sub_tx, mut sub_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let fwd_tx = private_tx.clone();
    tokio::spawn(async move {
        while let Some(event) = sub_rx.recv().await {
            if fwd_tx.send(event).is_err() {
                break;
            }
        }
    });
    state
        .registry
        .ws_subs
        .entry(normalized.clone())
        .or_default()
        .push(sub_tx.clone());
    local_subscriptions.insert(normalized.clone(), sub_tx);
    tracing::debug!(session_key = ?normalized, "WS client subscribed to session");
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let (private_tx, mut private_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let mut dashboard_rx = state.registry.dashboard_sender().subscribe();
    // Maps session_key → per-subscription sender. Dropping the sender closes
    // the per-subscription channel and removes this WS connection from ws_subs
    // at the next delivery pass.
    let mut local_subscriptions = HashMap::<SessionKey, mpsc::UnboundedSender<AgentEvent>>::new();
    let mut local_topics = HashSet::<WsTopic>::new();

    loop {
        tokio::select! {
            // 接收客户端消息
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<WsClientMsg>(&text) {
                            Ok(WsClientMsg::Subscribe { session_key }) => {
                                ensure_subscription(
                                    &state,
                                    &private_tx,
                                    &mut local_subscriptions,
                                    &session_key,
                                );
                            }
                            Ok(WsClientMsg::Unsubscribe { session_key }) => {
                                let normalized = normalize_runtime_session_identity(&session_key);
                                // Dropping the per-subscription sub_tx closes its channel.
                                // The registry's retain(!tx.is_closed()) will remove the
                                // dead entry on the next event delivery pass.
                                if let Some(_sub_tx) = local_subscriptions.remove(&normalized) {
                                    // Eagerly prune now-closed entries to keep ws_subs compact.
                                    state.registry.ws_subs.alter(&normalized, |_, mut vec| {
                                        vec.retain(|tx| !tx.is_closed());
                                        vec
                                    });
                                    tracing::debug!(session_key = ?normalized, "WS client unsubscribed from session");
                                }
                            }
                            Ok(WsClientMsg::SubscribeTopic { topic }) => {
                                local_topics.insert(topic);
                            }
                            Ok(WsClientMsg::UnsubscribeTopic { topic }) => {
                                local_topics.remove(&topic);
                            }
                            Ok(WsClientMsg::ResolveApproval {
                                approval_id,
                                decision,
                            }) => {
                                match ApprovalDecision::parse(&decision) {
                                    Some(parsed) => {
                                        if !state.approvals.resolve(&approval_id, parsed) {
                                            tracing::warn!(
                                                approval_id = %approval_id,
                                                "WS approval resolve ignored: unknown or expired id"
                                            );
                                        }
                                    }
                                    None => {
                                        tracing::warn!(
                                            approval_id = %approval_id,
                                            decision = %decision,
                                            "WS approval resolve ignored: invalid decision"
                                        );
                                    }
                                }
                            }
                            Ok(WsClientMsg::Message(inbound)) => {
                                // Subscribe to the original session key (single-agent / explicit mention).
                                ensure_subscription(
                                    &state,
                                    &private_tx,
                                    &mut local_subscriptions,
                                    &inbound.session_key,
                                );
                                // Multi-agent broadcast: pre-expand to get agent-scoped session keys
                                // and subscribe to each, so WS events from all agent sessions are
                                // delivered back to this connection.
                                let expanded = crate::im_sink::expand_for_multi_agent(
                                    &inbound,
                                    &state.registry,
                                );
                                for msg in &expanded {
                                    if msg.session_key.scope != inbound.session_key.scope {
                                        ensure_subscription(
                                            &state,
                                            &private_tx,
                                            &mut local_subscriptions,
                                            &msg.session_key,
                                        );
                                    }
                                }
                                // Route through spawn_im_turn — picks up expand_for_multi_agent
                                // internally, so single-agent and @all broadcast both work uniformly.
                                crate::im_sink::spawn_im_turn(
                                    state.registry.clone(),
                                    Arc::new(WsVirtualChannel),
                                    state.channel_registry.clone(),
                                    state.cfg.clone(),
                                    inbound,
                                    ProgressPresentationMode::FinalOnly,
                                );
                            }
                            Err(e) => {
                                tracing::warn!("WS: malformed message ({})", e);
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            // 推送 per-session 私有事件到客户端
            Some(event) = private_rx.recv() => {
                if let Ok(json) = serde_json::to_string(&event) {
                    if socket.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
            }
            result = dashboard_rx.recv() => {
                match result {
                    Ok(event) => {
                        if local_topics.iter().any(|topic| event.matches_topic(topic)) {
                            if let Ok(json) = serde_json::to_string(&event) {
                                if socket.send(Message::Text(json.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            else => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::SessionRegistry;
    use crate::channel_registry::ChannelRegistry;
    use crate::runtime::{ApprovalBroker, BackendRegistry};
    use crate::session::{SessionManager, SessionStorage};
    use crate::state::AppState;
    use std::sync::Arc;

    #[test]
    fn test_extract_bearer_valid() {
        assert_eq!(extract_bearer("Bearer abc123"), Some("abc123"));
    }

    #[test]
    fn test_extract_bearer_empty_token() {
        assert_eq!(extract_bearer("Bearer "), None);
        assert_eq!(extract_bearer("Bearer"), None);
    }

    #[test]
    fn test_extract_bearer_wrong_scheme() {
        assert_eq!(extract_bearer("Basic abc"), None);
        assert_eq!(extract_bearer(""), None);
    }

    #[test]
    fn test_ws_client_msg_parse_subscribe() {
        let json = r#"{"type":"Subscribe","session_key":{"channel":"ws","scope":"u1"}}"#;
        let msg: WsClientMsg = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, WsClientMsg::Subscribe { .. }));
    }

    #[test]
    fn test_ws_client_msg_parse_unsubscribe() {
        let json = r#"{"type":"Unsubscribe","session_key":{"channel":"ws","scope":"u1"}}"#;
        let msg: WsClientMsg = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, WsClientMsg::Unsubscribe { .. }));
    }

    #[test]
    fn test_ws_client_msg_parse_inbound() {
        let json = r#"{"id":"1","session_key":{"channel":"ws","scope":"u1"},"content":{"type":"Text","text":"hi"},"sender":"u","channel":"ws","timestamp":"2026-01-01T00:00:00Z","thread_ts":null}"#;
        let msg: WsClientMsg = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, WsClientMsg::Message(_)));
    }

    #[test]
    fn test_ws_client_msg_parse_resolve_approval() {
        let json =
            r#"{"type":"ResolveApproval","approval_id":"approval-1","decision":"allow-once"}"#;
        let msg: WsClientMsg = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, WsClientMsg::ResolveApproval { .. }));
    }

    #[test]
    fn test_ws_client_msg_parse_subscribe_topic() {
        let json = r#"{"type":"SubscribeTopic","topic":{"kind":"team","team_id":"team-1"}}"#;
        let msg: WsClientMsg = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, WsClientMsg::SubscribeTopic { .. }));
    }

    #[test]
    fn test_dashboard_event_topic_filtering() {
        let event = crate::protocol::DashboardEvent::TeamPendingCompletion {
            team_id: "team-1".into(),
            record: crate::agent_core::team::completion_routing::PendingRoutingRecord::from_envelope(
                crate::agent_core::team::completion_routing::TeamRoutingEnvelope {
                    run_id: "run-1".into(),
                    parent_run_id: None,
                    requester_session_key: Some(SessionKey::new("lark", "group:oc_demo")),
                    fallback_session_keys: vec![],
                    delivery_source: None,
                    team_id: "team-1".into(),
                    delivery_status: crate::agent_core::team::completion_routing::RoutingDeliveryStatus::PersistedPending,
                    event: crate::agent_core::team::completion_routing::TeamRoutingEvent::submitted("T001", "claw", "done"),
                }
            ),
        };
        let mut topics = HashSet::new();
        topics.insert(WsTopic::Task {
            team_id: "team-1".into(),
            task_id: "T001".into(),
        });
        assert!(topics.iter().any(|topic| event.matches_topic(topic)));
        topics.clear();
        topics.insert(WsTopic::Task {
            team_id: "team-1".into(),
            task_id: "T002".into(),
        });
        assert!(!topics.iter().any(|topic| event.matches_topic(topic)));
    }

    fn test_state() -> AppState {
        let cfg = Arc::new(crate::config::GatewayConfig::default());
        let storage = SessionStorage::new(
            std::env::temp_dir().join(format!("ws-handler-sessions-{}", uuid::Uuid::new_v4())),
        );
        let session_manager = Arc::new(SessionManager::new(storage));
        let (registry, _rx) = SessionRegistry::new(
            None,
            session_manager,
            String::new(),
            None,
            None,
            None,
            None,
            vec![],
        );

        AppState {
            registry,
            runtime_registry: Arc::new(BackendRegistry::new()),
            event_tx: tokio::sync::broadcast::channel(8).0,
            cfg,
            channel_registry: Arc::new(ChannelRegistry::new()),
            dingtalk_webhook_channel: None,
            runtime_token: Arc::new("ws-token".to_string()),
            approvals: ApprovalBroker::default(),
            scheduler_service: crate::scheduler_runtime::build_test_scheduler_service(),
            config_path: Arc::new(crate::config::config_file_path()),
        }
    }

    #[tokio::test]
    async fn subscriptions_only_receive_events_for_their_session() {
        let state = test_state();
        let (private_tx_a, mut private_rx_a) = mpsc::unbounded_channel::<AgentEvent>();
        let (private_tx_b, mut private_rx_b) = mpsc::unbounded_channel::<AgentEvent>();
        let mut subs_a = HashMap::<SessionKey, mpsc::UnboundedSender<AgentEvent>>::new();
        let mut subs_b = HashMap::<SessionKey, mpsc::UnboundedSender<AgentEvent>>::new();
        let session_a = SessionKey::with_instance("lark", "alpha", "group:one");
        let session_b = SessionKey::with_instance("lark", "beta", "group:two");

        ensure_subscription(&state, &private_tx_a, &mut subs_a, &session_a);
        ensure_subscription(&state, &private_tx_b, &mut subs_b, &session_b);

        let event_a = AgentEvent::Thinking {
            session_id: uuid::Uuid::new_v4(),
        };
        state.registry.ws_subs.alter(
            &normalize_runtime_session_identity(&session_a),
            |_, mut senders| {
                senders.retain(|tx| tx.send(event_a.clone()).is_ok());
                senders
            },
        );
        let delivered_a =
            tokio::time::timeout(std::time::Duration::from_secs(1), private_rx_a.recv())
                .await
                .unwrap()
                .unwrap();
        assert!(matches!(delivered_a, AgentEvent::Thinking { .. }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), private_rx_b.recv())
                .await
                .is_err()
        );

        let event_b = AgentEvent::TurnComplete {
            session_id: uuid::Uuid::new_v4(),
            full_text: "done".to_string(),
            sender: Some("beta".to_string()),
        };
        state.registry.ws_subs.alter(
            &normalize_runtime_session_identity(&session_b),
            |_, mut senders| {
                senders.retain(|tx| tx.send(event_b.clone()).is_ok());
                senders
            },
        );
        let delivered_b =
            tokio::time::timeout(std::time::Duration::from_secs(1), private_rx_b.recv())
                .await
                .unwrap()
                .unwrap();
        assert!(matches!(delivered_b, AgentEvent::TurnComplete { .. }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), private_rx_a.recv())
                .await
                .is_err()
        );
    }
}
