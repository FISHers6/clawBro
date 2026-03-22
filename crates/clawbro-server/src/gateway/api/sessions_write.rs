use crate::protocol::SessionKey;
use crate::session::key_to_session_id;
use crate::state::AppState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

use super::types::ApiErrorBody;

/// Query parameters to locate a session.
#[derive(Debug, Deserialize)]
pub struct SessionLocator {
    pub channel: String,
    pub scope: String,
    #[serde(default)]
    pub channel_instance: Option<String>,
}

impl SessionLocator {
    fn to_session_key(&self) -> SessionKey {
        match &self.channel_instance {
            Some(instance) => SessionKey::with_instance(&self.channel, instance, &self.scope),
            None => SessionKey::new(&self.channel, &self.scope),
        }
    }
}

/// Response body for a successful DELETE /api/sessions.
#[derive(Debug, Serialize)]
pub struct SessionDeleteResponse {
    pub ok: bool,
    pub session_key: SessionKey,
}

/// DELETE /api/sessions?channel=ws&scope=main
///
/// Clears conversation history for a session (messages + backend session IDs).
/// Returns 404 if the session does not exist, 200 on success.
pub async fn delete_session_history(
    Query(q): Query<SessionLocator>,
    State(state): State<AppState>,
) -> Result<Json<SessionDeleteResponse>, (StatusCode, Json<ApiErrorBody>)> {
    let session_key = q.to_session_key();
    let session_id = key_to_session_id(&session_key);
    let manager = state.registry.session_manager_ref();

    // Check session existence first (load_meta is a read-only probe).
    let exists = manager
        .load_meta(session_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiErrorBody {
                    error: e.to_string(),
                }),
            )
        })?
        .is_some();

    if !exists {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody {
                error: format!(
                    "session not found for channel='{}' scope='{}' instance='{}'",
                    q.channel,
                    q.scope,
                    q.channel_instance.as_deref().unwrap_or("")
                ),
            }),
        ));
    }

    manager.reset_conversation(session_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorBody {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(SessionDeleteResponse {
        ok: true,
        session_key,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_locator_with_instance_builds_correct_key() {
        let locator = SessionLocator {
            channel: "lark".to_string(),
            scope: "group:abc".to_string(),
            channel_instance: Some("beta".to_string()),
        };
        let key = locator.to_session_key();
        assert_eq!(key.channel, "lark");
        assert_eq!(key.scope, "group:abc");
        assert_eq!(key.channel_instance.as_deref(), Some("beta"));
    }

    #[test]
    fn session_locator_without_instance_builds_simple_key() {
        let locator = SessionLocator {
            channel: "ws".to_string(),
            scope: "main".to_string(),
            channel_instance: None,
        };
        let key = locator.to_session_key();
        assert_eq!(key.channel, "ws");
        assert_eq!(key.scope, "main");
        assert!(key.channel_instance.is_none());
    }
}
