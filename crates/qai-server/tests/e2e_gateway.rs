//! E2E tests: Gateway WS integration tests
//!
//! ## test_gateway_e2e_deepseek
//! Skipped automatically if OPENAI_API_KEY is not set.
//!
//! To run:
//!   OPENAI_API_KEY=sk-xxx \
//!   OPENAI_API_BASE=https://api.deepseek.com \
//!   QUICKAI_MODEL=deepseek-chat \
//!   QUICKAI_RUST_AGENT_BIN=/path/to/quickai-rust-agent \
//!   cargo test -p qai-server --test e2e_gateway -- --nocapture
//!
//! ## test_gateway_e2e_claude_agent
//! Requires `claude` CLI installed and authenticated.
//!
//! To run:
//!   QUICKAI_CLAUDE_AGENT_BIN=/path/to/quickai-claude-agent \
//!   cargo test -p qai-server --test e2e_gateway -- test_gateway_e2e_claude_agent --ignored --nocapture

use futures_util::{SinkExt, StreamExt};
use qai_agent::roster::AgentEntry;
use qai_protocol::{AgentEvent, InboundMsg, MsgContent, SessionKey};
use qai_runtime::{BackendFamily, BackendSpec, LaunchSpec};
use qai_server::{
    build_test_state_with_config,
    config::{BackendCatalogEntry, BackendFamilyConfig, BackendLaunchConfig, GatewayConfig},
    gateway, start_test_gateway, start_test_gateway_with_backend, start_test_gateway_with_config,
};
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio_tungstenite::tungstenite::Message as WsMsg;

#[tokio::test]
#[ignore = "requires OPENAI_API_KEY - run with: cargo test -p qai-server --test e2e_gateway -- --ignored --nocapture"]
async fn test_gateway_e2e_deepseek() {
    // Guard: skip if no API key
    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!("SKIP test_gateway_e2e_deepseek: OPENAI_API_KEY not set");
            return;
        }
    };
    let _ = api_key; // used via env, not directly

    // OPENAI_API_BASE and QUICKAI_MODEL must be set by the caller.
    // Example: OPENAI_API_BASE=https://api.deepseek.com QUICKAI_MODEL=deepseek-chat
    // Not mutated here to avoid unsafe set_var in async context.

    let agent_bin = std::env::var("QUICKAI_RUST_AGENT_BIN")
        .unwrap_or_else(|_| "quickai-rust-agent".to_string());

    // Start gateway
    let addr = start_test_gateway(&agent_bin)
        .await
        .expect("Failed to start test gateway");

    // Connect WebSocket
    let url = format!("ws://{}/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut ws_write, mut ws_read) = ws_stream.split();

    // Send InboundMsg
    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: SessionKey::new("ws", "e2e_test_user"),
        content: MsgContent::text("Reply with exactly the word: PONG"),
        sender: "e2e_test_user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: qai_protocol::MsgSource::Human,
    };

    let json = serde_json::to_string(&inbound).unwrap();
    ws_write
        .send(WsMsg::Text(json.into()))
        .await
        .expect("WS send failed");

    // Wait for TurnComplete event (up to 120s for real API)
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(WsMsg::Text(text)) => {
                    if let Ok(AgentEvent::TurnComplete { full_text, .. }) =
                        serde_json::from_str::<AgentEvent>(text.as_str())
                    {
                        return Some(full_text);
                    }
                }
                Ok(_other) => {} // Binary/Ping/Pong frames — ignored
                Err(e) => panic!("WS read error: {e}"),
            }
        }
        None
    })
    .await;

    match result {
        Ok(Some(text)) => {
            println!("E2E response: {}", text);
            assert!(!text.is_empty(), "Expected non-empty response");
        }
        Ok(None) => panic!("WS stream ended without TurnComplete"),
        Err(_) => panic!("E2E test timed out after 120s"),
    }
}

/// E2E test: Gateway WS → quickai-claude-agent (ACP) → claude CLI → reply
///
/// Requires the `claude` CLI installed and authenticated with an Anthropic account.
/// The `quickai-claude-agent` binary must also be available.
///
/// To run:
///   QUICKAI_CLAUDE_AGENT_BIN=/path/to/quickai-claude-agent \
///   cargo test -p qai-server --test e2e_gateway -- test_gateway_e2e_claude_agent --ignored --nocapture
#[tokio::test]
#[ignore = "requires claude CLI authenticated with Anthropic account"]
async fn test_gateway_e2e_claude_agent() {
    // Install a default rustls CryptoProvider (required by tokio-tungstenite 0.26 / rustls 0.23).
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Resolve the quickai-claude-agent binary path.
    // Priority:
    //   1. QUICKAI_CLAUDE_AGENT_BIN env var (explicit override)
    //   2. <workspace-root>/../../quickai-claude-agent/target/debug/quickai-claude-agent
    //      (auto-detect when running from inside the gateway workspace)
    //   3. "quickai-claude-agent" — rely on PATH
    let agent_bin = std::env::var("QUICKAI_CLAUDE_AGENT_BIN").unwrap_or_else(|_| {
        // CARGO_MANIFEST_DIR points to qai-server's directory during tests.
        // Navigate up to the monorepo root and then into the claude-agent target dir.
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate =
            manifest_dir.join("../../../../quickai-claude-agent/target/debug/quickai-claude-agent");
        if candidate.exists() {
            candidate
                .canonicalize()
                .unwrap_or(candidate)
                .to_string_lossy()
                .to_string()
        } else {
            "quickai-claude-agent".to_string()
        }
    });

    eprintln!("test_gateway_e2e_claude_agent: using binary = {agent_bin}");

    let addr = start_test_gateway_with_backend(BackendSpec {
        backend_id: "claude-main".to_string(),
        family: BackendFamily::Acp,
        adapter_key: "acp".into(),
        launch: LaunchSpec::Command {
            command: agent_bin,
            args: vec![],
            env: vec![],
        },
    })
    .await
    .expect("Failed to start test gateway with Claude ACP backend");

    // Connect WebSocket client.
    let url = format!("ws://{}/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut ws_write, mut ws_read) = ws_stream.split();

    // Send a simple prompt that should elicit a short response.
    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: SessionKey::new("ws", "e2e_claude_test_user"),
        content: MsgContent::text("say hello in one word"),
        sender: "e2e_claude_test_user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: qai_protocol::MsgSource::Human,
    };

    let json = serde_json::to_string(&inbound).unwrap();
    ws_write
        .send(WsMsg::Text(json.into()))
        .await
        .expect("WS send failed");

    // Wait for TurnComplete event — 60 seconds (claude CLI can be slow to start).
    let result = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(WsMsg::Text(text)) => {
                    if let Ok(AgentEvent::TurnComplete { full_text, .. }) =
                        serde_json::from_str::<AgentEvent>(text.as_str())
                    {
                        return Some(full_text);
                    }
                }
                Ok(_other) => {} // Binary/Ping/Pong frames — ignored
                Err(e) => panic!("WS read error: {e}"),
            }
        }
        None
    })
    .await;

    match result {
        Ok(Some(text)) => {
            println!("E2E claude-agent response: {}", text);
            assert!(
                !text.is_empty(),
                "Expected non-empty response from claude-agent"
            );
        }
        Ok(None) => panic!("WS stream ended without TurnComplete"),
        Err(_) => panic!("E2E test timed out after 60s — is claude CLI running?"),
    }
}

async fn connect_ws(
    addr: std::net::SocketAddr,
) -> (
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMsg,
    >,
    futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) {
    let url = format!("ws://{}/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    ws_stream.split()
}

async fn subscribe_ws(
    ws_write: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMsg,
    >,
    session_key: &SessionKey,
) {
    let json = serde_json::json!({
        "type": "Subscribe",
        "session_key": session_key,
    })
    .to_string();
    ws_write
        .send(WsMsg::Text(json.into()))
        .await
        .expect("WS subscribe failed");
}

async fn send_inbound(
    ws_write: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMsg,
    >,
    inbound: &InboundMsg,
) {
    let json = serde_json::to_string(inbound).unwrap();
    ws_write
        .send(WsMsg::Text(json.into()))
        .await
        .expect("WS send failed");
}

async fn next_event(
    ws_read: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    timeout_secs: u64,
) -> AgentEvent {
    tokio::time::timeout(Duration::from_secs(timeout_secs), async {
        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(WsMsg::Text(text)) => {
                    if let Ok(event) = serde_json::from_str::<AgentEvent>(text.as_str()) {
                        return event;
                    }
                }
                Ok(_) => {}
                Err(e) => panic!("WS read error: {e}"),
            }
        }
        panic!("WS stream ended before receiving event");
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for event after {timeout_secs}s"))
}

fn approval_fixture_bin() -> String {
    std::env::var("CARGO_BIN_EXE_qai_acp_approval_fixture").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = manifest_dir.join("../../target/debug/qai_acp_approval_fixture");
        candidate
            .canonicalize()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    })
}

fn acp_echo_fixture_bin() -> String {
    std::env::var("CARGO_BIN_EXE_qai_acp_echo_fixture").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = manifest_dir.join("../../target/debug/qai_acp_echo_fixture");
        candidate
            .canonicalize()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    })
}

fn acp_team_fixture_bin() -> String {
    std::env::var("CARGO_BIN_EXE_qai_acp_team_fixture").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = manifest_dir.join("../../target/debug/qai_acp_team_fixture");
        candidate
            .canonicalize()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    })
}

fn native_echo_fixture_bin() -> String {
    std::env::var("CARGO_BIN_EXE_qai_native_echo_fixture").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = manifest_dir.join("../../target/debug/qai_native_echo_fixture");
        candidate
            .canonicalize()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    })
}

fn native_team_fixture_bin() -> String {
    std::env::var("CARGO_BIN_EXE_qai_native_team_fixture").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = manifest_dir.join("../../target/debug/qai_native_team_fixture");
        candidate
            .canonicalize()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    })
}

fn team_cli_bin() -> String {
    std::env::var("CARGO_BIN_EXE_qai-team-cli").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = manifest_dir.join("../../target/debug/qai-team-cli");
        candidate
            .canonicalize()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    })
}

fn openclaw_gateway_fixture_bin() -> String {
    std::env::var("CARGO_BIN_EXE_qai_openclaw_gateway_fixture").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = manifest_dir.join("../../target/debug/qai_openclaw_gateway_fixture");
        candidate
            .canonicalize()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    })
}

struct OpenClawFixtureChild {
    child: Child,
    endpoint: String,
}

impl OpenClawFixtureChild {
    async fn spawn() -> Self {
        let bin = openclaw_gateway_fixture_bin();
        let port = reserve_ephemeral_port();
        let endpoint = format!("ws://127.0.0.1:{port}/ws");
        let child = Command::new(bin)
            .arg("--port")
            .arg(port.to_string())
            .kill_on_drop(true)
            .spawn()
            .expect("failed to spawn openclaw gateway fixture");
        wait_for_openclaw_fixture(&endpoint).await;
        Self { child, endpoint }
    }

    async fn shutdown(&mut self) {
        let _ = self.child.kill().await;
    }
}

fn reserve_ephemeral_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

async fn wait_for_openclaw_fixture(endpoint: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if tokio::time::Instant::now() >= deadline {
            panic!("timed out waiting for openclaw fixture at {endpoint}");
        }
        match tokio_tungstenite::connect_async(endpoint).await {
            Ok((stream, _)) => {
                drop(stream);
                return;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

#[tokio::test]
async fn test_gateway_e2e_custom_acp_approval_via_ws_resolution() {
    let fixture = approval_fixture_bin();
    let addr = start_test_gateway_with_backend(BackendSpec {
        backend_id: "approval-fixture".to_string(),
        family: BackendFamily::Acp,
        adapter_key: "acp".into(),
        launch: LaunchSpec::Command {
            command: fixture,
            args: vec![],
            env: vec![],
        },
    })
    .await
    .expect("Failed to start gateway with approval fixture");

    let (mut ws_write, mut ws_read) = connect_ws(addr).await;
    let session_key = SessionKey::new("ws", "approval-e2e-ws");
    subscribe_ws(&mut ws_write, &session_key).await;

    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text("trigger approval"),
        sender: "approval-e2e-ws".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: qai_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &inbound).await;

    let approval = loop {
        match next_event(&mut ws_read, 20).await {
            AgentEvent::ApprovalRequest {
                session_key: event_session_key,
                approval_id,
                ..
            } => {
                assert_eq!(event_session_key, session_key);
                break approval_id;
            }
            AgentEvent::TextDelta { .. } | AgentEvent::Thinking { .. } => {}
            other => panic!("unexpected event before approval: {other:?}"),
        }
    };

    let resolve = serde_json::json!({
        "type": "ResolveApproval",
        "approval_id": approval,
        "decision": "allow-once",
    })
    .to_string();
    ws_write
        .send(WsMsg::Text(resolve.into()))
        .await
        .expect("WS resolve failed");

    loop {
        match next_event(&mut ws_read, 20).await {
            AgentEvent::TurnComplete { full_text, .. } => {
                assert_eq!(
                    full_text,
                    "fixture awaiting approvalapproved via allow-once"
                );
                break;
            }
            AgentEvent::TextDelta { .. } | AgentEvent::Thinking { .. } => {}
            other => panic!("unexpected event after approval resolve: {other:?}"),
        }
    }
}

#[tokio::test]
async fn test_gateway_e2e_custom_acp_approval_via_slash_command() {
    let fixture = approval_fixture_bin();
    let addr = start_test_gateway_with_backend(BackendSpec {
        backend_id: "approval-fixture".to_string(),
        family: BackendFamily::Acp,
        adapter_key: "acp".into(),
        launch: LaunchSpec::Command {
            command: fixture,
            args: vec![],
            env: vec![],
        },
    })
    .await
    .expect("Failed to start gateway with approval fixture");

    let (mut ws_write, mut ws_read) = connect_ws(addr).await;
    let session_key = SessionKey::new("ws", "approval-e2e-slash");
    subscribe_ws(&mut ws_write, &session_key).await;

    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text("trigger approval"),
        sender: "approval-e2e-slash".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: qai_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &inbound).await;

    let approval = loop {
        match next_event(&mut ws_read, 20).await {
            AgentEvent::ApprovalRequest {
                session_key: event_session_key,
                approval_id,
                ..
            } => {
                assert_eq!(event_session_key, session_key);
                break approval_id;
            }
            AgentEvent::TextDelta { .. } | AgentEvent::Thinking { .. } => {}
            other => panic!("unexpected event before approval: {other:?}"),
        }
    };

    let slash = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text(format!("/approve {approval} allow-once")),
        sender: "approval-e2e-slash".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: qai_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &slash).await;

    loop {
        match next_event(&mut ws_read, 20).await {
            AgentEvent::TurnComplete { full_text, .. } => {
                if full_text == "fixture awaiting approvalapproved via allow-once" {
                    break;
                }
                assert!(
                    full_text.contains("已处理审批"),
                    "unexpected turn complete after slash approval: {full_text}"
                );
            }
            AgentEvent::TextDelta { .. } | AgentEvent::Thinking { .. } => {}
            other => panic!("unexpected event after slash approval: {other:?}"),
        }
    }
}

#[tokio::test]
async fn test_gateway_e2e_mixed_backends_route_by_target_agent() {
    let native_bin = native_echo_fixture_bin();
    let acp_bin = acp_echo_fixture_bin();

    let mut cfg = GatewayConfig::default();
    cfg.agent.backend_id = "native-main".to_string();
    cfg.backends = vec![
        BackendCatalogEntry {
            id: "native-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            launch: BackendLaunchConfig::Command {
                command: native_bin,
                args: vec![],
                env: Default::default(),
            },
        },
        BackendCatalogEntry {
            id: "acp-main".into(),
            family: BackendFamilyConfig::Acp,
            adapter_key: None,
            launch: BackendLaunchConfig::Command {
                command: acp_bin,
                args: vec![],
                env: Default::default(),
            },
        },
    ];
    cfg.agent_roster = vec![
        AgentEntry {
            name: "native".into(),
            mentions: vec!["@native".into()],
            backend_id: "native-main".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
        AgentEntry {
            name: "acp".into(),
            mentions: vec!["@acp".into()],
            backend_id: "acp-main".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
    ];

    let addr = start_test_gateway_with_config(cfg)
        .await
        .expect("failed to start mixed-backend test gateway");

    let (mut ws_write, mut ws_read) = connect_ws(addr).await;
    let session_key = SessionKey::new("ws", "mixed-backends");
    subscribe_ws(&mut ws_write, &session_key).await;

    let native_inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text("hello-native"),
        sender: "mixed-backends".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: Some("@native".into()),
        source: qai_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &native_inbound).await;

    loop {
        match next_event(&mut ws_read, 20).await {
            AgentEvent::TurnComplete { full_text, .. } => {
                assert_eq!(full_text, "native:hello-native");
                break;
            }
            AgentEvent::TextDelta { .. } | AgentEvent::Thinking { .. } => {}
            other => panic!("unexpected event during native turn: {other:?}"),
        }
    }

    let acp_inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text("hello-acp"),
        sender: "mixed-backends".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: Some("@acp".into()),
        source: qai_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &acp_inbound).await;

    loop {
        match next_event(&mut ws_read, 20).await {
            AgentEvent::TurnComplete { full_text, .. } => {
                assert_eq!(full_text, "acp:fixture");
                break;
            }
            AgentEvent::TextDelta { .. } | AgentEvent::Thinking { .. } => {}
            other => panic!("unexpected event during ACP turn: {other:?}"),
        }
    }
}

#[tokio::test]
async fn test_gateway_e2e_native_team_pipeline() {
    let fixture = native_team_fixture_bin();
    let scope = format!("group:team-e2e:{}", uuid::Uuid::new_v4());

    let mut cfg = GatewayConfig::default();
    cfg.agent.backend_id = "leader-main".to_string();
    cfg.backends = vec![
        BackendCatalogEntry {
            id: "leader-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            launch: BackendLaunchConfig::Command {
                command: fixture.clone(),
                args: vec![],
                env: Default::default(),
            },
        },
        BackendCatalogEntry {
            id: "worker-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            launch: BackendLaunchConfig::Command {
                command: fixture,
                args: vec![],
                env: Default::default(),
            },
        },
    ];
    cfg.agent_roster = vec![
        AgentEntry {
            name: "leader".into(),
            mentions: vec!["@leader".into()],
            backend_id: "leader-main".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
        AgentEntry {
            name: "worker".into(),
            mentions: vec!["@worker".into()],
            backend_id: "worker-main".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
    ];
    cfg.groups = vec![qai_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-e2e".into()),
        mode: qai_server::config::GroupModeConfig {
            interaction: qai_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: qai_server::config::GroupTeamConfig {
            roster: vec!["worker".into()],
            ..Default::default()
        },
    }];

    let addr = start_test_gateway_with_config(cfg)
        .await
        .expect("failed to start native-team gateway");

    let (mut ws_write, mut ws_read) = connect_ws(addr).await;
    let session_key = SessionKey::new("ws", &scope);
    subscribe_ws(&mut ws_write, &session_key).await;

    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text("start the team run"),
        sender: "team-e2e-user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: qai_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &inbound).await;

    let mut saw_planned = false;
    let mut saw_accepted = false;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match next_event(&mut ws_read, 10).await {
            AgentEvent::TurnComplete { full_text, .. } => {
                if full_text == "leader:planned:T001" {
                    saw_planned = true;
                }
                if full_text == "leader:accepted:T001" || full_text == "leader:done" {
                    saw_accepted = true;
                    break;
                }
            }
            AgentEvent::TextDelta { .. } | AgentEvent::Thinking { .. } => {}
            other => panic!("unexpected native team event: {other:?}"),
        }
    }

    assert!(saw_planned, "leader planning turn did not complete");
    assert!(saw_accepted, "leader acceptance turn did not complete");
}

#[tokio::test]
async fn test_gateway_e2e_mixed_team_native_leader_acp_specialist() {
    let leader_fixture = native_team_fixture_bin();
    let worker_fixture = acp_team_fixture_bin();
    let scope = format!("group:team-mixed-e2e:{}", uuid::Uuid::new_v4());

    let mut cfg = GatewayConfig::default();
    cfg.agent.backend_id = "leader-main".to_string();
    cfg.backends = vec![
        BackendCatalogEntry {
            id: "leader-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            launch: BackendLaunchConfig::Command {
                command: leader_fixture,
                args: vec![],
                env: Default::default(),
            },
        },
        BackendCatalogEntry {
            id: "worker-main".into(),
            family: BackendFamilyConfig::Acp,
            adapter_key: None,
            launch: BackendLaunchConfig::Command {
                command: worker_fixture,
                args: vec![],
                env: Default::default(),
            },
        },
    ];
    cfg.agent_roster = vec![
        AgentEntry {
            name: "leader".into(),
            mentions: vec!["@leader".into()],
            backend_id: "leader-main".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
        AgentEntry {
            name: "worker".into(),
            mentions: vec!["@worker".into()],
            backend_id: "worker-main".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
    ];
    cfg.groups = vec![qai_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-mixed-e2e".into()),
        mode: qai_server::config::GroupModeConfig {
            interaction: qai_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: qai_server::config::GroupTeamConfig {
            roster: vec!["worker".into()],
            ..Default::default()
        },
    }];

    let addr = start_test_gateway_with_config(cfg)
        .await
        .expect("failed to start mixed-team gateway");

    let (mut ws_write, mut ws_read) = connect_ws(addr).await;
    let session_key = SessionKey::new("ws", &scope);
    subscribe_ws(&mut ws_write, &session_key).await;

    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text("start the mixed backend team run"),
        sender: "team-mixed-e2e-user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: qai_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &inbound).await;

    let mut saw_planned = false;
    let mut saw_accepted = false;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match next_event(&mut ws_read, 15).await {
            AgentEvent::TurnComplete { full_text, .. } => {
                if full_text == "leader:planned:T001" {
                    saw_planned = true;
                }
                if full_text == "leader:accepted:T001" || full_text == "leader:done" {
                    saw_accepted = true;
                    break;
                }
            }
            AgentEvent::TextDelta { .. } | AgentEvent::Thinking { .. } => {}
            other => panic!("unexpected mixed team event: {other:?}"),
        }
    }

    assert!(saw_planned, "leader planning turn did not complete");
    assert!(saw_accepted, "leader acceptance turn did not complete");
}

#[tokio::test]
async fn test_gateway_e2e_mixed_team_native_leader_openclaw_specialist() {
    let leader_fixture = native_team_fixture_bin();
    let helper_bin = team_cli_bin();
    let mut openclaw = OpenClawFixtureChild::spawn().await;
    let scope = format!("group:team-openclaw-e2e:{}", uuid::Uuid::new_v4());

    let mut cfg = GatewayConfig::default();
    cfg.agent.backend_id = "leader-main".to_string();
    cfg.backends = vec![
        BackendCatalogEntry {
            id: "leader-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            launch: BackendLaunchConfig::Command {
                command: leader_fixture,
                args: vec![],
                env: Default::default(),
            },
        },
        BackendCatalogEntry {
            id: "worker-openclaw".into(),
            family: BackendFamilyConfig::OpenClawGateway,
            adapter_key: None,
            launch: BackendLaunchConfig::GatewayWs {
                endpoint: openclaw.endpoint.clone(),
                token: None,
                password: None,
                role: Some("operator".into()),
                scopes: vec!["operator.admin".into()],
                agent_id: Some("worker".into()),
                team_helper_command: Some(helper_bin),
                team_helper_args: vec![],
                lead_helper_mode: false,
            },
        },
    ];
    cfg.agent_roster = vec![
        AgentEntry {
            name: "leader".into(),
            mentions: vec!["@leader".into()],
            backend_id: "leader-main".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
        AgentEntry {
            name: "worker".into(),
            mentions: vec!["@worker".into()],
            backend_id: "worker-openclaw".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
    ];
    cfg.groups = vec![qai_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-openclaw-e2e".into()),
        mode: qai_server::config::GroupModeConfig {
            interaction: qai_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: qai_server::config::GroupTeamConfig {
            roster: vec!["worker".into()],
            ..Default::default()
        },
    }];

    let addr = start_test_gateway_with_config(cfg)
        .await
        .expect("failed to start openclaw mixed-team gateway");

    let (mut ws_write, mut ws_read) = connect_ws(addr).await;
    let session_key = SessionKey::new("ws", &scope);
    subscribe_ws(&mut ws_write, &session_key).await;

    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text("start the openclaw mixed backend team run"),
        sender: "team-openclaw-e2e-user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: qai_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &inbound).await;

    let mut saw_planned = false;
    let mut saw_help = false;
    let mut saw_checkpoint = false;
    let mut saw_accepted = false;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match next_event(&mut ws_read, 15).await {
            AgentEvent::TurnComplete { full_text, .. } => {
                if full_text == "leader:planned:T001" {
                    saw_planned = true;
                }
                if full_text == "leader:help:T001" {
                    saw_help = true;
                }
                if full_text == "leader:checkpoint:T001" {
                    saw_checkpoint = true;
                }
                if full_text == "leader:accepted:T001" || full_text == "leader:done" {
                    saw_accepted = true;
                    break;
                }
            }
            AgentEvent::TextDelta { .. } | AgentEvent::Thinking { .. } => {}
            other => panic!("unexpected openclaw mixed team event: {other:?}"),
        }
    }

    openclaw.shutdown().await;
    assert!(saw_planned, "leader planning turn did not complete");
    assert!(saw_help, "leader help turn did not complete");
    assert!(saw_checkpoint, "leader checkpoint turn did not complete");
    assert!(saw_accepted, "leader acceptance turn did not complete");
}

#[tokio::test]
async fn test_gateway_e2e_mixed_team_openclaw_leader_native_specialist() {
    let specialist_fixture = native_team_fixture_bin();
    let helper_bin = team_cli_bin();
    let mut openclaw = OpenClawFixtureChild::spawn().await;
    let scope = format!("group:team-openclaw-lead-native:{}", uuid::Uuid::new_v4());

    let mut cfg = GatewayConfig::default();
    cfg.agent.backend_id = "leader-openclaw".to_string();
    cfg.backends = vec![
        BackendCatalogEntry {
            id: "leader-openclaw".into(),
            family: BackendFamilyConfig::OpenClawGateway,
            adapter_key: None,
            launch: BackendLaunchConfig::GatewayWs {
                endpoint: openclaw.endpoint.clone(),
                token: None,
                password: None,
                role: Some("operator".into()),
                scopes: vec!["operator.admin".into()],
                agent_id: Some("leader".into()),
                team_helper_command: Some(helper_bin),
                team_helper_args: vec![],
                lead_helper_mode: true,
            },
        },
        BackendCatalogEntry {
            id: "worker-native".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            launch: BackendLaunchConfig::Command {
                command: specialist_fixture,
                args: vec![],
                env: Default::default(),
            },
        },
    ];
    cfg.agent_roster = vec![
        AgentEntry {
            name: "leader".into(),
            mentions: vec!["@leader".into()],
            backend_id: "leader-openclaw".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
        AgentEntry {
            name: "worker".into(),
            mentions: vec!["@worker".into()],
            backend_id: "worker-native".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
    ];
    cfg.groups = vec![qai_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-openclaw-lead-native".into()),
        mode: qai_server::config::GroupModeConfig {
            interaction: qai_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: qai_server::config::GroupTeamConfig {
            roster: vec!["worker".into()],
            ..Default::default()
        },
    }];

    let addr = start_test_gateway_with_config(cfg)
        .await
        .expect("failed to start openclaw-lead native-team gateway");

    let (mut ws_write, mut ws_read) = connect_ws(addr).await;
    let session_key = SessionKey::new("ws", &scope);
    subscribe_ws(&mut ws_write, &session_key).await;

    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text("start the openclaw leader mixed backend team run"),
        sender: "team-openclaw-lead-native-user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: qai_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &inbound).await;

    let mut saw_planned = false;
    let mut saw_accepted = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match next_event(&mut ws_read, 15).await {
            AgentEvent::TurnComplete { full_text, .. } => {
                if full_text == "openclaw-leader:planned:T001" {
                    saw_planned = true;
                }
                if full_text == "openclaw-leader:accepted:T001" || full_text == "leader:done" {
                    saw_accepted = true;
                    break;
                }
            }
            AgentEvent::TextDelta { .. } | AgentEvent::Thinking { .. } => {}
            other => panic!("unexpected openclaw leader/native worker event: {other:?}"),
        }
    }

    openclaw.shutdown().await;
    assert!(
        saw_planned,
        "openclaw leader planning turn did not complete"
    );
    assert!(
        saw_accepted,
        "openclaw leader acceptance turn did not complete"
    );
}

#[tokio::test]
async fn test_gateway_e2e_mixed_team_openclaw_leader_acp_specialist() {
    let specialist_fixture = acp_team_fixture_bin();
    let helper_bin = team_cli_bin();
    let mut openclaw = OpenClawFixtureChild::spawn().await;
    let scope = format!("group:team-openclaw-lead-acp:{}", uuid::Uuid::new_v4());

    let mut cfg = GatewayConfig::default();
    cfg.agent.backend_id = "leader-openclaw".to_string();
    cfg.backends = vec![
        BackendCatalogEntry {
            id: "leader-openclaw".into(),
            family: BackendFamilyConfig::OpenClawGateway,
            adapter_key: None,
            launch: BackendLaunchConfig::GatewayWs {
                endpoint: openclaw.endpoint.clone(),
                token: None,
                password: None,
                role: Some("operator".into()),
                scopes: vec!["operator.admin".into()],
                agent_id: Some("leader".into()),
                team_helper_command: Some(helper_bin),
                team_helper_args: vec![],
                lead_helper_mode: true,
            },
        },
        BackendCatalogEntry {
            id: "worker-acp".into(),
            family: BackendFamilyConfig::Acp,
            adapter_key: None,
            launch: BackendLaunchConfig::Command {
                command: specialist_fixture,
                args: vec![],
                env: Default::default(),
            },
        },
    ];
    cfg.agent_roster = vec![
        AgentEntry {
            name: "leader".into(),
            mentions: vec!["@leader".into()],
            backend_id: "leader-openclaw".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
        AgentEntry {
            name: "worker".into(),
            mentions: vec!["@worker".into()],
            backend_id: "worker-acp".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
    ];
    cfg.groups = vec![qai_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-openclaw-lead-acp".into()),
        mode: qai_server::config::GroupModeConfig {
            interaction: qai_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: qai_server::config::GroupTeamConfig {
            roster: vec!["worker".into()],
            ..Default::default()
        },
    }];

    let addr = start_test_gateway_with_config(cfg)
        .await
        .expect("failed to start openclaw-lead acp-team gateway");

    let (mut ws_write, mut ws_read) = connect_ws(addr).await;
    let session_key = SessionKey::new("ws", &scope);
    subscribe_ws(&mut ws_write, &session_key).await;

    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text("start the openclaw leader acp team run"),
        sender: "team-openclaw-lead-acp-user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: qai_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &inbound).await;

    let mut saw_planned = false;
    let mut saw_accepted = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match next_event(&mut ws_read, 15).await {
            AgentEvent::TurnComplete { full_text, .. } => {
                if full_text == "openclaw-leader:planned:T001" {
                    saw_planned = true;
                }
                if full_text == "openclaw-leader:accepted:T001" || full_text == "leader:done" {
                    saw_accepted = true;
                    break;
                }
            }
            AgentEvent::TextDelta { .. } | AgentEvent::Thinking { .. } => {}
            other => panic!("unexpected openclaw leader/acp worker event: {other:?}"),
        }
    }

    openclaw.shutdown().await;
    assert!(
        saw_planned,
        "openclaw leader planning turn did not complete"
    );
    assert!(
        saw_accepted,
        "openclaw leader acceptance turn did not complete"
    );
}

#[tokio::test]
async fn test_registry_mixed_team_native_leader_acp_specialist_pipeline() {
    let leader_fixture = native_team_fixture_bin();
    let worker_fixture = acp_team_fixture_bin();
    let scope = format!("group:team-mixed-registry:{}", uuid::Uuid::new_v4());

    let mut cfg = GatewayConfig::default();
    cfg.agent.backend_id = "leader-main".to_string();
    cfg.backends = vec![
        BackendCatalogEntry {
            id: "leader-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            launch: BackendLaunchConfig::Command {
                command: leader_fixture,
                args: vec![],
                env: Default::default(),
            },
        },
        BackendCatalogEntry {
            id: "worker-main".into(),
            family: BackendFamilyConfig::Acp,
            adapter_key: None,
            launch: BackendLaunchConfig::Command {
                command: worker_fixture,
                args: vec![],
                env: Default::default(),
            },
        },
    ];
    cfg.agent_roster = vec![
        AgentEntry {
            name: "leader".into(),
            mentions: vec!["@leader".into()],
            backend_id: "leader-main".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
        AgentEntry {
            name: "worker".into(),
            mentions: vec!["@worker".into()],
            backend_id: "worker-main".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
    ];
    cfg.groups = vec![qai_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-mixed-registry".into()),
        mode: qai_server::config::GroupModeConfig {
            interaction: qai_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: qai_server::config::GroupTeamConfig {
            roster: vec!["worker".into()],
            ..Default::default()
        },
    }];

    let state = build_test_state_with_config(cfg)
        .await
        .expect("failed to build mixed-team app state");
    let addr = gateway::server::start(state.clone(), "127.0.0.1", 0)
        .await
        .expect("failed to start gateway server");
    state.registry.set_team_tool_url(format!(
        "http://127.0.0.1:{}/runtime/team-tools?token={}",
        addr.port(),
        state.runtime_token
    ));

    let mut event_rx = state.event_tx.subscribe();
    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: SessionKey::new("ws", &scope),
        content: MsgContent::text("start the mixed backend team run"),
        sender: "team-mixed-registry-user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: qai_protocol::MsgSource::Human,
    };

    let handle_result = state.registry.handle(inbound).await;
    assert!(
        handle_result.is_ok(),
        "registry.handle failed: {handle_result:?}"
    );

    let mut saw_planned = false;
    let mut saw_accepted = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), event_rx.recv()).await {
            Ok(Ok(AgentEvent::TurnComplete { full_text, .. })) => {
                if full_text == "leader:planned:T001" {
                    saw_planned = true;
                }
                if full_text == "leader:accepted:T001" || full_text == "leader:done" {
                    saw_accepted = true;
                    break;
                }
            }
            Ok(Ok(AgentEvent::Error { message, .. })) => {
                panic!("unexpected runtime error: {message}");
            }
            Ok(Ok(_)) => {}
            Ok(Err(err)) => panic!("event channel recv failed: {err}"),
            Err(_) => {}
        }
    }

    assert!(saw_planned, "leader planning turn did not complete");
    assert!(saw_accepted, "leader acceptance turn did not complete");
}

#[tokio::test]
async fn test_registry_mixed_team_native_leader_openclaw_specialist_pipeline() {
    let leader_fixture = native_team_fixture_bin();
    let helper_bin = team_cli_bin();
    let mut openclaw = OpenClawFixtureChild::spawn().await;
    let scope = format!("group:team-openclaw-registry:{}", uuid::Uuid::new_v4());

    let mut cfg = GatewayConfig::default();
    cfg.agent.backend_id = "leader-main".to_string();
    cfg.backends = vec![
        BackendCatalogEntry {
            id: "leader-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            launch: BackendLaunchConfig::Command {
                command: leader_fixture,
                args: vec![],
                env: Default::default(),
            },
        },
        BackendCatalogEntry {
            id: "worker-openclaw".into(),
            family: BackendFamilyConfig::OpenClawGateway,
            adapter_key: None,
            launch: BackendLaunchConfig::GatewayWs {
                endpoint: openclaw.endpoint.clone(),
                token: None,
                password: None,
                role: Some("operator".into()),
                scopes: vec!["operator.admin".into()],
                agent_id: Some("worker".into()),
                team_helper_command: Some(helper_bin),
                team_helper_args: vec![],
                lead_helper_mode: false,
            },
        },
    ];
    cfg.agent_roster = vec![
        AgentEntry {
            name: "leader".into(),
            mentions: vec!["@leader".into()],
            backend_id: "leader-main".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
        AgentEntry {
            name: "worker".into(),
            mentions: vec!["@worker".into()],
            backend_id: "worker-openclaw".into(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        },
    ];
    cfg.groups = vec![qai_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-openclaw-registry".into()),
        mode: qai_server::config::GroupModeConfig {
            interaction: qai_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: qai_server::config::GroupTeamConfig {
            roster: vec!["worker".into()],
            ..Default::default()
        },
    }];

    let state = build_test_state_with_config(cfg)
        .await
        .expect("failed to build openclaw mixed-team app state");
    let addr = gateway::server::start(state.clone(), "127.0.0.1", 0)
        .await
        .expect("failed to start gateway server");
    state.registry.set_team_tool_url(format!(
        "http://127.0.0.1:{}/runtime/team-tools?token={}",
        addr.port(),
        state.runtime_token
    ));

    let mut event_rx = state.event_tx.subscribe();
    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: SessionKey::new("ws", &scope),
        content: MsgContent::text("start the openclaw mixed backend team run"),
        sender: "team-openclaw-registry-user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: qai_protocol::MsgSource::Human,
    };

    let handle_result = state.registry.handle(inbound).await;
    assert!(
        handle_result.is_ok(),
        "registry.handle failed: {handle_result:?}"
    );

    let mut saw_planned = false;
    let mut saw_help = false;
    let mut saw_checkpoint = false;
    let mut saw_accepted = false;
    let mut last_text = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), event_rx.recv()).await {
            Ok(Ok(AgentEvent::TurnComplete { full_text, .. })) => {
                last_text = Some(full_text.clone());
                if full_text == "leader:planned:T001" {
                    saw_planned = true;
                }
                if full_text == "leader:help:T001" {
                    saw_help = true;
                }
                if full_text == "leader:checkpoint:T001" {
                    saw_checkpoint = true;
                }
                if full_text == "leader:accepted:T001" || full_text == "leader:done" {
                    saw_accepted = true;
                    break;
                }
            }
            Ok(Ok(AgentEvent::Error { message, .. })) => {
                panic!("unexpected runtime error: {message}; last_text={last_text:?}");
            }
            Ok(Ok(_)) => {}
            Ok(Err(err)) => panic!("event channel recv failed: {err}; last_text={last_text:?}"),
            Err(_) => {}
        }
    }

    openclaw.shutdown().await;
    assert!(saw_planned, "leader planning turn did not complete");
    assert!(saw_help, "leader help turn did not complete");
    assert!(saw_checkpoint, "leader checkpoint turn did not complete");
    assert!(
        saw_accepted,
        "leader acceptance turn did not complete; last_text={last_text:?}"
    );
}
