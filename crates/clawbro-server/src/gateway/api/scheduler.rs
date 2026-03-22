use crate::scheduler::{ScheduledJob, ScheduledRun};
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;

use super::types::{ApiErrorBody, ApiListResponse};

pub async fn list_jobs(
    State(state): State<AppState>,
) -> Result<Json<ApiListResponse<ScheduledJob>>, (StatusCode, Json<ApiErrorBody>)> {
    let mut items = state
        .scheduler_service
        .list_jobs()
        .map_err(internal_error)?;
    items.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Json(ApiListResponse { items }))
}

pub async fn get_job(
    Path(job_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ScheduledJob>, (StatusCode, Json<ApiErrorBody>)> {
    state
        .scheduler_service
        .get_job_by_id(&job_id)
        .map_err(internal_error)?
        .map(Json)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiErrorBody {
                    error: format!("scheduler job '{}' not found", job_id),
                }),
            )
        })
}

pub async fn list_job_runs(
    Path(job_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ApiListResponse<ScheduledRun>>, (StatusCode, Json<ApiErrorBody>)> {
    // Query runs first; if non-empty the job must exist so we skip a DB round-trip.
    // Only on empty do we check existence to distinguish "no runs yet" from "job not found".
    let mut items = state
        .scheduler_service
        .list_run_history(Some(job_id.as_str()))
        .map_err(internal_error)?;

    if items.is_empty() {
        let exists = state
            .scheduler_service
            .get_job_by_id(&job_id)
            .map_err(internal_error)?
            .is_some();
        if !exists {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ApiErrorBody {
                    error: format!("scheduler job '{}' not found", job_id),
                }),
            ));
        }
    }

    items.sort_by(|a, b| a.scheduled_at.cmp(&b.scheduled_at));
    Ok(Json(ApiListResponse { items }))
}

pub async fn run_job_now(
    Path(job_id): Path<String>,
    State(state): State<AppState>,
) -> Result<StatusCode, (StatusCode, Json<ApiErrorBody>)> {
    let found = state
        .scheduler_service
        .request_run_now(&job_id, Utc::now())
        .map_err(internal_error)?;
    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody {
                error: format!("scheduler job '{}' not found", job_id),
            }),
        ))
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
