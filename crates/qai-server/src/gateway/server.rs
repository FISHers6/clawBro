use crate::{
    diagnostics::{
        collect_backend_diagnostics, collect_channel_diagnostics, collect_doctor_report,
        collect_health_report, collect_status_report, collect_team_diagnostics,
        collect_topology_diagnostic, BackendDiagnostic, ChannelDiagnostic, DoctorReport,
        HealthReport, StatusReport, TeamDiagnostic, TopologyDiagnostic,
    },
    state::AppState,
};
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use std::net::SocketAddr;
use tokio::net::TcpListener;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/doctor", get(doctor))
        .route("/diagnostics/backends", get(diagnostics_backends))
        .route("/diagnostics/teams", get(diagnostics_teams))
        .route("/diagnostics/channels", get(diagnostics_channels))
        .route("/diagnostics/topology", get(diagnostics_topology))
        .route("/ws", get(super::ws_handler::ws_upgrade))
        .route(
            "/runtime/team-tools",
            post(super::team_tools_handler::invoke_team_tool),
        )
        .with_state(state)
}

pub async fn start(state: AppState, host: &str, port: u16) -> anyhow::Result<SocketAddr> {
    let app = build_router(state);

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    let listener = TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;
    tracing::info!("Gateway listening on {}", bound_addr);

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("Gateway server failed");
    });

    Ok(bound_addr)
}

async fn health(State(state): State<AppState>) -> (StatusCode, Json<HealthReport>) {
    let body = collect_health_report(&state).await;
    let code = if body.ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body))
}

async fn status(State(state): State<AppState>) -> Json<StatusReport> {
    Json(collect_status_report(&state).await)
}

async fn doctor(State(state): State<AppState>) -> Json<DoctorReport> {
    Json(collect_doctor_report(&state).await)
}

async fn diagnostics_backends(State(state): State<AppState>) -> Json<Vec<BackendDiagnostic>> {
    Json(collect_backend_diagnostics(&state).await)
}

async fn diagnostics_teams(State(state): State<AppState>) -> Json<Vec<TeamDiagnostic>> {
    Json(collect_team_diagnostics(&state))
}

async fn diagnostics_channels(State(state): State<AppState>) -> Json<Vec<ChannelDiagnostic>> {
    Json(collect_channel_diagnostics(&state))
}

async fn diagnostics_topology(State(state): State<AppState>) -> Json<TopologyDiagnostic> {
    Json(collect_topology_diagnostic(&state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config, state::AppState};
    use axum::{body::Body, http::Request};
    use qai_agent::{
        team::{
            heartbeat::DispatchFn, orchestrator::TeamOrchestrator, registry::TaskRegistry,
            session::TeamSession,
        },
        roster::AgentEntry,
        SessionRegistry,
    };
    use qai_runtime::{
        BackendFamily, BackendRegistry, BackendSpec, LaunchSpec, QuickAiNativeBackendAdapter,
    };
    use qai_session::{SessionManager, SessionStorage};
    use std::sync::Arc;
    use tempfile::tempdir;
    use tower::util::ServiceExt;

    async fn test_state() -> AppState {
        let cfg = config::GatewayConfig {
            backends: vec![config::BackendCatalogEntry {
                id: "native-main".to_string(),
                family: config::BackendFamilyConfig::QuickAiNative,
                adapter_key: Some("native".to_string()),
                launch: config::BackendLaunchConfig::Embedded,
            }],
            channels: config::ChannelsSection {
                lark: Some(config::LarkSection {
                    enabled: true,
                    presentation: config::ProgressPresentationMode::FinalOnly,
                }),
                dingtalk: Some(config::DingTalkSection {
                    enabled: false,
                    presentation: config::ProgressPresentationMode::FinalOnly,
                }),
            },
            agent_roster: vec![AgentEntry {
                name: "claude".to_string(),
                mentions: vec!["@claude".to_string()],
                backend_id: "native-main".to_string(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            }],
            bindings: vec![config::BindingConfig::Channel {
                agent: "claude".to_string(),
                channel: "lark".to_string(),
            }],
            groups: vec![config::GroupConfig {
                scope: "group:lark:status".to_string(),
                name: Some("Status Group".to_string()),
                mode: config::GroupModeConfig {
                    channel: Some("lark".to_string()),
                    ..Default::default()
                },
                team: Default::default(),
            }],
            ..config::GatewayConfig::default()
        };
        let storage = SessionStorage::new(
            std::env::temp_dir().join(format!("server-status-{}", uuid::Uuid::new_v4())),
        );
        let session_manager = Arc::new(SessionManager::new(storage));
        let (registry, _rx) = SessionRegistry::new(
            None,
            session_manager,
            String::new(),
            None,
            None,
            None,
            None,
            vec![],
        );

        let runtime_registry = Arc::new(BackendRegistry::new());
        runtime_registry
            .register_adapter("native", Arc::new(QuickAiNativeBackendAdapter))
            .await;
        runtime_registry
            .register_backend(BackendSpec {
                backend_id: "native-main".into(),
                family: BackendFamily::QuickAiNative,
                adapter_key: "native".into(),
                launch: LaunchSpec::Embedded,
            })
            .await;
        let _ = runtime_registry.probe_backend("native-main").await.unwrap();

        AppState {
            registry,
            runtime_registry,
            event_tx: tokio::sync::broadcast::channel(8).0,
            cfg: Arc::new(cfg),
            runtime_token: Arc::new("status-token".to_string()),
            approvals: qai_runtime::ApprovalBroker::default(),
        }
    }

    #[tokio::test]
    async fn health_endpoint_reports_runtime_summary() {
        let app = build_router(test_state().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["state"], "ok");
        assert_eq!(json["backend_count"], 1);
        assert_eq!(json["unhealthy_backends"], 0);
    }

    #[tokio::test]
    async fn status_endpoint_includes_backend_and_team_summaries() {
        let state = test_state().await;
        let tmp = tempdir().unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir(
            "team-status",
            tmp.path().to_path_buf(),
        ));
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            task_registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        orch.set_lead_session_key(qai_protocol::SessionKey::new("lark", "group:status"));
        orch.set_scope(qai_protocol::SessionKey::new("lark", "group:status"));
        orch.set_lead_agent_name("claude".to_string());
        orch.set_available_specialists(vec!["codex".to_string()]);
        state
            .registry
            .register_team_orchestrator("team-status".to_string(), Arc::clone(&orch));

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["backends"][0]["backend_id"], "native-main");
        assert_eq!(json["backends"][0]["healthy"], true);
        assert_eq!(json["teams"][0]["team_id"], "team-status");
        assert_eq!(json["teams"][0]["tool_surface_ready"], false);
        assert_eq!(json["teams"][0]["artifact_health"]["root_present"], true);
        assert_eq!(json["teams"][0]["mcp_port"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn doctor_endpoint_reports_findings() {
        let state = test_state().await;
        let tmp = tempdir().unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir(
            "team-doctor",
            tmp.path().to_path_buf(),
        ));
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            task_registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        state
            .registry
            .register_team_orchestrator("team-doctor".to_string(), Arc::clone(&orch));

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/doctor")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["state"], "degraded");
        assert!(json["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["scope"] == "team"));
    }

    #[tokio::test]
    async fn diagnostics_endpoints_expose_backend_team_channel_and_topology_views() {
        let state = test_state().await;
        let app = build_router(state);

        let backends = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/diagnostics/backends")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(backends.status(), StatusCode::OK);
        let backends_json: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(backends.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(backends_json[0]["backend_id"], "native-main");

        let teams = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/diagnostics/teams")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(teams.status(), StatusCode::OK);

        let channels = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/diagnostics/channels")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(channels.status(), StatusCode::OK);
        let channels_json: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(channels.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(channels_json
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["channel"] == "lark"));

        let topology = app
            .oneshot(
                Request::builder()
                    .uri("/diagnostics/topology")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(topology.status(), StatusCode::OK);
        let topology_json: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(topology.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(topology_json["configured_channels"][0], "dingtalk");
        assert!(topology_json["configured_channels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry == "lark"));
    }
}
