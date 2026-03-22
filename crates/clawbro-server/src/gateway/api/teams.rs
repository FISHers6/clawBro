use crate::agent_core::team::orchestrator::{
    TeamArtifactHealthSummary, TeamRoutingStats, TeamRuntimeSummary, TeamState, TeamTaskCounts,
};
use crate::agent_core::team::session::{ChannelSendRecord, LeaderUpdateRecord};
use crate::agent_core::team::{
    completion_routing::{PendingRoutingRecord, TeamRoutingEnvelope},
    orchestrator::TeamOrchestrator,
};
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use std::sync::Arc;

use super::types::{ApiErrorBody, ApiListResponse};

const KNOWN_TEAM_ARTIFACTS: [(&str, &str); 5] = [
    ("team", "TEAM.md"),
    ("agents", "AGENTS.md"),
    ("tasks", "TASKS.md"),
    ("context", "CONTEXT.md"),
    ("heartbeat", "HEARTBEAT.md"),
];

#[derive(Debug, Clone, Serialize)]
pub struct TeamApiView {
    pub team_id: String,
    pub state: TeamState,
    pub scope: Option<String>,
    pub channel: Option<String>,
    pub channel_instance: Option<String>,
    pub lead_agent_name: Option<String>,
    pub specialists: Vec<String>,
    pub latest_leader_update: Option<LeaderUpdateRecord>,
    pub latest_channel_send: Option<ChannelSendRecord>,
    pub tool_surface_ready: bool,
    pub task_counts: TeamTaskCounts,
    pub artifact_health: TeamArtifactHealthSummary,
    pub routing_stats: TeamRoutingStats,
    pub healthy: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TeamArtifactView {
    pub name: String,
    pub file_name: String,
    pub path: String,
    pub present: bool,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TeamArtifactContentView {
    pub team_id: String,
    pub artifact: TeamArtifactView,
    pub content_type: String,
    pub content: String,
}

pub async fn list_teams(State(state): State<AppState>) -> Json<ApiListResponse<TeamApiView>> {
    Json(ApiListResponse {
        items: team_views(&state),
    })
}

pub async fn get_team(
    Path(team_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<TeamApiView>, (StatusCode, Json<ApiErrorBody>)> {
    // Use get_team_orchestrator as the authoritative 404 source — consistent
    // with all sub-routes (leader-updates, channel-sends, artifacts, etc.).
    let _ = get_team_orchestrator(&state, &team_id)?;

    let summary = state
        .registry
        .team_summaries()
        .into_iter()
        .find(|s| s.team_id == team_id)
        .ok_or_else(|| not_found("team", &team_id))?;

    let diagnostics = crate::diagnostics::collect_team_diagnostics(&state);
    let diagnostic = diagnostics.iter().find(|d| d.team_id == team_id);
    Ok(Json(team_view(summary, diagnostic)))
}

pub async fn list_team_leader_updates(
    Path(team_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ApiListResponse<LeaderUpdateRecord>>, (StatusCode, Json<ApiErrorBody>)> {
    let orchestrator = get_team_orchestrator(&state, &team_id)?;
    let items = orchestrator
        .session
        .load_leader_updates()
        .map_err(internal_error)?;
    Ok(Json(ApiListResponse { items }))
}

pub async fn list_team_channel_sends(
    Path(team_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ApiListResponse<ChannelSendRecord>>, (StatusCode, Json<ApiErrorBody>)> {
    let orchestrator = get_team_orchestrator(&state, &team_id)?;
    let items = orchestrator
        .session
        .load_channel_sends()
        .map_err(internal_error)?;
    Ok(Json(ApiListResponse { items }))
}

pub async fn list_team_routing_events(
    Path(team_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ApiListResponse<TeamRoutingEnvelope>>, (StatusCode, Json<ApiErrorBody>)> {
    let orchestrator = get_team_orchestrator(&state, &team_id)?;
    let items = orchestrator
        .session
        .load_routing_outcomes()
        .map_err(internal_error)?;
    Ok(Json(ApiListResponse { items }))
}

pub async fn list_team_pending_completions(
    Path(team_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ApiListResponse<PendingRoutingRecord>>, (StatusCode, Json<ApiErrorBody>)> {
    let orchestrator = get_team_orchestrator(&state, &team_id)?;
    let items = orchestrator
        .session
        .load_pending_completions()
        .map_err(internal_error)?;
    Ok(Json(ApiListResponse { items }))
}

pub async fn list_team_artifacts(
    Path(team_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ApiListResponse<TeamArtifactView>>, (StatusCode, Json<ApiErrorBody>)> {
    let orchestrator = get_team_orchestrator(&state, &team_id)?;
    Ok(Json(ApiListResponse {
        items: build_team_artifact_views(&orchestrator),
    }))
}

pub async fn get_team_artifact(
    Path((team_id, artifact_name)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<Json<TeamArtifactContentView>, (StatusCode, Json<ApiErrorBody>)> {
    let orchestrator = get_team_orchestrator(&state, &team_id)?;
    let (artifact_key, file_name) =
        known_team_artifact(&artifact_name).ok_or_else(|| not_found("artifact", &artifact_name))?;
    let content = orchestrator
        .session
        .read_root_artifact(file_name)
        .map_err(internal_error)?
        .ok_or_else(|| not_found("artifact", artifact_key))?;
    // Build only this artifact's view directly instead of building all 5 and finding 1.
    let path = orchestrator.session.dir.join(file_name);
    let metadata = std::fs::metadata(&path).ok().filter(|m| m.is_file());
    let artifact = TeamArtifactView {
        name: artifact_key.to_string(),
        file_name: file_name.to_string(),
        path: format!("./{file_name}"),
        present: metadata.is_some(),
        size_bytes: metadata.map(|m| m.len()),
    };
    Ok(Json(TeamArtifactContentView {
        team_id,
        artifact,
        content_type: "text/markdown".to_string(),
        content,
    }))
}

fn team_views(state: &AppState) -> Vec<TeamApiView> {
    let diagnostics = crate::diagnostics::collect_team_diagnostics(state);
    let diagnostic_map: std::collections::HashMap<_, _> = diagnostics
        .into_iter()
        .map(|entry| (entry.team_id.clone(), entry))
        .collect();
    let mut views: Vec<_> = state
        .registry
        .team_summaries()
        .into_iter()
        .map(|summary| {
            let diagnostic = diagnostic_map.get(&summary.team_id);
            team_view(summary, diagnostic)
        })
        .collect();
    views.sort_by(|a, b| a.team_id.cmp(&b.team_id));
    views
}

fn team_view(
    summary: TeamRuntimeSummary,
    diagnostic: Option<&crate::diagnostics::TeamDiagnostic>,
) -> TeamApiView {
    TeamApiView {
        team_id: summary.team_id,
        state: summary.state,
        scope: summary
            .lead_session_key
            .as_ref()
            .map(|session_key| session_key.scope.clone()),
        channel: summary
            .lead_session_key
            .as_ref()
            .map(|session_key| session_key.channel.clone()),
        channel_instance: summary
            .lead_session_key
            .as_ref()
            .and_then(|session_key| session_key.channel_instance.clone()),
        lead_agent_name: summary.lead_agent_name,
        specialists: summary.specialists,
        latest_leader_update: summary.latest_leader_update,
        latest_channel_send: summary.latest_channel_send,
        tool_surface_ready: summary.tool_surface_ready,
        task_counts: summary.task_counts,
        artifact_health: summary.artifact_health,
        routing_stats: summary.routing_stats,
        healthy: diagnostic.map(|entry| entry.healthy).unwrap_or(false),
        notes: diagnostic
            .map(|entry| entry.notes.clone())
            .unwrap_or_default(),
    }
}

fn build_team_artifact_views(orchestrator: &TeamOrchestrator) -> Vec<TeamArtifactView> {
    KNOWN_TEAM_ARTIFACTS
        .iter()
        .map(|(name, file_name)| {
            let path = orchestrator.session.dir.join(file_name);
            let metadata = std::fs::metadata(&path)
                .ok()
                .filter(|entry| entry.is_file());
            TeamArtifactView {
                name: (*name).to_string(),
                file_name: (*file_name).to_string(),
                path: format!("./{file_name}"),
                present: metadata.is_some(),
                size_bytes: metadata.map(|entry| entry.len()),
            }
        })
        .collect()
}

fn known_team_artifact(name: &str) -> Option<(&'static str, &'static str)> {
    KNOWN_TEAM_ARTIFACTS
        .iter()
        .copied()
        .find(|(key, _)| *key == name)
}

fn get_team_orchestrator(
    state: &AppState,
    team_id: &str,
) -> Result<Arc<TeamOrchestrator>, (StatusCode, Json<ApiErrorBody>)> {
    state
        .registry
        .get_team_orchestrator(team_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiErrorBody {
                    error: format!("team '{}' not found", team_id),
                }),
            )
        })
}

fn internal_error(err: anyhow::Error) -> (StatusCode, Json<ApiErrorBody>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiErrorBody {
            error: err.to_string(),
        }),
    )
}

fn not_found(kind: &str, id: &str) -> (StatusCode, Json<ApiErrorBody>) {
    (
        StatusCode::NOT_FOUND,
        Json(ApiErrorBody {
            error: format!("{kind} '{}' not found", id),
        }),
    )
}
