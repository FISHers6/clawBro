use crate::agent_core::roster::AgentEntry;
use crate::config::GatewayConfig;
use crate::runtime::BackendSpec;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;

use super::types::{derive_agent_identities, ApiErrorBody, ApiListResponse};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentApiView {
    pub name: String,
    pub mentions: Vec<String>,
    pub backend_id: String,
    pub role: String,
    pub identities: Vec<String>,
    pub persona_dir_configured: bool,
    pub workspace_dir_configured: bool,
    pub extra_skills_dir_count: usize,
    pub effective_mcp: Vec<String>,
}

pub async fn list_agents(State(state): State<AppState>) -> Json<ApiListResponse<AgentApiView>> {
    Json(ApiListResponse {
        items: agent_views(state.cfg.as_ref(), &state)
            .await
            .into_iter()
            .collect(),
    })
}

pub async fn get_agent(
    Path(name): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<AgentApiView>, (StatusCode, Json<ApiErrorBody>)> {
    let Some(roster) = state.registry.roster.as_ref() else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody {
                error: "agent roster not configured".to_string(),
            }),
        ));
    };

    let entry = roster.find_by_name(&name).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody {
                error: format!("agent '{}' not found", name),
            }),
        )
    })?;

    let backend_spec = state
        .runtime_registry
        .backend_spec(entry.runtime_backend_id())
        .await;
    Ok(Json(build_agent_view(
        state.cfg.as_ref(),
        entry,
        backend_spec.as_ref(),
    )))
}

async fn agent_views(cfg: &GatewayConfig, state: &AppState) -> Vec<AgentApiView> {
    let Some(roster) = state.registry.roster.as_ref() else {
        return Vec::new();
    };

    let specs = state.runtime_registry.all_backend_specs().await;
    let mut spec_map = std::collections::HashMap::new();
    for spec in specs {
        spec_map.insert(spec.backend_id.clone(), spec);
    }

    let mut agents: Vec<_> = roster
        .all_agents()
        .iter()
        .map(|entry| build_agent_view(cfg, entry, spec_map.get(entry.runtime_backend_id())))
        .collect();
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    agents
}

fn build_agent_view(
    cfg: &GatewayConfig,
    entry: &AgentEntry,
    backend_spec: Option<&BackendSpec>,
) -> AgentApiView {
    let identities = derive_agent_identities(cfg, &entry.name);
    let role = if identities.iter().any(|identity| identity == "front_bot") {
        "lead"
    } else if identities
        .iter()
        .any(|identity| identity == "roster_member")
    {
        "specialist"
    } else {
        "solo"
    };

    AgentApiView {
        name: entry.name.clone(),
        mentions: entry.mentions.clone(),
        backend_id: entry.backend_id.clone(),
        role: role.to_string(),
        identities,
        persona_dir_configured: entry.persona_dir.is_some(),
        workspace_dir_configured: entry.workspace_dir.is_some(),
        extra_skills_dir_count: entry.extra_skills_dirs.len(),
        effective_mcp: backend_spec
            .map(|spec| {
                spec.external_mcp_servers
                    .iter()
                    .map(|server| server.name.clone())
                    .collect()
            })
            .unwrap_or_default(),
    }
}
