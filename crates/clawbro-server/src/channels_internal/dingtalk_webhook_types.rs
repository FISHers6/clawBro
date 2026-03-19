use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DingTalkWebhookPayload {
    pub msgtype: String,
    #[serde(default)]
    pub text: Option<DingTalkWebhookText>,
    #[serde(default)]
    pub content: Option<DingTalkWebhookContent>,
    #[serde(rename = "msgId")]
    pub msg_id: String,
    #[serde(rename = "conversationType")]
    pub conversation_type: String,
    #[serde(rename = "conversationId")]
    pub conversation_id: String,
    #[serde(rename = "conversationTitle", default)]
    pub conversation_title: Option<String>,
    #[serde(rename = "senderId")]
    pub sender_id: String,
    #[serde(rename = "senderNick")]
    pub sender_nick: String,
    #[serde(rename = "senderPlatform", default)]
    pub sender_platform: Option<String>,
    #[serde(rename = "chatbotUserId", default)]
    pub chatbot_user_id: Option<String>,
    #[serde(rename = "openThreadId", default)]
    pub open_thread_id: Option<String>,
    #[serde(rename = "robotCode", default)]
    pub robot_code: Option<String>,
    #[serde(rename = "createAt", default)]
    pub create_at: Option<i64>,
    #[serde(rename = "isAdmin", default)]
    pub is_admin: Option<bool>,
    #[serde(rename = "isInAtList")]
    pub is_in_at_list: bool,
    #[serde(rename = "atUsers", default)]
    pub at_users: Vec<DingTalkWebhookAtUser>,
    #[serde(rename = "sessionWebhook")]
    pub session_webhook: String,
    #[serde(rename = "sessionWebhookExpiredTime")]
    pub session_webhook_expired_time: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DingTalkWebhookText {
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct DingTalkWebhookContent {
    #[serde(rename = "richText", default)]
    pub rich_text: Vec<DingTalkWebhookRichTextNode>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DingTalkWebhookAtUser {
    #[serde(rename = "dingtalkId")]
    pub dingtalk_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DingTalkWebhookRichTextNode {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default, rename = "type")]
    pub node_type: Option<String>,
    #[serde(default, rename = "downloadCode")]
    pub download_code: Option<String>,
    #[serde(default, rename = "pictureDownloadCode")]
    pub picture_download_code: Option<String>,
    #[serde(default, rename = "fileName")]
    pub file_name: Option<String>,
    #[serde(default, rename = "contentType")]
    pub content_type: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_deserializes_reference_fields() {
        let payload = r#"
        {
          "senderPlatform": "Mac",
          "conversationId": "cid5DEAySu/Fk+mtMwii4NLYQ==",
          "atUsers": [{ "dingtalkId": "$:LWCP_v1:$IHInSnicgNoAQhIiY9O0VGFxjAzvyVUf" }],
          "chatbotUserId": "$:LWCP_v1:$IHInSnicgNoAQhIiY9O0VGFxjAzvyVUf",
          "msgId": "msg/aa4qSVItb20ufU89R4V1A==",
          "senderNick": "User",
          "sessionWebhookExpiredTime": 1770982588732,
          "conversationType": "2",
          "isInAtList": true,
          "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=xxx",
          "text": { "content": "hello" },
          "robotCode": "normal",
          "msgtype": "text",
          "senderId": "sender-1"
        }
        "#;
        let parsed: DingTalkWebhookPayload = serde_json::from_str(payload).unwrap();
        assert_eq!(parsed.msgtype, "text");
        assert_eq!(parsed.msg_id, "msg/aa4qSVItb20ufU89R4V1A==");
        assert_eq!(parsed.conversation_type, "2");
        assert_eq!(parsed.conversation_id, "cid5DEAySu/Fk+mtMwii4NLYQ==");
        assert_eq!(parsed.sender_id, "sender-1");
        assert_eq!(parsed.sender_nick, "User");
        assert_eq!(parsed.text.as_ref().unwrap().content, "hello");
        assert!(parsed.content.is_none());
        assert_eq!(parsed.at_users.len(), 1);
        assert_eq!(
            parsed.at_users[0].dingtalk_id,
            "$:LWCP_v1:$IHInSnicgNoAQhIiY9O0VGFxjAzvyVUf"
        );
        assert_eq!(
            parsed.session_webhook,
            "https://oapi.dingtalk.com/robot/sendBySession?session=xxx"
        );
        assert_eq!(parsed.session_webhook_expired_time, 1_770_982_588_732);
        assert_eq!(parsed.robot_code.as_deref(), Some("normal"));
        assert_eq!(parsed.sender_platform.as_deref(), Some("Mac"));
        assert_eq!(parsed.chatbot_user_id.as_deref(), Some("$:LWCP_v1:$IHInSnicgNoAQhIiY9O0VGFxjAzvyVUf"));
    }
}
