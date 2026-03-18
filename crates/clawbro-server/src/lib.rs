//! ClawBro server library target.
//! Exposes modules and test helpers for integration tests.

pub mod channel_registry;
pub mod channels_internal;
pub mod cli;
pub mod config;
pub mod cron_internal;
pub mod delivery_resolver;
pub mod diagnostics;
pub mod embedded_agent;
pub mod gateway;
pub mod gateway_process;
pub mod im_sink;
pub mod progress_presentation;
pub mod protocol;
pub mod runtime;
pub mod agent_sdk_internal;
pub mod agent_core;
pub mod session;
pub mod skills_internal;
pub mod state;
pub mod team_runtime;

pub use gateway_process::run as run_gateway_process;

pub use config::GatewayConfig;
pub use state::{AppState, BrokerApprovalResolver};

use anyhow::Result;
use crate::skills_internal::SkillLoader;
use crate::agent_core::{ConductorRuntimeDispatch, SessionRegistry};
use crate::runtime::{
    acp::AcpBackendAdapter, ApprovalBroker, BackendFamily, BackendRegistry, BackendSpec,
    ClawBroNativeBackendAdapter, LaunchSpec, OpenClawBackendAdapter,
};
use crate::session::{SessionManager, SessionStorage};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// Start a gateway instance for testing.
/// The server runs until the tokio runtime shuts down (end of `#[tokio::test]`).
/// `agent_binary`: path to the ACP agent binary (e.g. `clawbro-rust-agent`).
/// Returns the bound SocketAddr (port 0 = OS-assigned).
pub async fn start_test_gateway(agent_binary: &str) -> Result<SocketAddr> {
    let mut cfg = GatewayConfig::default();
    cfg.agent.backend_id = "test-default".to_string();
    cfg.backends.push(backend_catalog_entry(BackendSpec {
        backend_id: "test-default".to_string(),
        family: BackendFamily::Acp,
        adapter_key: "acp".into(),
        launch: LaunchSpec::ExternalCommand {
            command: agent_binary.to_string(),
            args: vec![],
            env: vec![],
        },
        approval_mode: Default::default(),
        external_mcp_servers: vec![],
        provider_profile: None,
        acp_backend: None,
        acp_auth_method: None,
        codex_projection: None,
    }));
    start_test_gateway_with_config(cfg).await
}

/// Start a gateway instance for testing with an explicit runtime backend.
/// Returns the bound SocketAddr (port 0 = OS-assigned).
pub async fn start_test_gateway_with_backend(default_backend: BackendSpec) -> Result<SocketAddr> {
    let mut cfg = GatewayConfig::default();
    cfg.agent.backend_id = default_backend.backend_id.clone();
    cfg.backends.push(backend_catalog_entry(default_backend));
    start_test_gateway_with_config(cfg).await
}

/// Start a gateway instance for testing with an explicit `GatewayConfig`.
/// Returns the bound SocketAddr (port 0 = OS-assigned).
pub async fn start_test_gateway_with_config(cfg: GatewayConfig) -> Result<SocketAddr> {
    let state = build_test_state_with_config(cfg).await?;
    let runtime_token = state.runtime_token.clone();
    let addr = gateway::server::start(state.clone(), "127.0.0.1", 0).await?;
    state.registry.set_team_tool_url(format!(
        "http://127.0.0.1:{}/runtime/team-tools?token={}",
        addr.port(),
        runtime_token
    ));
    Ok(addr)
}

pub async fn build_test_state_with_config(cfg: GatewayConfig) -> Result<AppState> {
    cfg.validate_runtime_topology()?;
    let storage = SessionStorage::new(cfg.session.dir.clone());
    let session_manager = Arc::new(SessionManager::new(storage));

    let skill_loader = SkillLoader::new(vec![cfg.skills.dir.clone()]);
    let skills = skill_loader.load_all();
    let system_injection = skill_loader.build_system_injection(&skills);

    let approvals = ApprovalBroker::default();
    let runtime_registry = Arc::new(BackendRegistry::new());
    runtime_registry
        .register_adapter("acp", Arc::new(AcpBackendAdapter::new(approvals.clone())))
        .await;
    runtime_registry
        .register_adapter(
            "openclaw",
            Arc::new(OpenClawBackendAdapter::new(approvals.clone())),
        )
        .await;
    runtime_registry
        .register_adapter("native", Arc::new(ClawBroNativeBackendAdapter))
        .await;
    for backend in &cfg.backends {
        runtime_registry
            .register_backend(backend.to_backend_spec(
                cfg.resolve_provider_profile(backend.provider_profile.as_deref())?,
            ))
            .await;
    }
    let runtime_dispatch = Arc::new(ConductorRuntimeDispatch::new(Arc::clone(&runtime_registry)));
    let default_backend_id = cfg.resolved_default_backend_id();
    let (registry, _event_rx) = SessionRegistry::with_runtime_dispatch(
        default_backend_id,
        session_manager,
        system_injection,
        if cfg.agent_roster.is_empty() {
            None
        } else {
            Some(crate::agent_core::AgentRoster::new(cfg.agent_roster.clone()))
        },
        None,
        None,
        cfg.gateway.default_workspace.clone(),
        vec![cfg.skills.dir.clone()],
        runtime_dispatch,
    );
    let event_tx = registry.global_sender();
    registry.set_approval_resolver(Arc::new(BrokerApprovalResolver::new(approvals.clone())));

    let state = AppState {
        registry,
        runtime_registry,
        event_tx,
        cfg: Arc::new(cfg),
        runtime_token: Arc::new(uuid::Uuid::new_v4().to_string()),
        approvals,
    };

    team_runtime::wire_team_runtime(
        Arc::clone(&state.registry),
        state.cfg.as_ref(),
        Arc::new(channel_registry::ChannelRegistry::new()),
        Duration::from_millis(50),
    )
    .await?;

    Ok(state)
}

fn backend_catalog_entry(spec: BackendSpec) -> config::BackendCatalogEntry {
    let family = match spec.family {
        BackendFamily::Acp => config::BackendFamilyConfig::Acp,
        BackendFamily::OpenClawGateway => config::BackendFamilyConfig::OpenClawGateway,
        BackendFamily::ClawBroNative => config::BackendFamilyConfig::ClawBroNative,
    };
    let launch = match spec.launch {
        LaunchSpec::ExternalCommand { command, args, env } => {
            config::BackendLaunchConfig::ExternalCommand {
                command,
                args,
                env: env.into_iter().collect(),
            }
        }
        LaunchSpec::GatewayWs {
            endpoint,
            token,
            password,
            role,
            scopes,
            agent_id,
            team_helper_command,
            team_helper_args,
            lead_helper_mode,
        } => config::BackendLaunchConfig::GatewayWs {
            endpoint,
            token,
            password,
            role,
            scopes,
            agent_id,
            team_helper_command,
            team_helper_args,
            lead_helper_mode,
        },
        LaunchSpec::BundledCommand => config::BackendLaunchConfig::BundledCommand,
    };
    config::BackendCatalogEntry {
        id: spec.backend_id,
        family,
        adapter_key: Some(spec.adapter_key),
        acp_backend: spec.acp_backend,
        acp_auth_method: spec.acp_auth_method,
        codex: spec
            .codex_projection
            .map(|projection| config::BackendCodexConfig { projection }),
        provider_profile: spec
            .provider_profile
            .as_ref()
            .map(|profile| profile.id.clone()),
        approval: config::BackendApprovalConfig {
            mode: spec.approval_mode,
        },
        external_mcp_servers: spec
            .external_mcp_servers
            .into_iter()
            .map(|server| config::ExternalMcpServerConfig {
                name: server.name,
                url: match server.transport {
                    crate::runtime::ExternalMcpTransport::Sse { url } => url,
                },
            })
            .collect(),
        launch,
    }
}
