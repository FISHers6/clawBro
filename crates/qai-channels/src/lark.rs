//! Feishu/Lark WebSocket Channel
//! Implements Feishu long-connection WebSocket mode.
//! Env: LARK_APP_ID, LARK_APP_SECRET

use crate::mention_parsing::{derive_fanout_message_id, extract_agent_mentions};
use crate::traits::Channel;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use qai_protocol::{InboundMsg, MsgContent, OutboundMsg, SessionKey};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMsg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LarkTriggerMode {
    AllMessages,
    MentionOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LarkTriggerPolicy {
    pub group: LarkTriggerMode,
    pub dm: LarkTriggerMode,
}

impl LarkTriggerPolicy {
    pub const fn all_messages() -> Self {
        Self {
            group: LarkTriggerMode::AllMessages,
            dm: LarkTriggerMode::AllMessages,
        }
    }

    pub const fn from_require_mention_in_groups(require_mention_in_groups: bool) -> Self {
        Self {
            group: if require_mention_in_groups {
                LarkTriggerMode::MentionOnly
            } else {
                LarkTriggerMode::AllMessages
            },
            dm: LarkTriggerMode::AllMessages,
        }
    }
}

fn normalize_lark_text(text: &str) -> String {
    let mut tokens = text.split_whitespace().peekable();

    while let Some(token) = tokens.peek().copied() {
        if !is_lark_placeholder_mention(token) {
            break;
        }
        tokens.next();
    }

    tokens.collect::<Vec<_>>().join(" ")
}

fn is_lark_placeholder_mention(token: &str) -> bool {
    token
        .strip_prefix("@_")
        .map(|rest| {
            !rest.is_empty()
                && rest
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        })
        .unwrap_or(false)
}

fn has_lark_platform_trigger(text: &str) -> bool {
    text.split_whitespace().any(is_lark_placeholder_mention)
}

fn should_accept_lark_scope(
    policy: LarkTriggerPolicy,
    scope: &str,
    has_platform_trigger: bool,
) -> bool {
    let mode = if scope.starts_with("group:") {
        policy.group
    } else {
        policy.dm
    };

    match mode {
        LarkTriggerMode::AllMessages => true,
        LarkTriggerMode::MentionOnly => has_platform_trigger,
    }
}

const FEISHU_BASE: &str = "https://open.feishu.cn/open-apis";
/// WebSocket endpoint discovery URL (official Go SDK: /callback/ws/endpoint).
/// Posts {"AppID": ..., "AppSecret": ...} and returns {"code":0,"data":{"URL":"wss://..."}}
const FEISHU_WS_ENDPOINT_URL: &str = "https://open.feishu.cn/callback/ws/endpoint";

pub struct LarkChannel {
    pub instance_id: String,
    pub app_id: String,
    pub bot_name: Option<String>,
    known_bot_name_to_instance: HashMap<String, String>,
    group_ingress_owner: bool,
    pub app_secret: String,
    client: reqwest::Client,
    trigger_policy: LarkTriggerPolicy,
    accept_unmentioned_group_messages: bool,
    /// Cached access token: (token_string, time_fetched).
    /// Feishu app_access_token is valid for 7200s; we treat it as valid for 7000s
    /// to provide a safety margin against clock skew and network latency.
    token_cache: tokio::sync::Mutex<Option<(String, std::time::Instant)>>,
    seen_message_ids: tokio::sync::Mutex<HashMap<String, std::time::Instant>>,
}

impl LarkChannel {
    pub fn new(app_id: String, app_secret: String, trigger_policy: LarkTriggerPolicy) -> Self {
        Self::new_with_instance(
            "default",
            None,
            app_id,
            app_secret,
            trigger_policy,
            true,
            HashMap::new(),
            true,
        )
    }

    pub fn new_with_instance(
        instance_id: impl Into<String>,
        bot_name: Option<String>,
        app_id: String,
        app_secret: String,
        trigger_policy: LarkTriggerPolicy,
        accept_unmentioned_group_messages: bool,
        known_bot_name_to_instance: HashMap<String, String>,
        group_ingress_owner: bool,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            app_id,
            bot_name,
            known_bot_name_to_instance,
            group_ingress_owner,
            app_secret,
            client: reqwest::Client::new(),
            trigger_policy,
            accept_unmentioned_group_messages,
            token_cache: tokio::sync::Mutex::new(None),
            seen_message_ids: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    async fn should_accept_message(&self, message_id: &str) -> bool {
        const DEDUP_TTL_SECS: u64 = 600;

        let now = std::time::Instant::now();
        let mut seen = self.seen_message_ids.lock().await;
        seen.retain(|_, ts| now.duration_since(*ts).as_secs() < DEDUP_TTL_SECS);
        if seen.contains_key(message_id) {
            return false;
        }
        seen.insert(message_id.to_string(), now);
        true
    }

    /// Deprecated: use `LarkChannel::new()` with explicit trigger policy for full feature support.
    /// This method defaults to all-messages mode for both group and dm scopes.
    #[deprecated(note = "use LarkChannel::new() with explicit trigger policy")]
    pub fn from_env() -> Result<Self> {
        let app_id =
            std::env::var("LARK_APP_ID").map_err(|_| anyhow::anyhow!("LARK_APP_ID not set"))?;
        let app_secret = std::env::var("LARK_APP_SECRET")
            .map_err(|_| anyhow::anyhow!("LARK_APP_SECRET not set"))?;
        Ok(Self::new_with_instance(
            "default",
            None,
            app_id,
            app_secret,
            LarkTriggerPolicy::all_messages(),
            true,
            HashMap::new(),
            true,
        ))
    }

    async fn get_access_token(&self) -> Result<String> {
        // Feishu app_access_token TTL is 7200s; cache for 7000s to avoid using an expired token.
        const TOKEN_TTL_SECS: u64 = 7000;

        // Fast path: return cached token if still valid.
        // Lock is released before the HTTP request to avoid holding it across an await.
        {
            let cache = self.token_cache.lock().await;
            if let Some((ref token, fetched_at)) = *cache {
                if fetched_at.elapsed().as_secs() < TOKEN_TTL_SECS {
                    return Ok(token.clone());
                }
            }
        }

        // Slow path: fetch a new token from Feishu.
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
        let token = resp.app_access_token;
        // Update cache. Two concurrent fetches are harmless: both tokens are valid,
        // and the second write simply overwrites the first with an equivalent token.
        {
            let mut cache = self.token_cache.lock().await;
            *cache = Some((token.clone(), std::time::Instant::now()));
        }
        Ok(token)
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

#[derive(Clone, PartialEq, prost::Message)]
struct PbHeader {
    #[prost(string, tag = "1")]
    key: String,
    #[prost(string, tag = "2")]
    value: String,
}

#[derive(Clone, PartialEq, prost::Message)]
struct PbFrame {
    #[prost(uint64, tag = "1")]
    seq_id: u64,
    #[prost(uint64, tag = "2")]
    log_id: u64,
    #[prost(int32, tag = "3")]
    service: i32,
    #[prost(int32, tag = "4")]
    method: i32,
    #[prost(message, repeated, tag = "5")]
    headers: Vec<PbHeader>,
    #[prost(bytes = "vec", optional, tag = "8")]
    payload: Option<Vec<u8>>,
}

impl PbFrame {
    fn header_value(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|header| header.key == key)
            .map(|header| header.value.as_str())
    }
}

#[derive(Debug, Deserialize)]
struct LarkMsgEvent {
    #[serde(default)]
    header: LarkEventHeader,
    event: LarkMsgEventBody,
}

#[derive(Debug, Deserialize, Default)]
struct LarkEventHeader {
    #[serde(default)]
    app_id: String,
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
    #[serde(default)]
    mentions: Vec<LarkMention>,
}

#[derive(Deserialize)]
struct LarkTextContent {
    text: String,
}

#[derive(Debug, Deserialize, Default)]
struct LarkMention {
    #[serde(default)]
    name: String,
}

fn normalize_mention_name(value: &str) -> String {
    value.trim().trim_start_matches('@').to_lowercase()
}

fn current_instance_is_platform_mentioned(message: &LarkMessage, bot_name: Option<&str>) -> bool {
    let Some(bot_name) = bot_name else {
        return false;
    };
    let expected = normalize_mention_name(bot_name);
    !expected.is_empty()
        && message
            .mentions
            .iter()
            .map(|mention| normalize_mention_name(&mention.name))
            .any(|name| name == expected)
}

fn map_text_mentions_to_targets(
    text: &str,
    known_bot_name_to_instance: &HashMap<String, String>,
) -> Vec<String> {
    extract_agent_mentions(text)
        .into_iter()
        .filter(|mention| !is_lark_placeholder_mention(mention))
        .map(|mention| {
            let normalized = normalize_mention_name(&mention);
            known_bot_name_to_instance
                .get(&normalized)
                .map(|instance| format!("@{instance}"))
                .unwrap_or(mention)
        })
        .collect()
}

fn map_platform_mentions_to_targets(
    message: &LarkMessage,
    known_bot_name_to_instance: &HashMap<String, String>,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for mention in &message.mentions {
        let normalized = normalize_mention_name(&mention.name);
        if let Some(instance_id) = known_bot_name_to_instance.get(&normalized) {
            let target = format!("@{instance_id}");
            if seen.insert(target.clone()) {
                targets.push(target);
            }
        }
    }
    targets
}

fn target_channel_instance(
    target_agent: &str,
    known_bot_name_to_instance: &HashMap<String, String>,
) -> Option<String> {
    let candidate = target_agent.strip_prefix('@').map(str::trim)?;
    if known_bot_name_to_instance
        .values()
        .any(|value| value == candidate)
    {
        Some(candidate.to_string())
    } else {
        None
    }
}

fn decode_binary_frame(raw: &[u8]) -> Option<PbFrame> {
    PbFrame::decode(raw).ok()
}

fn decode_binary_event_payload(frame: &PbFrame) -> Option<serde_json::Value> {
    if frame.method != 1 {
        return None;
    }

    if frame.header_value("type")? != "event" {
        return None;
    }

    let payload = frame.payload.as_ref()?;
    serde_json::from_slice(payload).ok()
}

fn binary_ack_frame(frame: &PbFrame) -> Option<PbFrame> {
    if frame.method != 1 {
        return None;
    }

    let mut ack = frame.clone();
    ack.payload = Some(br#"{"code":200,"headers":{},"data":[]}"#.to_vec());
    ack.headers.push(PbHeader {
        key: "biz_rt".to_string(),
        value: "0".to_string(),
    });
    Some(ack)
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
            let client = self.client.clone();
            let url = format!("{FEISHU_BASE}/im/v1/messages/{message_id}/reply");
            let auth = format!("Bearer {token}");
            let body = serde_json::json!({
                "content": content_json,
                "msg_type": "text"
            });
            crate::send_with_retry(|| client.post(&url).header("Authorization", &auth).json(&body))
                .await?;
        } else if let Some(chat_id) = scope.strip_prefix("group:") {
            // Proactive group message — send to chat_id.
            let client = self.client.clone();
            let url = format!("{FEISHU_BASE}/im/v1/messages?receive_id_type=chat_id");
            let auth = format!("Bearer {token}");
            let body = serde_json::json!({
                "receive_id": chat_id,
                "content": content_json,
                "msg_type": "text"
            });
            crate::send_with_retry(|| client.post(&url).header("Authorization", &auth).json(&body))
                .await?;
        } else {
            // Proactive DM — scope is "user:{open_id}".
            if !scope.starts_with("user:") {
                tracing::warn!(
                    "Lark send: unexpected scope format '{}', attempting as open_id",
                    scope
                );
            }
            let open_id = scope.strip_prefix("user:").unwrap_or(scope.as_str());
            let client = self.client.clone();
            let url = format!("{FEISHU_BASE}/im/v1/messages?receive_id_type=open_id");
            let auth = format!("Bearer {token}");
            let body = serde_json::json!({
                "receive_id": open_id,
                "content": content_json,
                "msg_type": "text"
            });
            crate::send_with_retry(|| client.post(&url).header("Authorization", &auth).json(&body))
                .await?;
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
                                handle_event(self, data, &tx, &checker, self.trigger_policy).await;
                            }
                        }
                        _ => {}
                    }
                }
                Ok(WsMsg::Binary(raw)) => {
                    let Some(frame) = decode_binary_frame(&raw) else {
                        continue;
                    };

                    if let Some(ack) = binary_ack_frame(&frame) {
                        if let Err(e) = ws.send(WsMsg::Binary(ack.encode_to_vec().into())).await {
                            tracing::error!("Feishu WS binary ack send failed: {e}");
                            break;
                        }
                    }

                    if let Some(data) = decode_binary_event_payload(&frame) {
                        handle_event(self, data, &tx, &checker, self.trigger_policy).await;
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
    channel: &LarkChannel,
    data: serde_json::Value,
    tx: &mpsc::Sender<InboundMsg>,
    checker: &crate::allowlist::AllowlistChecker,
    trigger_policy: LarkTriggerPolicy,
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

    if !channel
        .should_accept_message(&event.event.message.message_id)
        .await
    {
        tracing::info!(
            message_id = %event.event.message.message_id,
            "Skipping duplicate Feishu inbound message"
        );
        return;
    }

    let raw_text = text_content.text.trim();
    if raw_text.is_empty() {
        return;
    }
    let has_platform_trigger = has_lark_platform_trigger(raw_text);
    let text = normalize_lark_text(raw_text);

    // Allowlist check
    if !checker.is_allowed("lark", &open_id) {
        tracing::debug!("AllowlistChecker: lark user {} denied", open_id);
        return;
    }

    // Derive scope: group uses chat_id, p2p uses open_id
    let scope = derive_scope(
        &event.event.message.chat_type,
        &event.event.message.chat_id,
        &open_id,
    );
    let is_group_scope = scope.starts_with("group:");
    let current_instance_mentioned =
        current_instance_is_platform_mentioned(&event.event.message, channel.bot_name.as_deref());

    if is_group_scope && !channel.group_ingress_owner {
        tracing::debug!(
            scope = %scope,
            instance = %channel.instance_id,
            "Lark group message skipped on non-owner instance"
        );
        return;
    }

    if is_group_scope {
        if has_platform_trigger {
            if !current_instance_mentioned && channel.known_bot_name_to_instance.is_empty() {
                tracing::debug!(
                    scope = %scope,
                    instance = %channel.instance_id,
                    app_id = %event.header.app_id,
                    configured_bot_name = channel.bot_name.as_deref().unwrap_or(""),
                    "Lark group message skipped because current instance was not mentioned"
                );
                return;
            }
        } else if !channel.accept_unmentioned_group_messages {
            tracing::debug!(
                scope = %scope,
                instance = %channel.instance_id,
                "Lark group message skipped on non-default instance without explicit mention"
            );
            return;
        }
    }

    if !should_accept_lark_scope(trigger_policy, &scope, has_platform_trigger) {
        tracing::debug!(
            scope = %scope,
            has_platform_trigger,
            ?trigger_policy,
            "Lark message skipped by trigger policy"
        );
        return;
    }

    if text.is_empty() {
        tracing::debug!(
            scope = %scope,
            message_id = %event.event.message.message_id,
            "Lark inbound dropped after platform mention normalization"
        );
        return;
    }

    let mut targets = Vec::new();
    if is_group_scope {
        targets.extend(map_platform_mentions_to_targets(
            &event.event.message,
            &channel.known_bot_name_to_instance,
        ));
    }
    for target in map_text_mentions_to_targets(&text, &channel.known_bot_name_to_instance) {
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    if targets.is_empty() && is_group_scope && current_instance_mentioned {
        targets.push(format!("@{}", channel.instance_id));
    }

    if targets.is_empty() {
        let inbound = InboundMsg {
            id: event.event.message.message_id,
            session_key: SessionKey::with_instance("lark", channel.instance_id.clone(), &scope),
            content: MsgContent::text(&text),
            sender: open_id,
            channel: "lark".to_string(),
            timestamp: Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: qai_protocol::MsgSource::Human,
        };
        let _ = tx.send(inbound).await;
        return;
    }

    for target_agent in targets {
        let channel_instance =
            target_channel_instance(&target_agent, &channel.known_bot_name_to_instance)
                .unwrap_or_else(|| channel.instance_id.clone());
        let inbound = InboundMsg {
            id: derive_fanout_message_id(&event.event.message.message_id, Some(&target_agent)),
            session_key: SessionKey::with_instance("lark", channel_instance, &scope),
            content: MsgContent::text(&text),
            sender: open_id.clone(),
            channel: "lark".to_string(),
            timestamp: Utc::now(),
            thread_ts: None,
            target_agent: Some(target_agent),
            source: qai_protocol::MsgSource::Human,
        };
        let _ = tx.send(inbound).await;
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize env-mutating tests to avoid races
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn instance_map() -> HashMap<String, String> {
        HashMap::from([
            ("claw".to_string(), "alpha".to_string()),
            ("claude".to_string(), "beta".to_string()),
        ])
    }

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
        assert_eq!(ch.instance_id, "default");
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
    fn test_extract_agent_mentions_basic() {
        assert_eq!(
            extract_agent_mentions("@claude review this"),
            vec!["@claude".to_string()]
        );
        assert!(extract_agent_mentions("no mention here").is_empty());
        assert_eq!(
            extract_agent_mentions("@codex please help @claude"),
            vec!["@codex".to_string(), "@claude".to_string()]
        );
        assert_eq!(
            extract_agent_mentions("hello @claude,"),
            vec!["@claude".to_string()]
        );
        assert!(extract_agent_mentions("").is_empty());
    }

    #[test]
    fn test_strip_lark_placeholder_mentions() {
        assert_eq!(normalize_lark_text("@_user_1 你好"), "你好");
        assert_eq!(
            normalize_lark_text("@_user_1 @_user_2 hello there"),
            "hello there"
        );
        assert_eq!(
            normalize_lark_text("@claude please review"),
            "@claude please review"
        );
        assert_eq!(normalize_lark_text("@_user_1"), "");
    }

    #[test]
    fn test_lark_platform_trigger_detected_from_placeholder() {
        assert!(has_lark_platform_trigger("@_user_1 你好"));
        assert!(has_lark_platform_trigger("hello @_user_1"));
        assert!(!has_lark_platform_trigger("@codex please review"));
    }

    #[test]
    fn test_lark_group_scope_from_event() {
        let scope = derive_scope("group", "oc_test_group", "ou_sender");
        assert_eq!(scope, "group:oc_test_group");
    }

    #[tokio::test]
    async fn test_lark_message_dedup_accepts_first_and_rejects_duplicate() {
        let channel = LarkChannel::new(
            "app".into(),
            "secret".into(),
            LarkTriggerPolicy::all_messages(),
        );
        assert!(channel.should_accept_message("om_1").await);
        assert!(!channel.should_accept_message("om_1").await);
        assert!(channel.should_accept_message("om_2").await);
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

    // ── trigger policy tests ───────────────────────────────────────────────

    fn policy(group: LarkTriggerMode, dm: LarkTriggerMode) -> LarkTriggerPolicy {
        LarkTriggerPolicy { group, dm }
    }

    #[test]
    fn test_lark_group_all_messages_always_processes() {
        let scope = "group:oc_test";
        assert!(should_accept_lark_scope(
            policy(LarkTriggerMode::AllMessages, LarkTriggerMode::AllMessages),
            scope,
            false
        ));
    }

    #[test]
    fn test_lark_group_mention_only_requires_platform_trigger() {
        let scope = "group:oc_test";
        let policy = policy(LarkTriggerMode::MentionOnly, LarkTriggerMode::AllMessages);
        assert!(!should_accept_lark_scope(policy, scope, false));
        assert!(should_accept_lark_scope(policy, scope, true));
    }

    #[test]
    fn test_lark_dm_all_messages_accepts_without_platform_trigger() {
        let scope = "user:ou_sender";
        assert!(should_accept_lark_scope(
            policy(LarkTriggerMode::MentionOnly, LarkTriggerMode::AllMessages),
            scope,
            false
        ));
    }

    #[test]
    fn test_lark_dm_mention_only_requires_platform_trigger() {
        let scope = "user:ou_sender";
        let policy = policy(LarkTriggerMode::AllMessages, LarkTriggerMode::MentionOnly);
        assert!(!should_accept_lark_scope(policy, scope, false));
        assert!(should_accept_lark_scope(policy, scope, true));
    }

    #[test]
    fn test_email_address_not_treated_as_platform_trigger() {
        assert!(!has_lark_platform_trigger("send to user@example.com"));
    }

    #[test]
    fn test_lark_channel_new_stores_policy() {
        let p = policy(LarkTriggerMode::MentionOnly, LarkTriggerMode::AllMessages);
        let ch = LarkChannel::new("id".to_string(), "secret".to_string(), p);
        assert_eq!(ch.trigger_policy, p);
        assert_eq!(ch.instance_id, "default");
    }

    #[test]
    fn test_lark_channel_new_with_instance_stores_instance_id() {
        let ch = LarkChannel::new_with_instance(
            "beta",
            Some("beta-bot".to_string()),
            "id".to_string(),
            "secret".to_string(),
            policy(LarkTriggerMode::AllMessages, LarkTriggerMode::AllMessages),
            false,
            HashMap::new(),
            false,
        );
        assert_eq!(ch.instance_id, "beta");
        assert_eq!(ch.bot_name.as_deref(), Some("beta-bot"));
    }

    #[tokio::test]
    async fn test_lark_inbound_session_key_carries_channel_instance() {
        let channel = LarkChannel::new_with_instance(
            "beta",
            Some("beta-bot".to_string()),
            "id".to_string(),
            "secret".to_string(),
            policy(LarkTriggerMode::AllMessages, LarkTriggerMode::AllMessages),
            false,
            HashMap::new(),
            false,
        );
        let checker = crate::allowlist::AllowlistChecker::load();
        let (tx, mut rx) = mpsc::channel(1);
        let event = serde_json::json!({
            "header": { "event_type": "im.message.receive_v1" },
            "event": {
                "sender": { "sender_id": { "open_id": "ou_beta" } },
                "message": {
                    "message_id": "om_beta_1",
                    "message_type": "text",
                    "content": "{\"text\":\"hello\"}",
                    "chat_type": "p2p",
                    "chat_id": ""
                }
            }
        });

        handle_event(
            &channel,
            event,
            &tx,
            &checker,
            policy(LarkTriggerMode::AllMessages, LarkTriggerMode::AllMessages),
        )
        .await;

        let inbound = rx.recv().await.expect("inbound");
        assert_eq!(
            inbound.session_key.channel_instance.as_deref(),
            Some("beta")
        );
    }

    #[tokio::test]
    async fn lark_group_message_only_dispatches_to_mentioned_instance() {
        let checker = crate::allowlist::AllowlistChecker::from_path(None::<std::path::PathBuf>);
        let (alpha_tx, mut alpha_rx) = mpsc::channel(1);
        let (beta_tx, mut beta_rx) = mpsc::channel(1);
        let alpha = LarkChannel::new_with_instance(
            "alpha",
            Some("claw".to_string()),
            "id-alpha".to_string(),
            "secret".to_string(),
            policy(LarkTriggerMode::MentionOnly, LarkTriggerMode::AllMessages),
            true,
            instance_map(),
            true,
        );
        let beta = LarkChannel::new_with_instance(
            "beta",
            Some("Claude".to_string()),
            "id-beta".to_string(),
            "secret".to_string(),
            policy(LarkTriggerMode::MentionOnly, LarkTriggerMode::AllMessages),
            false,
            instance_map(),
            false,
        );
        let payload = serde_json::json!({
            "header": { "event_type": "im.message.receive_v1", "app_id": "id-alpha" },
            "event": {
                "sender": { "sender_id": { "open_id": "ou_group_user" } },
                "message": {
                    "message_id": "om_group_alpha",
                    "message_type": "text",
                    "content": "{\"text\":\"@_user_1 你好\"}",
                    "chat_type": "group",
                    "chat_id": "oc_group_1",
                    "mentions": [{"name":"claw"}]
                }
            }
        });

        handle_event(
            &alpha,
            payload.clone(),
            &alpha_tx,
            &checker,
            alpha.trigger_policy,
        )
        .await;
        handle_event(&beta, payload, &beta_tx, &checker, beta.trigger_policy).await;

        let inbound = alpha_rx
            .recv()
            .await
            .expect("alpha should receive the message");
        assert_eq!(inbound.target_agent.as_deref(), Some("@alpha"));
        assert!(
            beta_rx.try_recv().is_err(),
            "beta should skip alpha-only mention"
        );
    }

    #[tokio::test]
    async fn lark_unmentioned_group_message_only_default_instance_accepts() {
        let checker = crate::allowlist::AllowlistChecker::from_path(None::<std::path::PathBuf>);
        let (alpha_tx, mut alpha_rx) = mpsc::channel(1);
        let (beta_tx, mut beta_rx) = mpsc::channel(1);
        let alpha = LarkChannel::new_with_instance(
            "alpha",
            Some("claw".to_string()),
            "id-alpha".to_string(),
            "secret".to_string(),
            policy(LarkTriggerMode::AllMessages, LarkTriggerMode::AllMessages),
            true,
            instance_map(),
            true,
        );
        let beta = LarkChannel::new_with_instance(
            "beta",
            Some("Claude".to_string()),
            "id-beta".to_string(),
            "secret".to_string(),
            policy(LarkTriggerMode::AllMessages, LarkTriggerMode::AllMessages),
            false,
            instance_map(),
            false,
        );
        let payload = serde_json::json!({
            "header": { "event_type": "im.message.receive_v1", "app_id": "id-alpha" },
            "event": {
                "sender": { "sender_id": { "open_id": "ou_group_user" } },
                "message": {
                    "message_id": "om_group_unmentioned",
                    "message_type": "text",
                    "content": "{\"text\":\"你好\"}",
                    "chat_type": "group",
                    "chat_id": "oc_group_1",
                    "mentions": []
                }
            }
        });

        handle_event(
            &alpha,
            payload.clone(),
            &alpha_tx,
            &checker,
            alpha.trigger_policy,
        )
        .await;
        handle_event(&beta, payload, &beta_tx, &checker, beta.trigger_policy).await;

        assert!(
            alpha_rx.recv().await.is_some(),
            "default instance should accept"
        );
        assert!(
            beta_rx.try_recv().is_err(),
            "non-default instance should skip"
        );
    }

    #[tokio::test]
    async fn lark_group_message_with_multiple_platform_mentions_fans_out() {
        let checker = crate::allowlist::AllowlistChecker::from_path(None::<std::path::PathBuf>);
        let (alpha_tx, mut alpha_rx) = mpsc::channel(4);
        let (beta_tx, mut beta_rx) = mpsc::channel(1);
        let alpha = LarkChannel::new_with_instance(
            "alpha",
            Some("claw".to_string()),
            "id-alpha".to_string(),
            "secret".to_string(),
            policy(LarkTriggerMode::MentionOnly, LarkTriggerMode::AllMessages),
            true,
            instance_map(),
            true,
        );
        let beta = LarkChannel::new_with_instance(
            "beta",
            Some("Claude".to_string()),
            "id-beta".to_string(),
            "secret".to_string(),
            policy(LarkTriggerMode::MentionOnly, LarkTriggerMode::AllMessages),
            false,
            instance_map(),
            false,
        );
        let payload = serde_json::json!({
            "header": { "event_type": "im.message.receive_v1", "app_id": "id-alpha" },
            "event": {
                "sender": { "sender_id": { "open_id": "ou_group_user" } },
                "message": {
                    "message_id": "om_group_multi",
                    "message_type": "text",
                    "content": "{\"text\":\"@_user_1 @_user_2 你好\"}",
                    "chat_type": "group",
                    "chat_id": "oc_group_1",
                    "mentions": [{"name":"claw"}, {"name":"Claude"}]
                }
            }
        });

        handle_event(
            &alpha,
            payload.clone(),
            &alpha_tx,
            &checker,
            alpha.trigger_policy,
        )
        .await;
        handle_event(&beta, payload, &beta_tx, &checker, beta.trigger_policy).await;

        let first = alpha_rx.recv().await.expect("first fanout inbound");
        let second = alpha_rx.recv().await.expect("second fanout inbound");
        let mut targets = vec![
            first.target_agent.unwrap_or_default(),
            second.target_agent.unwrap_or_default(),
        ];
        targets.sort();
        assert_eq!(targets, vec!["@alpha".to_string(), "@beta".to_string()]);
        assert_ne!(first.id, second.id);
        assert!(
            beta_rx.try_recv().is_err(),
            "non-owner instance must not duplicate fanout"
        );
    }

    #[test]
    fn test_lark_binary_event_payload_decodes() {
        let payload = serde_json::json!({
            "header": { "event_type": "im.message.receive_v1" },
            "event": {
                "sender": { "sender_id": { "open_id": "ou_binary" } },
                "message": {
                    "message_id": "om_binary_1",
                    "message_type": "text",
                    "content": "{\"text\":\"hello from binary\"}",
                    "chat_type": "p2p",
                    "chat_id": ""
                }
            }
        });
        let frame = PbFrame {
            seq_id: 7,
            log_id: 8,
            service: 9,
            method: 1,
            headers: vec![PbHeader {
                key: "type".to_string(),
                value: "event".to_string(),
            }],
            payload: Some(payload.to_string().into_bytes()),
        };

        let raw = frame.encode_to_vec();
        let decoded = decode_binary_frame(&raw).expect("binary frame should decode");
        let event = decode_binary_event_payload(&decoded).expect("event payload should decode");
        assert_eq!(
            event["event"]["sender"]["sender_id"]["open_id"],
            serde_json::json!("ou_binary")
        );
        assert_eq!(
            event["event"]["message"]["message_id"],
            serde_json::json!("om_binary_1")
        );
    }

    #[test]
    fn test_lark_binary_ack_frame_preserves_identity_fields() {
        let frame = PbFrame {
            seq_id: 11,
            log_id: 12,
            service: 13,
            method: 1,
            headers: vec![PbHeader {
                key: "type".to_string(),
                value: "event".to_string(),
            }],
            payload: Some(br#"{"header":{"event_type":"im.message.receive_v1"}}"#.to_vec()),
        };

        let ack = binary_ack_frame(&frame).expect("ack should be generated");
        assert_eq!(ack.seq_id, 11);
        assert_eq!(ack.log_id, 12);
        assert_eq!(ack.service, 13);
        assert_eq!(ack.method, 1);
        assert_eq!(
            ack.headers
                .iter()
                .find(|header| header.key == "biz_rt")
                .map(|header| header.value.as_str()),
            Some("0")
        );
        assert_eq!(
            ack.payload.as_deref(),
            Some(br#"{"code":200,"headers":{},"data":[]}"#.as_slice())
        );
    }

    #[tokio::test]
    async fn test_lark_binary_event_dispatches_inbound_message() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = crate::allowlist::AllowlistChecker::from_path(None::<std::path::PathBuf>);
        let channel = LarkChannel::new(
            "app".into(),
            "secret".into(),
            LarkTriggerPolicy::all_messages(),
        );
        let payload = serde_json::json!({
            "header": { "event_type": "im.message.receive_v1" },
            "event": {
                "sender": { "sender_id": { "open_id": "ou_binary_dispatch" } },
                "message": {
                    "message_id": "om_binary_dispatch",
                    "message_type": "text",
                    "content": "{\"text\":\"ping from binary\"}",
                    "chat_type": "p2p",
                    "chat_id": ""
                }
            }
        });

        handle_event(
            &channel,
            payload,
            &tx,
            &checker,
            LarkTriggerPolicy::all_messages(),
        )
        .await;

        let inbound = rx.recv().await.expect("inbound message should dispatch");
        assert_eq!(inbound.id, "om_binary_dispatch");
        assert_eq!(inbound.session_key.channel, "lark");
        assert_eq!(inbound.session_key.scope, "user:ou_binary_dispatch");
        assert_eq!(inbound.sender, "ou_binary_dispatch");
        assert_eq!(inbound.target_agent, None);
        match inbound.content {
            MsgContent::Text { text } => assert_eq!(text, "ping from binary"),
            other => panic!("unexpected content: {other:?}"),
        }
    }

    #[tokio::test]
    async fn lark_platform_mention_is_removed_from_model_visible_text() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = crate::allowlist::AllowlistChecker::from_path(None::<std::path::PathBuf>);
        let channel = LarkChannel::new(
            "app".into(),
            "secret".into(),
            LarkTriggerPolicy::all_messages(),
        );
        let payload = serde_json::json!({
            "header": { "event_type": "im.message.receive_v1" },
            "event": {
                "sender": { "sender_id": { "open_id": "ou_normalize" } },
                "message": {
                    "message_id": "om_normalize",
                    "message_type": "text",
                    "content": "{\"text\":\"@_user_1 你好\"}",
                    "chat_type": "p2p",
                    "chat_id": ""
                }
            }
        });

        handle_event(
            &channel,
            payload,
            &tx,
            &checker,
            LarkTriggerPolicy::all_messages(),
        )
        .await;

        let inbound = rx.recv().await.expect("message should dispatch");
        assert_eq!(inbound.target_agent, None);
        assert_eq!(inbound.content.as_text(), Some("你好"));
    }

    #[tokio::test]
    async fn lark_explicit_agent_mention_survives_normalization() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = crate::allowlist::AllowlistChecker::from_path(None::<std::path::PathBuf>);
        let channel = LarkChannel::new(
            "app".into(),
            "secret".into(),
            LarkTriggerPolicy::all_messages(),
        );
        let payload = serde_json::json!({
            "header": { "event_type": "im.message.receive_v1" },
            "event": {
                "sender": { "sender_id": { "open_id": "ou_route" } },
                "message": {
                    "message_id": "om_route",
                    "message_type": "text",
                    "content": "{\"text\":\"@_user_1 @codex 帮我看下\"}",
                    "chat_type": "p2p",
                    "chat_id": ""
                }
            }
        });

        handle_event(
            &channel,
            payload,
            &tx,
            &checker,
            LarkTriggerPolicy::all_messages(),
        )
        .await;

        let inbound = rx.recv().await.expect("message should dispatch");
        assert_eq!(inbound.target_agent.as_deref(), Some("@codex"));
        assert_eq!(inbound.content.as_text(), Some("@codex 帮我看下"));
    }

    #[tokio::test]
    async fn lark_message_with_only_platform_mention_is_dropped() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = crate::allowlist::AllowlistChecker::from_path(None::<std::path::PathBuf>);
        let channel = LarkChannel::new(
            "app".into(),
            "secret".into(),
            LarkTriggerPolicy::all_messages(),
        );
        let payload = serde_json::json!({
            "header": { "event_type": "im.message.receive_v1" },
            "event": {
                "sender": { "sender_id": { "open_id": "ou_empty" } },
                "message": {
                    "message_id": "om_empty",
                    "message_type": "text",
                    "content": "{\"text\":\"@_user_1\"}",
                    "chat_type": "p2p",
                    "chat_id": ""
                }
            }
        });

        handle_event(
            &channel,
            payload,
            &tx,
            &checker,
            LarkTriggerPolicy::all_messages(),
        )
        .await;

        assert!(
            rx.try_recv().is_err(),
            "mention-only message should be dropped"
        );
    }
}
