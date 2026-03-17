//! ACP integration tests for clawbro-rust-agent.
//!
//! Task 2: Stub handshake tests (no API key required)
//! Task 7: Full rig integration test (requires ANTHROPIC_API_KEY, #[ignore] by default)

use acp::Agent as _;
use agent_client_protocol as acp;
use std::process::Stdio;
use tokio::process::Command;
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

/// Client that collects received text chunks
struct CollectingClient {
    responses: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
}

#[async_trait::async_trait(?Send)]
impl acp::Client for CollectingClient {
    async fn request_permission(
        &self,
        _args: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        Ok(acp::RequestPermissionResponse::new(
            acp::RequestPermissionOutcome::Cancelled,
        ))
    }

    async fn session_notification(
        &self,
        notification: acp::SessionNotification,
    ) -> acp::Result<()> {
        if let acp::SessionUpdate::AgentMessageChunk(chunk) = notification.update {
            if let acp::ContentBlock::Text(t) = chunk.content {
                self.responses.borrow_mut().push(t.text);
            }
        }
        Ok(())
    }
}

/// Spawn binary without any API keys (forces stub / echo mode)
fn spawn_stub() -> tokio::process::Child {
    Command::new(env!("CARGO_BIN_EXE_clawbro-rust-agent"))
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("DEEPSEEK_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn clawbro-rust-agent")
}

// ─── Task 2: ACP Handshake (stub mode) ───────────────────────────────────────

/// Full ACP handshake: initialize → new_session → prompt → EndTurn + echo reply
#[tokio::test(flavor = "current_thread")]
async fn test_acp_handshake() {
    let mut child = spawn_stub();
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let responses = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let responses_clone = responses.clone();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let (conn, handle_io) = acp::ClientSideConnection::new(
                CollectingClient {
                    responses: responses_clone,
                },
                stdin.compat_write(),
                stdout.compat(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            tokio::task::spawn_local(handle_io);

            // 1. initialize
            let init_resp = conn
                .initialize(acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                .await
                .expect("initialize failed");
            assert_eq!(init_resp.protocol_version, acp::ProtocolVersion::V1);
            assert_eq!(
                init_resp.agent_info.as_ref().map(|i| i.name.as_str()),
                Some("clawbro-rust-agent"),
            );

            // 2. new_session
            let session = conn
                .new_session(acp::NewSessionRequest::new(std::path::PathBuf::from(".")))
                .await
                .expect("new_session failed");
            assert!(
                !session.session_id.0.is_empty(),
                "session_id must not be empty"
            );

            // 3. prompt — stub echoes back
            let resp = conn
                .prompt(acp::PromptRequest::new(
                    session.session_id.clone(),
                    vec![acp::ContentBlock::Text(acp::TextContent::new(
                        "hello world",
                    ))],
                ))
                .await
                .expect("prompt failed");
            assert_eq!(resp.stop_reason, acp::StopReason::EndTurn);
        })
        .await;

    let collected = responses.borrow().join("");
    assert!(
        collected.contains("Echo"),
        "Expected echo notification, got: {collected:?}"
    );

    child.kill().await.ok();
}

/// Each call to new_session should return a unique session ID
#[tokio::test(flavor = "current_thread")]
async fn test_multiple_sessions() {
    let mut child = spawn_stub();
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let (conn, handle_io) = acp::ClientSideConnection::new(
                CollectingClient {
                    responses: std::rc::Rc::new(std::cell::RefCell::new(vec![])),
                },
                stdin.compat_write(),
                stdout.compat(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            tokio::task::spawn_local(handle_io);

            conn.initialize(acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                .await
                .expect("initialize failed");

            let s1 = conn
                .new_session(acp::NewSessionRequest::new("."))
                .await
                .expect("new_session 1 failed");
            let s2 = conn
                .new_session(acp::NewSessionRequest::new("."))
                .await
                .expect("new_session 2 failed");

            assert_ne!(
                s1.session_id.0, s2.session_id.0,
                "session IDs must be unique"
            );
        })
        .await;

    child.kill().await.ok();
}

/// Empty prompt (no content blocks) should return EndTurn without crashing
#[tokio::test(flavor = "current_thread")]
async fn test_empty_prompt_returns_end_turn() {
    let mut child = spawn_stub();
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let (conn, handle_io) = acp::ClientSideConnection::new(
                CollectingClient {
                    responses: std::rc::Rc::new(std::cell::RefCell::new(vec![])),
                },
                stdin.compat_write(),
                stdout.compat(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            tokio::task::spawn_local(handle_io);

            conn.initialize(acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                .await
                .unwrap();

            let session = conn
                .new_session(acp::NewSessionRequest::new("."))
                .await
                .unwrap();

            let resp = conn
                .prompt(acp::PromptRequest::new(session.session_id, vec![]))
                .await
                .unwrap();

            assert_eq!(resp.stop_reason, acp::StopReason::EndTurn);
        })
        .await;

    child.kill().await.ok();
}

// ─── Task 7: Full rig integration test (requires ANTHROPIC_API_KEY) ──────────

/// Full rig-core test via ACP: spawn agent with real API key, send a prompt,
/// verify a non-empty response is received as a session_notification.
///
/// Run with:
///   ANTHROPIC_API_KEY=sk-... cargo test --test acp_handshake test_full_rig_prompt -- --include-ignored
#[tokio::test(flavor = "current_thread")]
#[ignore = "requires ANTHROPIC_API_KEY env var"]
async fn test_full_rig_prompt() {
    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY not set");

    let mut child = Command::new(env!("CARGO_BIN_EXE_clawbro-rust-agent"))
        .env("ANTHROPIC_API_KEY", &api_key)
        .env("CLAWBRO_MODEL", "claude-haiku-4-5-20251001")
        .env("CLAWBRO_SYSTEM_PROMPT", "Reply in one word only.")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn clawbro-rust-agent");

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let chunks = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
    let chunks_clone = chunks.clone();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let (conn, handle_io) = acp::ClientSideConnection::new(
                CollectingClient {
                    responses: chunks_clone,
                },
                stdin.compat_write(),
                stdout.compat(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            tokio::task::spawn_local(handle_io);

            conn.initialize(acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                .await
                .unwrap();

            let session = conn
                .new_session(acp::NewSessionRequest::new("."))
                .await
                .unwrap();

            let resp = conn
                .prompt(acp::PromptRequest::new(
                    session.session_id,
                    vec!["Say hello in one word".into()],
                ))
                .await
                .unwrap();

            assert_eq!(resp.stop_reason, acp::StopReason::EndTurn);
        })
        .await;

    let full_response = chunks.borrow().join("");
    assert!(
        !full_response.is_empty(),
        "Expected non-empty LLM response, got empty"
    );
    println!("Agent response: {full_response}");

    child.kill().await.ok();
}
