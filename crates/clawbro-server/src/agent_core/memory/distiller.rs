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
        // Use char-safe truncation to avoid panicking on multi-byte UTF-8 (e.g. Chinese text)
        let mem_preview: String = current_memory.chars().take(200).collect();
        let logs_preview: String = logs.chars().take(200).collect();
        Ok(format!(
            "[distilled]\n{}\n---\n{}",
            mem_preview, logs_preview
        ))
    }
}

/// AcpDistiller: 通过 clawbro-rust-agent ACP 进行蒸馏
pub struct AcpDistiller {
    pub binary: String,
}

impl AcpDistiller {
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
        }
    }
}

#[async_trait]
impl MemoryDistiller for AcpDistiller {
    async fn distill(&self, logs: &str, current_memory: &str) -> Result<String> {
        use crate::runtime::acp::session_driver::{run_command_turn, AcpCommandConfig};
        use crate::runtime::{
            ApprovalBroker, RuntimeContext, RuntimeRole, RuntimeSessionSpec, ToolSurfaceSpec,
        };
        let user_text =
            format!("## Conversation Logs\n\n{logs}\n\n## Current Memory\n\n{current_memory}");
        let binary = self.binary.clone();
        let args = if is_clawbro_binary(&binary) {
            vec!["acp-agent".to_string()]
        } else {
            vec![]
        };
        let turn = tokio::task::spawn_blocking(move || -> Result<_> {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let local = tokio::task::LocalSet::new();
            rt.block_on(local.run_until(async move {
                let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
                run_command_turn(
                    &AcpCommandConfig {
                        command: binary,
                        args,
                        env: vec![],
                    },
                    None, // memory distiller uses generic ACP path
                    None, // no ACP auth-method negotiation for the generic distiller path
                    None, // no Codex projection for the generic distiller path
                    RuntimeSessionSpec {
                        backend_id: "memory-distiller".into(),
                        participant_name: None,
                        session_key: crate::protocol::SessionKey::new("ws", "distiller"),
                        role: RuntimeRole::Solo,
                        workspace_dir: None,
                        prompt_text: format!(
                            "<system_context>\n{}\n</system_context>\n\n{}",
                            DISTILL_PROMPT, user_text
                        ),
                        tool_surface: ToolSurfaceSpec {
                            team_tools: false,
                            allowed_team_tools: vec![],
                            schedule_tools: false,
                            allowed_schedule_tools: vec![],
                            external_mcp: false,
                            backend_native_tools: false,
                        },
                        approval_mode: Default::default(),
                        external_mcp_servers: vec![],
                        team_tool_url: None,
                        provider_profile: None,
                        backend_session_id: None,
                        context: RuntimeContext {
                            system_prompt: Some(DISTILL_PROMPT.to_string()),
                            workspace_native_files: Vec::new(),
                            memory_summary: None,
                            agent_memory: None,
                            team_manifest: None,
                            task_reminder: None,
                            history_messages: Vec::new(),
                            history_lines: Vec::new(),
                            user_input: Some(user_text.clone()),
                            ..RuntimeContext::default()
                        },
                    },
                    crate::runtime::RuntimeEventSink::new(tx),
                    ApprovalBroker::default(),
                )
                .await
            }))
        })
        .await
        .map_err(|e| anyhow::anyhow!("memory distiller thread join failed: {e}"))??;
        Ok(turn.full_text)
    }
}

fn is_clawbro_binary(binary: &str) -> bool {
    std::path::Path::new(binary)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == "clawbro" || name == "clawbro.exe")
        .unwrap_or(false)
}
