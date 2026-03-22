use crate::runtime::{ApprovalDecision, PermissionRequest};
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;

use super::types::{ApiErrorBody, ApiListResponse};

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalView {
    pub approval_id: String,
    pub prompt: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub host: Option<String>,
    pub agent_id: Option<String>,
    pub expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalResolutionView {
    pub approval_id: String,
    pub decision: String,
    pub resolved: bool,
}

pub async fn list_approvals(State(state): State<AppState>) -> Json<ApiListResponse<ApprovalView>> {
    Json(ApiListResponse {
        items: state
            .approvals
            .pending_requests()
            .into_iter()
            .map(approval_view)
            .collect(),
    })
}

pub async fn get_approval(
    Path(approval_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ApprovalView>, (StatusCode, Json<ApiErrorBody>)> {
    state
        .approvals
        .get_request(&approval_id)
        .map(approval_view)
        .map(Json)
        .ok_or_else(|| not_found(approval_id))
}

pub async fn approve_approval(
    Path(approval_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ApprovalResolutionView>, (StatusCode, Json<ApiErrorBody>)> {
    resolve(&state, approval_id, ApprovalDecision::AllowOnce).map(Json)
}

pub async fn deny_approval(
    Path(approval_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ApprovalResolutionView>, (StatusCode, Json<ApiErrorBody>)> {
    resolve(&state, approval_id, ApprovalDecision::Deny).map(Json)
}

fn resolve(
    state: &AppState,
    approval_id: String,
    decision: ApprovalDecision,
) -> Result<ApprovalResolutionView, (StatusCode, Json<ApiErrorBody>)> {
    let resolved = state.approvals.resolve(&approval_id, decision);
    if !resolved {
        return Err(not_found(approval_id));
    }
    Ok(ApprovalResolutionView {
        approval_id,
        decision: match decision {
            ApprovalDecision::AllowOnce => "allow-once",
            ApprovalDecision::AllowAlways => "allow-always",
            ApprovalDecision::Deny => "deny",
        }
        .to_string(),
        resolved,
    })
}

fn approval_view(request: PermissionRequest) -> ApprovalView {
    ApprovalView {
        approval_id: request.id,
        prompt: request.prompt,
        command: request.command,
        cwd: request.cwd,
        host: request.host,
        agent_id: request.agent_id,
        expires_at_ms: request.expires_at_ms,
    }
}

fn not_found(approval_id: String) -> (StatusCode, Json<ApiErrorBody>) {
    (
        StatusCode::NOT_FOUND,
        Json(ApiErrorBody {
            error: format!("approval '{}' not found or already resolved", approval_id),
        }),
    )
}
