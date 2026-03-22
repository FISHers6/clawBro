use crate::{
    gateway::api,
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
    routing::{delete, get, patch, post, put},
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
        .route("/api/agents", get(api::agents::list_agents))
        .route("/api/agents", post(api::agents_write::create_agent))
        .route("/api/agents/{name}", get(api::agents::get_agent).patch(api::agents_write::patch_agent).delete(api::agents_write::delete_agent))
        .route("/api/approvals", get(api::approvals::list_approvals))
        .route("/api/approvals/{approval_id}", get(api::approvals::get_approval))
        .route("/api/approvals/{approval_id}/approve", post(api::approvals::approve_approval))
        .route("/api/approvals/{approval_id}/deny", post(api::approvals::deny_approval))
        .route("/api/backends", get(api::backends::list_backends))
        .route("/api/backends/{backend_id}", get(api::backends::get_backend))
        .route("/api/channels", get(api::channels::list_channels))
        .route("/api/channels/{channel_id}", get(api::channels::get_channel))
        .route("/api/config/effective", get(api::config::get_effective_config))
        .route("/api/config/spec", get(api::config::get_config_spec))
        .route("/api/config/raw", get(api::config_write::get_raw_config))
        .route("/api/config/raw", put(api::config_write::put_raw_config))
        .route("/api/config/validate", post(api::config_write::validate_config))
        .route("/api/skills", get(api::skills::list_skills))
        .route("/api/agents/{name}/skills", get(api::skills::get_agent_skills))
        .route("/api/scheduler/jobs", get(api::scheduler::list_jobs))
        .route("/api/scheduler/jobs/{job_id}", get(api::scheduler::get_job))
        .route(
            "/api/scheduler/jobs/{job_id}/runs",
            get(api::scheduler::list_job_runs),
        )
        .route(
            "/api/scheduler/jobs/{job_id}/run-now",
            post(api::scheduler::run_job_now),
        )
        .route("/api/sessions", get(api::sessions::list_sessions))
        .route("/api/sessions", delete(api::sessions_write::delete_session_history))
        .route("/api/sessions/detail", get(api::sessions::get_session))
        .route("/api/sessions/messages", get(api::sessions::get_session_messages))
        .route("/api/sessions/events", get(api::sessions::get_session_events))
        .route("/api/tasks", get(api::tasks::list_tasks))
        .route("/api/tasks/{task_id}", get(api::tasks::get_task))
        .route("/api/teams", get(api::teams::list_teams))
        .route("/api/teams/{team_id}", get(api::teams::get_team))
        .route(
            "/api/teams/{team_id}/artifacts",
            get(api::teams::list_team_artifacts),
        )
        .route(
            "/api/teams/{team_id}/artifacts/{artifact_name}",
            get(api::teams::get_team_artifact),
        )
        .route(
            "/api/teams/{team_id}/tasks/{task_id}",
            get(api::tasks::get_team_task),
        )
        .route(
            "/api/teams/{team_id}/tasks/{task_id}/artifacts",
            get(api::tasks::list_team_task_artifacts),
        )
        .route(
            "/api/teams/{team_id}/tasks/{task_id}/artifacts/{artifact_name}",
            get(api::tasks::get_team_task_artifact),
        )
        .route(
            "/api/teams/{team_id}/leader-updates",
            get(api::teams::list_team_leader_updates),
        )
        .route(
            "/api/teams/{team_id}/channel-sends",
            get(api::teams::list_team_channel_sends),
        )
        .route(
            "/api/teams/{team_id}/routing-events",
            get(api::teams::list_team_routing_events),
        )
        .route(
            "/api/teams/{team_id}/pending-completions",
            get(api::teams::list_team_pending_completions),
        )
        .route("/api/teams/{team_id}/tasks", get(api::tasks::list_team_tasks))
        .route("/api/chat", post(api::chat::chat_send))
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
        let webhook_path = crate::channels_internal::dingtalk_webhook::normalize_webhook_path(
            &webhook_cfg.webhook_path,
        );
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
    use crate::agent_core::{
        roster::AgentEntry,
        team::{
            heartbeat::DispatchFn,
            orchestrator::TeamOrchestrator,
            registry::{CreateTask, TaskRegistry},
            session::{TaskArtifactMeta, TeamSession},
        },
        SessionRegistry,
    };
    use crate::runtime::{
        BackendFamily, BackendRegistry, BackendSpec, ClawBroNativeBackendAdapter, LaunchSpec,
    };
    use crate::scheduler::{
        CreateJobRequest, CreateTargetRequest, RequestedTargetKind, ScheduleInput,
        SessionTargetRequest, SourceKind,
    };
    use crate::session::{SessionManager, SessionStorage};
    use crate::{config, state::AppState};
    use axum::{body::Body, http::Request};
    use serde_json::Value;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tower::util::ServiceExt;
    use uuid::Uuid;

    async fn test_state() -> AppState {
        let cfg = config::GatewayConfig {
            agent: config::AgentSection {
                backend_id: "native-main".to_string(),
            },
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
            Some("native-main".to_string()),
            session_manager,
            String::new(),
            Some(crate::agent_core::AgentRoster::new(cfg.agent_roster.clone())),
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
            scheduler_service: crate::scheduler_runtime::build_test_scheduler_service(),
            config_path: Arc::new(crate::config::config_file_path()),
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

    #[tokio::test]
    async fn api_agents_lists_roster_without_runtime_state_field() {
        let app = build_router(test_state().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/agents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let items = json["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["name"], "claude");
        assert_eq!(items[0]["backend_id"], "native-main");
        assert!(items[0].get("effective_backend_id").is_none());
        assert_eq!(items[0]["role"], "solo");
        assert!(items[0].get("runtime_state").is_none());
        assert_eq!(items[0]["persona_dir_configured"], false);
        assert_eq!(items[0]["workspace_dir_configured"], false);
        assert_eq!(items[0]["extra_skills_dir_count"], 0);
        assert!(items[0].get("persona_dir").is_none());
        assert!(items[0].get("workspace_dir").is_none());
        assert!(items[0].get("extra_skills_dirs").is_none());
    }

    #[tokio::test]
    async fn api_agent_detail_returns_not_found_for_unknown_agent() {
        let app = build_router(test_state().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/agents/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_backends_and_channels_return_json_lists() {
        let app = build_router(test_state().await);

        let backends = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/backends")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(backends.status(), StatusCode::OK);
        let backends_json: Value = serde_json::from_slice(
            &axum::body::to_bytes(backends.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(backends_json["items"][0]["backend_id"], "native-main");
        assert_eq!(
            backends_json["items"][0]["supports_native_local_skills"],
            false
        );
        assert_eq!(backends_json["items"][0]["launch"]["type"], "bundled_command");

        let channels = app
            .oneshot(
                Request::builder()
                    .uri("/api/channels")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(channels.status(), StatusCode::OK);
        let channels_json: Value = serde_json::from_slice(
            &axum::body::to_bytes(channels.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(channels_json["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["channel"] == "lark" && entry["presentation"] == "final_only"));
    }

    #[tokio::test]
    async fn api_effective_config_includes_team_scopes_and_bindings() {
        let app = build_router(test_state().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/config/effective")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["default_backend_id"], "native-main");
        assert!(json["roster_agents"].as_array().unwrap().len() >= 1);
        assert!(json["team_scopes"].is_array());
        assert!(json["channels"].as_array().unwrap().iter().any(|v| v == "lark"));
    }

    #[tokio::test]
    async fn api_config_spec_redacts_secrets_and_exposes_backend_spec() {
        let mut state = test_state().await;
        {
            let cfg = Arc::make_mut(&mut state.cfg);
            cfg.auth.ws_token = Some("super-secret".to_string());
            cfg.channels.lark = Some(config::LarkSection {
                enabled: true,
                presentation: config::ProgressPresentationMode::FinalOnly,
                trigger_policy: None,
                default_instance: Some("alpha".to_string()),
                instances: vec![config::LarkInstanceConfig {
                    id: "alpha".to_string(),
                    app_id: "cli_alpha".to_string(),
                    app_secret: "secret_alpha".to_string(),
                    bot_name: Some("Claw".to_string()),
                }],
            });
            cfg.backends[0].external_mcp_servers = vec![config::ExternalMcpServerConfig {
                name: "docs".to_string(),
                url: "http://127.0.0.1:9901/sse".to_string(),
            }];
        }

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/config/spec")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["auth"]["ws_token_configured"], true);
        assert_eq!(json["channels"]["lark"]["instances"][0]["id"], "alpha");
        assert_eq!(
            json["channels"]["lark"]["instances"][0]["app_secret_configured"],
            true
        );
        assert!(json["channels"]["lark"]["instances"][0]
            .get("app_secret")
            .is_none());
        assert_eq!(json["agent_roster"][0]["name"], "claude");
        assert_eq!(json["agent_roster"][0]["workspace_dir_configured"], false);
        assert!(json["agent_roster"][0].get("workspace_dir").is_none());
        assert_eq!(json["gateway"]["default_workspace_configured"], false);
        assert!(json["gateway"].get("default_workspace").is_none());
        assert_eq!(json["skills"]["global_dir_count"], 0);
        assert!(json["skills"].get("dir").is_none());
        assert_eq!(json["session"]["dir_configured"], true);
        assert!(json["session"].get("dir").is_none());
        assert_eq!(json["memory"]["shared_dir_configured"], true);
        assert!(json["memory"].get("shared_dir").is_none());
        assert_eq!(json["scheduler"]["db_path_configured"], false);
        assert!(json["scheduler"].get("db_path").is_none());
        assert_eq!(json["backends"][0]["external_mcp_servers"][0]["name"], "docs");
        assert_eq!(json["backends"][0]["launch"]["type"], "bundled_command");
    }

    #[tokio::test]
    async fn api_sessions_support_query_param_identity_and_message_reads() {
        let state = test_state().await;
        let key = crate::protocol::SessionKey::with_instance("lark", "alpha", "group:oc_test");
        let session_id = state.registry.session_manager_ref().get_or_create(&key).await.unwrap();
        state.registry
            .session_manager_ref()
            .append_message(
                session_id,
                &crate::session::StoredMessage {
                    id: Uuid::new_v4(),
                    role: "assistant".to_string(),
                    content: "hello from session".to_string(),
                    timestamp: chrono::Utc::now(),
                    sender: Some("claude".to_string()),
                    tool_calls: None,
                    fragment_event_ids: None,
                    aggregation_mode: None,
                },
            )
            .await
            .unwrap();
        state
            .registry
            .session_manager_ref()
            .append_event(
                session_id,
                &crate::session::StoredSessionEvent {
                    timestamp: chrono::Utc::now(),
                    event: crate::protocol::AgentEvent::Thinking { session_id },
                },
            )
            .await
            .unwrap();

        let app = build_router(state);
        let detail = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/detail?channel=lark&scope=group:oc_test&channel_instance=alpha")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(detail.status(), StatusCode::OK);

        let list = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions?channel=lark&scope=group:oc_test&channel_instance=alpha")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_body = axum::body::to_bytes(list.into_body(), usize::MAX)
            .await
            .unwrap();
        let list_json: Value = serde_json::from_slice(&list_body).unwrap();
        assert_eq!(list_json["items"].as_array().unwrap().len(), 1);
        assert_eq!(list_json["items"][0]["message_count"], 1);

        let list_without_instance = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions?channel=lark&scope=group:oc_test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_without_instance.status(), StatusCode::OK);
        let list_without_instance_body =
            axum::body::to_bytes(list_without_instance.into_body(), usize::MAX)
                .await
                .unwrap();
        let list_without_instance_json: Value =
            serde_json::from_slice(&list_without_instance_body).unwrap();
        assert_eq!(list_without_instance_json["items"].as_array().unwrap().len(), 1);

        let bad_filter = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions?channel=lark")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad_filter.status(), StatusCode::BAD_REQUEST);

        let messages = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/messages?channel=lark&scope=group:oc_test&channel_instance=alpha")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(messages.status(), StatusCode::OK);
        let body = axum::body::to_bytes(messages.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["items"][0]["content"], "hello from session");

        let events = app
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/events?channel=lark&scope=group:oc_test&channel_instance=alpha")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(events.status(), StatusCode::OK);
        let event_body = axum::body::to_bytes(events.into_body(), usize::MAX)
            .await
            .unwrap();
        let event_json: Value = serde_json::from_slice(&event_body).unwrap();
        assert_eq!(event_json["items"][0]["event_type"], "thinking");
        assert_eq!(event_json["items"][0]["payload"]["session_id"], session_id.to_string());
        assert!(event_json["items"][0]["payload"].get("type").is_none());
    }

    #[tokio::test]
    async fn api_approvals_lists_and_resolves_pending_requests() {
        let state = test_state().await;
        let request = crate::runtime::PermissionRequest {
            id: "approval-1".to_string(),
            prompt: "allow?".to_string(),
            command: Some("git status".to_string()),
            cwd: None,
            host: None,
            agent_id: Some("claude".to_string()),
            expires_at_ms: None,
        };
        let pending = state.approvals.register(&request);
        let app = build_router(state.clone());

        let list = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/approvals")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);

        let detail = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/approvals/approval-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(detail.status(), StatusCode::OK);

        let approve = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/approvals/approval-1/approve")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(approve.status(), StatusCode::OK);
        assert_eq!(
            pending.wait().await,
            crate::runtime::ApprovalDecision::AllowOnce
        );

        let missing = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/approvals/missing/approve")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_skills_expose_host_and_agent_effective_views() {
        let mut state = test_state().await;
        let tmp = tempdir().unwrap();
        let managed_root = tmp.path().join("managed-skills");
        let managed_skill = managed_root.join("api-skill");
        std::fs::create_dir_all(&managed_skill).unwrap();
        std::fs::write(
            managed_skill.join("SKILL.md"),
            "---\nname: api-skill\nversion: 1.2.3\n---\nUse the api skill.\n",
        )
        .unwrap();

        Arc::make_mut(&mut state.cfg).skills.dir = managed_root;
        Arc::make_mut(&mut state.cfg).groups[0].mode.interaction =
            config::InteractionMode::Team;
        Arc::make_mut(&mut state.cfg).groups[0].mode.front_bot = Some("claude".to_string());
        Arc::make_mut(&mut state.cfg).groups[0].team.roster = vec!["claude".to_string()];
        let app = build_router(state);

        let skills = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/skills")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(skills.status(), StatusCode::OK);
        let skills_json: Value = serde_json::from_slice(
            &axum::body::to_bytes(skills.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(skills_json["host_skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "scheduler"));
        assert!(skills_json["host_skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "[host]/scheduler/SKILL.md"));
        assert!(skills_json["effective_skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "api-skill"));
        assert!(skills_json["roots"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| !entry["path"].as_str().unwrap_or_default().starts_with('/')));
        assert!(skills_json["effective_skills"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| !entry["path"].as_str().unwrap_or_default().starts_with('/')));

        let agent_skills = app
            .oneshot(
                Request::builder()
                    .uri("/api/agents/claude/skills")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(agent_skills.status(), StatusCode::OK);
        let agent_json: Value = serde_json::from_slice(
            &axum::body::to_bytes(agent_skills.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(agent_json["role"], "lead");
        assert!(agent_json["host_skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "canonical-team-lead"));
        assert!(agent_json["effective_skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "api-skill"));
        assert!(agent_json["roots"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| !entry["path"].as_str().unwrap_or_default().starts_with('/')));
    }

    #[tokio::test]
    async fn api_scheduler_jobs_detail_and_runs() {
        let state = test_state().await;
        let job = state
            .scheduler_service
            .create_job(
                CreateJobRequest {
                    name: "api-job".to_string(),
                    schedule: ScheduleInput::Every {
                        interval_ms: 60_000,
                    },
                    timezone: Some("UTC".to_string()),
                    target: CreateTargetRequest::Session(SessionTargetRequest {
                        requested_kind: RequestedTargetKind::AgentTurn,
                        session_key: "scheduler:test".to_string(),
                        prompt: "ping".to_string(),
                        agent: Some("claude".to_string()),
                        preconditions: vec![],
                    }),
                    max_retries: 0,
                    source_kind: SourceKind::HumanCli,
                    source_actor: "tester".to_string(),
                    source_session_key: Some("session:test".to_string()),
                    created_via: "api-test".to_string(),
                    requested_by_role: Some("user".to_string()),
                },
                chrono::Utc::now(),
            )
            .unwrap();
        let app = build_router(state);

        let jobs = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/scheduler/jobs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(jobs.status(), StatusCode::OK);
        let jobs_json: Value = serde_json::from_slice(
            &axum::body::to_bytes(jobs.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(jobs_json["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == job.id));

        let detail = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/scheduler/jobs/{}", job.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(detail.status(), StatusCode::OK);

        let runs = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/scheduler/jobs/{}/runs", job.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(runs.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_teams_return_api_owned_view_with_scope_and_specialists() {
        let state = test_state().await;
        let tmp = tempdir().unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("team-api", tmp.path().to_path_buf()));
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            task_registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        orch.set_scope(crate::protocol::SessionKey::with_instance(
            "lark",
            "beta",
            "group:team-api",
        ));
        orch.set_lead_session_key(crate::protocol::SessionKey::with_instance(
            "lark",
            "beta",
            "group:team-api",
        ));
        orch.set_lead_agent_name("claude".to_string());
        orch.set_available_specialists(vec!["claw".to_string()]);
        state
            .registry
            .register_team_orchestrator("team-api".to_string(), Arc::clone(&orch));

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/teams")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["items"][0]["team_id"], "team-api");
        assert_eq!(json["items"][0]["channel"], "lark");
        assert_eq!(json["items"][0]["channel_instance"], "beta");
        assert_eq!(json["items"][0]["scope"], "group:team-api");
        assert_eq!(json["items"][0]["lead_agent_name"], "claude");
        assert_eq!(json["items"][0]["specialists"][0], "claw");
    }

    #[tokio::test]
    async fn api_team_journals_expose_leader_updates_channel_sends_routing_and_pending() {
        let state = test_state().await;
        let tmp = tempdir().unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir(
            "team-journals",
            tmp.path().to_path_buf(),
        ));
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            task_registry,
            Arc::clone(&session),
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        let lead_key = crate::protocol::SessionKey::with_instance("lark", "beta", "group:journal");
        orch.set_scope(lead_key.clone());
        orch.set_lead_session_key(lead_key.clone());
        orch.set_lead_agent_name("claude".to_string());
        orch.set_available_specialists(vec!["claw".to_string()]);
        session
            .record_leader_update(
                Some(&lead_key),
                None,
                "claude",
                crate::agent_core::team::session::LeaderUpdateKind::PostUpdate,
                "task started",
                Some("T001"),
            )
            .unwrap();
        session
            .record_channel_send(
                "lark",
                Some("beta"),
                Some("beta"),
                "group:journal",
                Some(&lead_key),
                None,
                None,
                None,
                crate::agent_core::team::session::ChannelSendSourceKind::Milestone,
                "claude",
                Some("T001"),
                None,
                "milestone sent",
                crate::agent_core::team::session::ChannelSendStatus::Sent,
                None,
            )
            .unwrap();
        let envelope = crate::agent_core::team::completion_routing::TeamRoutingEnvelope {
            run_id: "run-1".to_string(),
            parent_run_id: None,
            requester_session_key: Some(lead_key.clone()),
            fallback_session_keys: vec![],
            delivery_source: None,
            team_id: "team-journals".to_string(),
            delivery_status:
                crate::agent_core::team::completion_routing::RoutingDeliveryStatus::PersistedPending,
            event: crate::agent_core::team::completion_routing::TeamRoutingEvent::submitted(
                "T001", "claw", "ready for review",
            ),
        };
        session.append_routing_outcome(&envelope).unwrap();
        session.append_pending_completion(&envelope).unwrap();
        state
            .registry
            .register_team_orchestrator("team-journals".to_string(), Arc::clone(&orch));

        let app = build_router(state);

        let leader_updates = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/teams/team-journals/leader-updates")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(leader_updates.status(), StatusCode::OK);
        let leader_updates_json: Value = serde_json::from_slice(
            &axum::body::to_bytes(leader_updates.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(leader_updates_json["items"][0]["source_agent"], "claude");
        assert_eq!(leader_updates_json["items"][0]["task_id"], "T001");

        let channel_sends = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/teams/team-journals/channel-sends")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(channel_sends.status(), StatusCode::OK);
        let channel_sends_json: Value = serde_json::from_slice(
            &axum::body::to_bytes(channel_sends.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(channel_sends_json["items"][0]["source_kind"], "milestone");
        assert_eq!(channel_sends_json["items"][0]["source_agent"], "claude");

        let routing_events = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/teams/team-journals/routing-events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(routing_events.status(), StatusCode::OK);
        let routing_events_json: Value = serde_json::from_slice(
            &axum::body::to_bytes(routing_events.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            routing_events_json["items"][0]["delivery_status"],
            "PersistedPending"
        );
        assert_eq!(routing_events_json["items"][0]["event"]["task_id"], "T001");

        let pending = app
            .oneshot(
                Request::builder()
                    .uri("/api/teams/team-journals/pending-completions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(pending.status(), StatusCode::OK);
        let pending_json: Value = serde_json::from_slice(
            &axum::body::to_bytes(pending.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(pending_json["items"][0]["envelope"]["run_id"], "run-1");
        assert_eq!(
            pending_json["items"][0]["review"]["review_kind"],
            "Submitted"
        );
    }

    #[tokio::test]
    async fn api_team_task_detail_and_artifacts_expose_workspace_files() {
        let state = test_state().await;
        let tmp = tempdir().unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        task_registry
            .create_task(CreateTask {
                id: "T010".to_string(),
                title: "Document sessions".to_string(),
                assignee_hint: Some("claw".to_string()),
                deps: vec![],
                timeout_secs: 1800,
                spec: Some("Read ~/.clawbro/sessions".to_string()),
                success_criteria: Some("Report directory layout".to_string()),
            })
            .unwrap();
        let session = Arc::new(TeamSession::from_dir(
            "team-artifacts",
            tmp.path().to_path_buf(),
        ));
        session
            .write_task_meta(
                "T010",
                &TaskArtifactMeta {
                    id: "T010".to_string(),
                    title: "Document sessions".to_string(),
                    assignee_hint: Some("claw".to_string()),
                    status: "claimed:claw:2026-03-21T00:00:00Z".to_string(),
                    deps: vec![],
                    success_criteria: Some("Report directory layout".to_string()),
                    created_at: "2026-03-21T00:00:00Z".to_string(),
                    updated_at: "2026-03-21T00:01:00Z".to_string(),
                    done_at: None,
                    claimed_by: Some("claw".to_string()),
                    submitted_by: None,
                    accepted_by: None,
                    spec_path: "tasks/T010/spec.md".to_string(),
                    plan_path: "tasks/T010/plan.md".to_string(),
                    progress_path: "tasks/T010/progress.md".to_string(),
                    result_path: "tasks/T010/result.md".to_string(),
                },
            )
            .unwrap();
        session
            .write_task_spec("T010", "# Spec\nRead the sessions directory")
            .unwrap();
        session
            .write_task_plan("T010", "# Plan\n- [ ] Count directories")
            .unwrap();
        session
            .append_task_progress("T010", "checkpoint: counted directories")
            .unwrap();
        session
            .write_task_result("T010", "# Result\nFound 121 session directories")
            .unwrap();
        session
            .write_task_review_feedback("T010", "Please expand on edge cases.")
            .unwrap();

        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            task_registry,
            Arc::clone(&session),
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        orch.set_scope(crate::protocol::SessionKey::with_instance(
            "lark",
            "beta",
            "group:artifacts",
        ));
        orch.set_lead_session_key(crate::protocol::SessionKey::with_instance(
            "lark",
            "beta",
            "group:artifacts",
        ));
        orch.set_lead_agent_name("claude".to_string());
        orch.set_available_specialists(vec!["claw".to_string()]);
        state
            .registry
            .register_team_orchestrator("team-artifacts".to_string(), Arc::clone(&orch));

        let app = build_router(state);

        let detail = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/teams/team-artifacts/tasks/T010")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(detail.status(), StatusCode::OK);
        let detail_json: Value = serde_json::from_slice(
            &axum::body::to_bytes(detail.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(detail_json["id"], "T010");
        assert_eq!(detail_json["artifact_meta"]["claimed_by"], "claw");
        assert!(detail_json["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "result" && entry["present"] == true));

        let artifacts = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/teams/team-artifacts/tasks/T010/artifacts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(artifacts.status(), StatusCode::OK);
        let artifacts_json: Value = serde_json::from_slice(
            &axum::body::to_bytes(artifacts.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(artifacts_json["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "review-feedback" && entry["present"] == true));

        let result = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/teams/team-artifacts/tasks/T010/artifacts/result")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::OK);
        let result_json: Value = serde_json::from_slice(
            &axum::body::to_bytes(result.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result_json["artifact"]["name"], "result");
        assert_eq!(result_json["content_type"], "text/markdown");
        assert!(result_json["content"]
            .as_str()
            .unwrap()
            .contains("Found 121 session directories"));

        let unknown = app
            .oneshot(
                Request::builder()
                    .uri("/api/teams/team-artifacts/tasks/T010/artifacts/secret-dotenv")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_global_task_detail_rejects_ambiguous_task_ids_across_teams() {
        let state = test_state().await;
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));

        for (team_id, group_scope) in [("team-a", "group:team-a"), ("team-b", "group:team-b")] {
            let tmp = tempdir().unwrap();
            let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
            task_registry
                .create_task(CreateTask {
                    id: "T001".to_string(),
                    title: format!("Task for {team_id}"),
                    assignee_hint: Some("claw".to_string()),
                    deps: vec![],
                    timeout_secs: 300,
                    spec: Some("Same local id".to_string()),
                    success_criteria: Some("Return conflict".to_string()),
                })
                .unwrap();
            let session = Arc::new(TeamSession::from_dir(team_id, tmp.path().to_path_buf()));
            let orch = TeamOrchestrator::new(
                task_registry,
                session,
                dispatch_fn.clone(),
                std::time::Duration::from_secs(60),
            );
            orch.set_scope(crate::protocol::SessionKey::with_instance("lark", "beta", group_scope));
            orch.set_lead_session_key(crate::protocol::SessionKey::with_instance(
                "lark",
                "beta",
                group_scope,
            ));
            orch.set_lead_agent_name("claude".to_string());
            orch.set_available_specialists(vec!["claw".to_string()]);
            state
                .registry
                .register_team_orchestrator(team_id.to_string(), Arc::clone(&orch));
        }

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tasks/T001")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn api_team_root_artifacts_expose_team_context_files() {
        let state = test_state().await;
        let tmp = tempdir().unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("team-root-files", tmp.path().to_path_buf()));
        session.write_team_md("# Team\nLead is claude").unwrap();
        session.write_agents_md("# Agents\n- claude\n- claw").unwrap();
        session.write_context_md("# Context\nTrack the current user goal").unwrap();
        session.write_heartbeat_md("# Heartbeat\nCheck stale tasks").unwrap();
        session
            .sync_tasks_md(&task_registry)
            .unwrap();

        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            task_registry,
            Arc::clone(&session),
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        orch.set_scope(crate::protocol::SessionKey::with_instance(
            "lark",
            "beta",
            "group:root-files",
        ));
        orch.set_lead_session_key(crate::protocol::SessionKey::with_instance(
            "lark",
            "beta",
            "group:root-files",
        ));
        orch.set_lead_agent_name("claude".to_string());
        orch.set_available_specialists(vec!["claw".to_string()]);
        state
            .registry
            .register_team_orchestrator("team-root-files".to_string(), Arc::clone(&orch));

        let app = build_router(state);

        let artifacts = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/teams/team-root-files/artifacts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(artifacts.status(), StatusCode::OK);
        let artifacts_json: Value = serde_json::from_slice(
            &axum::body::to_bytes(artifacts.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(artifacts_json["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "team" && entry["present"] == true));
        assert!(artifacts_json["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "context" && entry["present"] == true));

        let team_md = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/teams/team-root-files/artifacts/team")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(team_md.status(), StatusCode::OK);
        let team_md_json: Value = serde_json::from_slice(
            &axum::body::to_bytes(team_md.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(team_md_json["artifact"]["name"], "team");
        assert_eq!(team_md_json["content_type"], "text/markdown");
        assert!(team_md_json["content"]
            .as_str()
            .unwrap()
            .contains("Lead is claude"));

        let unknown = app
            .oneshot(
                Request::builder()
                    .uri("/api/teams/team-root-files/artifacts/secrets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    }
}
