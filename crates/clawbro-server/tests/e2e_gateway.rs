//! E2E tests: Gateway WS integration tests
//!
//! ## test_gateway_e2e_deepseek
//! Skipped automatically if OPENAI_API_KEY is not set.
//!
//! To run:
//!   OPENAI_API_KEY=sk-xxx \
//!   OPENAI_API_BASE=https://api.deepseek.com \
//!   CLAWBRO_MODEL=deepseek-chat \
//!   CLAWBRO_RUST_AGENT_BIN=/path/to/clawbro-rust-agent \
//!   cargo test -p clawbro-server --test e2e_gateway -- --nocapture
//!
//! ## legacy test_gateway_e2e_clawbro_claude_agent
//! Legacy coverage only. The active Claude product path uses `claude-agent-acp`.
//! Requires `claude` CLI installed and authenticated.
//!
//! To run:
//!   LEGACY_CLAWBRO_CLAUDE_AGENT=1 \
//!   CLAWBRO_CLAUDE_AGENT_BIN=/path/to/clawbro-claude-agent \
//!   cargo test -p clawbro-server --test e2e_gateway -- test_gateway_e2e_legacy_clawbro_claude_agent --ignored --nocapture

use futures_util::{SinkExt, StreamExt};
use clawbro_agent::roster::AgentEntry;
use clawbro_protocol::{AgentEvent, InboundMsg, MsgContent, SessionKey};
use clawbro_runtime::{AcpBackend, ApprovalMode, BackendFamily, BackendSpec, LaunchSpec};
use clawbro_server::{
    build_test_state_with_config,
    config::{BackendCatalogEntry, BackendFamilyConfig, BackendLaunchConfig, GatewayConfig},
    gateway, start_test_gateway, start_test_gateway_with_backend, start_test_gateway_with_config,
};
use std::collections::BTreeSet;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio_tungstenite::tungstenite::Message as WsMsg;

#[derive(Debug, Default)]
struct TurnTrace {
    approvals: usize,
    text_deltas: usize,
    thinking_events: usize,
    tool_names: BTreeSet<String>,
    tool_start_ids: BTreeSet<String>,
    tool_result_ids: BTreeSet<String>,
    tool_failures: Vec<String>,
    final_text: Option<String>,
}

impl TurnTrace {
    fn record(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::TextDelta { .. } => self.text_deltas += 1,
            AgentEvent::ApprovalRequest { .. } => self.approvals += 1,
            AgentEvent::ToolCallStart {
                tool_name, call_id, ..
            } => {
                self.tool_names.insert(tool_name.clone());
                self.tool_start_ids.insert(call_id.clone());
            }
            AgentEvent::ToolCallResult { call_id, .. } => {
                self.tool_result_ids.insert(call_id.clone());
            }
            AgentEvent::ToolCallFailed {
                tool_name,
                call_id,
                error,
                ..
            } => {
                self.tool_failures
                    .push(format!("{tool_name}:{call_id}:{error}"));
            }
            AgentEvent::Thinking { .. } => self.thinking_events += 1,
            AgentEvent::TurnComplete { full_text, .. } => {
                self.final_text = Some(full_text.clone());
            }
            AgentEvent::Error { message, .. } => {
                self.tool_failures.push(format!("runtime_error:{message}"));
            }
        }
    }

    fn summary(&self, label: &str) -> String {
        format!(
            "{label}: approvals={}, text_deltas={}, thinking={}, tool_names={:?}, tool_start_ids={:?}, tool_result_ids={:?}, tool_failures={:?}, final_text={:?}",
            self.approvals,
            self.text_deltas,
            self.thinking_events,
            self.tool_names,
            self.tool_start_ids,
            self.tool_result_ids,
            self.tool_failures,
            self.final_text
        )
    }
}

#[tokio::test]
#[ignore = "requires OPENAI_API_KEY - run with: cargo test -p clawbro-server --test e2e_gateway -- --ignored --nocapture"]
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

    // OPENAI_API_BASE and CLAWBRO_MODEL must be set by the caller.
    // Example: OPENAI_API_BASE=https://api.deepseek.com CLAWBRO_MODEL=deepseek-chat
    // Not mutated here to avoid unsafe set_var in async context.

    let agent_bin = std::env::var("CLAWBRO_RUST_AGENT_BIN")
        .unwrap_or_else(|_| "clawbro-rust-agent".to_string());

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
        source: clawbro_protocol::MsgSource::Human,
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

/// Legacy E2E test: Gateway WS → clawbro-claude-agent (ACP) → claude CLI → reply
///
/// This path is no longer part of the active ClawBro product runtime matrix.
/// It remains only as a legacy compatibility check when explicitly enabled.
///
/// To run:
///   LEGACY_CLAWBRO_CLAUDE_AGENT=1 \
///   CLAWBRO_CLAUDE_AGENT_BIN=/path/to/clawbro-claude-agent \
///   cargo test -p clawbro-server --test e2e_gateway -- test_gateway_e2e_legacy_clawbro_claude_agent --ignored --nocapture
#[tokio::test]
#[ignore = "requires claude CLI authenticated with Anthropic account"]
async fn test_gateway_e2e_legacy_clawbro_claude_agent() {
    if std::env::var("LEGACY_CLAWBRO_CLAUDE_AGENT").ok().as_deref() != Some("1") {
        eprintln!(
            "SKIP test_gateway_e2e_legacy_clawbro_claude_agent: set LEGACY_CLAWBRO_CLAUDE_AGENT=1 to run the deprecated clawbro-claude-agent path"
        );
        return;
    }

    // Install a default rustls CryptoProvider (required by tokio-tungstenite 0.26 / rustls 0.23).
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Resolve the clawbro-claude-agent binary path.
    // Priority:
    //   1. CLAWBRO_CLAUDE_AGENT_BIN env var (explicit override)
    //   2. <workspace-root>/../../clawbro-claude-agent/target/debug/clawbro-claude-agent
    //      (auto-detect when running from inside the gateway workspace)
    //   3. "clawbro-claude-agent" — rely on PATH
    let agent_bin = std::env::var("CLAWBRO_CLAUDE_AGENT_BIN").unwrap_or_else(|_| {
        // CARGO_MANIFEST_DIR points to clawbro-server's directory during tests.
        // Navigate up to the monorepo root and then into the claude-agent target dir.
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate =
            manifest_dir.join("../../../../clawbro-claude-agent/target/debug/clawbro-claude-agent");
        if candidate.exists() {
            candidate
                .canonicalize()
                .unwrap_or(candidate)
                .to_string_lossy()
                .to_string()
        } else {
            "clawbro-claude-agent".to_string()
        }
    });

    eprintln!("test_gateway_e2e_legacy_clawbro_claude_agent: using binary = {agent_bin}");

    let addr = start_test_gateway_with_backend(BackendSpec {
        backend_id: "claude-main".to_string(),
        family: BackendFamily::Acp,
        adapter_key: "acp".into(),
        launch: LaunchSpec::ExternalCommand {
            command: agent_bin,
            args: vec![],
            env: vec![],
        },
        external_mcp_servers: vec![],
        provider_profile: None,
        acp_backend: None,
        acp_auth_method: None,
        codex_projection: None,
        approval_mode: Default::default(),
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
        source: clawbro_protocol::MsgSource::Human,
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

/// E2E test: Gateway WS -> codex-acp bridge -> codex CLI -> tool approval -> final reply
///
/// Requires:
/// - `codex` CLI installed and logged in on the local machine
/// - network access for `npx @zed-industries/codex-acp@0.9.5`
///
/// To run:
///   cargo test -p clawbro-server --test e2e_gateway -- test_gateway_e2e_codex_bridge --ignored --nocapture
#[tokio::test]
#[ignore = "requires codex CLI authenticated locally and codex-acp bridge via npx"]
async fn test_gateway_e2e_codex_bridge() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let workspace = std::env::current_dir()
        .expect("current dir")
        .canonicalize()
        .expect("canonical workspace");

    let mut cfg = GatewayConfig::default();
    cfg.gateway.default_workspace = Some(workspace.clone());
    cfg.agent.backend_id = "codex-main".to_string();
    let (acp_auth_method, launch_env) = if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
        if api_key.is_empty() {
            panic!("OPENAI_API_KEY is set but empty");
        }
        let mut env = std::collections::BTreeMap::new();
        env.insert("OPENAI_API_KEY".into(), api_key);
        (
            Some(clawbro_server::config::AcpAuthMethodConfig::OpenaiApiKey),
            env,
        )
    } else if let Ok(api_key) = std::env::var("CODEX_API_KEY") {
        if api_key.is_empty() {
            panic!("CODEX_API_KEY is set but empty");
        }
        let mut env = std::collections::BTreeMap::new();
        env.insert("CODEX_API_KEY".into(), api_key);
        (
            Some(clawbro_server::config::AcpAuthMethodConfig::CodexApiKey),
            env,
        )
    } else {
        let mut env = std::collections::BTreeMap::new();
        env.insert("HOME".into(), "/Users/fishers".into());
        (Some(clawbro_server::config::AcpAuthMethodConfig::Chatgpt), env)
    };
    cfg.backends.push(BackendCatalogEntry {
        id: "codex-main".into(),
        family: BackendFamilyConfig::Acp,
        adapter_key: None,
        acp_backend: Some(clawbro_server::config::AcpBackendConfig::Codex),
        acp_auth_method,
        codex: None,
        provider_profile: None,
        approval: Default::default(),
        external_mcp_servers: vec![],
        launch: BackendLaunchConfig::ExternalCommand {
            command: "npx".into(),
            args: vec!["--yes".into(), "@zed-industries/codex-acp@0.9.5".into()],
            env: launch_env,
        },
    });

    let addr = start_test_gateway_with_config(cfg)
        .await
        .expect("Failed to start test gateway with Codex ACP backend");

    let (mut ws_write, mut ws_read) = connect_ws(addr).await;
    let session_key = SessionKey::new("ws", "e2e_codex_bridge_user");
    subscribe_ws(&mut ws_write, &session_key).await;

    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text("Reply with exactly the word: PONG"),
        sender: "e2e_codex_bridge_user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: clawbro_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &inbound).await;

    let result = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            match next_event(&mut ws_read, 120).await {
                AgentEvent::TurnComplete { full_text, .. } => break full_text,
                AgentEvent::ApprovalRequest { approval_id, .. } => {
                    let resolve = serde_json::json!({
                        "type": "ResolveApproval",
                        "approval_id": approval_id,
                        "decision": "allow-once",
                    })
                    .to_string();
                    ws_write
                        .send(WsMsg::Text(resolve.into()))
                        .await
                        .expect("WS codex text-turn resolve failed");
                }
                AgentEvent::TextDelta { .. }
                | AgentEvent::Thinking { .. }
                | AgentEvent::ToolCallStart { .. }
                | AgentEvent::ToolCallResult { .. }
                | AgentEvent::ToolCallFailed { .. } => {}
                other => panic!("unexpected event during codex text turn: {other:?}"),
            }
        }
    })
    .await
    .expect("timed out waiting for codex text turn");
    assert!(
        result.trim().contains("PONG"),
        "expected PONG-like reply, got: {result}"
    );

    let proof = format!("CODEX_BRIDGE_PROOF_{}", uuid::Uuid::new_v4().simple());
    let proof_path = std::env::temp_dir().join("clawbro-codex-e2e-proof.txt");
    std::fs::write(&proof_path, &proof).expect("write proof file");

    let tool_inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text(format!(
            "Read the file {} and reply with its exact contents only.",
            proof_path.display()
        )),
        sender: "e2e_codex_bridge_user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: clawbro_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &tool_inbound).await;

    let approval_id = loop {
        match next_event(&mut ws_read, 120).await {
            AgentEvent::ApprovalRequest { approval_id, .. } => break approval_id,
            AgentEvent::TextDelta { .. }
            | AgentEvent::Thinking { .. }
            | AgentEvent::ToolCallStart { .. }
            | AgentEvent::ToolCallResult { .. } => {}
            other => panic!("unexpected event before codex approval: {other:?}"),
        }
    };

    let resolve = serde_json::json!({
        "type": "ResolveApproval",
        "approval_id": approval_id,
        "decision": "allow-once",
    })
    .to_string();
    ws_write
        .send(WsMsg::Text(resolve.into()))
        .await
        .expect("WS codex resolve failed");

    let final_text = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            match next_event(&mut ws_read, 120).await {
                AgentEvent::TurnComplete { full_text, .. } => break full_text,
                AgentEvent::TextDelta { .. }
                | AgentEvent::Thinking { .. }
                | AgentEvent::ToolCallStart { .. }
                | AgentEvent::ToolCallResult { .. }
                | AgentEvent::ToolCallFailed { .. } => {}
                other => panic!("unexpected event after codex approval: {other:?}"),
            }
        }
    })
    .await
    .expect("timed out waiting for codex tool turn");

    assert!(
        final_text.contains(&proof),
        "expected final codex reply to contain proof token, got: {final_text}"
    );
}

/// E2E test: Gateway WS -> codex-acp bridge -> Codex local_config_projection ->
/// DeepSeek OpenAI-compatible provider -> tool approval -> final reply
///
/// Requires:
/// - `codex` CLI installed locally
/// - `DEEPSEEK_API_KEY` set to a valid key
/// - network access for `npx @zed-industries/codex-acp@0.9.5`
///
/// To run:
///   DEEPSEEK_API_KEY=sk-xxx \
///   cargo test -p clawbro-server --test e2e_gateway -- test_gateway_e2e_codex_local_config_deepseek --ignored --nocapture
#[tokio::test]
#[ignore = "requires codex CLI plus DEEPSEEK_API_KEY for local_config_projection"]
async fn test_gateway_e2e_codex_local_config_deepseek() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let api_key = match std::env::var("DEEPSEEK_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!(
                "SKIP test_gateway_e2e_codex_local_config_deepseek: DEEPSEEK_API_KEY not set"
            );
            return;
        }
    };
    let _ = api_key;

    let workspace = std::env::current_dir()
        .expect("current dir")
        .canonicalize()
        .expect("canonical workspace");

    let mut cfg = GatewayConfig::default();
    cfg.gateway.default_workspace = Some(workspace.clone());
    cfg.agent.backend_id = "codex-main".to_string();
    cfg.provider_profiles
        .push(clawbro_server::config::ProviderProfileConfig {
            id: "deepseek-openai".into(),
            protocol: clawbro_server::config::ProviderProfileProtocolConfig::OpenaiCompatible {
                base_url: "https://api.deepseek.com/v1".into(),
                auth_token_env: "DEEPSEEK_API_KEY".into(),
                default_model: "deepseek-chat".into(),
            },
        });
    cfg.backends.push(BackendCatalogEntry {
        id: "codex-main".into(),
        family: BackendFamilyConfig::Acp,
        adapter_key: None,
        acp_backend: Some(clawbro_server::config::AcpBackendConfig::Codex),
        acp_auth_method: None,
        codex: Some(clawbro_server::config::BackendCodexConfig {
            projection: clawbro_runtime::CodexProjectionMode::LocalConfig,
        }),
        provider_profile: Some("deepseek-openai".into()),
        approval: Default::default(),
        external_mcp_servers: vec![],
        launch: BackendLaunchConfig::ExternalCommand {
            command: "npx".into(),
            args: vec!["--yes".into(), "@zed-industries/codex-acp@0.9.5".into()],
            env: std::collections::BTreeMap::new(),
        },
    });

    let addr = start_test_gateway_with_config(cfg)
        .await
        .expect("Failed to start test gateway with Codex local_config backend");

    let (mut ws_write, mut ws_read) = connect_ws(addr).await;
    let session_key = SessionKey::new("ws", "e2e_codex_local_config_user");
    subscribe_ws(&mut ws_write, &session_key).await;

    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text("Reply with exactly the word: PONG"),
        sender: "e2e_codex_local_config_user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: clawbro_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &inbound).await;

    let result = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            match next_event(&mut ws_read, 120).await {
                AgentEvent::TurnComplete { full_text, .. } => break full_text,
                AgentEvent::ApprovalRequest { approval_id, .. } => {
                    let resolve = serde_json::json!({
                        "type": "ResolveApproval",
                        "approval_id": approval_id,
                        "decision": "allow-once",
                    })
                    .to_string();
                    ws_write
                        .send(WsMsg::Text(resolve.into()))
                        .await
                        .expect("WS codex local_config text-turn resolve failed");
                }
                AgentEvent::TextDelta { .. }
                | AgentEvent::Thinking { .. }
                | AgentEvent::ToolCallStart { .. }
                | AgentEvent::ToolCallResult { .. }
                | AgentEvent::ToolCallFailed { .. } => {}
                other => panic!("unexpected event during codex local_config text turn: {other:?}"),
            }
        }
    })
    .await
    .expect("timed out waiting for codex local_config text turn");
    assert!(
        result.trim().contains("PONG"),
        "expected PONG-like reply, got: {result}"
    );

    let proof = format!("CODEX_LOCAL_CONFIG_PROOF_{}", uuid::Uuid::new_v4().simple());
    let proof_path = std::env::temp_dir().join("clawbro-codex-local-config-proof.txt");
    std::fs::write(&proof_path, &proof).expect("write proof file");

    let tool_inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text(format!(
            "Read the file {} and reply with its exact contents only.",
            proof_path.display()
        )),
        sender: "e2e_codex_local_config_user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: clawbro_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &tool_inbound).await;

    let approval_id = loop {
        match next_event(&mut ws_read, 120).await {
            AgentEvent::ApprovalRequest { approval_id, .. } => break approval_id,
            AgentEvent::TextDelta { .. }
            | AgentEvent::Thinking { .. }
            | AgentEvent::ToolCallStart { .. }
            | AgentEvent::ToolCallResult { .. } => {}
            other => panic!("unexpected event before codex local_config approval: {other:?}"),
        }
    };

    let resolve = serde_json::json!({
        "type": "ResolveApproval",
        "approval_id": approval_id,
        "decision": "allow-once",
    })
    .to_string();
    ws_write
        .send(WsMsg::Text(resolve.into()))
        .await
        .expect("WS codex local_config resolve failed");

    let final_text = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            match next_event(&mut ws_read, 120).await {
                AgentEvent::TurnComplete { full_text, .. } => break full_text,
                AgentEvent::TextDelta { .. }
                | AgentEvent::Thinking { .. }
                | AgentEvent::ToolCallStart { .. }
                | AgentEvent::ToolCallResult { .. }
                | AgentEvent::ToolCallFailed { .. } => {}
                other => panic!("unexpected event after codex local_config approval: {other:?}"),
            }
        }
    })
    .await
    .expect("timed out waiting for codex local_config tool turn");

    assert!(
        final_text.contains(&proof),
        "expected final codex local_config reply to contain proof token, got: {final_text}"
    );
}

/// Run manually with:
///   OPENAI_API_KEY=... cargo test -p clawbro-server --test e2e_gateway -- test_gateway_e2e_codex_local_config_aicodewith --ignored --nocapture
#[tokio::test]
#[ignore = "requires codex CLI plus OPENAI_API_KEY for local_config_projection"]
async fn test_gateway_e2e_codex_local_config_aicodewith() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!(
                "SKIP test_gateway_e2e_codex_local_config_aicodewith: OPENAI_API_KEY not set"
            );
            return;
        }
    };
    let _ = api_key;

    let workspace = std::env::current_dir()
        .expect("current dir")
        .canonicalize()
        .expect("canonical workspace");

    let mut cfg = GatewayConfig::default();
    cfg.gateway.default_workspace = Some(workspace.clone());
    cfg.agent.backend_id = "codex-main".to_string();
    cfg.provider_profiles
        .push(clawbro_server::config::ProviderProfileConfig {
            id: "aicodewith-openai".into(),
            protocol: clawbro_server::config::ProviderProfileProtocolConfig::OpenaiCompatible {
                base_url: "https://api.aicodewith.com/chatgpt/v1".into(),
                auth_token_env: "OPENAI_API_KEY".into(),
                default_model: "gpt-5.3-codex".into(),
            },
        });
    cfg.backends.push(BackendCatalogEntry {
        id: "codex-main".into(),
        family: BackendFamilyConfig::Acp,
        adapter_key: None,
        acp_backend: Some(clawbro_server::config::AcpBackendConfig::Codex),
        // local_config projection pre-writes CODEX_HOME/auth.json and also injects
        // OPENAI_API_KEY into the child process env so that the authenticate() call
        // (openai_api_key) can confirm auth to codex-acp before it starts processing.
        acp_auth_method: Some(clawbro_server::config::AcpAuthMethodConfig::OpenaiApiKey),
        codex: Some(clawbro_server::config::BackendCodexConfig {
            projection: clawbro_runtime::CodexProjectionMode::LocalConfig,
        }),
        provider_profile: Some("aicodewith-openai".into()),
        approval: Default::default(),
        external_mcp_servers: vec![],
        launch: BackendLaunchConfig::ExternalCommand {
            command: "npx".into(),
            args: vec!["--yes".into(), "@zed-industries/codex-acp".into()],
            env: std::collections::BTreeMap::new(),
        },
    });

    let addr = start_test_gateway_with_config(cfg)
        .await
        .expect("Failed to start test gateway with Codex local_config backend");

    let (mut ws_write, mut ws_read) = connect_ws(addr).await;
    // Use a unique session key per test run to ensure new_session() is always called
    // instead of load_session() with a stale prior session ID.
    let session_key = SessionKey::new(
        "ws",
        format!("e2e_codex_aicodewith_{}", uuid::Uuid::new_v4().simple()),
    );
    subscribe_ws(&mut ws_write, &session_key).await;

    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text("Reply with exactly the word: PONG"),
        sender: "e2e_codex_local_config_aicodewith_user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: clawbro_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &inbound).await;

    let mut text_turn_trace = TurnTrace::default();
    let result = tokio::time::timeout(Duration::from_secs(240), async {
        loop {
            let event = next_event(&mut ws_read, 240).await;
            text_turn_trace.record(&event);
            match event {
                AgentEvent::TurnComplete { full_text, .. } => break full_text,
                AgentEvent::ApprovalRequest { approval_id, .. } => {
                    let resolve = serde_json::json!({
                        "type": "ResolveApproval",
                        "approval_id": approval_id,
                        "decision": "allow-once",
                    })
                    .to_string();
                    ws_write
                        .send(WsMsg::Text(resolve.into()))
                        .await
                        .expect("WS codex aicodewith text-turn resolve failed");
                }
                AgentEvent::TextDelta { .. }
                | AgentEvent::Thinking { .. }
                | AgentEvent::ToolCallStart { .. }
                | AgentEvent::ToolCallResult { .. }
                | AgentEvent::ToolCallFailed { .. } => {}
                other => panic!("unexpected event during codex aicodewith text turn: {other:?}"),
            }
        }
    })
    .await
    .expect("timed out waiting for codex aicodewith text turn");
    eprintln!("{}", text_turn_trace.summary("codex_aicodewith_text_turn"));
    assert!(
        result.trim().contains("PONG"),
        "expected PONG-like reply, got: {result}"
    );
    assert!(
        text_turn_trace.tool_failures.is_empty(),
        "text turn should not fail during system processing: {}",
        text_turn_trace.summary("codex_aicodewith_text_turn")
    );

    let proof = format!("CODEX_AICODEWITH_PROOF_{}", uuid::Uuid::new_v4().simple());
    let proof_path = std::env::temp_dir().join("clawbro-codex-aicodewith-proof.txt");
    std::fs::write(&proof_path, &proof).expect("write proof file");

    let tool_inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text(format!(
            "Read the file {} and reply with its exact contents only.",
            proof_path.display()
        )),
        sender: "e2e_codex_local_config_aicodewith_user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: clawbro_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &tool_inbound).await;

    // codex-acp may or may not call request_permission for file reads depending
    // on its internal policy. Handle both: approval-gated and auto-approved paths.
    let mut tool_turn_trace = TurnTrace::default();
    let final_text = tokio::time::timeout(Duration::from_secs(240), async {
        loop {
            let event = next_event(&mut ws_read, 240).await;
            tool_turn_trace.record(&event);
            match event {
                AgentEvent::TurnComplete { full_text, .. } => break full_text,
                AgentEvent::ApprovalRequest { approval_id, .. } => {
                    let resolve = serde_json::json!({
                        "type": "ResolveApproval",
                        "approval_id": approval_id,
                        "decision": "allow-once",
                    })
                    .to_string();
                    ws_write
                        .send(WsMsg::Text(resolve.into()))
                        .await
                        .expect("WS codex aicodewith resolve failed");
                }
                AgentEvent::TextDelta { .. }
                | AgentEvent::Thinking { .. }
                | AgentEvent::ToolCallStart { .. }
                | AgentEvent::ToolCallResult { .. }
                | AgentEvent::ToolCallFailed { .. } => {}
                other => panic!("unexpected event during codex aicodewith tool turn: {other:?}"),
            }
        }
    })
    .await
    .expect("timed out waiting for codex aicodewith tool turn");
    eprintln!("{}", tool_turn_trace.summary("codex_aicodewith_tool_turn"));

    assert!(
        final_text.contains(&proof),
        "expected final codex aicodewith reply to contain proof token, got: {final_text}"
    );
    assert!(
        !tool_turn_trace.tool_start_ids.is_empty(),
        "expected file-read turn to emit ToolCallStart: {}",
        tool_turn_trace.summary("codex_aicodewith_tool_turn")
    );
    assert!(
        !tool_turn_trace.tool_result_ids.is_empty(),
        "expected file-read turn to emit ToolCallResult: {}",
        tool_turn_trace.summary("codex_aicodewith_tool_turn")
    );
    assert!(
        tool_turn_trace.tool_failures.is_empty(),
        "tool turn should not emit ToolCallFailed/Error: {}",
        tool_turn_trace.summary("codex_aicodewith_tool_turn")
    );
    assert!(
        tool_turn_trace
            .tool_start_ids
            .iter()
            .all(|call_id| tool_turn_trace.tool_result_ids.contains(call_id)),
        "every started tool call should resolve successfully: {}",
        tool_turn_trace.summary("codex_aicodewith_tool_turn")
    );
}

#[tokio::test]
async fn test_gateway_ws_inbound_auto_subscribes_socket_to_session() {
    let fixture = native_echo_fixture_bin();
    let addr = start_test_gateway_with_backend(BackendSpec {
        backend_id: "native-echo".to_string(),
        family: BackendFamily::QuickAiNative,
        adapter_key: "native".into(),
        launch: LaunchSpec::ExternalCommand {
            command: fixture,
            args: vec![],
            env: vec![],
        },
        external_mcp_servers: vec![],
        provider_profile: None,
        acp_backend: None,
        acp_auth_method: None,
        codex_projection: None,
        approval_mode: Default::default(),
    })
    .await
    .expect("Failed to start gateway with native echo fixture");

    let (mut ws_write, mut ws_read) = connect_ws(addr).await;
    let session_key = SessionKey::new("ws", "auto-subscribe");
    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text("hello-auto-subscribe"),
        sender: "auto-subscribe".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: clawbro_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &inbound).await;

    loop {
        match next_event(&mut ws_read, 20).await {
            AgentEvent::TurnComplete {
                session_id: _,
                sender: _,
                full_text,
            } => {
                assert_eq!(full_text, "native:hello-auto-subscribe");
                break;
            }
            AgentEvent::TextDelta { .. } | AgentEvent::Thinking { .. } => {}
            other => panic!("unexpected event without explicit subscribe: {other:?}"),
        }
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
    std::env::var("CARGO_BIN_EXE_clawbro_acp_approval_fixture").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = manifest_dir.join("../../target/debug/clawbro_acp_approval_fixture");
        candidate
            .canonicalize()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    })
}

fn acp_echo_fixture_bin() -> String {
    std::env::var("CARGO_BIN_EXE_clawbro_acp_echo_fixture").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = manifest_dir.join("../../target/debug/clawbro_acp_echo_fixture");
        candidate
            .canonicalize()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    })
}

fn acp_team_fixture_bin() -> String {
    std::env::var("CARGO_BIN_EXE_clawbro_acp_team_fixture").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = manifest_dir.join("../../target/debug/clawbro_acp_team_fixture");
        candidate
            .canonicalize()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    })
}

fn native_echo_fixture_bin() -> String {
    std::env::var("CARGO_BIN_EXE_clawbro_native_echo_fixture").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = manifest_dir.join("../../target/debug/clawbro_native_echo_fixture");
        candidate
            .canonicalize()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    })
}

fn native_team_fixture_bin() -> String {
    std::env::var("CARGO_BIN_EXE_clawbro_native_team_fixture").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = manifest_dir.join("../../target/debug/clawbro_native_team_fixture");
        candidate
            .canonicalize()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    })
}

fn native_team_missing_completion_fixture_bin() -> String {
    std::env::var("CARGO_BIN_EXE_clawbro_native_team_missing_completion_fixture").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate =
            manifest_dir.join("../../target/debug/clawbro_native_team_missing_completion_fixture");
        candidate
            .canonicalize()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    })
}

fn team_cli_bin() -> String {
    std::env::var("CARGO_BIN_EXE_clawbro-team-cli").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = manifest_dir.join("../../target/debug/clawbro-team-cli");
        candidate
            .canonicalize()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    })
}

fn team_id_for_scope(scope: &str) -> String {
    clawbro_agent::team::session::stable_team_id("ws", scope)
}

fn openclaw_gateway_fixture_bin() -> String {
    std::env::var("CARGO_BIN_EXE_clawbro_openclaw_gateway_fixture").unwrap_or_else(|_| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = manifest_dir.join("../../target/debug/clawbro_openclaw_gateway_fixture");
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
        launch: LaunchSpec::ExternalCommand {
            command: fixture,
            args: vec![],
            env: vec![],
        },
        external_mcp_servers: vec![],
        provider_profile: None,
        acp_backend: None,
        acp_auth_method: None,
        codex_projection: None,
        approval_mode: Default::default(),
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
        source: clawbro_protocol::MsgSource::Human,
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
            AgentEvent::TextDelta { .. }
            | AgentEvent::Thinking { .. }
            | AgentEvent::ToolCallStart { .. }
            | AgentEvent::ToolCallResult { .. }
            | AgentEvent::ToolCallFailed { .. } => {}
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
        launch: LaunchSpec::ExternalCommand {
            command: fixture,
            args: vec![],
            env: vec![],
        },
        external_mcp_servers: vec![],
        provider_profile: None,
        acp_backend: None,
        acp_auth_method: None,
        codex_projection: None,
        approval_mode: Default::default(),
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
        source: clawbro_protocol::MsgSource::Human,
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
        source: clawbro_protocol::MsgSource::Human,
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
            AgentEvent::TextDelta { .. }
            | AgentEvent::Thinking { .. }
            | AgentEvent::ToolCallStart { .. }
            | AgentEvent::ToolCallResult { .. }
            | AgentEvent::ToolCallFailed { .. } => {}
            other => panic!("unexpected event after slash approval: {other:?}"),
        }
    }
}

#[tokio::test]
async fn test_gateway_e2e_codex_auto_allow_projects_mode_on_new_and_load_session() {
    let fixture = approval_fixture_bin();
    let addr = start_test_gateway_with_backend(BackendSpec {
        backend_id: "codex-fixture".to_string(),
        family: BackendFamily::Acp,
        adapter_key: "acp".into(),
        launch: LaunchSpec::ExternalCommand {
            command: fixture,
            args: vec![],
            env: vec![],
        },
        external_mcp_servers: vec![],
        provider_profile: None,
        acp_backend: Some(AcpBackend::Codex),
        acp_auth_method: None,
        codex_projection: None,
        approval_mode: ApprovalMode::AutoAllow,
    })
    .await
    .expect("Failed to start gateway with codex approval fixture");

    let (mut ws_write, mut ws_read) = connect_ws(addr).await;
    let session_key = SessionKey::new("ws", "codex-auto-allow");
    subscribe_ws(&mut ws_write, &session_key).await;

    for turn_idx in 0..2 {
        let inbound = InboundMsg {
            id: uuid::Uuid::new_v4().to_string(),
            session_key: session_key.clone(),
            content: MsgContent::text(format!("turn-{turn_idx}")),
            sender: "codex-auto-allow".to_string(),
            channel: "ws".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: clawbro_protocol::MsgSource::Human,
        };
        send_inbound(&mut ws_write, &inbound).await;

        loop {
            match next_event(&mut ws_read, 20).await {
                AgentEvent::TurnComplete { full_text, .. } => {
                    assert_eq!(full_text, "fixture full-access:0");
                    break;
                }
                AgentEvent::ApprovalRequest { .. } => {
                    panic!("codex auto-allow turn {turn_idx} should not request approval")
                }
                AgentEvent::TextDelta { .. }
                | AgentEvent::Thinking { .. }
                | AgentEvent::ToolCallStart { .. }
                | AgentEvent::ToolCallResult { .. }
                | AgentEvent::ToolCallFailed { .. } => {}
                other => panic!("unexpected event during codex auto-allow turn: {other:?}"),
            }
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
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "native-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
                command: native_bin,
                args: vec![],
                env: Default::default(),
            },
        },
        BackendCatalogEntry {
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "acp-main".into(),
            family: BackendFamilyConfig::Acp,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
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
        source: clawbro_protocol::MsgSource::Human,
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
        source: clawbro_protocol::MsgSource::Human,
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
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "leader-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
                command: fixture.clone(),
                args: vec![],
                env: Default::default(),
            },
        },
        BackendCatalogEntry {
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "worker-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
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
    cfg.groups = vec![clawbro_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-e2e".into()),
        mode: clawbro_server::config::GroupModeConfig {
            interaction: clawbro_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: clawbro_server::config::GroupTeamConfig {
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
        source: clawbro_protocol::MsgSource::Human,
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
async fn test_gateway_e2e_native_dm_team_pipeline() {
    let fixture = native_team_fixture_bin();
    let scope = format!("user:team-dm-e2e:{}", uuid::Uuid::new_v4());

    let mut cfg = GatewayConfig::default();
    cfg.agent.backend_id = "leader-main".to_string();
    cfg.backends = vec![
        BackendCatalogEntry {
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "leader-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
                command: fixture.clone(),
                args: vec![],
                env: Default::default(),
            },
        },
        BackendCatalogEntry {
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "worker-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
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
    cfg.team_scopes = vec![clawbro_server::config::TeamScopeConfig {
        scope: scope.clone(),
        name: Some("dm-team-e2e".into()),
        mode: clawbro_server::config::GroupModeConfig {
            interaction: clawbro_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: clawbro_server::config::GroupTeamConfig {
            roster: vec!["worker".into()],
            ..Default::default()
        },
    }];

    let addr = start_test_gateway_with_config(cfg)
        .await
        .expect("failed to start native dm-team gateway");

    let (mut ws_write, mut ws_read) = connect_ws(addr).await;
    let session_key = SessionKey::new("ws", &scope);
    subscribe_ws(&mut ws_write, &session_key).await;

    let inbound = InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: session_key.clone(),
        content: MsgContent::text("start the dm team run"),
        sender: "team-dm-e2e-user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: clawbro_protocol::MsgSource::Human,
    };
    send_inbound(&mut ws_write, &inbound).await;

    let mut saw_planned = false;
    let mut saw_accepted = false;
    let mut saw_worker_visible = false;

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
                if full_text.starts_with("worker:") {
                    saw_worker_visible = true;
                }
            }
            AgentEvent::TextDelta { .. } | AgentEvent::Thinking { .. } => {}
            other => panic!("unexpected native dm team event: {other:?}"),
        }
    }

    assert!(saw_planned, "leader planning turn did not complete");
    assert!(saw_accepted, "leader acceptance turn did not complete");
    assert!(
        !saw_worker_visible,
        "dm team should not surface raw specialist chatter to the user session"
    );
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
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "leader-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
                command: leader_fixture,
                args: vec![],
                env: Default::default(),
            },
        },
        BackendCatalogEntry {
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "worker-main".into(),
            family: BackendFamilyConfig::Acp,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
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
    cfg.groups = vec![clawbro_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-mixed-e2e".into()),
        mode: clawbro_server::config::GroupModeConfig {
            interaction: clawbro_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: clawbro_server::config::GroupTeamConfig {
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
        source: clawbro_protocol::MsgSource::Human,
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
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "leader-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
                command: leader_fixture,
                args: vec![],
                env: Default::default(),
            },
        },
        BackendCatalogEntry {
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "worker-openclaw".into(),
            family: BackendFamilyConfig::OpenClawGateway,
            adapter_key: None,
            external_mcp_servers: vec![],
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
    cfg.groups = vec![clawbro_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-openclaw-e2e".into()),
        mode: clawbro_server::config::GroupModeConfig {
            interaction: clawbro_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: clawbro_server::config::GroupTeamConfig {
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
        source: clawbro_protocol::MsgSource::Human,
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
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "leader-openclaw".into(),
            family: BackendFamilyConfig::OpenClawGateway,
            adapter_key: None,
            external_mcp_servers: vec![],
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
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "worker-native".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
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
    cfg.groups = vec![clawbro_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-openclaw-lead-native".into()),
        mode: clawbro_server::config::GroupModeConfig {
            interaction: clawbro_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: clawbro_server::config::GroupTeamConfig {
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
        source: clawbro_protocol::MsgSource::Human,
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
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "leader-openclaw".into(),
            family: BackendFamilyConfig::OpenClawGateway,
            adapter_key: None,
            external_mcp_servers: vec![],
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
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "worker-acp".into(),
            family: BackendFamilyConfig::Acp,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
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
    cfg.groups = vec![clawbro_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-openclaw-lead-acp".into()),
        mode: clawbro_server::config::GroupModeConfig {
            interaction: clawbro_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: clawbro_server::config::GroupTeamConfig {
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
        source: clawbro_protocol::MsgSource::Human,
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
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "leader-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
                command: leader_fixture,
                args: vec![],
                env: Default::default(),
            },
        },
        BackendCatalogEntry {
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "worker-main".into(),
            family: BackendFamilyConfig::Acp,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
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
    cfg.groups = vec![clawbro_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-mixed-registry".into()),
        mode: clawbro_server::config::GroupModeConfig {
            interaction: clawbro_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: clawbro_server::config::GroupTeamConfig {
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
        source: clawbro_protocol::MsgSource::Human,
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
async fn test_registry_team_missing_completion_resets_claim_and_specialist_session() {
    let leader_fixture = native_team_fixture_bin();
    let worker_fixture = native_team_missing_completion_fixture_bin();
    let scope = format!("group:team-missing-completion:{}", uuid::Uuid::new_v4());

    let mut cfg = GatewayConfig::default();
    cfg.agent.backend_id = "leader-main".to_string();
    cfg.backends = vec![
        BackendCatalogEntry {
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "leader-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
                command: leader_fixture,
                args: vec![],
                env: Default::default(),
            },
        },
        BackendCatalogEntry {
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "worker-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
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
    cfg.groups = vec![clawbro_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-missing-completion".into()),
        mode: clawbro_server::config::GroupModeConfig {
            interaction: clawbro_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: clawbro_server::config::GroupTeamConfig {
            roster: vec!["worker".into()],
            ..Default::default()
        },
    }];

    let state = build_test_state_with_config(cfg)
        .await
        .expect("failed to build missing-completion app state");
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
        content: MsgContent::text("start missing completion flow"),
        sender: "team-missing-completion-user".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: clawbro_protocol::MsgSource::Human,
    };

    let handle_result = state.registry.handle(inbound).await;
    assert!(
        handle_result.is_ok(),
        "registry.handle failed: {handle_result:?}"
    );

    let mut saw_planned = false;
    let mut saw_missing = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), event_rx.recv()).await {
            Ok(Ok(AgentEvent::TurnComplete { full_text, .. })) => {
                if full_text == "leader:planned:T001" {
                    saw_planned = true;
                }
                if full_text == "leader:missing:T001" {
                    saw_missing = true;
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
    assert!(
        saw_missing,
        "leader did not receive missing-completion notification turn"
    );

    let team_summary = state
        .registry
        .team_summaries()
        .into_iter()
        .find(|summary| summary.lead_session_key == Some(SessionKey::new("ws", &scope)))
        .expect("expected team summary for missing completion scope");
    assert_eq!(team_summary.task_counts.total, 1);
    assert_eq!(team_summary.task_counts.pending, 1);
    assert_eq!(team_summary.task_counts.claimed, 0);
    assert_eq!(team_summary.task_counts.done, 0);
    assert_eq!(team_summary.task_counts.submitted, 0);

    let specialist_key = SessionKey::new(
        "specialist",
        format!("{}:worker", team_id_for_scope(&scope)),
    );
    let specialist_session_id = state
        .registry
        .session_manager_ref()
        .get_or_create(&specialist_key)
        .await
        .expect("expected specialist session id");
    let specialist_messages = state
        .registry
        .session_manager_ref()
        .storage()
        .load_recent_messages(specialist_session_id, 10)
        .await
        .expect("expected specialist messages to load");
    assert!(
        specialist_messages.is_empty(),
        "specialist transcript should be reset after missing completion, got: {:?}",
        specialist_messages
    );
    let specialist_meta = state
        .registry
        .session_manager_ref()
        .load_meta(specialist_session_id)
        .await
        .expect("expected specialist meta to load")
        .expect("specialist meta should exist");
    assert!(
        specialist_meta.backend_session_ids.is_empty(),
        "specialist backend session bindings should be reset after missing completion"
    );
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
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "leader-main".into(),
            family: BackendFamilyConfig::QuickAiNative,
            adapter_key: None,
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::ExternalCommand {
                command: leader_fixture,
                args: vec![],
                env: Default::default(),
            },
        },
        BackendCatalogEntry {
            acp_backend: None,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            id: "worker-openclaw".into(),
            family: BackendFamilyConfig::OpenClawGateway,
            adapter_key: None,
            external_mcp_servers: vec![],
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
    cfg.groups = vec![clawbro_server::config::GroupConfig {
        scope: scope.clone(),
        name: Some("team-openclaw-registry".into()),
        mode: clawbro_server::config::GroupModeConfig {
            interaction: clawbro_server::config::InteractionMode::Team,
            auto_promote: false,
            front_bot: Some("leader".into()),
            channel: Some("ws".into()),
        },
        team: clawbro_server::config::GroupTeamConfig {
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
        source: clawbro_protocol::MsgSource::Human,
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
