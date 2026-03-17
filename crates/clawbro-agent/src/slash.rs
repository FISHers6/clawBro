// clawbro-gateway/crates/clawbro-agent/src/slash.rs
/// Parsed slash command from user input.
#[derive(Debug, Clone, PartialEq)]
pub enum SlashCommand {
    /// /backend <id-or-agent-name> — 切换当前 session 的 runtime backend
    SetBackend(String),
    /// /reset — 清除当前 session 的对话历史
    Reset,
    /// /help — 显示可用命令列表
    Help,
    /// /remember <content> — 将内容追加写入 agent 记忆文件
    Remember(String),
    /// /memory [<@agent>] — 查看记忆内容（无参数=共享记忆，@agent=指定 agent 记忆）
    Memory(Option<String>),
    /// /forget <keyword> — 从记忆中删除包含关键词的条目
    Forget(String),
    /// /memory reset — 清空当前 scope 的共享记忆
    MemoryReset,
    /// /workspace — 查看当前 session 工作区目录
    /// /workspace <path> — 设置当前 session 工作区目录
    Workspace(Option<String>),
    /// /approve <id> <allow-once|allow-always|deny> — 响应待处理审批
    Approve {
        approval_id: String,
        decision: String,
    },
    /// /team [status] — 查看当前 Team 任务状态（仅 Team mode 下有效）
    TeamStatus,
    /// /clear — 清除对话历史 + 团队工作区（tasks、events、milestones 等全部重置）
    Clear,
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
            "/backend" => {
                let name = arg.map(|s| s.trim()).filter(|s| !s.is_empty())?;
                Some(Self::SetBackend(name.to_string()))
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
                    Some(rest) if !rest.is_empty() => {
                        // /memory @agent or /memory agent — strip leading '@'
                        let agent = rest.strip_prefix('@').unwrap_or(rest).to_string();
                        Some(Self::Memory(Some(agent)))
                    }
                    _ => Some(Self::Memory(None)),
                }
            }
            "/forget" => {
                let keyword = arg.map(|s| s.trim()).filter(|s| !s.is_empty())?;
                Some(Self::Forget(keyword.to_string()))
            }
            "/workspace" => {
                let path = arg.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                Some(Self::Workspace(path))
            }
            "/team" => {
                let sub = arg.map(|s| s.trim()).unwrap_or("status");
                match sub {
                    "status" | "" => Some(Self::TeamStatus),
                    _ => None,
                }
            }
            "/clear" => Some(Self::Clear),
            "/approve" => {
                let arg = arg.map(str::trim).filter(|s| !s.is_empty())?;
                let mut parts = arg.split_whitespace();
                let approval_id = parts.next()?.trim();
                let decision = parts.next()?.trim();
                if approval_id.is_empty() || decision.is_empty() || parts.next().is_some() {
                    return None;
                }
                Some(Self::Approve {
                    approval_id: approval_id.to_string(),
                    decision: decision.to_string(),
                })
            }
            _ => None,
        }
    }

    /// 命令执行后返回给用户的确认文本
    pub fn confirmation_text(&self) -> String {
        match self {
            Self::SetBackend(name) => {
                format!("✅ Backend 已切换到 {name}\n下次消息将使用新 backend 处理")
            }
            Self::Reset => "✅ 对话历史已清除".to_string(),
            Self::Help => "可用命令：\n/backend <backend-id|agent-name> — 切换 backend\n/reset — 清除对话历史\n/clear — 清除对话历史 + 团队工作区（tasks、events 全部重置）\n/help — 显示帮助\n/remember <内容> — 写入记忆\n/memory — 查看共享记忆\n/memory @agent — 查看指定 agent 记忆\n/memory reset — 清空当前 scope 的共享记忆\n/forget <关键词> — 删除记忆条目\n/workspace — 查看当前 session 工作区目录\n/workspace /path — 设置 session 工作区目录\n/approve <id> <allow-once|allow-always|deny> — 响应待处理审批\n/team status — 查看 Team 任务状态（Team mode）".to_string(),
            Self::Remember(content) => format!("✅ 已记录：{content}"),
            // Unreachable in practice: registry's handle_slash returns early with real content.
            Self::Memory(_) => unreachable!(
                "Memory must be handled by handle_slash (returns early with real content), not confirmation_text"
            ),
            Self::Forget(keyword) => format!("✅ 已删除包含「{keyword}」的记忆条目"),
            Self::MemoryReset => unreachable!(
                "MemoryReset must be handled by handle_slash (two-step confirmation), not confirmation_text"
            ),
            Self::Workspace(_) => unreachable!(
                "Workspace must be handled by handle_slash (returns early with real content), not confirmation_text"
            ),
            Self::Approve { .. } => unreachable!(
                "Approve must be handled by handle_slash (returns early with real content), not confirmation_text"
            ),
            Self::TeamStatus => unreachable!(
                "TeamStatus must be handled by handle_slash (returns early with real content)"
            ),
            Self::Clear => unreachable!(
                "Clear must be handled by handle_slash (async team workspace reset)"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_backend_name() {
        let cmd = SlashCommand::parse("/backend claude-main").unwrap();
        assert!(matches!(cmd, SlashCommand::SetBackend(ref s) if s == "claude-main"));
    }

    #[test]
    fn test_parse_backend_agent_name() {
        let cmd = SlashCommand::parse("/backend reviewer").unwrap();
        assert!(matches!(cmd, SlashCommand::SetBackend(ref s) if s == "reviewer"));
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
    fn test_confirmation_text_backend() {
        let cmd = SlashCommand::SetBackend("claude-main".to_string());
        assert!(cmd.confirmation_text().contains("claude-main"));
    }

    #[test]
    fn test_parse_backend_whitespace_only() {
        assert!(SlashCommand::parse("/backend ").is_none());
        assert!(SlashCommand::parse("/backend   ").is_none());
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
    fn test_parse_memory_no_agent() {
        assert_eq!(
            SlashCommand::parse("/memory"),
            Some(SlashCommand::Memory(None))
        );
    }

    #[test]
    fn test_parse_memory_with_at_agent() {
        assert_eq!(
            SlashCommand::parse("/memory @reviewer"),
            Some(SlashCommand::Memory(Some("reviewer".to_string())))
        );
    }

    #[test]
    fn test_parse_memory_with_agent_no_at() {
        // Agent name without '@' prefix is also accepted
        assert_eq!(
            SlashCommand::parse("/memory reviewer"),
            Some(SlashCommand::Memory(Some("reviewer".to_string())))
        );
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

    #[test]
    fn test_parse_workspace_no_arg() {
        assert_eq!(
            SlashCommand::parse("/workspace"),
            Some(SlashCommand::Workspace(None))
        );
    }

    #[test]
    fn test_parse_workspace_with_path() {
        assert_eq!(
            SlashCommand::parse("/workspace /projects/my-app"),
            Some(SlashCommand::Workspace(Some(
                "/projects/my-app".to_string()
            )))
        );
    }

    #[test]
    fn test_parse_approve() {
        assert_eq!(
            SlashCommand::parse("/approve approval-1 allow-once"),
            Some(SlashCommand::Approve {
                approval_id: "approval-1".into(),
                decision: "allow-once".into(),
            })
        );
    }

    #[test]
    fn test_parse_team_status() {
        assert_eq!(
            SlashCommand::parse("/team status"),
            Some(SlashCommand::TeamStatus)
        );
    }

    #[test]
    fn test_parse_team_no_subcommand() {
        assert_eq!(SlashCommand::parse("/team"), Some(SlashCommand::TeamStatus));
    }
}
