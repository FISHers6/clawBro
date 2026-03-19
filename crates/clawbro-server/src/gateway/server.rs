use crate::{
    diagnostics::{
        collect_backend_diagnostics, collect_channel_diagnostics, collect_doctor_report,
        collect_health_report, collect_status_report, collect_team_diagnostics,
        collect_topology_diagnostic, BackendDiagnostic, ChannelDiagnostic, DoctorReport,
        HealthReport, StatusReport, TeamDiagnostic, TopologyDiagnostic,
    },
    im_sink::spawn_im_turn,
    state::AppState,
};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

pub fn build_router(state: AppState) -> Router {
    let mut router = Router::new()
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
        );

    if let Some(webhook_cfg) = state
        .cfg
        .channels
        .dingtalk_webhook
        .as_ref()
        .filter(|section| section.enabled)
    {
        let webhook_path =
            crate::channels_internal::dingtalk_webhook::normalize_webhook_path(&webhook_cfg.webhook_path);
        router = router.route(&webhook_path, post(dingtalk_webhook));
    }

    router.with_state(state)
}

pub async fn start(state: AppState, host: &str, port: u16) -> anyhow::Result<SocketAddr> {
    let app = build_router(state);

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    let listener = TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;
    tracing::info!("Gateway listening on {}", bound_addr);

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("Gateway HTTP server terminated unexpectedly: {e}");
            std::process::exit(1);
        }
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

async fn dingtalk_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, Json<serde_json::Value>) {
    let Some(channel) = state.dingtalk_webhook_channel.clone() else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "ok": false, "error": "dingtalk_webhook_not_enabled" })),
        );
    };
    let presentation = state
        .cfg
        .channels
        .dingtalk_webhook
        .as_ref()
        .map(|section| section.presentation)
        .unwrap_or_default();
    match channel.ingest(&headers, &body) {
        Ok(ingress) => {
            let state_for_dispatch = state.clone();
            let channel_for_dispatch = channel.clone();
            tokio::spawn(async move {
                let messages = channel_for_dispatch.to_inbound_messages(ingress).await;
                for inbound in messages {
                    spawn_im_turn(
                        state_for_dispatch.registry.clone(),
                        channel_for_dispatch.clone() as Arc<dyn crate::channels_internal::Channel>,
                        state_for_dispatch.channel_registry.clone(),
                        state_for_dispatch.cfg.clone(),
                        inbound,
                        presentation,
                    );
                }
            });
            (
                StatusCode::OK,
                Json(serde_json::json!({ "ok": true, "accepted": true })),
            )
        }
        Err(reason) => {
            let (status, body) = match reason {
                crate::channels_internal::dingtalk_webhook::DingTalkWebhookRejectReason::MissingToken
                | crate::channels_internal::dingtalk_webhook::DingTalkWebhookRejectReason::InvalidToken => (
                    StatusCode::UNAUTHORIZED,
                    serde_json::json!({ "ok": false, "error": "invalid_token" }),
                ),
                crate::channels_internal::dingtalk_webhook::DingTalkWebhookRejectReason::InvalidPayload => (
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({ "ok": false, "error": "invalid_payload" }),
                ),
                other => (
                    StatusCode::OK,
                    serde_json::json!({
                        "ok": true,
                        "accepted": false,
                        "ignored": format!("{other:?}")
                    }),
                ),
            };
            (status, Json(body))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config, state::AppState};
    use axum::{body::Body, http::Request};
    use crate::agent_core::{
        roster::AgentEntry,
        team::{
            heartbeat::DispatchFn, orchestrator::TeamOrchestrator, registry::TaskRegistry,
            session::TeamSession,
        },
        SessionRegistry,
    };
    use crate::runtime::{
        BackendFamily, BackendRegistry, BackendSpec, ClawBroNativeBackendAdapter, LaunchSpec,
    };
    use crate::session::{SessionManager, SessionStorage};
    use std::sync::Arc;
    use tempfile::tempdir;
    use tower::util::ServiceExt;

    async fn test_state() -> AppState {
        let cfg = config::GatewayConfig {
            backends: vec![config::BackendCatalogEntry {
                id: "native-main".to_string(),
                family: config::BackendFamilyConfig::ClawBroNative,
                adapter_key: Some("native".to_string()),
                acp_backend: None,
                acp_auth_method: None,
                codex: None,
                provider_profile: None,
                approval: Default::default(),
                external_mcp_servers: vec![],
                launch: config::BackendLaunchConfig::BundledCommand,
            }],
            channels: config::ChannelsSection {
                lark: Some(config::LarkSection {
                    enabled: true,
                    presentation: config::ProgressPresentationMode::FinalOnly,
                    trigger_policy: None,
                    default_instance: None,
                    instances: vec![],
                }),
                dingtalk: Some(config::DingTalkSection {
                    enabled: false,
                    presentation: config::ProgressPresentationMode::FinalOnly,
                }),
                dingtalk_webhook: None,
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
            .register_adapter("native", Arc::new(ClawBroNativeBackendAdapter))
            .await;
        runtime_registry
            .register_backend(BackendSpec {
                backend_id: "native-main".into(),
                family: BackendFamily::ClawBroNative,
                adapter_key: "native".into(),
                launch: LaunchSpec::BundledCommand,
                approval_mode: Default::default(),
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
            })
            .await;
        let _ = runtime_registry.probe_backend("native-main").await.unwrap();

        AppState {
            registry,
            runtime_registry,
            event_tx: tokio::sync::broadcast::channel(8).0,
            cfg: Arc::new(cfg),
            channel_registry: Arc::new(crate::channel_registry::ChannelRegistry::new()),
            dingtalk_webhook_channel: None,
            runtime_token: Arc::new("status-token".to_string()),
            approvals: crate::runtime::ApprovalBroker::default(),
        }
    }

    async fn test_state_with_dingtalk_webhook() -> AppState {
        let mut state = test_state().await;
        let webhook_cfg = config::DingTalkWebhookSection {
            enabled: true,
            secret_key: "SEC-test".to_string(),
            webhook_path: "/dingtalk-channel/message".to_string(),
            access_token: None,
            presentation: config::ProgressPresentationMode::FinalOnly,
        };
        let channel = Arc::new(crate::channels_internal::DingTalkWebhookChannel::new(
            webhook_cfg.clone(),
        ));
        let mut channels = crate::channel_registry::ChannelRegistry::new();
        channels.register(
            "dingtalk_webhook",
            Option::<String>::None,
            channel.clone() as Arc<dyn crate::channels_internal::Channel>,
            true,
        );
        Arc::make_mut(&mut state.cfg).channels.dingtalk_webhook = Some(webhook_cfg);
        state.channel_registry = Arc::new(channels);
        state.dingtalk_webhook_channel = Some(channel);
        state
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
        orch.set_lead_session_key(crate::protocol::SessionKey::new("lark", "group:status"));
        orch.set_scope(crate::protocol::SessionKey::new("lark", "group:status"));
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

    #[tokio::test]
    async fn dingtalk_webhook_route_accepts_valid_group_message() {
        let app = build_router(test_state_with_dingtalk_webhook().await);
        let payload = serde_json::json!({
            "senderPlatform": "Mac",
            "conversationId": "cid-group-1",
            "atUsers": [{ "dingtalkId": "bot-1" }],
            "chatbotUserId": "bot-1",
            "msgId": "msg-1",
            "senderNick": "User",
            "senderId": "user-1",
            "sessionWebhookExpiredTime": 1770982588732i64,
            "conversationType": "2",
            "isInAtList": true,
            "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=xxx",
            "text": { "content": "hello @claude" },
            "robotCode": "normal",
            "msgtype": "text"
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/dingtalk-channel/message")
                    .header("token", "SEC-test")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["accepted"], true);
    }

    #[tokio::test]
    async fn dingtalk_webhook_route_rejects_invalid_token() {
        let app = build_router(test_state_with_dingtalk_webhook().await);
        let payload = serde_json::json!({
            "senderPlatform": "Mac",
            "conversationId": "cid-group-1",
            "atUsers": [{ "dingtalkId": "bot-1" }],
            "chatbotUserId": "bot-1",
            "msgId": "msg-2",
            "senderNick": "User",
            "senderId": "user-1",
            "sessionWebhookExpiredTime": 1770982588732i64,
            "conversationType": "2",
            "isInAtList": true,
            "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=xxx",
            "text": { "content": "hello @claude" },
            "robotCode": "normal",
            "msgtype": "text"
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/dingtalk-channel/message")
                    .header("token", "SEC-other")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
