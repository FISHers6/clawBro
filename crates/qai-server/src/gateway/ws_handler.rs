use crate::state::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::IntoResponse,
};
use qai_protocol::{AgentEvent, InboundMsg, SessionKey};
use serde::Deserialize;
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

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let (private_tx, mut private_rx) = mpsc::unbounded_channel::<AgentEvent>();

    loop {
        tokio::select! {
            // 接收客户端消息
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<WsClientMsg>(&text) {
                            Ok(WsClientMsg::Subscribe { session_key }) => {
                                state.registry.ws_subs
                                    .entry(session_key)
                                    .or_default()
                                    .push(private_tx.clone());
                                tracing::debug!("WS client subscribed to session");
                            }
                            Ok(WsClientMsg::Unsubscribe { session_key }) => {
                                // Best-effort: prune dead/closed senders from this session's subscriber list.
                                // Full unsubscribe by sender ID requires a subscription token (future work).
                                state.registry.ws_subs.alter(&session_key, |_, mut vec| {
                                    // A sender is "alive" if it can receive (closed channel = dead)
                                    vec.retain(|tx| !tx.is_closed());
                                    vec
                                });
                                tracing::debug!("WS client unsubscribed from {:?}", session_key);
                            }
                            Ok(WsClientMsg::Message(inbound)) => {
                                let registry = state.registry.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = registry.handle(inbound).await {
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
}
