use crate::channels_internal::dingtalk_webhook_types::DingTalkWebhookPayload;
use crate::protocol::SessionKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DingTalkWebhookMapped {
    pub session_key: SessionKey,
    pub sender_id: String,
    pub sender_nick: String,
    pub msg_id: String,
    pub text: String,
    pub session_webhook: String,
    pub robot_code: Option<String>,
}

pub fn map_payload(
    payload: &DingTalkWebhookPayload,
    text: String,
) -> Option<DingTalkWebhookMapped> {
    match payload.conversation_type.as_str() {
        "2" => Some(DingTalkWebhookMapped {
            session_key: SessionKey::new(
                "dingtalk_webhook",
                format!("group:{}", payload.conversation_id),
            ),
            sender_id: payload.sender_id.clone(),
            sender_nick: payload.sender_nick.clone(),
            msg_id: payload.msg_id.clone(),
            text,
            session_webhook: payload.session_webhook.clone(),
            robot_code: payload.robot_code.clone(),
        }),
        _ => None,
    }
}
