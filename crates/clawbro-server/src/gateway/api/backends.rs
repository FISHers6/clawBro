use crate::config::{
    BackendCatalogEntry, BackendLaunchConfig, ExternalMcpServerConfig, GatewayConfig,
    ProviderProfileConfig,
};
use crate::diagnostics::{collect_backend_diagnostics, AcpSupportCategory, BackendDiagnostic};
use crate::runtime::BackendSpec;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use std::collections::HashMap;

use super::types::{ApiErrorBody, ApiListResponse};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BackendApiView {
    pub backend_id: String,
    pub family: String,
    pub adapter_key: String,
    pub registered: bool,
    pub adapter_registered: bool,
    pub probed: bool,
    pub healthy: bool,
    pub error: Option<String>,
    pub capability_profile: Option<crate::runtime::CapabilityProfile>,
    pub approval_mode: crate::runtime::ApprovalMode,
    pub provider_profile_id: Option<String>,
    pub provider_profile: Option<ProviderProfileConfig>,
    pub external_mcp_servers: Vec<ExternalMcpServerConfig>,
    pub launch: BackendLaunchView,
    pub supports_native_local_skills: bool,
    pub acp_backend: Option<crate::runtime::AcpBackend>,
    pub acp_support_category: Option<AcpSupportCategory>,
    pub codex_projection: Option<crate::runtime::CodexProjectionMode>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BackendLaunchView {
    ExternalCommand {
        command: String,
        args: Vec<String>,
        env_keys: Vec<String>,
    },
    GatewayWs {
        endpoint: String,
        token_configured: bool,
        password_configured: bool,
        role: Option<String>,
        scopes: Vec<String>,
        agent_id: Option<String>,
        team_helper_command: Option<String>,
        team_helper_args: Vec<String>,
        lead_helper_mode: bool,
    },
    BundledCommand,
}

pub async fn list_backends(State(state): State<AppState>) -> Json<ApiListResponse<BackendApiView>> {
    Json(ApiListResponse {
        items: backend_views(state.cfg.as_ref(), &state).await,
    })
}

pub async fn get_backend(
    Path(backend_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<BackendApiView>, (StatusCode, Json<ApiErrorBody>)> {
    // 404 early from config — no async work needed for a missing backend.
    let backend = state
        .cfg
        .backends
        .iter()
        .find(|b| b.id == backend_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiErrorBody {
                    error: format!("backend '{}' not found", backend_id),
                }),
            )
        })?;

    let diagnostics = collect_backend_diagnostics(&state).await;
    let diagnostic = diagnostics.iter().find(|d| d.backend_id == backend_id);
    let backend_specs = state.runtime_registry.all_backend_specs().await;
    let backend_spec = backend_specs.iter().find(|s| s.backend_id == backend_id);

    Ok(Json(backend_config_view(
        state.cfg.as_ref(),
        backend,
        backend_spec,
        diagnostic,
    )))
}

pub(crate) fn backend_config_view(
    cfg: &GatewayConfig,
    backend: &BackendCatalogEntry,
    backend_spec: Option<&BackendSpec>,
    diagnostic: Option<&BackendDiagnostic>,
) -> BackendApiView {
    BackendApiView {
        backend_id: backend.id.clone(),
        family: format!("{:?}", backend.family),
        adapter_key: backend.adapter_key().to_string(),
        registered: diagnostic.map(|entry| entry.registered).unwrap_or(false),
        adapter_registered: diagnostic
            .map(|entry| entry.adapter_registered)
            .unwrap_or(false),
        probed: diagnostic.map(|entry| entry.probed).unwrap_or(false),
        healthy: diagnostic.map(|entry| entry.healthy).unwrap_or(false),
        error: diagnostic.and_then(|entry| entry.error.clone()),
        capability_profile: diagnostic.and_then(|entry| entry.capability_profile.clone()),
        approval_mode: backend.approval.mode,
        provider_profile_id: backend.provider_profile.clone(),
        provider_profile: backend
            .provider_profile
            .as_deref()
            .and_then(|profile_id| cfg.provider_profiles.iter().find(|p| p.id == profile_id))
            .cloned(),
        external_mcp_servers: backend.external_mcp_servers.clone(),
        launch: launch_view(&backend.launch),
        supports_native_local_skills: backend_spec
            .map(BackendSpec::supports_native_local_skills)
            .unwrap_or(false),
        acp_backend: diagnostic.and_then(|entry| entry.acp_backend),
        acp_support_category: diagnostic.and_then(|entry| entry.acp_support_category),
        codex_projection: backend.codex.as_ref().map(|cfg| cfg.projection),
        notes: diagnostic
            .map(|entry| entry.notes.clone())
            .unwrap_or_default(),
    }
}

async fn backend_views(cfg: &GatewayConfig, state: &AppState) -> Vec<BackendApiView> {
    let diagnostics = collect_backend_diagnostics(state).await;
    let diagnostic_map: HashMap<_, _> = diagnostics
        .iter()
        .map(|entry| (entry.backend_id.as_str(), entry))
        .collect();
    let backend_specs = state.runtime_registry.all_backend_specs().await;
    let spec_map: HashMap<_, _> = backend_specs
        .iter()
        .map(|entry| (entry.backend_id.as_str(), entry))
        .collect();

    let mut items: Vec<_> = cfg
        .backends
        .iter()
        .map(|backend| {
            backend_config_view(
                cfg,
                backend,
                spec_map.get(backend.id.as_str()).copied(),
                diagnostic_map.get(backend.id.as_str()).copied(),
            )
        })
        .collect();
    items.sort_by(|a, b| a.backend_id.cmp(&b.backend_id));
    items
}

fn launch_view(launch: &BackendLaunchConfig) -> BackendLaunchView {
    match launch {
        BackendLaunchConfig::ExternalCommand { command, args, env } => {
            let mut env_keys: Vec<_> = env.keys().cloned().collect();
            env_keys.sort();
            BackendLaunchView::ExternalCommand {
                command: command.clone(),
                args: args.clone(),
                env_keys,
            }
        }
        BackendLaunchConfig::GatewayWs {
            endpoint,
            token,
            password,
            role,
            scopes,
            agent_id,
            team_helper_command,
            team_helper_args,
            lead_helper_mode,
        } => BackendLaunchView::GatewayWs {
            endpoint: endpoint.clone(),
            token_configured: token.as_ref().is_some_and(|value| !value.is_empty()),
            password_configured: password.as_ref().is_some_and(|value| !value.is_empty()),
            role: role.clone(),
            scopes: scopes.clone(),
            agent_id: agent_id.clone(),
            team_helper_command: team_helper_command.clone(),
            team_helper_args: team_helper_args.clone(),
            lead_helper_mode: *lead_helper_mode,
        },
        BackendLaunchConfig::BundledCommand => BackendLaunchView::BundledCommand,
    }
}
