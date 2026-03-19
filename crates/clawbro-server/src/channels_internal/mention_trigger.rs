//! MentionTrigger — Bot @mention Bot 异步触发
//!
//! 当任意 bot 回复后，扫描其输出中的 `@botname` 模式：
//!   - 若 botname 在已注册的 roster 列表中
//!   - 且当前消息来源不是 BotMention（防递归）
//!     → 生成一条新 InboundMsg { source: BotMention } 发到 channel
//!
//! 防递归设计：
//!   - 单层：BotMention 消息不再触发 MentionTrigger（source 检查）
//!   - 双层保险：hop_count 在 InboundMsg 中（由 registry 控制，此处不涉及）

use crate::protocol::{InboundMsg, MsgContent, MsgSource, SessionKey};
use chrono::Utc;
use tokio::sync::mpsc;
use uuid::Uuid;

// ─── MentionTrigger ──────────────────────────────────────────────────────────

pub struct MentionTrigger {
    /// 所有已注册 bot 的 mention 名称（如 "claude", "codex"）
    registered_bots: Vec<String>,
    /// 发送新 InboundMsg 的 channel（连接到 SessionRegistry 的入站队列）
    tx: mpsc::Sender<InboundMsg>,
}

impl MentionTrigger {
    pub fn new(registered_bots: Vec<String>, tx: mpsc::Sender<InboundMsg>) -> Self {
        Self {
            registered_bots,
            tx,
        }
    }

    /// 扫描 bot 输出，检测 @mention，生成 BotMention InboundMsg
    ///
    /// # 参数
    /// - `output`      — bot 回复的完整文本
    /// - `sender_name` — 发出此回复的 bot 名（不触发自己）
    /// - `scope`       — 群组 SessionKey（共享同一会话）
    /// - `source`      — 当前消息的来源（BotMention 时不再扫描，防递归）
    pub fn scan_and_dispatch(
        &self,
        output: &str,
        sender_name: &str,
        scope: &SessionKey,
        source: &MsgSource,
    ) {
        // 防递归（last-line defense）：automated 来源消息不触发新的 BotMention。
        // 外层 registry.rs Hook 3 已过滤大部分情况；这里是模块内的独立防御：
        // - BotMention: 直接递归 (Bot A → Bot B → Bot A)
        // - Heartbeat: Specialist 回复不应再触发额外 bot 调用
        // - TeamNotify: Lead 进度摘要中的 @bot 提及不应触发新 Specialist 任务
        if matches!(
            *source,
            MsgSource::BotMention | MsgSource::Heartbeat | MsgSource::TeamNotify
        ) {
            return;
        }

        for bot_name in &self.registered_bots {
            // 不触发自己
            if bot_name == sender_name {
                continue;
            }

            let pattern = format!("@{}", bot_name);
            if let Some(pos) = output.find(&pattern) {
                // 提取 @bot 之后的指令。
                // 支持两种格式：
                //   同行格式: "@codex 实现JWT"   → instruction = "实现JWT"
                //   换行格式: "@codex\n实现JWT"  → instruction = "实现JWT"
                let rest = &output[pos + pattern.len()..];
                let instruction = rest
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .unwrap_or("");

                if instruction.is_empty() {
                    continue;
                }

                let msg = InboundMsg {
                    id: Uuid::new_v4().to_string(),
                    session_key: scope.clone(),
                    content: MsgContent::text(instruction),
                    sender: sender_name.to_string(),
                    channel: scope.channel.clone(),
                    timestamp: Utc::now(),
                    thread_ts: None,
                    target_agent: Some(format!("@{}", bot_name)),
                    source: MsgSource::BotMention,
                };

                // try_send: 非阻塞，若 channel 满则丢弃（避免背压）
                if let Err(e) = self.tx.try_send(msg) {
                    tracing::warn!(
                        bot = %bot_name,
                        "MentionTrigger: failed to send BotMention: {}", e
                    );
                }
            }
        }
    }
}

// ─── 测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_trigger(bots: &[&str]) -> (MentionTrigger, mpsc::Receiver<InboundMsg>) {
        let (tx, rx) = mpsc::channel(16);
        let trigger = MentionTrigger::new(bots.iter().map(|s| s.to_string()).collect(), tx);
        (trigger, rx)
    }

    fn scope() -> SessionKey {
        SessionKey::new("lark", "group:test_room")
    }

    #[test]
    fn test_mention_generates_inbound_msg() {
        let (trigger, mut rx) = make_trigger(&["claude", "codex"]);
        trigger.scan_and_dispatch(
            "@codex 帮我写 jwt.rs",
            "claude",
            &scope(),
            &MsgSource::Human,
        );

        let msg = rx.try_recv().unwrap();
        assert_eq!(msg.target_agent, Some("@codex".to_string()));
        assert_eq!(msg.source, MsgSource::BotMention);
        assert!(msg.content.as_text().unwrap().contains("帮我写 jwt.rs"));
    }

    #[test]
    fn test_no_self_mention() {
        let (trigger, mut rx) = make_trigger(&["claude", "codex"]);
        // Claude 提到自己（@claude），不应触发
        trigger.scan_and_dispatch(
            "@claude 我需要更多信息",
            "claude",
            &scope(),
            &MsgSource::Human,
        );
        assert!(rx.try_recv().is_err(), "should not trigger self-mention");
    }

    #[test]
    fn test_bot_mention_source_no_recursion() {
        let (trigger, mut rx) = make_trigger(&["claude", "codex"]);
        // 来源已经是 BotMention → 不再触发
        trigger.scan_and_dispatch(
            "@claude 继续处理",
            "codex",
            &scope(),
            &MsgSource::BotMention,
        );
        assert!(rx.try_recv().is_err(), "BotMention source must not recurse");
    }

    #[test]
    fn test_no_mention_no_msg() {
        let (trigger, mut rx) = make_trigger(&["claude", "codex"]);
        trigger.scan_and_dispatch(
            "普通回复，无 @mention。",
            "claude",
            &scope(),
            &MsgSource::Human,
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_multiple_mentions_multiple_msgs() {
        let (trigger, mut rx) = make_trigger(&["claude", "codex", "gemini"]);
        // 一条消息中提到两个不同 bot
        trigger.scan_and_dispatch(
            "@codex 实现 JWT\n@gemini 写测试",
            "claude",
            &scope(),
            &MsgSource::Human,
        );
        let msg1 = rx.try_recv().unwrap();
        let msg2 = rx.try_recv().unwrap();
        let targets: Vec<_> = [&msg1, &msg2]
            .iter()
            .map(|m| m.target_agent.as_deref().unwrap_or(""))
            .collect();
        assert!(targets.contains(&"@codex"));
        assert!(targets.contains(&"@gemini"));
    }

    #[test]
    fn test_newline_separated_mention_triggers() {
        let (trigger, mut rx) = make_trigger(&["claude", "codex"]);
        // 换行格式：@mention 后紧接换行，指令在下一行
        trigger.scan_and_dispatch(
            "@codex\n帮我实现 JWT 验证",
            "claude",
            &scope(),
            &MsgSource::Human,
        );
        let msg = rx
            .try_recv()
            .expect("newline-separated @mention should trigger");
        assert_eq!(msg.target_agent, Some("@codex".to_string()));
        assert_eq!(msg.content.as_text().unwrap(), "帮我实现 JWT 验证");
    }

    #[test]
    fn test_unknown_bot_not_triggered() {
        let (trigger, mut rx) = make_trigger(&["claude"]);
        // @unknown 不在 roster 中
        trigger.scan_and_dispatch(
            "@unknown do something",
            "claude",
            &scope(),
            &MsgSource::Human,
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_team_notify_source_no_recursion() {
        let (trigger, mut rx) = make_trigger(&["claude", "codex"]);
        // Lead 处理 TeamNotify 后回复中提到 @codex — 不应触发新 Specialist 调用
        trigger.scan_and_dispatch(
            "@codex 已完成任务 T001，感谢配合。",
            "claude",
            &scope(),
            &MsgSource::TeamNotify,
        );
        assert!(
            rx.try_recv().is_err(),
            "TeamNotify source must not trigger BotMention"
        );
    }

    #[test]
    fn test_heartbeat_source_no_recursion() {
        let (trigger, mut rx) = make_trigger(&["claude", "codex"]);
        // Specialist 回复（Heartbeat 来源）不应触发额外 bot 调用
        trigger.scan_and_dispatch(
            "任务完成。请 @claude 确认结果。",
            "codex",
            &scope(),
            &MsgSource::Heartbeat,
        );
        assert!(
            rx.try_recv().is_err(),
            "Heartbeat source must not trigger BotMention"
        );
    }
}
