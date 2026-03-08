//! RelayEngine — 同步透明委托（Relay Mode）
//!
//! Lead 在回复中使用 `[RELAY: @agent <指令>]` 语法，RelayEngine 检测到后：
//!   1. 同步触发 Specialist turn（等待结果）
//!   2. 将 RELAY 标记替换为 Specialist 的回复文本
//!
//! 与 Team Mode（异步派发）的区别：
//!   - Relay Mode：同步、立即等待结果、适合单步委托（≤20s）
//!   - Team Mode：异步、OrchestratorHeartbeat 调度、适合并行长任务
//!
//! 语法：`[RELAY: @agentname <指令文本>]`
//! 示例：`[RELAY: @codex 帮我验证 JWT 签名逻辑]`

use anyhow::Result;
use qai_protocol::{MsgContent, SessionKey};
use std::future::Future;
use std::pin::Pin;

// ─── 类型 ────────────────────────────────────────────────────────────────────

/// 从 Lead 输出中解析出的一个 RELAY 指令
#[derive(Debug, Clone, PartialEq)]
pub struct RelayMarker {
    /// 原始匹配文本，如 `[RELAY: @codex verify JWT]`
    pub raw: String,
    /// Agent 名（无 @），如 `codex`
    pub agent_name: String,
    /// 传给 Specialist 的指令
    pub instruction: String,
}

/// 同步调度函数签名（由 registry 或 main.rs 注入）：
///   (target_agent: "@codex", content, session_key) → specialist reply or None
pub type RelayDispatchFn = Box<
    dyn Fn(
            String,
            MsgContent,
            SessionKey,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>>> + Send>>
        + Send
        + Sync,
>;

// ─── 解析 ────────────────────────────────────────────────────────────────────

/// 从文本中提取所有 `[RELAY: @agent <指令>]` 标记（纯函数，无副作用）
pub fn extract_relay_markers(text: &str) -> Vec<RelayMarker> {
    // 手动解析：避免 regex 依赖
    let mut markers = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("[RELAY:") {
        let after_start = &remaining[start..];
        if let Some(end) = after_start.find(']') {
            let raw = &after_start[..=end];
            // raw = "[RELAY: @codex verify JWT]"
            let inner = raw
                .trim_start_matches("[RELAY:")
                .trim_end_matches(']')
                .trim();
            // inner = "@codex verify JWT"
            if let Some(rest) = inner.strip_prefix('@') {
                // rest = "codex verify JWT"
                if let Some(space) = rest.find(char::is_whitespace) {
                    let agent_name = rest[..space].trim().to_string();
                    let instruction = rest[space..].trim().to_string();
                    if !agent_name.is_empty() && !instruction.is_empty() {
                        markers.push(RelayMarker {
                            raw: raw.to_string(),
                            agent_name,
                            instruction,
                        });
                    }
                }
            }
            remaining = &after_start[end + 1..];
        } else {
            break;
        }
    }

    markers
}

// ─── RelayEngine ─────────────────────────────────────────────────────────────

pub struct RelayEngine {
    dispatch_fn: RelayDispatchFn,
}

impl RelayEngine {
    pub fn new(dispatch_fn: RelayDispatchFn) -> Self {
        Self { dispatch_fn }
    }

    /// 处理 Lead 回复中的 RELAY 标记，返回替换后的文本
    ///
    /// - 若无 RELAY 标记，原样返回
    /// - 若 Specialist 回复成功，用其回复替换标记
    /// - 若 Specialist 失败，用错误提示替换标记
    ///
    /// `source` 必须不是 `BotMention`（Relay 是同步委托，不是 Bot 自发触发）
    pub async fn process(&self, lead_reply: &str, scope: &SessionKey) -> Result<String> {
        let markers = extract_relay_markers(lead_reply);
        if markers.is_empty() {
            return Ok(lead_reply.to_string());
        }

        let mut result = lead_reply.to_string();
        for marker in &markers {
            let target = format!("@{}", marker.agent_name);
            let content = MsgContent::text(&marker.instruction);
            let key = scope.clone();

            match (self.dispatch_fn)(target, content, key).await {
                Ok(Some(specialist_reply)) => {
                    result = result.replace(&marker.raw, &specialist_reply);
                }
                Ok(None) => {
                    // Specialist 无输出
                    let fallback = format!("[{} 无回复]", marker.agent_name);
                    result = result.replace(&marker.raw, &fallback);
                }
                Err(e) => {
                    tracing::warn!(
                        agent = %marker.agent_name,
                        "relay dispatch error: {:#}", e
                    );
                    let fallback = format!("[relay error: {}]", e);
                    result = result.replace(&marker.raw, &fallback);
                }
            }
        }

        Ok(result)
    }
}

// ─── 测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_single_relay_marker() {
        let text = "需要代码层面验证，[RELAY: @codex 帮我验证 JWT 签名逻辑] 完成后告知。";
        let markers = extract_relay_markers(text);
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].agent_name, "codex");
        assert_eq!(markers[0].instruction, "帮我验证 JWT 签名逻辑");
        assert_eq!(markers[0].raw, "[RELAY: @codex 帮我验证 JWT 签名逻辑]");
    }

    #[test]
    fn test_extract_multiple_relay_markers() {
        let text = "[RELAY: @codex implement JWT] and [RELAY: @gemini review docs]";
        let markers = extract_relay_markers(text);
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].agent_name, "codex");
        assert_eq!(markers[1].agent_name, "gemini");
    }

    #[test]
    fn test_extract_no_relay_markers() {
        let text = "普通回复，无 RELAY 标记。@codex 这里不是标记语法。";
        let markers = extract_relay_markers(text);
        assert!(markers.is_empty());
    }

    #[test]
    fn test_extract_incomplete_syntax_ignored() {
        // 缺少 @ 前缀
        let text = "[RELAY: codex verify this]";
        let markers = extract_relay_markers(text);
        assert!(markers.is_empty());
    }

    #[test]
    fn test_extract_no_instruction_ignored() {
        // @codex 后面没有指令
        let text = "[RELAY: @codex]";
        let markers = extract_relay_markers(text);
        assert!(markers.is_empty());
    }

    #[tokio::test]
    async fn test_relay_engine_replaces_marker() {
        let engine = RelayEngine::new(Box::new(|agent, content, _key| {
            Box::pin(async move {
                assert_eq!(agent, "@codex");
                let text = match content {
                    MsgContent::Text { text } => text,
                    _ => panic!("expected text"),
                };
                assert!(text.contains("verify JWT"));
                Ok(Some(format!("Codex回复: JWT已验证")))
            })
        }));

        let scope = SessionKey::new("lark", "group:test");
        let lead_reply = "分析完成。[RELAY: @codex verify JWT signature] 请继续。";
        let result = engine.process(lead_reply, &scope).await.unwrap();

        assert!(result.contains("Codex回复: JWT已验证"));
        assert!(!result.contains("[RELAY:"));
    }

    #[tokio::test]
    async fn test_relay_engine_no_markers_passthrough() {
        let engine = RelayEngine::new(Box::new(|_agent, _content, _key| {
            Box::pin(async { panic!("should not be called") })
        }));

        let scope = SessionKey::new("lark", "group:test");
        let lead_reply = "普通回复，无委托。";
        let result = engine.process(lead_reply, &scope).await.unwrap();
        assert_eq!(result, "普通回复，无委托。");
    }

    #[tokio::test]
    async fn test_relay_engine_specialist_no_reply() {
        let engine = RelayEngine::new(Box::new(|_agent, _content, _key| {
            Box::pin(async { Ok(None) })
        }));

        let scope = SessionKey::new("lark", "group:test");
        let result = engine
            .process("[RELAY: @codex do something]", &scope)
            .await
            .unwrap();
        assert!(result.contains("codex 无回复") || result.contains("无回复"));
    }
}
