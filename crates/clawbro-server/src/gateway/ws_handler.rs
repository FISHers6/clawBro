use crate::state::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::IntoResponse,
};
use crate::agent_core::TurnExecutionContext;
use crate::protocol::{normalize_conversation_identity, AgentEvent, InboundMsg, SessionKey};
use crate::runtime::ApprovalDecision;
use serde::Deserialize;
use std::collections::HashMap;
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
    let normalized = normalize_conversation_identity(session_key);
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
    // Maps session_key → per-subscription sender. Dropping the sender closes
    // the per-subscription channel and removes this WS connection from ws_subs
    // at the next delivery pass.
    let mut local_subscriptions = HashMap::<SessionKey, mpsc::UnboundedSender<AgentEvent>>::new();

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
                                let normalized = normalize_conversation_identity(&session_key);
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
                                // A socket sending a turn should also receive the resulting runtime
                                // events for that same session without requiring a separate explicit
                                // Subscribe round-trip first.
                                ensure_subscription(
                                    &state,
                                    &private_tx,
                                    &mut local_subscriptions,
                                    &inbound.session_key,
                                );
                                let registry = state.registry.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = registry
                                        .handle_with_context(inbound, TurnExecutionContext::default())
                                        .await
                                    {
                                        tracing::error!("Registry handle error: {e}");
                                    }
                                });
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
            else => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
