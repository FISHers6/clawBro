use crate::{
    channels_internal::ws_virtual::WsVirtualChannel,
    config::ProgressPresentationMode,
    gateway::api::types::ApiErrorBody,
    im_sink::spawn_im_turn,
    protocol::{InboundMsg, MsgContent, MsgSource, SessionKey},
    state::AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct ChatSendBody {
    pub message: String,
    pub scope: Option<String>,
    pub agent: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ChatSendResponse {
    pub turn_id: String,
    pub session_key: SessionKey,
}

pub async fn chat_send(
    State(state): State<AppState>,
    Json(body): Json<ChatSendBody>,
) -> Result<Json<ChatSendResponse>, (StatusCode, Json<ApiErrorBody>)> {
    let message = body.message.trim().to_string();
    if message.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody {
                error: "message must not be empty".to_string(),
            }),
        ));
    }

    let scope = body
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("main")
        .to_string();

    let session_key = SessionKey::new("ws", &scope);
    let turn_id = uuid::Uuid::new_v4().to_string();

    let inbound = InboundMsg {
        id: turn_id.clone(),
        session_key: session_key.clone(),
        content: MsgContent::text(&message),
        sender: "web".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: body.agent,
        source: MsgSource::Human,
    };

    spawn_im_turn(
        state.registry.clone(),
        Arc::new(WsVirtualChannel),
        state.channel_registry.clone(),
        state.cfg.clone(),
        inbound,
        ProgressPresentationMode::FinalOnly,
    );

    Ok(Json(ChatSendResponse {
        turn_id,
        session_key,
    }))
}

#[cfg(test)]
mod tests {
    /// Returns the resolved scope string, applying the same defaulting logic as `chat_send`.
    fn resolve_scope(scope: Option<&str>) -> String {
        scope
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("main")
            .to_string()
    }

    #[test]
    fn default_scope_is_main() {
        assert_eq!(resolve_scope(None), "main");
    }

    #[test]
    fn custom_scope_passes_through() {
        assert_eq!(resolve_scope(Some("  group:abc  ")), "group:abc");
    }

    #[test]
    fn empty_scope_string_falls_back_to_main() {
        assert_eq!(resolve_scope(Some("   ")), "main");
    }
}
