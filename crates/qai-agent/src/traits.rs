use std::path::PathBuf;
use uuid::Uuid;

/// Agent 在团队中的角色（决定 SystemPromptBuilder 的行为）
#[derive(Debug, Clone, Copy, Default, PartialEq)]
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
    pub session_key: qai_protocol::SessionKey,
    pub participant_name: Option<String>,
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
    /// URL of the running TeamMcpServer (e.g. "http://127.0.0.1:54321/sse").
    /// Set only for Specialist turns when TeamOrchestrator is wired and running.
    pub mcp_server_url: Option<String>,
    /// URL of the family-agnostic Team Tool RPC endpoint.
    pub team_tool_url: Option<String>,
    /// Canonical shared memory / contextual summary for this turn.
    pub shared_memory: Option<String>,
    /// Canonical long-term memory for solo/lead turns.
    pub agent_memory: Option<String>,
    /// Canonical team manifest for lead/specialist turns.
    pub team_manifest: Option<String>,
}

impl Default for AgentCtx {
    fn default() -> Self {
        Self {
            session_id: Uuid::nil(),
            session_key: qai_protocol::SessionKey::new("unknown", "unknown"),
            participant_name: None,
            user_text: String::new(),
            history: vec![],
            system_injection: String::new(),
            workspace_dir: None,
            agent_role: AgentRole::Solo,
            team_dir: None,
            task_reminder: None,
            mcp_server_url: None,
            team_tool_url: None,
            shared_memory: None,
            agent_memory: None,
            team_manifest: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HistoryMsg {
    pub role: String,
    pub content: String,
}
