use anyhow::Result;
use async_trait::async_trait;
use qai_protocol::AgentEvent;
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Agent 执行上下文
#[derive(Debug, Clone)]
pub struct AgentCtx {
    pub session_id: Uuid,
    pub user_text: String,
    pub history: Vec<HistoryMsg>,
    pub system_injection: String, // skills 注入文本
    /// Resolved workspace for this turn.
    pub workspace_dir: Option<std::path::PathBuf>,
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
    async fn run(
        &self,
        ctx: AgentCtx,
        event_tx: broadcast::Sender<AgentEvent>,
    ) -> Result<String>; // 返回完整回复文本
}

pub type BoxEngine = Arc<dyn AgentEngine>;
