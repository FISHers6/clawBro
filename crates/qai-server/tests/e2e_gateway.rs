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
use qai_protocol::{AgentEvent, InboundMsg, MsgContent, SessionKey};
use qai_server::{start_test_gateway, start_test_gateway_with_engine};
use std::time::Duration;
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

    // Build engine config using the ClaudeAgent variant.
    let engine_config = qai_agent::EngineConfig::ClaudeAgent {
        binary: Some(agent_bin),
    };

    // Start the gateway with a ClaudeAgent engine.
    let addr = start_test_gateway_with_engine(engine_config)
        .await
        .expect("Failed to start test gateway with ClaudeAgent engine");

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
