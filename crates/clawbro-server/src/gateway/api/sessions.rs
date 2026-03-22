use crate::protocol::AgentEvent;
use crate::session::{
    key::key_to_session_id, SessionMeta, SessionStatus, StoredMessage, StoredSessionEvent,
};
use crate::state::AppState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::types::{ApiErrorBody, ApiListResponse, ApiSessionQuery};

#[derive(Debug, Clone, Serialize)]
pub struct SessionMetaView {
    pub session_id: String,
    pub channel: String,
    pub scope: String,
    pub channel_instance: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: usize,
    pub status: String,
    pub backend_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_since: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredMessageView {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub sender: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredSessionEventView {
    pub timestamp: DateTime<Utc>,
    pub event_type: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionFilterQuery {
    pub channel: Option<String>,
    pub scope: Option<String>,
    #[serde(default)]
    pub channel_instance: Option<String>,
}

pub async fn list_sessions(
    Query(query): Query<SessionFilterQuery>,
    State(state): State<AppState>,
) -> Result<Json<ApiListResponse<SessionMetaView>>, (StatusCode, Json<ApiErrorBody>)> {
    let has_channel = query.channel.is_some();
    let has_scope = query.scope.is_some();
    let has_instance = query.channel_instance.is_some();
    if (has_channel || has_scope || has_instance) && !(has_channel && has_scope) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody {
                error: "session filters require both 'channel' and 'scope'; 'channel_instance' is only valid with both".to_string(),
            }),
        ));
    }

    let mut items: Vec<_> = state
        .registry
        .session_manager_ref()
        .list_metas()
        .await
        .map_err(internal_error)?
        .into_iter()
        .map(session_meta_view)
        .collect();

    if let (Some(channel), Some(scope)) = (query.channel.as_deref(), query.scope.as_deref()) {
        items.retain(|item| {
            item.channel == channel
                && item.scope == scope
                && query.channel_instance.as_ref().is_none_or(|instance| {
                    item.channel_instance.as_deref() == Some(instance.as_str())
                })
        });
    }

    Ok(Json(ApiListResponse { items }))
}

pub async fn get_session(
    Query(query): Query<ApiSessionQuery>,
    State(state): State<AppState>,
) -> Result<Json<SessionMetaView>, (StatusCode, Json<ApiErrorBody>)> {
    let session_key = query.to_session_key();
    let session_id = key_to_session_id(&session_key);
    state
        .registry
        .session_manager_ref()
        .load_meta(session_id)
        .await
        .map_err(internal_error)?
        .map(session_meta_view)
        .map(Json)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiErrorBody {
                    error: format!(
                        "session not found for channel='{}' scope='{}' instance='{}'",
                        query.channel,
                        query.scope,
                        query.channel_instance.as_deref().unwrap_or("")
                    ),
                }),
            )
        })
}

pub async fn get_session_messages(
    Query(query): Query<ApiSessionQuery>,
    State(state): State<AppState>,
) -> Result<Json<ApiListResponse<StoredMessageView>>, (StatusCode, Json<ApiErrorBody>)> {
    let session_key = query.to_session_key();
    let session_id = key_to_session_id(&session_key);
    let manager = state.registry.session_manager_ref();
    let messages = manager
        .storage()
        .load_messages(session_id)
        .await
        .map_err(internal_error)?;
    if messages.is_empty()
        && manager
            .load_meta(session_id)
            .await
            .map_err(internal_error)?
            .is_none()
    {
        return Err(not_found(&query));
    }

    let messages = messages.into_iter().map(stored_message_view).collect();

    Ok(Json(ApiListResponse { items: messages }))
}

pub async fn get_session_events(
    Query(query): Query<ApiSessionQuery>,
    State(state): State<AppState>,
) -> Result<Json<ApiListResponse<StoredSessionEventView>>, (StatusCode, Json<ApiErrorBody>)> {
    let session_key = query.to_session_key();
    let session_id = key_to_session_id(&session_key);
    let manager = state.registry.session_manager_ref();
    let events = manager
        .storage()
        .load_events(session_id)
        .await
        .map_err(internal_error)?;
    if events.is_empty()
        && manager
            .load_meta(session_id)
            .await
            .map_err(internal_error)?
            .is_none()
    {
        return Err(not_found(&query));
    }

    let events = events.into_iter().map(stored_session_event_view).collect();

    Ok(Json(ApiListResponse { items: events }))
}

fn session_meta_view(meta: SessionMeta) -> SessionMetaView {
    let (status, backend_id, running_since) = match meta.session_status {
        SessionStatus::Idle => ("idle".to_string(), None, None),
        SessionStatus::Running {
            backend_id,
            started_at,
        } => ("running".to_string(), Some(backend_id), Some(started_at)),
    };
    SessionMetaView {
        session_id: meta.session_id.to_string(),
        channel: meta.channel,
        scope: meta.scope,
        channel_instance: meta.channel_instance,
        created_at: meta.created_at,
        updated_at: meta.updated_at,
        message_count: meta.message_count,
        status,
        backend_id,
        running_since,
    }
}

fn stored_message_view(message: StoredMessage) -> StoredMessageView {
    StoredMessageView {
        id: message.id.to_string(),
        role: message.role,
        content: message.content,
        timestamp: message.timestamp,
        sender: message.sender,
    }
}

fn stored_session_event_view(event: StoredSessionEvent) -> StoredSessionEventView {
    let (event_type, payload) = stable_event_projection(&event.event);
    StoredSessionEventView {
        timestamp: event.timestamp,
        event_type,
        payload,
    }
}

fn stable_event_projection(event: &AgentEvent) -> (String, serde_json::Value) {
    use serde_json::json;

    match event {
        AgentEvent::TextDelta { session_id, delta } => (
            "text_delta".to_string(),
            json!({
                "session_id": session_id,
                "delta": delta,
            }),
        ),
        AgentEvent::ApprovalRequest {
            session_id,
            session_key,
            approval_id,
            prompt,
            command,
            cwd,
            host,
            agent_id,
            expires_at_ms,
        } => (
            "approval_request".to_string(),
            json!({
                "session_id": session_id,
                "session_key": session_key,
                "approval_id": approval_id,
                "prompt": prompt,
                "command": command,
                "cwd": cwd,
                "host": host,
                "agent_id": agent_id,
                "expires_at_ms": expires_at_ms,
            }),
        ),
        AgentEvent::ToolCallStart {
            session_id,
            tool_name,
            call_id,
        } => (
            "tool_call_start".to_string(),
            json!({
                "session_id": session_id,
                "tool_name": tool_name,
                "call_id": call_id,
            }),
        ),
        AgentEvent::ToolCallResult {
            session_id,
            call_id,
            result,
        } => (
            "tool_call_result".to_string(),
            json!({
                "session_id": session_id,
                "call_id": call_id,
                "result": result,
            }),
        ),
        AgentEvent::ToolCallFailed {
            session_id,
            tool_name,
            call_id,
            error,
        } => (
            "tool_call_failed".to_string(),
            json!({
                "session_id": session_id,
                "tool_name": tool_name,
                "call_id": call_id,
                "error": error,
            }),
        ),
        AgentEvent::Thinking { session_id } => (
            "thinking".to_string(),
            json!({
                "session_id": session_id,
            }),
        ),
        AgentEvent::TurnComplete {
            session_id,
            full_text,
            sender,
        } => (
            "turn_complete".to_string(),
            json!({
                "session_id": session_id,
                "full_text": full_text,
                "sender": sender,
            }),
        ),
        AgentEvent::Error {
            session_id,
            message,
        } => (
            "error".to_string(),
            json!({
                "session_id": session_id,
                "message": message,
            }),
        ),
    }
}

fn internal_error(err: anyhow::Error) -> (StatusCode, Json<ApiErrorBody>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiErrorBody {
            error: err.to_string(),
        }),
    )
}

fn not_found(query: &ApiSessionQuery) -> (StatusCode, Json<ApiErrorBody>) {
    (
        StatusCode::NOT_FOUND,
        Json(ApiErrorBody {
            error: format!(
                "session not found for channel='{}' scope='{}' instance='{}'",
                query.channel,
                query.scope,
                query.channel_instance.as_deref().unwrap_or("")
            ),
        }),
    )
}
