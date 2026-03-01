use anyhow::Result;
use async_trait::async_trait;

pub const DISTILL_PROMPT: &str = "\
You are a memory distillation system. Based on the conversation logs and current memory, \
write an updated memory document that captures the most important long-term information.\n\
Focus on: user preferences, key facts, recurring topics, important decisions.\n\
Be concise (under 400 words). Use Markdown format with ## sections.\n\
Output ONLY the memory content — no preamble, no explanation.";

#[async_trait]
pub trait MemoryDistiller: Send + Sync {
    async fn distill(&self, logs: &str, current_memory: &str) -> Result<String>;
}

/// NoopDistiller: 用于测试，直接返回 logs + current_memory 的前 400 词
pub struct NoopDistiller;

#[async_trait]
impl MemoryDistiller for NoopDistiller {
    async fn distill(&self, logs: &str, current_memory: &str) -> Result<String> {
        let mem_preview = &current_memory[..current_memory.len().min(200)];
        let logs_preview = &logs[..logs.len().min(200)];
        Ok(format!("[distilled]\n{}\n---\n{}", mem_preview, logs_preview))
    }
}

/// AcpDistiller: 通过 quickai-rust-agent ACP 进行蒸馏
pub struct AcpDistiller {
    pub binary: String,
}

impl AcpDistiller {
    pub fn new(binary: impl Into<String>) -> Self {
        Self { binary: binary.into() }
    }
}

#[async_trait]
impl MemoryDistiller for AcpDistiller {
    async fn distill(&self, logs: &str, current_memory: &str) -> Result<String> {
        use crate::acp_engine::{AcpEngine, AcpEngineConfig};
        use crate::traits::{AgentCtx, AgentEngine};
        use tokio::sync::broadcast;
        use uuid::Uuid;

        let engine = AcpEngine::new(AcpEngineConfig {
            command: self.binary.clone(),
            args: vec![],
            env: vec![],
        });

        let user_text = format!(
            "## Conversation Logs\n\n{logs}\n\n## Current Memory\n\n{current_memory}"
        );

        let ctx = AgentCtx {
            session_id: Uuid::new_v4(),
            user_text,
            history: vec![],
            system_injection: DISTILL_PROMPT.to_string(),
        };

        let (tx, _) = broadcast::channel(16);
        engine.run(ctx, tx).await
    }
}
