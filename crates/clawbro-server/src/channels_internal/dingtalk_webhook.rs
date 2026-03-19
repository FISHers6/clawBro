use crate::channels_internal::allowlist::AllowlistChecker;
use crate::channels_internal::dingtalk_webhook_dedup::DingTalkWebhookDedup;
use crate::channels_internal::dingtalk_webhook_mapper::{map_payload, DingTalkWebhookMapped};
use crate::channels_internal::dingtalk_webhook_reply::{
    send_text_by_robot_access_token, send_text_by_session_webhook,
};
use crate::channels_internal::dingtalk_webhook_richtext::{
    inject_resolved_image_urls, parse_richtext_nodes, resolve_richtext_image_urls,
    RichTextImageTask,
};
use crate::channels_internal::dingtalk_webhook_types::DingTalkWebhookPayload;
use crate::channels_internal::mention_parsing::{derive_fanout_message_id, extract_agent_mentions};
use crate::channels_internal::traits::Channel;
use crate::config::DingTalkWebhookSection;
use crate::protocol::{InboundMsg, MsgContent, MsgSource, OutboundMsg};
use anyhow::Result;
use async_trait::async_trait;
use axum::http::HeaderMap;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::mpsc;

pub const DINGTALK_TOKEN_HEADER: &str = "token";
const DEFAULT_DEDUP_CAPACITY: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DingTalkWebhookRejectReason {
    MissingToken,
    InvalidToken,
    InvalidPayload,
    UnsupportedDirectMessage,
    UnsupportedMessageType,
    EmptyContent,
    GroupMessageWithoutMention,
    UserNotAllowed,
    DuplicateEvent,
}

#[derive(Debug, Clone)]
pub struct DingTalkWebhookIngress {
    pub mapped: DingTalkWebhookMapped,
    pub target_agents: Vec<String>,
    pub rich_text_images: Vec<RichTextImageTask>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionWebhookLease {
    webhook_url: String,
    expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplyRoute {
    SessionWebhook(String),
    RobotAccessToken,
}

pub struct DingTalkWebhookChannel {
    secret_key: String,
    webhook_path: String,
    access_token: Option<String>,
    client: reqwest::Client,
    allowlist: AllowlistChecker,
    dedup: Mutex<DingTalkWebhookDedup>,
    session_webhooks: Mutex<HashMap<String, SessionWebhookLease>>,
}

impl DingTalkWebhookChannel {
    pub fn new(config: DingTalkWebhookSection) -> Self {
        Self {
            secret_key: config.secret_key.trim().to_string(),
            webhook_path: normalize_webhook_path(&config.webhook_path),
            access_token: config
                .access_token
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            client: reqwest::Client::new(),
            allowlist: AllowlistChecker::load(),
            dedup: Mutex::new(DingTalkWebhookDedup::new(DEFAULT_DEDUP_CAPACITY)),
            session_webhooks: Mutex::new(HashMap::new()),
        }
    }

    pub fn webhook_path(&self) -> &str {
        &self.webhook_path
    }

    pub fn ingest(
        &self,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<DingTalkWebhookIngress, DingTalkWebhookRejectReason> {
        verify_token(&self.secret_key, extract_token_header(headers))?;
        let payload =
            parse_payload(body).map_err(|_| DingTalkWebhookRejectReason::InvalidPayload)?;
        dedup_event(&self.dedup, &payload.msg_id)?;
        let (mapped, rich_text_images) = validate_payload_for_phase1(&self.allowlist, &payload)?;
        self.record_session_webhook_lease(
            &mapped.session_key.scope,
            &mapped.session_webhook,
            payload.session_webhook_expired_time,
        );
        let target_agents = extract_agent_mentions(&mapped.text);
        Ok(DingTalkWebhookIngress {
            mapped,
            target_agents,
            rich_text_images,
        })
    }

    pub async fn to_inbound_messages(&self, ingress: DingTalkWebhookIngress) -> Vec<InboundMsg> {
        let DingTalkWebhookIngress {
            mut mapped,
            target_agents,
            rich_text_images,
        } = ingress;
        if !rich_text_images.is_empty() {
            let resolved_urls = resolve_richtext_image_urls(
                &self.client,
                self.access_token.as_deref(),
                mapped.robot_code.as_deref(),
                &rich_text_images,
            )
            .await;
            mapped.text = inject_resolved_image_urls(&mapped.text, &resolved_urls);
        }
        let make_inbound = |id: String, target_agent: Option<String>| InboundMsg {
            id,
            session_key: mapped.session_key.clone(),
            content: MsgContent::text(mapped.text.clone()),
            sender: mapped.sender_id.clone(),
            channel: "dingtalk_webhook".to_string(),
            timestamp: Utc::now(),
            thread_ts: Some(mapped.session_webhook.clone()),
            target_agent,
            source: MsgSource::Human,
        };
        if target_agents.is_empty() {
            vec![make_inbound(mapped.msg_id, None)]
        } else {
            target_agents
                .into_iter()
                .map(|target_agent| {
                    let id = derive_fanout_message_id(&mapped.msg_id, Some(&target_agent));
                    make_inbound(id, Some(target_agent))
                })
                .collect()
        }
    }

    fn record_session_webhook_lease(&self, scope: &str, webhook_url: &str, expires_at_ms: i64) {
        let lease = SessionWebhookLease {
            webhook_url: webhook_url.to_string(),
            expires_at_ms,
        };
        self.session_webhooks
            .lock()
            .expect("dingtalk webhook lease mutex poisoned")
            .insert(scope.to_string(), lease);
    }

    fn resolve_reply_route(
        &self,
        scope: &str,
        thread_ts: Option<&str>,
        now_ms: i64,
    ) -> Option<ReplyRoute> {
        let leases = self
            .session_webhooks
            .lock()
            .expect("dingtalk webhook lease mutex poisoned");
        if let Some(thread_ts) = thread_ts.map(str::trim).filter(|value| !value.is_empty()) {
            match leases.get(scope) {
                Some(lease) if lease.webhook_url == thread_ts && lease.expires_at_ms <= now_ms => {}
                _ => return Some(ReplyRoute::SessionWebhook(thread_ts.to_string())),
            }
        }
        if let Some(lease) = leases.get(scope) {
            if lease.expires_at_ms > now_ms {
                return Some(ReplyRoute::SessionWebhook(lease.webhook_url.clone()));
            }
        }
        drop(leases);
        self.access_token
            .as_ref()
            .map(|_| ReplyRoute::RobotAccessToken)
    }
}

#[async_trait]
impl Channel for DingTalkWebhookChannel {
    fn name(&self) -> &str {
        "dingtalk_webhook"
    }

    async fn send(&self, msg: &OutboundMsg) -> Result<()> {
        let text = match &msg.content {
            MsgContent::Text { text } => text.as_str(),
            _ => "[unsupported content type]",
        };
        match self.resolve_reply_route(
            &msg.session_key.scope,
            msg.thread_ts.as_deref(),
            Utc::now().timestamp_millis(),
        ) {
            Some(ReplyRoute::SessionWebhook(session_webhook)) => {
                send_text_by_session_webhook(&self.client, &self.secret_key, &session_webhook, text)
                    .await
            }
            Some(ReplyRoute::RobotAccessToken) => {
                let access_token = self
                    .access_token
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("dingtalk_webhook access token missing"))?;
                send_text_by_robot_access_token(&self.client, &self.secret_key, access_token, text)
                    .await
            }
            None => Err(anyhow::anyhow!(
                "dingtalk_webhook has no valid sessionWebhook lease and no access_token fallback"
            )),
        }
    }

    async fn listen(&self, _tx: mpsc::Sender<InboundMsg>) -> Result<()> {
        anyhow::bail!("dingtalk_webhook uses HTTP ingress and does not support listen()")
    }
}

pub fn normalize_webhook_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        "/channels/dingtalk/webhook".to_string()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

pub fn extract_token_header(headers: &HeaderMap) -> Option<&str> {
    headers.get(DINGTALK_TOKEN_HEADER)?.to_str().ok()
}

pub fn verify_token(
    secret_key: &str,
    token: Option<&str>,
) -> Result<(), DingTalkWebhookRejectReason> {
    let token = token.map(str::trim).filter(|value| !value.is_empty());
    let secret = secret_key.trim();
    let token = token.ok_or(DingTalkWebhookRejectReason::MissingToken)?;
    if token == secret {
        Ok(())
    } else {
        Err(DingTalkWebhookRejectReason::InvalidToken)
    }
}

pub fn parse_payload(body: &[u8]) -> anyhow::Result<DingTalkWebhookPayload> {
    Ok(serde_json::from_slice(body)?)
}

pub fn derive_scope(payload: &DingTalkWebhookPayload) -> Option<String> {
    match payload.conversation_type.as_str() {
        "2" => Some(format!("group:{}", payload.conversation_id)),
        _ => None,
    }
}

pub fn should_process_group_message(payload: &DingTalkWebhookPayload) -> bool {
    payload.is_in_at_list
        || payload.at_users.iter().any(|user| {
            payload
                .chatbot_user_id
                .as_deref()
                .is_some_and(|bot_id| user.dingtalk_id == bot_id)
        })
}

pub fn check_allowlist(checker: &AllowlistChecker, payload: &DingTalkWebhookPayload) -> bool {
    checker.is_allowed("dingtalk", &payload.sender_id)
}

pub fn dedup_event(
    dedup: &Mutex<DingTalkWebhookDedup>,
    event_id: &str,
) -> Result<(), DingTalkWebhookRejectReason> {
    let mut guard = dedup.lock().expect("dingtalk webhook dedup mutex poisoned");
    if guard.record_if_new(event_id) {
        Ok(())
    } else {
        Err(DingTalkWebhookRejectReason::DuplicateEvent)
    }
}

pub fn validate_payload_for_phase1(
    checker: &AllowlistChecker,
    payload: &DingTalkWebhookPayload,
) -> Result<(DingTalkWebhookMapped, Vec<RichTextImageTask>), DingTalkWebhookRejectReason> {
    if derive_scope(payload).is_none() {
        return Err(DingTalkWebhookRejectReason::UnsupportedDirectMessage);
    }
    if !should_process_group_message(payload) {
        return Err(DingTalkWebhookRejectReason::GroupMessageWithoutMention);
    }
    if !check_allowlist(checker, payload) {
        return Err(DingTalkWebhookRejectReason::UserNotAllowed);
    }
    let (text, rich_text_images) = extract_phase1_text(payload)?;
    let mapped =
        map_payload(payload, text).ok_or(DingTalkWebhookRejectReason::UnsupportedDirectMessage)?;
    Ok((mapped, rich_text_images))
}

fn extract_phase1_text(
    payload: &DingTalkWebhookPayload,
) -> Result<(String, Vec<RichTextImageTask>), DingTalkWebhookRejectReason> {
    if let Some(text) = payload
        .text
        .as_ref()
        .map(|text| text.content.trim())
        .filter(|text| !text.is_empty())
    {
        return Ok((text.to_string(), Vec::new()));
    }
    if payload.msgtype.eq_ignore_ascii_case("richText") || payload.content.is_some() {
        let rich_text = payload
            .content
            .as_ref()
            .map(|content| content.rich_text.as_slice())
            .unwrap_or(&[]);
        let render = parse_richtext_nodes(rich_text);
        let text = render.text.trim().to_string();
        if !text.is_empty() {
            return Ok((text, render.images));
        }
        return Err(DingTalkWebhookRejectReason::EmptyContent);
    }
    Err(DingTalkWebhookRejectReason::UnsupportedMessageType)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn sample_group_payload() -> DingTalkWebhookPayload {
        serde_json::from_value(serde_json::json!({
            "senderPlatform": "Mac",
            "conversationId": "cid-group-1",
            "atUsers": [{ "dingtalkId": "bot-1" }],
            "chatbotUserId": "bot-1",
            "msgId": "msg-1",
            "senderNick": "User",
            "senderId": "user-1",
            "sessionWebhookExpiredTime": 1770982588732i64,
            "conversationType": "2",
            "isInAtList": true,
            "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=xxx",
            "text": { "content": "hello @claude" },
            "robotCode": "normal",
            "msgtype": "text"
        }))
        .unwrap()
    }

    fn checker_from_json(json: &str) -> AllowlistChecker {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(json.as_bytes()).unwrap();
        AllowlistChecker::from_path(Some(f.path()))
    }

    #[test]
    fn dingtalk_webhook_verification_accepts_exact_token_match() {
        let mut headers = HeaderMap::new();
        headers.insert(DINGTALK_TOKEN_HEADER, HeaderValue::from_static("SEC-test"));
        let token = extract_token_header(&headers);
        assert_eq!(verify_token("SEC-test", token), Ok(()));
    }

    #[test]
    fn dingtalk_webhook_verification_rejects_missing_token() {
        let headers = HeaderMap::new();
        let token = extract_token_header(&headers);
        assert_eq!(
            verify_token("SEC-test", token),
            Err(DingTalkWebhookRejectReason::MissingToken)
        );
    }

    #[test]
    fn dingtalk_webhook_verification_rejects_mismatched_token() {
        let mut headers = HeaderMap::new();
        headers.insert(DINGTALK_TOKEN_HEADER, HeaderValue::from_static("SEC-other"));
        let token = extract_token_header(&headers);
        assert_eq!(
            verify_token("SEC-test", token),
            Err(DingTalkWebhookRejectReason::InvalidToken)
        );
    }

    #[test]
    fn dingtalk_webhook_scope_maps_group_messages() {
        let payload = sample_group_payload();
        assert_eq!(derive_scope(&payload).as_deref(), Some("group:cid-group-1"));
    }

    #[test]
    fn dingtalk_webhook_scope_rejects_direct_messages_in_phase1() {
        let mut payload = sample_group_payload();
        payload.conversation_type = "1".to_string();
        assert_eq!(derive_scope(&payload), None);
    }

    #[test]
    fn dingtalk_webhook_mention_accepts_group_when_in_at_list() {
        let payload = sample_group_payload();
        assert!(should_process_group_message(&payload));
    }

    #[test]
    fn dingtalk_webhook_mention_rejects_group_without_mention() {
        let mut payload = sample_group_payload();
        payload.is_in_at_list = false;
        payload.at_users.clear();
        assert!(!should_process_group_message(&payload));
    }

    #[test]
    fn dingtalk_webhook_allowlist_uses_existing_allowlist_checker() {
        let payload = sample_group_payload();
        let checker = checker_from_json(
            r#"{"dingtalk":{"enabled":true,"mode":"allowlist","users":["user-1"]}}"#,
        );
        assert!(check_allowlist(&checker, &payload));

        let denied = checker_from_json(
            r#"{"dingtalk":{"enabled":true,"mode":"allowlist","users":["user-2"]}}"#,
        );
        assert!(!check_allowlist(&denied, &payload));
    }

    #[test]
    fn dingtalk_webhook_dedup_rejects_duplicate_msg_id() {
        let dedup = Mutex::new(DingTalkWebhookDedup::new(16));
        assert_eq!(dedup_event(&dedup, "msg-1"), Ok(()));
        assert_eq!(
            dedup_event(&dedup, "msg-1"),
            Err(DingTalkWebhookRejectReason::DuplicateEvent)
        );
    }

    #[test]
    fn dingtalk_webhook_validate_payload_for_phase1_accepts_group_mentions() {
        let payload = sample_group_payload();
        let checker = checker_from_json(
            r#"{"dingtalk":{"enabled":true,"mode":"allowlist","users":["user-1"]}}"#,
        );
        let (mapped, rich_text_images) = validate_payload_for_phase1(&checker, &payload).unwrap();
        assert_eq!(mapped.session_key.channel, "dingtalk_webhook");
        assert_eq!(mapped.session_key.scope, "group:cid-group-1");
        assert_eq!(mapped.text, "hello @claude");
        assert!(rich_text_images.is_empty());
    }

    #[tokio::test]
    async fn dingtalk_webhook_ingest_builds_fanout_messages_from_agent_mentions() {
        let channel = DingTalkWebhookChannel::new(DingTalkWebhookSection {
            enabled: true,
            secret_key: "SEC-test".to_string(),
            webhook_path: "/channels/dingtalk/webhook".to_string(),
            access_token: None,
            presentation: crate::config::ProgressPresentationMode::FinalOnly,
        });
        let mut headers = HeaderMap::new();
        headers.insert(DINGTALK_TOKEN_HEADER, HeaderValue::from_static("SEC-test"));
        let body = serde_json::to_vec(&sample_group_payload()).unwrap();
        let ingress = channel.ingest(&headers, &body).unwrap();
        let messages = channel.to_inbound_messages(ingress).await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].target_agent.as_deref(), Some("@claude"));
        assert_eq!(
            messages[0].thread_ts.as_deref(),
            Some("https://oapi.dingtalk.com/robot/sendBySession?session=xxx")
        );
    }

    #[tokio::test]
    async fn dingtalk_webhook_richtext_images_are_left_as_placeholders_without_access_token() {
        let channel = DingTalkWebhookChannel::new(DingTalkWebhookSection {
            enabled: true,
            secret_key: "SEC-test".to_string(),
            webhook_path: "/channels/dingtalk/webhook".to_string(),
            access_token: None,
            presentation: crate::config::ProgressPresentationMode::FinalOnly,
        });
        let mut headers = HeaderMap::new();
        headers.insert(DINGTALK_TOKEN_HEADER, HeaderValue::from_static("SEC-test"));
        let body = serde_json::to_vec(&serde_json::json!({
            "senderPlatform": "Mac",
            "conversationId": "cid-group-1",
            "atUsers": [{ "dingtalkId": "bot-1" }],
            "chatbotUserId": "bot-1",
            "msgId": "msg-rich-1",
            "senderNick": "User",
            "senderId": "user-1",
            "sessionWebhookExpiredTime": 1770982588732i64,
            "conversationType": "2",
            "isInAtList": true,
            "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=xxx",
            "robotCode": "robot-code-1",
            "msgtype": "richText",
            "content": {
                "richText": [
                    { "text": "look" },
                    { "type": "picture", "downloadCode": "dc-1" }
                ]
            }
        }))
        .unwrap();
        let ingress = channel.ingest(&headers, &body).unwrap();
        let messages = channel.to_inbound_messages(ingress).await;
        assert_eq!(messages.len(), 1);
        match &messages[0].content {
            MsgContent::Text { text } => assert_eq!(text, "look [image]"),
            other => panic!("expected text content, got {other:?}"),
        }
    }

    #[test]
    fn normalize_webhook_path_adds_leading_slash() {
        assert_eq!(
            normalize_webhook_path("dingtalk-channel/message"),
            "/dingtalk-channel/message"
        );
    }

    #[test]
    fn resolve_reply_route_prefers_valid_cached_lease_when_thread_ts_missing() {
        let channel = DingTalkWebhookChannel::new(DingTalkWebhookSection {
            enabled: true,
            secret_key: "SEC-test".to_string(),
            webhook_path: "/channels/dingtalk/webhook".to_string(),
            access_token: Some("token-1".to_string()),
            presentation: crate::config::ProgressPresentationMode::FinalOnly,
        });
        channel.record_session_webhook_lease("group:cid-group-1", "https://session", 2_000);
        assert_eq!(
            channel.resolve_reply_route("group:cid-group-1", None, 1_000),
            Some(ReplyRoute::SessionWebhook("https://session".to_string()))
        );
    }

    #[test]
    fn resolve_reply_route_falls_back_to_access_token_when_cached_lease_expired() {
        let channel = DingTalkWebhookChannel::new(DingTalkWebhookSection {
            enabled: true,
            secret_key: "SEC-test".to_string(),
            webhook_path: "/channels/dingtalk/webhook".to_string(),
            access_token: Some("token-1".to_string()),
            presentation: crate::config::ProgressPresentationMode::FinalOnly,
        });
        channel.record_session_webhook_lease("group:cid-group-1", "https://session", 1_000);
        assert_eq!(
            channel.resolve_reply_route("group:cid-group-1", Some("https://session"), 2_000),
            Some(ReplyRoute::RobotAccessToken)
        );
    }

    #[test]
    fn resolve_reply_route_uses_fresh_thread_ts_even_without_cached_lease() {
        let channel = DingTalkWebhookChannel::new(DingTalkWebhookSection {
            enabled: true,
            secret_key: "SEC-test".to_string(),
            webhook_path: "/channels/dingtalk/webhook".to_string(),
            access_token: None,
            presentation: crate::config::ProgressPresentationMode::FinalOnly,
        });
        assert_eq!(
            channel.resolve_reply_route("group:cid-group-1", Some("https://session-live"), 2_000),
            Some(ReplyRoute::SessionWebhook(
                "https://session-live".to_string()
            ))
        );
    }
}
