use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 消息来源（区分人工、Bot 触发、内部调度等）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MsgSource {
    /// 人工在 IM 发送的消息（默认）
    #[default]
    Human,
    /// Bot A 输出触发 Bot B（MentionTrigger 扫描 @mention）
    BotMention,
    /// RelayEngine 内部同步委托（`[RELAY: @agent ...]` 语法）
    Relay,
    /// Cron 定时任务触发
    Cron,
    /// OrchestratorHeartbeat 派发给 Specialist
    Heartbeat,
    /// Gateway → Lead: 通知 Specialist 任务完成 / 全部完成
    TeamNotify,
}

/// 入站消息（来自任意 Channel 或 WebSocket 客户端）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMsg {
    pub id: String,
    pub session_key: SessionKey,
    pub content: MsgContent,
    pub sender: String,
    pub channel: String,
    pub timestamp: DateTime<Utc>,
    pub thread_ts: Option<String>,
    #[serde(default)]
    pub target_agent: Option<String>, // @mention extracted by Channel (e.g. "@claude")
    /// 消息来源（默认 Human，向后兼容）
    #[serde(default)]
    pub source: MsgSource,
}

/// 会话定位键（参考 zeroclaw dmScope 设计）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub channel: String,
    pub scope: String,
}

impl SessionKey {
    pub fn new(channel: impl Into<String>, scope: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            scope: scope.into(),
        }
    }
}

/// 消息内容
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MsgContent {
    Text {
        text: String,
    },
    Image {
        url: String,
        caption: Option<String>,
    },
    File {
        url: String,
        name: String,
    },
}

impl MsgContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
    pub fn as_text(&self) -> Option<&str> {
        if let Self::Text { text } = self {
            Some(text)
        } else {
            None
        }
    }
}

/// 出站消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundMsg {
    pub session_key: SessionKey,
    pub content: MsgContent,
    pub reply_to: Option<String>,
    pub thread_ts: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_key_equality() {
        let k1 = SessionKey::new("dingtalk", "user_123");
        let k2 = SessionKey::new("dingtalk", "user_123");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_msg_content_serialization() {
        let msg = MsgContent::text("hello");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"Text\""));
    }

    #[test]
    fn test_team_notify_variant_exists() {
        let src = MsgSource::TeamNotify;
        // Must serialize without panic
        let json = serde_json::to_string(&src).unwrap();
        assert!(json.contains("team_notify"));
        // Must deserialize back
        let back: MsgSource = serde_json::from_str(&json).unwrap();
        assert_eq!(back, MsgSource::TeamNotify);
    }
}
