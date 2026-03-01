//! DingTalk Stream Mode Channel (MVP #1)
//! 文档参考: https://open.dingtalk.com/document/orgapp/stream
//! 认证: DINGTALK_APP_KEY + DINGTALK_APP_SECRET 环境变量

use crate::traits::Channel;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use qai_protocol::{InboundMsg, MsgContent, OutboundMsg, SessionKey};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMsg;
use uuid::Uuid;

/// Extract first @mention from message text (e.g. "@claude review" → Some("@claude"))
/// DingTalk strips the @robot_name from the message text already, so remaining
/// @mentions are user-directed agent mentions.
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

/// Derive the session scope from a DingTalk event's inner data object.
///
/// - Group chat (`conversationType == "2"`): `"group:{conversationId}"`
/// - Private chat (anything else, including `"1"`): `"user:{senderId}"`
fn derive_scope(data: &serde_json::Value) -> String {
    let conversation_type = data["conversationType"].as_str().unwrap_or("1");
    if conversation_type == "2" {
        let conversation_id = data["conversationId"].as_str().unwrap_or("unknown");
        format!("group:{}", conversation_id)
    } else {
        let sender_id = data["senderId"].as_str().unwrap_or("unknown");
        format!("user:{}", sender_id)
    }
}

#[derive(Debug, Clone)]
pub struct DingTalkConfig {
    pub app_key: String,
    pub app_secret: String,
}

impl DingTalkConfig {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            app_key: std::env::var("DINGTALK_APP_KEY")
                .map_err(|_| anyhow::anyhow!("DINGTALK_APP_KEY not set"))?,
            app_secret: std::env::var("DINGTALK_APP_SECRET")
                .map_err(|_| anyhow::anyhow!("DINGTALK_APP_SECRET not set"))?,
        })
    }
}

pub struct DingTalkChannel {
    config: DingTalkConfig,
    client: reqwest::Client,
}

impl DingTalkChannel {
    pub fn new(config: DingTalkConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    async fn get_access_token(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct TokenResp {
            access_token: String,
        }
        let resp: TokenResp = self
            .client
            .post("https://api.dingtalk.com/v1.0/oauth2/accessToken")
            .json(&serde_json::json!({
                "appKey": self.config.app_key,
                "appSecret": self.config.app_secret
            }))
            .send()
            .await?
            .json()
            .await?;
        Ok(resp.access_token)
    }
}

#[async_trait]
impl Channel for DingTalkChannel {
    fn name(&self) -> &str {
        "dingtalk"
    }

    async fn send(&self, msg: &OutboundMsg) -> Result<()> {
        let text = match &msg.content {
            MsgContent::Text { text } => text.clone(),
            _ => "[unsupported content type]".to_string(),
        };
        let scope = &msg.session_key.scope;

        if let Some(webhook_url) = &msg.thread_ts {
            // Preferred: in-thread reply via sessionWebhook (works for both group and DM).
            // No auth header needed — the access_token is embedded in the URL.
            self.client
                .post(webhook_url)
                .json(&serde_json::json!({
                    "msgtype": "text",
                    "text": { "content": text }
                }))
                .send()
                .await?;
        } else if let Some(conversation_id) = scope.strip_prefix("group:") {
            // Proactive group message via openConversationId.
            let token = self.get_access_token().await?;
            self.client
                .post("https://api.dingtalk.com/v1.0/robot/groupMessages/send")
                .header("x-acs-dingtalk-access-token", &token)
                .json(&serde_json::json!({
                    "robotCode": self.config.app_key,
                    "openConversationId": conversation_id,
                    "msgKey": "sampleText",
                    "msgParam": serde_json::json!({ "content": text }).to_string(),
                }))
                .send()
                .await?;
        } else {
            // Proactive DM via batchSend — scope is "user:{senderId}".
            let user_id = scope.strip_prefix("user:").unwrap_or(scope.as_str());
            let token = self.get_access_token().await?;
            self.client
                .post("https://api.dingtalk.com/v1.0/robot/oToMessages/batchSend")
                .header("x-acs-dingtalk-access-token", &token)
                .json(&serde_json::json!({
                    "robotCode": self.config.app_key,
                    "userIds": [user_id],
                    "msgKey": "sampleText",
                    "msgParam": serde_json::json!({ "content": text }).to_string(),
                }))
                .send()
                .await?;
        }
        Ok(())
    }

    async fn listen(&self, tx: mpsc::Sender<InboundMsg>) -> Result<()> {
        let token = self.get_access_token().await?;
        let endpoint_resp: serde_json::Value = self
            .client
            .post("https://api.dingtalk.com/v1.0/gateway/connections/open")
            .header("x-acs-dingtalk-access-token", &token)
            .json(&serde_json::json!({
                "clientId": self.config.app_key,
                "clientSecret": self.config.app_secret,
                "subscriptions": [
                    { "type": "EVENT", "topic": "chat_update_pull_v1" }
                ]
            }))
            .send()
            .await?
            .json()
            .await?;

        let ws_url = endpoint_resp["endpoint"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No endpoint in DingTalk response"))?
            .to_string();
        let ticket = endpoint_resp["ticket"]
            .as_str()
            .unwrap_or("")
            .to_string();

        tracing::info!("DingTalk Stream Mode connecting: {}", ws_url);

        let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| anyhow::anyhow!("WS connect failed: {e}"))?;

        // 发送注册帧
        let register = serde_json::json!({
            "specVersion": "1.0",
            "stage": "REGISTER",
            "headers": {
                "chId": "ch1",
                "chType": "STREAM",
                "topic": "/v1.0/im/bot/messages/get",
                "contentType": "application/json"
            },
            "data": ticket
        });
        ws.send(WsMsg::Text(register.to_string().into())).await?;

        let checker = crate::allowlist::AllowlistChecker::load();

        while let Some(Ok(msg)) = ws.next().await {
            if let WsMsg::Text(text) = msg {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(text.as_str()) {
                    // Extract eventId from outer frame headers — stable per-message ID for dedup.
                    // Falls back to UUID only if the field is absent (non-standard event).
                    let event_id = v["headers"]["eventId"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| Uuid::new_v4().to_string());

                    if let Some(data_str) = v["data"].as_str() {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(data_str) {
                            let user_id =
                                data["senderId"].as_str().unwrap_or("unknown").to_string();
                            // Allowlist check uses senderId regardless of chat type.
                            if !checker.is_allowed("dingtalk", &user_id) {
                                tracing::debug!("AllowlistChecker: dingtalk user {} denied", user_id);
                                continue;
                            }
                            // Derive scope: group chat uses conversationId, private chat uses senderId.
                            let scope = derive_scope(&data);
                            let content_text = data["text"]["content"]
                                .as_str()
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            // sessionWebhook is the in-thread reply URL provided by DingTalk Stream Mode.
                            // Stored in thread_ts so DingTalkChannel::send() can POST to it directly.
                            let session_webhook =
                                data["sessionWebhook"].as_str().map(str::to_string);
                            if !content_text.is_empty() {
                                // Extract first @mention for agent routing
                                let target_agent = extract_first_mention(&content_text);
                                let inbound = InboundMsg {
                                    id: event_id,
                                    session_key: SessionKey::new("dingtalk", &scope),
                                    content: MsgContent::text(&content_text),
                                    sender: user_id,
                                    channel: "dingtalk".to_string(),
                                    timestamp: Utc::now(),
                                    thread_ts: session_webhook,
                                    target_agent,
                                };
                                let _ = tx.send(inbound).await;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn update_typing(&self, _scope: &str) -> Result<()> {
        // DingTalk 不支持 typing indicator
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize env-mutating tests to avoid races
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_dingtalk_config_from_env_missing() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("DINGTALK_APP_KEY");
            std::env::remove_var("DINGTALK_APP_SECRET");
        }
        assert!(DingTalkConfig::from_env().is_err());
    }

    #[test]
    fn test_dingtalk_config_from_env_ok() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("DINGTALK_APP_KEY", "test_key");
            std::env::set_var("DINGTALK_APP_SECRET", "test_secret");
        }
        let cfg = DingTalkConfig::from_env().unwrap();
        assert_eq!(cfg.app_key, "test_key");
        assert_eq!(cfg.app_secret, "test_secret");
        unsafe {
            std::env::remove_var("DINGTALK_APP_KEY");
            std::env::remove_var("DINGTALK_APP_SECRET");
        }
    }

    #[test]
    fn test_dingtalk_channel_name() {
        let cfg = DingTalkConfig {
            app_key: "k".to_string(),
            app_secret: "s".to_string(),
        };
        let ch = DingTalkChannel::new(cfg);
        assert_eq!(ch.name(), "dingtalk");
    }

    /// Verify that a group chat event (conversationType="2") produces scope "group:{conversationId}".
    #[test]
    fn test_dingtalk_group_scope_from_event() {
        let data = serde_json::json!({
            "senderId": "sender_001",
            "conversationType": "2",
            "conversationId": "cid_test",
            "text": { "content": "hello group" }
        });
        let scope = derive_scope(&data);
        assert_eq!(scope, "group:cid_test");
    }

    /// Verify that a private chat event (conversationType="1") produces scope "user:{senderId}".
    #[test]
    fn test_dingtalk_private_scope_from_event() {
        let data = serde_json::json!({
            "senderId": "sender_001",
            "conversationType": "1",
            "text": { "content": "hello private" }
        });
        let scope = derive_scope(&data);
        assert_eq!(scope, "user:sender_001");
    }

    /// Verify that a private chat event with no conversationType defaults to private scope.
    #[test]
    fn test_dingtalk_private_scope_default_when_type_absent() {
        let data = serde_json::json!({
            "senderId": "sender_002",
            "text": { "content": "hello" }
        });
        let scope = derive_scope(&data);
        assert_eq!(scope, "user:sender_002");
    }

    /// Verify that eventId is correctly extracted from the outer WS frame headers,
    /// sessionWebhook is correctly extracted from the inner data payload,
    /// and that conversationType/conversationId are parsed to produce the correct scope.
    #[test]
    fn test_dingtalk_event_id_and_webhook_parsing() {
        let outer_frame = serde_json::json!({
            "specVersion": "1.0",
            "type": "EVENT",
            "headers": {
                "appId": "test_app",
                "eventId": "event_abc123",
                "topic": "/v1.0/im/bot/messages/get"
            },
            "data": serde_json::json!({
                "senderId": "user_001",
                "conversationType": "2",
                "conversationId": "cid_group_001",
                "text": { "content": "  hello world  " },
                "sessionWebhook": "https://oapi.dingtalk.com/robot/send?access_token=tok",
                "sessionWebhookExpiredTime": 9_999_999_999_i64
            }).to_string()
        });

        let event_id = outer_frame["headers"]["eventId"].as_str().unwrap_or("");
        assert_eq!(event_id, "event_abc123");

        let data_str = outer_frame["data"].as_str().unwrap();
        let data: serde_json::Value = serde_json::from_str(data_str).unwrap();

        let webhook = data["sessionWebhook"].as_str().unwrap_or("");
        assert_eq!(webhook, "https://oapi.dingtalk.com/robot/send?access_token=tok");

        let sender = data["senderId"].as_str().unwrap();
        assert_eq!(sender, "user_001");

        let content = data["text"]["content"].as_str().unwrap().trim();
        assert_eq!(content, "hello world");

        // Group chat scope must use conversationId
        let scope = derive_scope(&data);
        assert_eq!(scope, "group:cid_group_001");
    }

    /// Verify that a frame without eventId falls back gracefully (non-empty string).
    #[test]
    fn test_dingtalk_event_id_fallback_when_absent() {
        let outer_frame = serde_json::json!({
            "type": "EVENT",
            "headers": {},
            "data": "{}"
        });

        // When absent, eventId returns None; production code falls back to UUID.
        let event_id = outer_frame["headers"]["eventId"].as_str();
        assert!(event_id.is_none(), "should be None when field absent");
    }

    /// Verify that a frame without sessionWebhook produces None (triggers batchSend fallback).
    #[test]
    fn test_dingtalk_session_webhook_absent() {
        let data_str = serde_json::json!({
            "senderId": "user_002",
            "text": { "content": "ping" }
        })
        .to_string();
        let data: serde_json::Value = serde_json::from_str(&data_str).unwrap();
        let webhook = data["sessionWebhook"].as_str().map(str::to_string);
        assert!(webhook.is_none(), "sessionWebhook absent → None → batchSend fallback");
    }
}
