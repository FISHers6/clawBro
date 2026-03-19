use crate::state::AppState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use crate::runtime::{TeamToolRequest, TeamToolResponse};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct TeamToolQuery {
    token: String,
}

pub async fn invoke_team_tool(
    Query(query): Query<TeamToolQuery>,
    State(state): State<AppState>,
    Json(req): Json<TeamToolRequest>,
) -> impl IntoResponse {
    if query.token != *state.runtime_token {
        return (
            StatusCode::UNAUTHORIZED,
            Json(TeamToolResponse {
                ok: false,
                message: "invalid runtime token".to_string(),
                payload: None,
            }),
        );
    }

    match state
        .registry
        .invoke_team_tool(&req.session_key, req.call)
        .await
    {
        Ok(resp) => (StatusCode::OK, Json(resp)),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(TeamToolResponse {
                ok: false,
                message: err.to_string(),
                payload: None,
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::GatewayConfig, gateway, state::AppState};
    use crate::agent_core::SessionRegistry;
    use crate::runtime::{TeamToolCall, TeamToolRequest};
    use crate::session::{SessionManager, SessionStorage};
    use crate::skills_internal::SkillLoader;
    use std::sync::Arc;

    #[tokio::test]
    async fn team_tools_endpoint_executes_real_registry_call() {
        use crate::agent_core::team::{
            heartbeat::DispatchFn, orchestrator::TeamOrchestrator, registry::TaskRegistry,
            session::TeamSession,
        };
        use tempfile::tempdir;

        let cfg = GatewayConfig::default();
        let storage = SessionStorage::new(cfg.session.dir.clone());
        let session_manager = Arc::new(SessionManager::new(storage));
        let skill_loader = SkillLoader::new(vec![cfg.skills.dir.clone()]);
        let skills = skill_loader.load_all();
        let system_injection = skill_loader.build_system_injection(&skills);
        let (registry, _rx) = SessionRegistry::new(
            None,
            session_manager,
            system_injection,
            None,
            None,
            None,
            None,
            vec![cfg.skills.dir.clone()],
        );

        let tmp = tempdir().unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir(
            "server-team",
            tmp.path().to_path_buf(),
        ));
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            Arc::clone(&task_registry),
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        let lead_key = crate::protocol::SessionKey::new("lark", "group:server");
        orch.set_lead_session_key(lead_key.clone());
        orch.set_scope(lead_key.clone());
        registry.register_team_orchestrator(
            crate::agent_core::team::session::stable_team_id_for_session_key(&lead_key),
            Arc::clone(&orch),
        );

        let state = AppState {
            registry: Arc::clone(&registry),
            runtime_registry: Arc::new(crate::runtime::BackendRegistry::new()),
            event_tx: registry.global_sender(),
            cfg: Arc::new(cfg),
            channel_registry: Arc::new(crate::channel_registry::ChannelRegistry::new()),
            dingtalk_webhook_channel: None,
            runtime_token: Arc::new("test-token".to_string()),
            approvals: crate::runtime::ApprovalBroker::default(),
        };
        let addr = gateway::server::start(state, "127.0.0.1", 0).await.unwrap();

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://127.0.0.1:{}/runtime/team-tools?token=test-token",
                addr.port()
            ))
            .json(&TeamToolRequest {
                session_key: lead_key,
                call: TeamToolCall::CreateTask {
                    id: Some("T300".into()),
                    title: "ship docs".into(),
                    assignee: Some("codex".into()),
                    spec: None,
                    deps: vec![],
                    success_criteria: None,
                },
            })
            .send()
            .await
            .unwrap();

        assert!(resp.status().is_success());
        let body: TeamToolResponse = resp.json().await.unwrap();
        assert!(body.ok);
        assert!(body.message.contains("T300"));

        let task = task_registry.get_task("T300").unwrap().unwrap();
        assert_eq!(task.title, "ship docs");
    }
}
