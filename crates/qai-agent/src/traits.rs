use anyhow::Result;
use async_trait::async_trait;
use qai_protocol::AgentEvent;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Agent 在团队中的角色（决定 SystemPromptBuilder 的行为）
#[derive(Debug, Clone, Default, PartialEq)]
pub enum AgentRole {
    /// 独立模式：读 MEMORY.md；无 team 文件（默认）
    #[default]
    Solo,
    /// 主导模式：读 MEMORY.md；写 TEAM.md/TASKS.md；协调专才
    Lead,
    /// 专才模式：不读 MEMORY.md；读 TEAM.md/TASKS.md/CONTEXT.md；执行原子任务
    Specialist,
}

/// Agent 执行上下文
#[derive(Debug, Clone)]
pub struct AgentCtx {
    pub session_id: Uuid,
    pub user_text: String,
    pub history: Vec<HistoryMsg>,
    pub system_injection: String, // skills 注入文本
    /// Resolved workspace for this turn.
    pub workspace_dir: Option<PathBuf>,
    /// Agent 在团队中的角色（默认 Solo）
    pub agent_role: AgentRole,
    /// Team session 目录（含 TEAM.md / TASKS.md / CONTEXT.md），Team Mode 时有效
    pub team_dir: Option<PathBuf>,
    /// 注入 Layer 0 的任务提醒文本（Specialist / Lead 有任务时有效）
    pub task_reminder: Option<String>,
}

impl Default for AgentCtx {
    fn default() -> Self {
        Self {
            session_id: Uuid::nil(),
            user_text: String::new(),
            history: vec![],
            system_injection: String::new(),
            workspace_dir: None,
            agent_role: AgentRole::Solo,
            team_dir: None,
            task_reminder: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HistoryMsg {
    pub role: String,
    pub content: String,
}

/// 统一 AgentEngine trait（所有 engine 实现此 trait）
#[async_trait]
pub trait AgentEngine: Send + Sync {
    fn name(&self) -> &str;

    /// 执行一次 Agent 对话，通过 broadcast channel 流式发出事件
    async fn run(&self, ctx: AgentCtx, event_tx: broadcast::Sender<AgentEvent>) -> Result<String>; // 返回完整回复文本
}

pub type BoxEngine = Arc<dyn AgentEngine>;
