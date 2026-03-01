// quickai-gateway/crates/qai-agent/src/slash.rs
/// Parsed slash command from user input.
#[derive(Debug, Clone, PartialEq)]
pub enum SlashCommand {
    /// /engine <name> — 切换当前 session 的 Agent 引擎
    /// Valid names: "rust", "claude", "codex", or any custom ACP binary path
    SetEngine(String),
    /// /reset — 清除当前 session 的对话历史
    Reset,
    /// /help — 显示可用命令列表
    Help,
    /// /remember <content> — 将内容追加写入 agent 记忆文件
    Remember(String),
    /// /memory — 查看当前 agent 记忆内容
    Memory,
    /// /forget <keyword> — 从记忆中删除包含关键词的条目
    Forget(String),
    /// /memory reset — 清空当前 agent 记忆文件
    MemoryReset,
}

impl SlashCommand {
    /// 解析用户输入，如果是已知 slash command 返回 Some，否则 None
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        if !input.starts_with('/') {
            return None;
        }
        let mut parts = input.splitn(2, ' ');
        let cmd = parts.next()?; // safe: trimmed non-empty string starting with '/'
        let arg = parts.next();
        match cmd {
            "/engine" => {
                let name = arg.map(|s| s.trim()).filter(|s| !s.is_empty())?;
                Some(Self::SetEngine(name.to_string()))
            }
            "/reset" => Some(Self::Reset),
            "/help" => Some(Self::Help),
            "/remember" => {
                let content = arg.map(|s| s.trim()).filter(|s| !s.is_empty())?;
                Some(Self::Remember(content.to_string()))
            }
            "/memory" => {
                match arg.map(|s| s.trim()) {
                    Some("reset") => Some(Self::MemoryReset),
                    _ => Some(Self::Memory),
                }
            }
            "/forget" => {
                let keyword = arg.map(|s| s.trim()).filter(|s| !s.is_empty())?;
                Some(Self::Forget(keyword.to_string()))
            }
            _ => None,
        }
    }

    /// 命令执行后返回给用户的确认文本
    pub fn confirmation_text(&self) -> String {
        match self {
            Self::SetEngine(name) => format!("✅ 引擎已切换到 {name}\n下次消息将使用新引擎处理"),
            Self::Reset => "✅ 对话历史已清除".to_string(),
            Self::Help => "可用命令：\n/engine <rust|claude|codex> — 切换引擎\n/reset — 清除历史\n/help — 显示帮助\n/remember <内容> — 写入记忆\n/memory — 查看记忆\n/memory reset — 清空记忆\n/forget <关键词> — 删除记忆条目".to_string(),
            Self::Remember(content) => format!("✅ 已记录：{content}"),
            // Unreachable in practice: registry's handle_slash returns early with real content.
            Self::Memory => "正在读取记忆…".to_string(),
            Self::Forget(keyword) => format!("✅ 已删除包含「{keyword}」的记忆条目"),
            Self::MemoryReset => unreachable!(
                "MemoryReset must be handled by handle_slash (two-step confirmation), not confirmation_text"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_engine_claude() {
        let cmd = SlashCommand::parse("/engine claude").unwrap();
        assert!(matches!(cmd, SlashCommand::SetEngine(ref s) if s == "claude"));
    }

    #[test]
    fn test_parse_engine_rust() {
        let cmd = SlashCommand::parse("/engine rust").unwrap();
        assert!(matches!(cmd, SlashCommand::SetEngine(ref s) if s == "rust"));
    }

    #[test]
    fn test_parse_reset() {
        let cmd = SlashCommand::parse("/reset").unwrap();
        assert!(matches!(cmd, SlashCommand::Reset));
    }

    #[test]
    fn test_parse_help() {
        let cmd = SlashCommand::parse("/help").unwrap();
        assert!(matches!(cmd, SlashCommand::Help));
    }

    #[test]
    fn test_parse_not_slash() {
        assert!(SlashCommand::parse("hello").is_none());
        assert!(SlashCommand::parse("").is_none());
    }

    #[test]
    fn test_parse_unknown_command() {
        assert!(SlashCommand::parse("/unknown_cmd").is_none());
    }

    #[test]
    fn test_confirmation_text_engine() {
        let cmd = SlashCommand::SetEngine("claude".to_string());
        assert!(cmd.confirmation_text().contains("claude"));
    }

    #[test]
    fn test_parse_engine_whitespace_only() {
        // /engine with only whitespace should return None (no engine name)
        assert!(SlashCommand::parse("/engine ").is_none());
        assert!(SlashCommand::parse("/engine   ").is_none());
    }

    #[test]
    fn test_parse_remember() {
        let cmd = SlashCommand::parse("/remember 我们用 Redis").unwrap();
        assert!(matches!(cmd, SlashCommand::Remember(ref s) if s == "我们用 Redis"));
    }

    #[test]
    fn test_parse_remember_requires_content() {
        assert!(SlashCommand::parse("/remember").is_none());
        assert!(SlashCommand::parse("/remember   ").is_none());
    }

    #[test]
    fn test_parse_memory() {
        let cmd = SlashCommand::parse("/memory").unwrap();
        assert!(matches!(cmd, SlashCommand::Memory));
    }

    #[test]
    fn test_parse_forget() {
        let cmd = SlashCommand::parse("/forget Redis").unwrap();
        assert!(matches!(cmd, SlashCommand::Forget(ref s) if s == "Redis"));
    }

    #[test]
    fn test_parse_memory_reset() {
        let cmd = SlashCommand::parse("/memory reset").unwrap();
        assert!(matches!(cmd, SlashCommand::MemoryReset));
    }
}
