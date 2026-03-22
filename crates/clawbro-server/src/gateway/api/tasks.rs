use crate::agent_core::team::{
    orchestrator::TeamOrchestrator, registry::Task, session::TaskArtifactMeta,
};
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use std::path::Path as StdPath;
use std::sync::Arc;

use super::types::{ApiErrorBody, ApiListResponse};

#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    pub team_id: String,
    pub id: String,
    pub title: String,
    pub status_raw: String,
    pub assignee_hint: Option<String>,
    pub retry_count: i32,
    pub timeout_secs: i32,
    pub spec: Option<String>,
    pub success_criteria: Option<String>,
    pub completion_note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskDetailView {
    #[serde(flatten)]
    pub task: TaskView,
    pub artifacts: Vec<TaskArtifactView>,
    pub artifact_meta: Option<TaskArtifactMeta>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskArtifactView {
    pub name: String,
    pub file_name: String,
    pub path: String,
    pub present: bool,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskArtifactContentView {
    pub team_id: String,
    pub task_id: String,
    pub artifact: TaskArtifactView,
    pub content_type: String,
    pub content: String,
}

const KNOWN_TASK_ARTIFACTS: [(&str, &str); 6] = [
    ("meta", "meta.json"),
    ("spec", "spec.md"),
    ("plan", "plan.md"),
    ("progress", "progress.md"),
    ("result", "result.md"),
    ("review-feedback", "review-feedback.md"),
];

pub async fn list_tasks(State(state): State<AppState>) -> Json<ApiListResponse<TaskView>> {
    Json(ApiListResponse {
        items: collect_all_tasks(&state),
    })
}

pub async fn get_task(
    Path(task_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<TaskDetailView>, (StatusCode, Json<ApiErrorBody>)> {
    // Collect all matches first before doing any disk IO, so we can return
    // 409 CONFLICT without having wasted a build_task_detail() call.
    let mut matches: Vec<(String, Task)> = Vec::new();

    for summary in state.registry.team_summaries() {
        let Some(orchestrator) = state.registry.get_team_orchestrator(&summary.team_id) else {
            continue;
        };
        if let Some(task) = orchestrator
            .registry
            .get_task(&task_id)
            .map_err(internal_error)?
        {
            matches.push((summary.team_id.clone(), task));
            if matches.len() > 1 {
                return Err((
                    StatusCode::CONFLICT,
                    Json(ApiErrorBody {
                        error: format!(
                            "task '{}' exists in multiple teams; use /api/teams/{{team_id}}/tasks/{}",
                            task_id, task_id
                        ),
                    }),
                ));
            }
        }
    }

    match matches.into_iter().next() {
        Some((team_id, task)) => {
            let orchestrator = state
                .registry
                .get_team_orchestrator(&team_id)
                .ok_or_else(|| {
                    (
                        StatusCode::NOT_FOUND,
                        Json(ApiErrorBody {
                            error: "team no longer available".to_string(),
                        }),
                    )
                })?;
            let detail = build_task_detail(&orchestrator.session, &team_id, task)?;
            Ok(Json(detail))
        }
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody {
                error: format!("task '{}' not found", task_id),
            }),
        )),
    }
}

pub async fn list_team_tasks(
    Path(team_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ApiListResponse<TaskView>>, (StatusCode, Json<ApiErrorBody>)> {
    let orchestrator = get_team_orchestrator(&state, &team_id)?;

    let tasks = orchestrator
        .registry
        .list_all()
        .map_err(internal_error)?
        .into_iter()
        .map(|task| task_view(&team_id, task))
        .collect();
    Ok(Json(ApiListResponse { items: tasks }))
}

pub async fn get_team_task(
    Path((team_id, task_id)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<Json<TaskDetailView>, (StatusCode, Json<ApiErrorBody>)> {
    let orchestrator = get_team_orchestrator(&state, &team_id)?;
    let task = orchestrator
        .registry
        .get_task(&task_id)
        .map_err(internal_error)?
        .ok_or_else(|| not_found("task", &task_id))?;
    let detail = build_task_detail(&orchestrator.session, &team_id, task)?;
    Ok(Json(detail))
}

pub async fn list_team_task_artifacts(
    Path((team_id, task_id)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<Json<ApiListResponse<TaskArtifactView>>, (StatusCode, Json<ApiErrorBody>)> {
    let orchestrator = get_team_orchestrator(&state, &team_id)?;
    ensure_task_exists(&orchestrator, &task_id)?;
    Ok(Json(ApiListResponse {
        items: build_artifact_views(&orchestrator.session, &task_id),
    }))
}

pub async fn get_team_task_artifact(
    Path((team_id, task_id, artifact_name)): Path<(String, String, String)>,
    State(state): State<AppState>,
) -> Result<Json<TaskArtifactContentView>, (StatusCode, Json<ApiErrorBody>)> {
    let orchestrator = get_team_orchestrator(&state, &team_id)?;
    ensure_task_exists(&orchestrator, &task_id)?;
    let (artifact_key, file_name) =
        known_task_artifact(&artifact_name).ok_or_else(|| not_found("artifact", &artifact_name))?;
    let content = orchestrator
        .session
        .read_task_artifact(&task_id, file_name)
        .map_err(internal_error)?
        .ok_or_else(|| not_found("artifact", artifact_key))?;
    // Build this artifact's view directly — avoids scanning all 6 artifacts and
    // prevents content/present inconsistency if the file is deleted between two reads.
    let path = orchestrator.session.task_dir(&task_id).join(file_name);
    let metadata = std::fs::metadata(&path).ok().filter(|m| m.is_file());
    let artifact = TaskArtifactView {
        name: artifact_key.to_string(),
        file_name: file_name.to_string(),
        path: format!("./tasks/{task_id}/{file_name}"),
        present: metadata.is_some(),
        size_bytes: metadata.map(|m| m.len()),
    };
    Ok(Json(TaskArtifactContentView {
        team_id,
        task_id,
        artifact,
        content_type: artifact_content_type(file_name).to_string(),
        content,
    }))
}

fn collect_all_tasks(state: &AppState) -> Vec<TaskView> {
    let mut tasks = Vec::new();
    for summary in state.registry.team_summaries() {
        if let Some(orchestrator) = state.registry.get_team_orchestrator(&summary.team_id) {
            if let Ok(team_tasks) = orchestrator.registry.list_all() {
                tasks.extend(
                    team_tasks
                        .into_iter()
                        .map(|task| task_view(&summary.team_id, task)),
                );
            }
        }
    }
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    tasks
}

fn task_view(team_id: &str, task: Task) -> TaskView {
    TaskView {
        team_id: team_id.to_string(),
        id: task.id,
        title: task.title,
        status_raw: task.status_raw,
        assignee_hint: task.assignee_hint,
        retry_count: task.retry_count,
        timeout_secs: task.timeout_secs,
        spec: task.spec,
        success_criteria: task.success_criteria,
        completion_note: task.completion_note,
    }
}

fn build_task_detail(
    session: &crate::agent_core::team::session::TeamSession,
    team_id: &str,
    task: Task,
) -> Result<TaskDetailView, (StatusCode, Json<ApiErrorBody>)> {
    let task_id = task.id.clone();
    let artifact_meta = session.read_task_meta(&task_id).map_err(internal_error)?;
    Ok(TaskDetailView {
        task: task_view(team_id, task),
        artifacts: build_artifact_views(session, &task_id),
        artifact_meta,
    })
}

fn build_artifact_views(
    session: &crate::agent_core::team::session::TeamSession,
    task_id: &str,
) -> Vec<TaskArtifactView> {
    KNOWN_TASK_ARTIFACTS
        .iter()
        .map(|(name, file_name)| {
            let path = session.task_dir(task_id).join(file_name);
            let metadata = std::fs::metadata(&path)
                .ok()
                .filter(|entry| entry.is_file());
            TaskArtifactView {
                name: (*name).to_string(),
                file_name: (*file_name).to_string(),
                path: format!("./tasks/{task_id}/{file_name}"),
                present: metadata.is_some(),
                size_bytes: metadata.map(|entry| entry.len()),
            }
        })
        .collect()
}

fn known_task_artifact(name: &str) -> Option<(&'static str, &'static str)> {
    KNOWN_TASK_ARTIFACTS
        .iter()
        .copied()
        .find(|(key, _)| *key == name)
}

fn artifact_content_type(file_name: &str) -> &'static str {
    match StdPath::new(file_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
    {
        "json" => "application/json",
        _ => "text/markdown",
    }
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

fn ensure_task_exists(
    orchestrator: &crate::agent_core::team::orchestrator::TeamOrchestrator,
    task_id: &str,
) -> Result<(), (StatusCode, Json<ApiErrorBody>)> {
    let exists = orchestrator
        .registry
        .get_task(task_id)
        .map_err(internal_error)?
        .is_some();
    if exists {
        Ok(())
    } else {
        Err(not_found("task", task_id))
    }
}

fn not_found(kind: &str, id: &str) -> (StatusCode, Json<ApiErrorBody>) {
    (
        StatusCode::NOT_FOUND,
        Json(ApiErrorBody {
            error: format!("{kind} '{}' not found", id),
        }),
    )
}

fn internal_error(err: anyhow::Error) -> (StatusCode, Json<ApiErrorBody>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiErrorBody {
            error: err.to_string(),
        }),
    )
}
