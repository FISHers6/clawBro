use crate::contract::RuntimeEvent;
use anyhow::{anyhow, Context, Result};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

type GatewayStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const OPENCLAW_PROTOCOL_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum GatewayFrame {
    #[serde(rename = "req")]
    Request {
        id: String,
        method: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<Value>,
    },
    #[serde(rename = "res")]
    Response {
        id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<GatewayErrorShape>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        final_flag: Option<bool>,
    },
    #[serde(rename = "event")]
    Event {
        event: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state_version: Option<Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayErrorShape {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayInbound {
    Runtime(RuntimeEvent),
    HelperResult(Value),
    FinalText(String),
    Started { run_id: Option<String> },
    Ack,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecApprovalRequested {
    id: String,
    command: String,
    cwd: Option<String>,
    host: Option<String>,
    agent_id: Option<String>,
    session_key: Option<String>,
    expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenClawConnectConfig {
    pub endpoint: String,
    pub token: Option<String>,
    pub password: Option<String>,
    pub role: String,
    pub scopes: Vec<String>,
}

pub struct OpenClawGatewayClient {
    stream: GatewayStream,
    background_request_ids: HashSet<String>,
}

impl OpenClawGatewayClient {
    pub async fn connect(config: &OpenClawConnectConfig) -> Result<Self> {
        let (stream, _) = connect_async(&config.endpoint).await.with_context(|| {
            format!("failed to connect to OpenClaw gateway {}", config.endpoint)
        })?;
        let mut client = Self {
            stream,
            background_request_ids: HashSet::new(),
        };
        let nonce = client.wait_for_challenge_nonce().await?;
        client.send_connect(config, nonce.as_deref()).await?;
        Ok(client)
    }

    pub async fn resolve_session_key(
        &mut self,
        session_key: &str,
        agent_id: Option<&str>,
    ) -> Result<String> {
        let mut params = json!({ "key": session_key });
        if let Some(agent_id) = agent_id.filter(|id| !id.trim().is_empty()) {
            params["agentId"] = Value::String(agent_id.to_string());
        }
        let payload = self.request_json("sessions.resolve", Some(params)).await?;
        if let Some(key) = payload.get("key").and_then(Value::as_str) {
            return Ok(key.to_string());
        }
        Err(anyhow!("sessions.resolve succeeded but returned no key"))
    }

    pub async fn send_chat(&mut self, session_key: &str, text: &str) -> Result<Option<String>> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let run_id = uuid::Uuid::new_v4().to_string();
        self.send_frame(GatewayFrame::Request {
            id: request_id.clone(),
            method: "chat.send".into(),
            params: Some(serde_json::json!({
                "sessionKey": session_key,
                "message": text,
                "idempotencyKey": run_id,
            })),
        })
        .await?;

        loop {
            match self.read_frame().await? {
                GatewayFrame::Response {
                    id,
                    ok,
                    payload,
                    error,
                    ..
                } if id == request_id => {
                    if !ok {
                        return Err(anyhow!(
                            "chat.send failed: {}",
                            error_message(error.as_ref())
                        ));
                    }
                    let run_id = payload
                        .as_ref()
                        .and_then(|v| v.get("runId"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                        .or_else(|| {
                            payload.and_then(|v| {
                                v.get("idempotencyKey")
                                    .and_then(Value::as_str)
                                    .map(ToOwned::to_owned)
                            })
                        });
                    return Ok(run_id);
                }
                GatewayFrame::Event { .. } => {}
                _ => {}
            }
        }
    }

    pub async fn get_exec_approvals(&mut self) -> Result<Value> {
        self.request_json("exec.approvals.get", Some(json!({})))
            .await
    }

    pub async fn set_exec_approvals(
        &mut self,
        file: Value,
        base_hash: Option<&str>,
    ) -> Result<Value> {
        let mut params = json!({ "file": file });
        if let Some(base_hash) = base_hash.filter(|value| !value.trim().is_empty()) {
            params["baseHash"] = Value::String(base_hash.to_string());
        }
        self.request_json("exec.approvals.set", Some(params)).await
    }

    pub async fn resolve_exec_approval(&mut self, approval_id: &str, decision: &str) -> Result<()> {
        let request_id = uuid::Uuid::new_v4().to_string();
        self.background_request_ids.insert(request_id.clone());
        if let Err(err) = self
            .send_frame(GatewayFrame::Request {
                id: request_id.clone(),
                method: "exec.approval.resolve".into(),
                params: Some(json!({
                    "id": approval_id,
                    "decision": decision,
                })),
            })
            .await
        {
            self.background_request_ids.remove(&request_id);
            return Err(err);
        }
        Ok(())
    }

    pub async fn read_inbound(
        &mut self,
        session_key: &str,
        run_id: Option<&str>,
        streamed_prefix: &str,
    ) -> Result<Option<GatewayInbound>> {
        loop {
            let frame = self.read_frame().await?;
            match frame {
                GatewayFrame::Event { event, payload, .. } if event == "chat" => {
                    let Some(payload) = payload else {
                        continue;
                    };
                    let payload_session_key = payload.get("sessionKey").and_then(Value::as_str);
                    if payload_session_key != Some(session_key) {
                        continue;
                    }
                    if let Some(expected_run_id) = run_id {
                        let payload_run_id = payload.get("runId").and_then(Value::as_str);
                        if payload_run_id != Some(expected_run_id) {
                            continue;
                        }
                    }
                    let state = payload.get("state").and_then(Value::as_str);
                    match state {
                        Some("delta") => {
                            let full = extract_chat_text(payload.get("message"));
                            let delta = full
                                .strip_prefix(streamed_prefix)
                                .map(ToOwned::to_owned)
                                .unwrap_or(full);
                            if delta.is_empty() {
                                continue;
                            }
                            return Ok(Some(GatewayInbound::Runtime(RuntimeEvent::TextDelta {
                                text: delta,
                            })));
                        }
                        Some("final") => {
                            return Ok(Some(GatewayInbound::FinalText(extract_chat_text(
                                payload.get("message"),
                            ))));
                        }
                        Some("error") => {
                            let error = payload
                                .get("errorMessage")
                                .and_then(Value::as_str)
                                .unwrap_or("OpenClaw chat error")
                                .to_string();
                            return Ok(Some(GatewayInbound::Runtime(RuntimeEvent::TurnFailed {
                                error,
                            })));
                        }
                        Some("aborted") => {
                            return Ok(Some(GatewayInbound::Runtime(RuntimeEvent::TurnFailed {
                                error: "OpenClaw chat aborted".into(),
                            })));
                        }
                        _ => continue,
                    }
                }
                GatewayFrame::Event { event, payload, .. } if event == "agent" => {
                    let Some(payload) = payload else {
                        continue;
                    };
                    if !agent_event_matches_session(&payload, session_key, run_id) {
                        continue;
                    }
                    if let Some(helper_result) = parse_helper_result_event(&payload)? {
                        return Ok(Some(GatewayInbound::HelperResult(helper_result)));
                    }
                }
                GatewayFrame::Event { event, payload, .. }
                    if event == "exec.approval.requested" =>
                {
                    let Some(requested) = parse_exec_approval_requested(payload.as_ref()) else {
                        continue;
                    };
                    if !approval_matches_session(&requested, session_key, run_id) {
                        continue;
                    }
                    let prompt = render_exec_approval_prompt(&requested);
                    let event = RuntimeEvent::ApprovalRequest(crate::contract::PermissionRequest {
                        id: requested.id.clone(),
                        prompt,
                        command: Some(requested.command.clone()),
                        cwd: requested.cwd.clone(),
                        host: requested.host.clone(),
                        agent_id: requested.agent_id.clone(),
                        expires_at_ms: requested.expires_at_ms,
                    });
                    return Ok(Some(GatewayInbound::Runtime(event)));
                }
                GatewayFrame::Response { id, ok: true, .. }
                    if self.background_request_ids.remove(&id) =>
                {
                    return Ok(Some(GatewayInbound::Ack));
                }
                GatewayFrame::Response {
                    id,
                    ok: false,
                    error,
                    ..
                } if self.background_request_ids.remove(&id) => {
                    return Ok(Some(GatewayInbound::Runtime(RuntimeEvent::TurnFailed {
                        error: format!(
                            "OpenClaw approval resolve failed: {}",
                            error_message(error.as_ref())
                        ),
                    })));
                }
                GatewayFrame::Response {
                    ok: false, error, ..
                } => {
                    return Ok(Some(GatewayInbound::Runtime(RuntimeEvent::TurnFailed {
                        error: error_message(error.as_ref()),
                    })));
                }
                GatewayFrame::Response {
                    ok: true,
                    payload,
                    final_flag,
                    ..
                } if final_flag.unwrap_or(false) => {
                    let text = payload
                        .as_ref()
                        .map(|value| extract_chat_text(Some(value)))
                        .unwrap_or_default();
                    return Ok(Some(GatewayInbound::FinalText(text)));
                }
                GatewayFrame::Response {
                    ok: true, payload, ..
                } => {
                    let run_id = payload
                        .as_ref()
                        .and_then(|v| v.get("runId"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    return Ok(Some(GatewayInbound::Started { run_id }));
                }
                GatewayFrame::Event { .. } => continue,
                GatewayFrame::Request { .. } => continue,
            }
        }
    }

    async fn wait_for_challenge_nonce(&mut self) -> Result<Option<String>> {
        loop {
            let frame = self.read_frame().await?;
            if let GatewayFrame::Event { event, payload, .. } = frame {
                if event == "connect.challenge" {
                    let nonce = payload
                        .as_ref()
                        .and_then(|v| v.get("nonce"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    return Ok(nonce);
                }
            }
        }
    }

    async fn send_connect(
        &mut self,
        config: &OpenClawConnectConfig,
        _nonce: Option<&str>,
    ) -> Result<()> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let scopes = if config.scopes.is_empty() {
            vec!["operator.admin".to_string()]
        } else {
            config.scopes.clone()
        };
        self.send_frame(GatewayFrame::Request {
            id: request_id.clone(),
            method: "connect".into(),
            params: Some(json!({
                "minProtocol": OPENCLAW_PROTOCOL_VERSION,
                "maxProtocol": OPENCLAW_PROTOCOL_VERSION,
                "client": {
                    "id": "gateway-client",
                    "displayName": "quickai-runtime",
                    "version": env!("CARGO_PKG_VERSION"),
                    "platform": std::env::consts::OS,
                    "mode": "backend",
                    "instanceId": format!("quickai-{}", uuid::Uuid::new_v4()),
                },
                "role": config.role,
                "scopes": scopes,
                "caps": [],
                "commands": [],
                "permissions": {},
                "locale": "en-US",
                "userAgent": format!("quickai-runtime/{}", env!("CARGO_PKG_VERSION")),
                "auth": build_auth(config),
            })),
        })
        .await?;

        loop {
            match self.read_frame().await? {
                GatewayFrame::Response {
                    id,
                    ok,
                    payload,
                    error,
                    ..
                } if id == request_id => {
                    if !ok {
                        return Err(anyhow!("connect failed: {}", error_message(error.as_ref())));
                    }
                    let payload = payload.unwrap_or_default();
                    if payload.get("type").and_then(Value::as_str) != Some("hello-ok") {
                        return Err(anyhow!("connect succeeded but hello-ok payload missing"));
                    }
                    return Ok(());
                }
                GatewayFrame::Event { .. } => {}
                _ => {}
            }
        }
    }

    async fn send_frame(&mut self, frame: GatewayFrame) -> Result<()> {
        let raw = serde_json::to_string(&frame)?;
        self.stream.send(Message::Text(raw.into())).await?;
        Ok(())
    }

    async fn read_frame(&mut self) -> Result<GatewayFrame> {
        loop {
            let Some(message) = self.stream.next().await else {
                return Err(anyhow!("OpenClaw gateway closed connection"));
            };
            let message = message?;
            match message {
                Message::Text(raw) => return Ok(serde_json::from_str(&raw)?),
                Message::Binary(raw) => return Ok(serde_json::from_slice(&raw)?),
                Message::Ping(payload) => {
                    self.stream.send(Message::Pong(payload)).await?;
                }
                Message::Pong(_) => {}
                Message::Frame(_) => {}
                Message::Close(frame) => {
                    return Err(anyhow!(
                        "OpenClaw gateway closed: {}",
                        frame
                            .as_ref()
                            .map(|f| f.reason.to_string())
                            .unwrap_or_else(|| "no reason".into())
                    ))
                }
            }
        }
    }

    async fn request_json(&mut self, method: &str, params: Option<Value>) -> Result<Value> {
        let request_id = uuid::Uuid::new_v4().to_string();
        self.send_frame(GatewayFrame::Request {
            id: request_id.clone(),
            method: method.to_string(),
            params,
        })
        .await?;

        loop {
            match self.read_frame().await? {
                GatewayFrame::Response {
                    id,
                    ok,
                    payload,
                    error,
                    ..
                } if id == request_id => {
                    if !ok {
                        return Err(anyhow!(
                            "{} failed: {}",
                            method,
                            error_message(error.as_ref())
                        ));
                    }
                    return Ok(payload.unwrap_or_default());
                }
                GatewayFrame::Event { .. } => {}
                _ => {}
            }
        }
    }
}

fn parse_exec_approval_requested(payload: Option<&Value>) -> Option<ExecApprovalRequested> {
    let payload = payload?;
    let id = payload.get("id")?.as_str()?.trim();
    if id.is_empty() {
        return None;
    }
    let request = payload.get("request")?;
    let command = request.get("command")?.as_str()?.trim();
    if command.is_empty() {
        return None;
    }
    Some(ExecApprovalRequested {
        id: id.to_string(),
        command: command.to_string(),
        cwd: request
            .get("cwd")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        host: request
            .get("host")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        agent_id: request
            .get("agentId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        session_key: request
            .get("sessionKey")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        expires_at_ms: payload.get("expiresAtMs").and_then(Value::as_u64),
    })
}

fn approval_matches_session(
    requested: &ExecApprovalRequested,
    session_key: &str,
    _run_id: Option<&str>,
) -> bool {
    match requested.session_key.as_deref() {
        Some(request_session_key) => request_session_key == session_key,
        None => true,
    }
}

fn agent_event_matches_session(payload: &Value, session_key: &str, run_id: Option<&str>) -> bool {
    let payload_session_key = payload.get("sessionKey").and_then(Value::as_str);
    if payload_session_key != Some(session_key) {
        return false;
    }
    if let Some(expected_run_id) = run_id {
        let payload_run_id = payload.get("runId").and_then(Value::as_str);
        if payload_run_id != Some(expected_run_id) {
            return false;
        }
    }
    true
}

fn parse_helper_result_event(payload: &Value) -> Result<Option<Value>> {
    if payload.get("stream").and_then(Value::as_str) != Some("tool") {
        return Ok(None);
    }
    let Some(data) = payload.get("data") else {
        return Ok(None);
    };
    if data.get("phase").and_then(Value::as_str) != Some("result") {
        return Ok(None);
    }
    let result = data
        .get("result")
        .or_else(|| data.get("partialResult"))
        .or_else(|| data.get("details"));
    let Some(text) = result.and_then(extract_tool_result_text) else {
        return Ok(None);
    };
    let Ok(json) = serde_json::from_str::<Value>(&text) else {
        return Ok(None);
    };
    Ok(Some(json))
}

fn render_exec_approval_prompt(requested: &ExecApprovalRequested) -> String {
    let mut parts = vec![format!(
        "OpenClaw exec approval required: `{}`",
        requested.command
    )];
    if let Some(host) = requested.host.as_deref() {
        parts.push(format!("host={host}"));
    }
    if let Some(agent_id) = requested.agent_id.as_deref() {
        parts.push(format!("agent={agent_id}"));
    }
    if let Some(cwd) = requested.cwd.as_deref() {
        parts.push(format!("cwd={cwd}"));
    }
    parts.join(" | ")
}

fn build_auth(config: &OpenClawConnectConfig) -> Option<Value> {
    if config.token.is_none() && config.password.is_none() {
        return None;
    }
    Some(serde_json::json!({
        "token": config.token,
        "password": config.password,
    }))
}

fn error_message(error: Option<&GatewayErrorShape>) -> String {
    error
        .and_then(|err| err.message.clone().or(err.code.clone()))
        .unwrap_or_else(|| "OpenClaw gateway request failed".into())
}

fn extract_chat_text(message: Option<&Value>) -> String {
    let Some(message) = message else {
        return String::new();
    };
    if let Some(text) = message.get("text").and_then(Value::as_str) {
        return text.to_string();
    }
    message
        .get("content")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find_map(|item| {
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    item.get("text")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                } else {
                    None
                }
            })
        })
        .unwrap_or_default()
}

fn extract_tool_result_text(result: &Value) -> Option<String> {
    match result {
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Value::Object(map) => {
            let content = map.get("content")?.as_array()?;
            let texts = content
                .iter()
                .filter_map(|item| {
                    let obj = item.as_object()?;
                    (obj.get("type").and_then(Value::as_str) == Some("text"))
                        .then(|| obj.get("text").and_then(Value::as_str))
                        .flatten()
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .map(ToOwned::to_owned)
                })
                .collect::<Vec<_>>();
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        _ => None,
    }
}

pub fn canonical_openclaw_session_key(session_key: &qai_protocol::SessionKey) -> String {
    format!("{}:{}", session_key.channel, session_key.scope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_session_key_is_stable() {
        let key = qai_protocol::SessionKey::new("lark", "group:abc");
        assert_eq!(canonical_openclaw_session_key(&key), "lark:group:abc");
    }

    #[test]
    fn extracts_text_from_chat_message_content() {
        let payload = serde_json::json!({
            "content": [
                { "type": "text", "text": "hello world" }
            ]
        });
        assert_eq!(extract_chat_text(Some(&payload)), "hello world");
    }

    #[test]
    fn parses_exec_approval_requested_payload() {
        let payload = json!({
            "id": "approval-1",
            "request": {
                "command": "git status",
                "cwd": "/tmp/repo",
                "host": "gateway",
                "agentId": "main",
                "sessionKey": "lark:group:abc"
            },
            "createdAtMs": 1,
            "expiresAtMs": 2
        });

        let parsed = parse_exec_approval_requested(Some(&payload)).unwrap();
        assert_eq!(parsed.id, "approval-1");
        assert_eq!(parsed.command, "git status");
        assert_eq!(parsed.cwd.as_deref(), Some("/tmp/repo"));
        assert_eq!(parsed.session_key.as_deref(), Some("lark:group:abc"));
    }

    #[test]
    fn approval_matching_respects_session_key_when_present() {
        let requested = ExecApprovalRequested {
            id: "approval-1".into(),
            command: "git status".into(),
            cwd: None,
            host: Some("gateway".into()),
            agent_id: None,
            session_key: Some("ws:user".into()),
            expires_at_ms: Some(123),
        };

        assert!(approval_matches_session(&requested, "ws:user", None));
        assert!(!approval_matches_session(&requested, "ws:other", None));
    }

    #[test]
    fn render_exec_approval_prompt_is_human_readable() {
        let requested = ExecApprovalRequested {
            id: "approval-1".into(),
            command: "git status".into(),
            cwd: Some("/tmp/repo".into()),
            host: Some("gateway".into()),
            agent_id: Some("main".into()),
            session_key: None,
            expires_at_ms: Some(123),
        };

        let prompt = render_exec_approval_prompt(&requested);
        assert!(prompt.contains("git status"));
        assert!(prompt.contains("host=gateway"));
        assert!(prompt.contains("agent=main"));
        assert!(prompt.contains("cwd=/tmp/repo"));
    }

    #[test]
    fn parses_helper_result_from_agent_tool_event() {
        let payload = serde_json::json!({
            "sessionKey": "specialist:team-1:openclaw",
            "runId": "run-1",
            "stream": "tool",
            "data": {
                "phase": "result",
                "name": "exec",
                "toolCallId": "tool-1",
                "result": {
                    "content": [{
                        "type": "text",
                        "text": "{\"ok\":true,\"action\":\"submit_task_result\",\"task_id\":\"T1\",\"summary\":\"done\"}"
                    }]
                }
            }
        });

        let parsed = parse_helper_result_event(&payload)
            .unwrap()
            .expect("helper result should parse");
        assert_eq!(parsed["action"], "submit_task_result");
        assert_eq!(parsed["task_id"], "T1");
        assert_eq!(parsed["ok"], true);
    }

    #[test]
    fn ignores_non_json_tool_result_text() {
        let payload = serde_json::json!({
            "sessionKey": "specialist:team-1:openclaw",
            "runId": "run-1",
            "stream": "tool",
            "data": {
                "phase": "result",
                "name": "exec",
                "result": { "content": [{ "type": "text", "text": "plain output" }] }
            }
        });

        assert!(parse_helper_result_event(&payload).unwrap().is_none());
    }
}
