use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use qai_runtime::openclaw::client::GatewayFrame;
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::process::Command;
use uuid::Uuid;

#[derive(Clone, Default)]
struct FixtureState {
    approvals_file: Arc<Mutex<Value>>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("qai-openclaw-gateway-fixture: {err:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut port = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => port = args.next().and_then(|value| value.parse::<u16>().ok()),
            "--help" | "-h" => {
                println!("qai_openclaw_gateway_fixture --port <port>");
                return Ok(());
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }
    let port = port.ok_or_else(|| anyhow!("--port is required"))?;
    let state = FixtureState {
        approvals_file: Arc::new(Mutex::new(json!({
            "version": 1,
            "agents": {}
        }))),
    };
    let app = Router::new()
        .route("/ws", get(ws_upgrade))
        .with_state(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind fixture gateway on {addr}"))?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<FixtureState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: FixtureState) {
    let _ = send_frame(
        &mut socket,
        GatewayFrame::Event {
            event: "connect.challenge".into(),
            payload: Some(json!({ "nonce": "fixture-nonce" })),
            seq: None,
            state_version: None,
        },
    )
    .await;

    while let Some(Ok(message)) = socket.recv().await {
        let Message::Text(text) = message else {
            continue;
        };
        let Ok(frame) = serde_json::from_str::<GatewayFrame>(&text) else {
            continue;
        };
        let GatewayFrame::Request { id, method, params } = frame else {
            continue;
        };
        if let Err(err) = handle_request(&mut socket, &state, id, method, params).await {
            let _ = send_frame(
                &mut socket,
                GatewayFrame::Response {
                    id: Uuid::new_v4().to_string(),
                    ok: false,
                    payload: None,
                    error: Some(qai_runtime::openclaw::client::GatewayErrorShape {
                        code: Some("fixture_error".into()),
                        message: Some(err.to_string()),
                    }),
                    final_flag: None,
                },
            )
            .await;
        }
    }
}

async fn handle_request(
    socket: &mut WebSocket,
    state: &FixtureState,
    id: String,
    method: String,
    params: Option<Value>,
) -> Result<()> {
    match method.as_str() {
        "connect" => {
            send_frame(
                socket,
                GatewayFrame::Response {
                    id,
                    ok: true,
                    payload: Some(json!({
                        "type": "hello-ok",
                        "protocol": 3
                    })),
                    error: None,
                    final_flag: None,
                },
            )
            .await?;
        }
        "sessions.resolve" => {
            let key = params
                .as_ref()
                .and_then(|v| v.get("key"))
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("sessions.resolve missing key"))?;
            send_frame(
                socket,
                GatewayFrame::Response {
                    id,
                    ok: true,
                    payload: Some(json!({
                        "key": key,
                        "sessionId": format!("sess-{key}")
                    })),
                    error: None,
                    final_flag: None,
                },
            )
            .await?;
        }
        "exec.approvals.get" => {
            let file = state.approvals_file.lock().unwrap().clone();
            send_frame(
                socket,
                GatewayFrame::Response {
                    id,
                    ok: true,
                    payload: Some(json!({
                        "hash": "fixture-hash",
                        "file": file
                    })),
                    error: None,
                    final_flag: None,
                },
            )
            .await?;
        }
        "exec.approvals.set" => {
            let file = params
                .as_ref()
                .and_then(|v| v.get("file"))
                .cloned()
                .ok_or_else(|| anyhow!("exec.approvals.set missing file"))?;
            *state.approvals_file.lock().unwrap() = file.clone();
            send_frame(
                socket,
                GatewayFrame::Response {
                    id,
                    ok: true,
                    payload: Some(json!({
                        "hash": "fixture-hash",
                        "file": file
                    })),
                    error: None,
                    final_flag: None,
                },
            )
            .await?;
        }
        "chat.send" => {
            let params = params.ok_or_else(|| anyhow!("chat.send missing params"))?;
            let session_key = params
                .get("sessionKey")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("chat.send missing sessionKey"))?;
            let message = params
                .get("message")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("chat.send missing message"))?;
            let run_id = params
                .get("idempotencyKey")
                .and_then(Value::as_str)
                .unwrap_or("fixture-run");
            send_frame(
                socket,
                GatewayFrame::Response {
                    id,
                    ok: true,
                    payload: Some(json!({ "runId": run_id })),
                    error: None,
                    final_flag: None,
                },
            )
            .await?;

            if let Some(helper_results) = run_leader_helpers(message).await? {
                for (idx, helper_result) in helper_results.iter().enumerate() {
                    send_frame(
                        socket,
                        GatewayFrame::Event {
                            event: "agent".into(),
                            payload: Some(json!({
                                "sessionKey": session_key,
                                "runId": run_id,
                                "stream": "tool",
                                "data": {
                                    "phase": "result",
                                    "name": "exec",
                                    "toolCallId": format!("tool-{}", idx + 1),
                                    "result": {
                                        "content": [{
                                            "type": "text",
                                            "text": helper_result
                                        }]
                                    }
                                }
                            })),
                            seq: Some((idx + 1) as u64),
                            state_version: None,
                        },
                    )
                    .await?;
                }
                let task_id = extract_task_id(message).unwrap_or_else(|| "T001".to_string());
                let final_text = if message.contains("已提交待验收") {
                    format!("openclaw-leader:accepted:{task_id}")
                } else {
                    format!("openclaw-leader:planned:{task_id}")
                };
                send_frame(
                    socket,
                    GatewayFrame::Event {
                        event: "chat".into(),
                        payload: Some(json!({
                            "sessionKey": session_key,
                            "runId": run_id,
                            "state": "final",
                            "message": {
                                "content": [{
                                    "type": "text",
                                    "text": final_text
                                }]
                            }
                        })),
                        seq: Some((helper_results.len() as u64) + 1),
                        state_version: None,
                    },
                )
                .await?;
            } else if let Some(helper_results) = run_specialist_helper(message).await? {
                for (idx, helper_result) in helper_results.iter().enumerate() {
                    send_frame(
                        socket,
                        GatewayFrame::Event {
                            event: "agent".into(),
                            payload: Some(json!({
                                "sessionKey": session_key,
                                "runId": run_id,
                                "stream": "tool",
                                "data": {
                                    "phase": "result",
                                    "name": "exec",
                                    "toolCallId": format!("tool-{}", idx + 1),
                                    "result": {
                                        "content": [{
                                            "type": "text",
                                            "text": helper_result
                                        }]
                                    }
                                }
                            })),
                            seq: Some((idx + 1) as u64),
                            state_version: None,
                        },
                    )
                    .await?;
                }
                let task_id = extract_task_id(message).unwrap_or_else(|| "T001".to_string());
                send_frame(
                    socket,
                    GatewayFrame::Event {
                        event: "chat".into(),
                        payload: Some(json!({
                            "sessionKey": session_key,
                            "runId": run_id,
                            "state": "final",
                            "message": {
                                "content": [{
                                    "type": "text",
                                    "text": format!("openclaw-worker:submitted:{task_id}")
                                }]
                            }
                        })),
                        seq: Some((helper_results.len() as u64) + 1),
                        state_version: None,
                    },
                )
                .await?;
            } else {
                send_frame(
                    socket,
                    GatewayFrame::Event {
                        event: "chat".into(),
                        payload: Some(json!({
                            "sessionKey": session_key,
                            "runId": run_id,
                            "state": "final",
                            "message": {
                                "content": [{
                                    "type": "text",
                                    "text": "openclaw:fixture"
                                }]
                            }
                        })),
                        seq: Some(1),
                        state_version: None,
                    },
                )
                .await?;
            }
        }
        other => {
            return Err(anyhow!("unsupported fixture method: {other}"));
        }
    }
    Ok(())
}

async fn send_frame(socket: &mut WebSocket, frame: GatewayFrame) -> Result<()> {
    socket
        .send(Message::Text(serde_json::to_string(&frame)?.into()))
        .await?;
    Ok(())
}

async fn run_specialist_helper(prompt: &str) -> Result<Option<Vec<String>>> {
    let Some(submit_template) = extract_backticked_command(prompt, "submit-task-result") else {
        return Ok(None);
    };
    let task_id = extract_task_id(prompt).unwrap_or_else(|| "T001".to_string());
    let mut results = Vec::new();

    if let Some(help_template) = extract_backticked_command(prompt, "request-help") {
        let help = help_template
            .replace("<task-id>", &shell_quote(&task_id))
            .replace(
                "<message>",
                &shell_quote("openclaw worker needs a quick hint"),
            );
        results.push(run_helper_command(&help).await?);
    }

    if let Some(checkpoint_template) = extract_backticked_command(prompt, "checkpoint-task") {
        let checkpoint = checkpoint_template
            .replace("<task-id>", &shell_quote(&task_id))
            .replace("<note>", &shell_quote("openclaw worker checkpoint"));
        results.push(run_helper_command(&checkpoint).await?);
    }

    let command = submit_template
        .replace("<task-id>", &shell_quote(&task_id))
        .replace("<summary>", &shell_quote("openclaw worker fixture result"));
    results.push(run_helper_command(&command).await?);
    Ok(Some(results))
}

async fn run_leader_helpers(prompt: &str) -> Result<Option<Vec<String>>> {
    let Some(create_template) = extract_backticked_command(prompt, "create-task") else {
        return Ok(None);
    };
    let task_id = extract_task_id(prompt).unwrap_or_else(|| "T001".to_string());
    let mut results = Vec::new();

    if prompt.contains("已提交待验收") {
        if let Some(accept_template) = extract_backticked_command(prompt, "accept-task") {
            let command = accept_template.replace("<task-id>", &shell_quote(&task_id));
            results.push(run_helper_command(&command).await?);
        }
        return Ok(Some(results));
    }

    let create = create_template
        .replace("<task-id>", &shell_quote(&task_id))
        .replace("<title>", &shell_quote("openclaw leader fixture task"))
        .replace("<agent>", &shell_quote("worker"));
    results.push(run_helper_command(&create).await?);

    if let Some(assign_template) = extract_backticked_command(prompt, "assign-task") {
        let assign = assign_template
            .replace("<task-id>", &shell_quote(&task_id))
            .replace("<agent>", &shell_quote("worker"));
        results.push(run_helper_command(&assign).await?);
    }

    if let Some(start_template) = extract_backticked_command(prompt, "start-execution") {
        results.push(run_helper_command(&start_template).await?);
    }

    Ok(Some(results))
}

async fn run_helper_command(command: &str) -> Result<String> {
    let output = Command::new("sh")
        .arg("-lc")
        .arg(command)
        .output()
        .await
        .with_context(|| format!("failed to execute helper command: {command}"))?;

    if !output.status.success() {
        return Err(anyhow!(
            "helper command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn extract_backticked_command(prompt: &str, needle: &str) -> Option<String> {
    for line in prompt.lines() {
        let Some(start) = line.find('`') else {
            continue;
        };
        let Some(end) = line[start + 1..].find('`') else {
            continue;
        };
        let command = &line[start + 1..start + 1 + end];
        if command.contains(needle) {
            return Some(command.to_string());
        }
    }
    None
}

fn extract_task_id(text: &str) -> Option<String> {
    for token in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-') {
        if token.starts_with('T')
            && token.len() > 1
            && token[1..].chars().all(|c| c.is_ascii_digit())
        {
            return Some(token.to_string());
        }
    }
    None
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
