use clawbro_protocol::SessionKey;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum MemoryTarget {
    Shared,
    Agent { persona_dir: PathBuf },
}

#[derive(Debug, Clone)]
pub enum MemoryEvent {
    /// 用户主动 /remember
    UserRemember {
        scope: SessionKey,
        target: MemoryTarget,
        content: String,
    },
    /// 每轮 agent 对话完成
    TurnCompleted {
        scope: SessionKey,
        agent: String,
        persona_dir: PathBuf,
        turn_count: u64,
    },
    /// 会话空闲超时
    SessionIdle {
        scope: SessionKey,
        agent: String,
        persona_dir: PathBuf,
    },
    /// Cron 任务完成，结果写入共享记忆
    CronJobCompleted {
        scope: SessionKey,
        agent: String,
        persona_dir: PathBuf,
        result_summary: String,
    },
    /// 夜间聚合：把所有 agent 私有记忆合并到共享记忆
    NightlyConsolidation {
        scope: SessionKey,
        /// 参与聚合的所有 agent persona_dir 列表
        agent_dirs: Vec<(String, PathBuf)>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawbro_protocol::SessionKey;
    use std::path::PathBuf;

    #[test]
    fn test_memory_event_clone() {
        let e = MemoryEvent::UserRemember {
            scope: SessionKey::new("dingtalk", "group_1"),
            target: MemoryTarget::Shared,
            content: "hello".to_string(),
        };
        let _ = e.clone(); // must be Clone
    }

    #[test]
    fn test_memory_target_agent() {
        let t = MemoryTarget::Agent {
            persona_dir: PathBuf::from("/tmp/agent"),
        };
        matches!(t, MemoryTarget::Agent { .. });
    }
}
