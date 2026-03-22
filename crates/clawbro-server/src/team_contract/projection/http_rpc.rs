use axum::http::StatusCode;

use crate::runtime::{TeamToolRequest, TeamToolResponse};
use crate::state::AppState;

pub async fn invoke_team_http_request(
    state: &AppState,
    provided_token: &str,
    request: TeamToolRequest,
) -> (StatusCode, TeamToolResponse) {
    if provided_token != *state.runtime_token {
        return (
            StatusCode::UNAUTHORIZED,
            TeamToolResponse {
                ok: false,
                message: "invalid runtime token".to_string(),
                payload: None,
            },
        );
    }

    match state
        .registry
        .invoke_team_tool(&request.session_key, request.call)
        .await
    {
        Ok(resp) => (StatusCode::OK, resp),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            TeamToolResponse {
                ok: false,
                message: err.to_string(),
                payload: None,
            },
        ),
    }
}
