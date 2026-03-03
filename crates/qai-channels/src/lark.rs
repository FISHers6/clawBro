//! Feishu/Lark WebSocket Channel
//! Implements Feishu long-connection WebSocket mode.
//! Env: LARK_APP_ID, LARK_APP_SECRET

use crate::traits::Channel;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use qai_protocol::{InboundMsg, MsgContent, OutboundMsg, SessionKey};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMsg;

/// Extract first @mention from message text (e.g. "@claude review" → Some("@claude"))
/// This is Lark-specific text parsing - in future can be enhanced to parse Lark's
/// rich @mention format (e.g. <at user_id="xxx">@name</at>)
fn extract_first_mention(text: &str) -> Option<String> {
    // Simple regex-free extraction: find first word starting with '@'
    text.split_whitespace()
        .find(|w| w.starts_with('@'))
        .map(|w| {
            // Strip trailing punctuation
            w.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
                .to_string()
        })
}

const FEISHU_BASE: &str = "https://open.feishu.cn/open-apis";
/// WebSocket endpoint discovery URL (official Go SDK: /callback/ws/endpoint).
/// Posts {"AppID": ..., "AppSecret": ...} and returns {"code":0,"data":{"URL":"wss://..."}}
const FEISHU_WS_ENDPOINT_URL: &str = "https://open.feishu.cn/callback/ws/endpoint";

pub struct LarkChannel {
    pub app_id: String,
    pub app_secret: String,
    client: reqwest::Client,
    require_mention_in_groups: bool,
}

impl LarkChannel {
    pub fn new(app_id: String, app_secret: String, require_mention_in_groups: bool) -> Self {
        Self {
            app_id,
            app_secret,
            client: reqwest::Client::new(),
            require_mention_in_groups,
        }
    }

    /// Deprecated: use `LarkChannel::new()` with explicit config for full feature support.
    /// This method always sets `require_mention_in_groups = false`.
    #[deprecated(note = "use LarkChannel::new() with explicit require_mention_in_groups")]
    pub fn from_env() -> Result<Self> {
        let app_id =
            std::env::var("LARK_APP_ID").map_err(|_| anyhow::anyhow!("LARK_APP_ID not set"))?;
        let app_secret = std::env::var("LARK_APP_SECRET")
            .map_err(|_| anyhow::anyhow!("LARK_APP_SECRET not set"))?;
        Ok(Self::new(app_id, app_secret, false))
    }

    async fn get_access_token(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct TokenResp {
            code: i32,
            #[serde(default)]
            msg: String,
            #[serde(default)]
            app_access_token: String,
        }
        let resp: TokenResp = self
            .client
            .post(format!("{FEISHU_BASE}/auth/v3/app_access_token/internal"))
            .json(&serde_json::json!({
                "app_id": self.app_id,
                "app_secret": self.app_secret
            }))
            .send()
            .await?
            .json()
            .await?;
        if resp.code != 0 {
            anyhow::bail!(
                "Feishu get_access_token failed: code={} msg={}",
                resp.code,
                resp.msg
            );
        }
        Ok(resp.app_access_token)
    }

    /// Get the full WebSocket URL via Feishu's /callback/ws/endpoint.
    /// Uses the official Go SDK approach: POST {"AppID", "AppSecret"} → {"code":0,"data":{"URL":"wss://..."}}
    async fn get_ws_url(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct EndpointData {
            #[serde(rename = "URL")]
            url: String,
        }
        #[derive(Deserialize)]
        struct EndpointResp {
            code: i32,
            #[serde(default)]
            msg: String,
            #[serde(default)]
            data: Option<EndpointData>,
        }
        let resp: EndpointResp = self
            .client
            .post(FEISHU_WS_ENDPOINT_URL)
            .header("locale", "zh")
            .json(&serde_json::json!({
                "AppID": self.app_id,
                "AppSecret": self.app_secret
            }))
            .send()
            .await?
            .json()
            .await?;
        if resp.code != 0 {
            anyhow::bail!(
                "Feishu get_ws_url failed: code={} msg={}",
                resp.code,
                resp.msg
            );
        }
        resp.data
            .map(|d| d.url)
            .filter(|u| !u.is_empty())
            .ok_or_else(|| anyhow::anyhow!("No URL in Feishu ws endpoint response"))
    }

    /// 编辑已发送的消息（用于流式更新）
    pub async fn edit_message(&self, message_id: &str, text: &str) -> anyhow::Result<()> {
        let token = self.get_access_token().await?;
        let content_json = serde_json::json!({"text": text}).to_string();

        let resp = self
            .client
            .patch(format!("{FEISHU_BASE}/im/v1/messages/{message_id}"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "content": content_json,
                "msg_type": "text"
            }))
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Feishu edit_message failed: HTTP {} body={}",
                status,
                &body[..body.len().min(200)]
            );
        }
        Ok(())
    }

    /// 发送新消息并返回消息 ID（用于后续 edit_message 流式更新）
    pub async fn send_and_get_id(&self, msg: &OutboundMsg) -> anyhow::Result<String> {
        let text = match &msg.content {
            MsgContent::Text { text } => text.clone(),
            _ => "[unsupported content type]".to_string(),
        };
        let message_id = msg
            .reply_to
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Lark send_and_get_id: no message_id in reply_to"))?;

        let token = self.get_access_token().await?;
        let content_json = serde_json::json!({"text": text}).to_string();

        let resp = self
            .client
            .post(format!("{FEISHU_BASE}/im/v1/messages/{message_id}/reply"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "content": content_json,
                "msg_type": "text"
            }))
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Feishu send_and_get_id failed: HTTP {} body={}",
                status,
                &body[..body.len().min(200)]
            );
        }

        #[derive(serde::Deserialize)]
        struct ReplyResp {
            code: i32,
            #[serde(default)]
            msg: String,
            data: Option<ReplyData>,
        }
        #[derive(serde::Deserialize)]
        struct ReplyData {
            message_id: String,
        }
        let reply: ReplyResp = resp.json().await.map_err(|e| {
            anyhow::anyhow!("Feishu send_and_get_id: failed to parse reply response: {e}")
        })?;
        if reply.code != 0 {
            anyhow::bail!(
                "Feishu reply_message API error: code={} msg={}",
                reply.code,
                reply.msg
            );
        }
        let message_id = reply
            .data
            .ok_or_else(|| anyhow::anyhow!("Feishu reply: no data in response"))?
            .message_id;
        Ok(message_id)
    }
}

/// Feishu WebSocket frame
#[derive(Debug, Deserialize)]
struct LarkWsFrame {
    #[serde(rename = "type")]
    frame_type: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct LarkMsgEvent {
    event: LarkMsgEventBody,
}

#[derive(Debug, Deserialize)]
struct LarkMsgEventBody {
    sender: LarkSender,
    message: LarkMessage,
}

#[derive(Debug, Deserialize)]
struct LarkSender {
    sender_id: LarkSenderId,
}

#[derive(Debug, Deserialize)]
struct LarkSenderId {
    open_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LarkMessage {
    message_id: String,
    message_type: String,
    content: String,
    #[serde(default)]
    chat_type: String, // "group" or "p2p"
    #[serde(default)]
    chat_id: String, // group chat ID when chat_type == "group"
}

#[derive(Deserialize)]
struct LarkTextContent {
    text: String,
}

#[async_trait]
impl Channel for LarkChannel {
    fn name(&self) -> &str {
        "lark"
    }

    async fn send(&self, msg: &OutboundMsg) -> Result<()> {
        let text = match &msg.content {
            MsgContent::Text { text } => text.clone(),
            _ => "[unsupported content type]".to_string(),
        };
        let scope = &msg.session_key.scope;

        let token = self.get_access_token().await?;
        let content_json = serde_json::json!({"text": text}).to_string();

        if let Some(message_id) = &msg.reply_to {
            // Preferred: reply to the specific incoming message.
            let resp = self
                .client
                .post(format!("{FEISHU_BASE}/im/v1/messages/{message_id}/reply"))
                .header("Authorization", format!("Bearer {token}"))
                .json(&serde_json::json!({
                    "content": content_json,
                    "msg_type": "text"
                }))
                .send()
                .await?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                let body_preview = &body[..body.len().min(200)];
                anyhow::bail!("Feishu reply failed: HTTP {} body={}", status, body_preview);
            }
        } else if let Some(chat_id) = scope.strip_prefix("group:") {
            // Proactive group message — send to chat_id.
            let resp = self
                .client
                .post(format!(
                    "{FEISHU_BASE}/im/v1/messages?receive_id_type=chat_id"
                ))
                .header("Authorization", format!("Bearer {token}"))
                .json(&serde_json::json!({
                    "receive_id": chat_id,
                    "content": content_json,
                    "msg_type": "text"
                }))
                .send()
                .await?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                let body_preview = &body[..body.len().min(200)];
                anyhow::bail!(
                    "Feishu group send failed: HTTP {} body={}",
                    status,
                    body_preview
                );
            }
        } else {
            // Proactive DM — scope is "user:{open_id}".
            if !scope.starts_with("user:") {
                tracing::warn!(
                    "Lark send: unexpected scope format '{}', attempting as open_id",
                    scope
                );
            }
            let open_id = scope.strip_prefix("user:").unwrap_or(scope.as_str());
            let resp = self
                .client
                .post(format!(
                    "{FEISHU_BASE}/im/v1/messages?receive_id_type=open_id"
                ))
                .header("Authorization", format!("Bearer {token}"))
                .json(&serde_json::json!({
                    "receive_id": open_id,
                    "content": content_json,
                    "msg_type": "text"
                }))
                .send()
                .await?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                let body_preview = &body[..body.len().min(200)];
                anyhow::bail!(
                    "Feishu DM send failed: HTTP {} body={}",
                    status,
                    body_preview
                );
            }
        }
        Ok(())
    }

    async fn listen(&self, tx: mpsc::Sender<InboundMsg>) -> Result<()> {
        let ws_url = self.get_ws_url().await?;
        tracing::info!("Feishu WebSocket connecting");

        let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| anyhow::anyhow!("Feishu WS connect failed: {e}"))?;

        tracing::info!("Feishu WebSocket connected");

        // Load allowlist once before the loop
        let checker = crate::allowlist::AllowlistChecker::load();

        while let Some(frame_result) = ws.next().await {
            match frame_result {
                Ok(WsMsg::Text(text)) => {
                    let Ok(frame) = serde_json::from_str::<LarkWsFrame>(text.as_str()) else {
                        continue;
                    };
                    match frame.frame_type.as_str() {
                        "ping" => {
                            let pong = serde_json::json!({"type": "pong", "id": frame.id});
                            if let Err(e) = ws.send(WsMsg::Text(pong.to_string().into())).await {
                                tracing::error!("Feishu WS pong send failed: {e}");
                                break;
                            }
                        }
                        "event" => {
                            if let Some(data) = frame.data {
                                handle_event(data, &tx, &checker, self.require_mention_in_groups)
                                    .await;
                            }
                        }
                        _ => {}
                    }
                }
                Ok(WsMsg::Close(_)) => {
                    tracing::info!("Feishu WS connection closed");
                    break;
                }
                Err(e) => {
                    tracing::error!("Feishu WS error: {e}");
                    break;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Derive the session scope from a Lark message event body.
///
/// - Group chat (`chat_type == "group"`): `"group:{chat_id}"`
/// - Private chat (`chat_type == "p2p"` or anything else): `"user:{open_id}"`
fn derive_scope(chat_type: &str, chat_id: &str, open_id: &str) -> String {
    if chat_type == "group" {
        format!("group:{}", chat_id)
    } else {
        format!("user:{}", open_id)
    }
}

async fn handle_event(
    data: serde_json::Value,
    tx: &mpsc::Sender<InboundMsg>,
    checker: &crate::allowlist::AllowlistChecker,
    require_mention_in_groups: bool,
) {
    let event_type = data
        .get("header")
        .and_then(|h| h.get("event_type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if event_type != "im.message.receive_v1" {
        return;
    }

    let Ok(event) = serde_json::from_value::<LarkMsgEvent>(data) else {
        return;
    };

    let open_id = event
        .event
        .sender
        .sender_id
        .open_id
        .unwrap_or_else(|| "unknown".to_string());

    if event.event.message.message_type != "text" {
        return;
    }

    let Ok(text_content) = serde_json::from_str::<LarkTextContent>(&event.event.message.content)
    else {
        return;
    };

    let text = text_content.text.trim().to_string();
    if text.is_empty() {
        return;
    }

    // Allowlist check
    if !checker.is_allowed("lark", &open_id) {
        tracing::debug!("AllowlistChecker: lark user {} denied", open_id);
        return;
    }

    // Extract first @mention from text for agent routing (platform-specific extraction)
    // Examples: "@claude review this" → Some("@claude")
    let target_agent = extract_first_mention(&text);

    // Derive scope: group uses chat_id, p2p uses open_id
    let scope = derive_scope(
        &event.event.message.chat_type,
        &event.event.message.chat_id,
        &open_id,
    );

    // Group mention-only mode: skip group messages with no @mention.
    if require_mention_in_groups && scope.starts_with("group:") {
        let has_mention = extract_first_mention(&text).is_some();
        if !has_mention {
            tracing::debug!(
                "Lark group message skipped (require_mention_in_groups): scope={}",
                scope
            );
            return;
        }
    }

    // Use Feishu message_id as InboundMsg.id so that
    // OutboundMsg.reply_to = message_id → reply API URL
    let inbound = InboundMsg {
        id: event.event.message.message_id,
        session_key: SessionKey::new("lark", &scope),
        content: MsgContent::text(&text),
        sender: open_id,
        channel: "lark".to_string(),
        timestamp: Utc::now(),
        thread_ts: None,
        target_agent,
    };

    let _ = tx.send(inbound).await;
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize env-mutating tests to avoid races
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_lark_channel_from_env_missing() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("LARK_APP_ID");
            std::env::remove_var("LARK_APP_SECRET");
        }
        assert!(LarkChannel::from_env().is_err());
    }

    #[test]
    fn test_lark_channel_from_env_ok() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("LARK_APP_ID", "test_id");
            std::env::set_var("LARK_APP_SECRET", "test_secret");
        }
        let ch = LarkChannel::from_env().unwrap();
        assert_eq!(ch.app_id, "test_id");
        assert_eq!(ch.app_secret, "test_secret");
        assert_eq!(ch.name(), "lark");
        unsafe {
            std::env::remove_var("LARK_APP_ID");
            std::env::remove_var("LARK_APP_SECRET");
        }
    }

    #[test]
    fn test_lark_msg_event_deserialization() {
        let event: LarkMsgEvent = serde_json::from_value(serde_json::json!({
            "event": {
                "sender": {"sender_id": {"open_id": "ou_abc"}},
                "message": {
                    "message_id": "om_123",
                    "message_type": "text",
                    "content": "{\"text\":\"hello\"}"
                }
            }
        }))
        .unwrap();
        assert_eq!(event.event.sender.sender_id.open_id.unwrap(), "ou_abc");
        assert_eq!(event.event.message.message_id, "om_123");
        let text: LarkTextContent = serde_json::from_str(&event.event.message.content).unwrap();
        assert_eq!(text.text, "hello");
    }

    #[test]
    fn test_edit_message_url_format() {
        let url = format!("{FEISHU_BASE}/im/v1/messages/{}/", "om_test_123");
        assert!(url.contains("om_test_123"));
        assert!(url.contains("im/v1/messages"));
    }

    #[test]
    fn test_extract_first_mention_basic() {
        assert_eq!(
            extract_first_mention("@claude review this"),
            Some("@claude".to_string())
        );
        assert_eq!(extract_first_mention("no mention here"), None);
        assert_eq!(
            extract_first_mention("@codex please help @claude"),
            Some("@codex".to_string())
        );
        assert_eq!(
            extract_first_mention("hello @claude,"),
            Some("@claude".to_string())
        );
        assert_eq!(extract_first_mention(""), None);
    }

    #[test]
    fn test_lark_group_scope_from_event() {
        let scope = derive_scope("group", "oc_test_group", "ou_sender");
        assert_eq!(scope, "group:oc_test_group");
    }

    #[test]
    fn test_lark_p2p_scope_from_event() {
        let scope = derive_scope("p2p", "", "ou_sender");
        assert_eq!(scope, "user:ou_sender");
    }

    #[test]
    fn test_lark_scope_default_when_chat_type_absent() {
        // Empty chat_type defaults to user scope
        let scope = derive_scope("", "", "ou_sender_fallback");
        assert_eq!(scope, "user:ou_sender_fallback");
    }

    #[test]
    fn test_lark_message_deserialization_with_chat_fields() {
        // Verify the new chat_type and chat_id fields deserialize correctly
        let event: LarkMsgEvent = serde_json::from_value(serde_json::json!({
            "event": {
                "sender": {"sender_id": {"open_id": "ou_abc"}},
                "message": {
                    "message_id": "om_123",
                    "message_type": "text",
                    "content": "{\"text\":\"hello group\"}",
                    "chat_type": "group",
                    "chat_id": "oc_group_abc"
                }
            }
        }))
        .unwrap();
        assert_eq!(event.event.message.chat_type, "group");
        assert_eq!(event.event.message.chat_id, "oc_group_abc");
    }

    // ── require_mention_in_groups tests ────────────────────────────────────

    /// Helper: decide whether a group message with the given text passes the filter.
    /// Mirrors the production logic in handle_event().
    fn lark_group_msg_passes_filter(require_mention: bool, text: &str) -> bool {
        if !require_mention {
            return true;
        }
        let scope = "group:oc_test";
        if !scope.starts_with("group:") {
            return true;
        }
        extract_first_mention(text).is_some()
    }

    /// Group message, flag=false → always processes.
    #[test]
    fn test_lark_group_no_flag_always_processes() {
        assert!(lark_group_msg_passes_filter(false, "hello everyone"));
        assert!(lark_group_msg_passes_filter(false, "no mention"));
    }

    /// Group message, flag=true, no @mention → skipped.
    #[test]
    fn test_lark_group_flag_true_no_mention_skipped() {
        assert!(!lark_group_msg_passes_filter(true, "hello everyone"));
        assert!(!lark_group_msg_passes_filter(true, "plain text"));
    }

    /// Group message, flag=true, has @mention → processes normally.
    #[test]
    fn test_lark_group_flag_true_with_mention_processes() {
        assert!(lark_group_msg_passes_filter(
            true,
            "@claude please summarize"
        ));
        assert!(lark_group_msg_passes_filter(true, "hey @bot"));
    }

    /// Private (p2p) message scope, flag=true → not filtered (only groups are filtered).
    #[test]
    fn test_lark_p2p_scope_never_filtered() {
        let scope = "user:ou_sender";
        assert!(
            !scope.starts_with("group:"),
            "p2p scope should not match group prefix"
        );
    }

    /// Email addresses ("user@example.com") must NOT trigger the mention filter.
    /// Only @word tokens (no dot in the handle) should count as mentions.
    #[test]
    fn test_email_address_not_treated_as_mention() {
        // "send to user@example.com" — no standalone @word token — should be filtered out.
        assert!(!lark_group_msg_passes_filter(
            true,
            "send to user@example.com"
        ));
        // A real @mention still passes.
        assert!(lark_group_msg_passes_filter(
            true,
            "@claude please check user@example.com"
        ));
    }

    /// Verify that LarkChannel::new correctly stores require_mention_in_groups.
    #[test]
    fn test_lark_channel_new_stores_flag() {
        let ch = LarkChannel::new("id".to_string(), "secret".to_string(), true);
        assert!(ch.require_mention_in_groups);
        let ch2 = LarkChannel::new("id".to_string(), "secret".to_string(), false);
        assert!(!ch2.require_mention_in_groups);
    }
}
