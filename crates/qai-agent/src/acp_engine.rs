//! AcpEngine: 启动任意 ACP server 二进制（quickai-rust-agent, quickai-claude-agent, codex-acp 等）
//! 通信: stdio ACP 协议（agent-client-protocol 0.9 crates.io）
//! !Send 隔离: 每次 run() 都在独立 std::thread + current_thread runtime + LocalSet 中执行

use crate::traits::{AgentCtx, AgentEngine};
use anyhow::Result;
use async_trait::async_trait;
use qai_protocol::AgentEvent;
use tokio::sync::broadcast;

/// ACP Engine 配置
#[derive(Debug, Clone)]
pub struct AcpEngineConfig {
    /// ACP server 命令（binary 名称或绝对路径）
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// If set, the subprocess will be spawned in this directory.
    pub workspace_dir: Option<std::path::PathBuf>,
}

impl AcpEngineConfig {
    pub fn quickai_rust_agent() -> Self {
        Self {
            command: "quickai-rust-agent".to_string(),
            args: vec![],
            env: vec![],
            workspace_dir: None,
        }
    }

    pub fn quickai_claude_agent() -> Self {
        Self {
            command: "quickai-claude-agent".to_string(),
            args: vec![],
            env: vec![],
            workspace_dir: None,
        }
    }

    pub fn codex_acp() -> Self {
        Self {
            command: "codex-acp".to_string(),
            args: vec![],
            env: vec![],
            workspace_dir: None,
        }
    }
}

pub struct AcpEngine {
    config: AcpEngineConfig,
}

impl AcpEngine {
    pub fn new(config: AcpEngineConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentEngine for AcpEngine {
    fn name(&self) -> &str {
        &self.config.command
    }

    async fn run(&self, ctx: AgentCtx, event_tx: broadcast::Sender<AgentEvent>) -> Result<String> {
        let mut config = self.config.clone();
        if ctx.workspace_dir.is_some() {
            config.workspace_dir = ctx.workspace_dir.clone();
        }
        let (result_tx, result_rx) = tokio::sync::oneshot::channel::<Result<String>>();

        // !Send 隔离：在独立 OS thread 中运行 ACP 客户端
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to build current_thread runtime");

            let local = tokio::task::LocalSet::new();
            let result = rt.block_on(
                local.run_until(async move { run_acp_session(config, ctx, event_tx).await }),
            );
            let _ = result_tx.send(result);
        });

        result_rx
            .await
            .map_err(|_| anyhow::anyhow!("ACP thread panicked"))?
    }
}

/// 在 LocalSet 中运行完整 ACP 客户端会话
async fn run_acp_session(
    config: AcpEngineConfig,
    ctx: AgentCtx,
    event_tx: broadcast::Sender<AgentEvent>,
) -> Result<String> {
    use acp::Agent as _;
    use agent_client_protocol as acp;
    use tokio::process::Command;
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args)
        .envs(config.env.iter().cloned())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());
    if let Some(ref ws) = config.workspace_dir {
        if ws.exists() {
            cmd.current_dir(ws);
        } else {
            tracing::warn!(path = %ws.display(), "Workspace directory does not exist; running in default directory");
        }
    }
    let mut child = cmd.spawn()?;

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let outgoing = stdin.compat_write();
    let incoming = stdout.compat();

    let session_id = ctx.session_id;
    let accumulated = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    let accumulated_clone = accumulated.clone();
    let event_tx_clone = event_tx.clone();

    struct EventClient {
        session_id: uuid::Uuid,
        accumulated: std::rc::Rc<std::cell::RefCell<String>>,
        event_tx: broadcast::Sender<AgentEvent>,
    }

    #[async_trait::async_trait(?Send)]
    impl acp::Client for EventClient {
        async fn request_permission(
            &self,
            args: acp::RequestPermissionRequest,
        ) -> acp::Result<acp::RequestPermissionResponse> {
            // 自动选择第一个 Allow 选项（Tool Policy 过滤在 Phase 2 实现）
            let outcome = args
                .options
                .iter()
                .find(|o| {
                    matches!(
                        o.kind,
                        acp::PermissionOptionKind::AllowOnce
                            | acp::PermissionOptionKind::AllowAlways
                    )
                })
                .map(|o| {
                    acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                        o.option_id.clone(),
                    ))
                })
                .unwrap_or(acp::RequestPermissionOutcome::Cancelled);
            Ok(acp::RequestPermissionResponse::new(outcome))
        }

        async fn session_notification(
            &self,
            notification: acp::SessionNotification,
        ) -> acp::Result<()> {
            if let acp::SessionUpdate::AgentMessageChunk(chunk) = notification.update {
                if let acp::ContentBlock::Text(t) = chunk.content {
                    self.accumulated.borrow_mut().push_str(&t.text);
                    let _ = self.event_tx.send(AgentEvent::TextDelta {
                        session_id: self.session_id,
                        delta: t.text,
                    });
                }
            }
            Ok(())
        }
    }

    let client = EventClient {
        session_id,
        accumulated: accumulated_clone,
        event_tx: event_tx_clone,
    };

    let (conn, handle_io) = acp::ClientSideConnection::new(client, outgoing, incoming, |fut| {
        tokio::task::spawn_local(fut);
    });
    tokio::task::spawn_local(handle_io);

    // ACP 握手
    let init_resp = conn.initialize(
        acp::InitializeRequest::new(acp::ProtocolVersion::V1).client_info(
            acp::Implementation::new("quickai-gateway", env!("CARGO_PKG_VERSION")),
        ),
    )
    .await
    .map_err(|e| anyhow::anyhow!("ACP initialize failed: {e:?}"))?;

    // Whitelist known quickai agents that support SSE MCP but don't yet declare it in
    // initialize(). This is a temporary workaround until Fix-A/B (explicit capability
    // declaration in the agent binaries) is landed.
    let agent_name = init_resp
        .agent_info
        .as_ref()
        .map(|i| i.name.as_str())
        .unwrap_or("");
    let supports_sse_mcp = init_resp.agent_capabilities.mcp_capabilities.sse
        || matches!(
            agent_name,
            "quickai-rust-agent" | "quickai-claude-agent"
        );

    let session_root = config
        .workspace_dir
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let mcp_servers = build_mcp_servers(supports_sse_mcp, ctx.mcp_server_url.as_deref());
    if !mcp_servers.is_empty() {
        tracing::debug!(url = %ctx.mcp_server_url.as_deref().unwrap_or(""), "Injecting team-tools MCP server into ACP session");
    }

    let sess = conn
        .new_session(acp::NewSessionRequest::new(session_root).mcp_servers(mcp_servers))
        .await
        .map_err(|e| anyhow::anyhow!("ACP new_session failed: {e:?}"))?;

    let _ = event_tx.send(AgentEvent::Thinking { session_id });

    let prompt_text = build_prompt_text(&ctx);
    // Send prompt, but always send TurnComplete regardless of success/failure
    // so WebSocket clients don't hang waiting for the event.
    let prompt_result = conn
        .prompt(acp::PromptRequest::new(
            sess.session_id,
            vec![acp::ContentBlock::Text(acp::TextContent::new(prompt_text))],
        ))
        .await
        .map_err(|e| anyhow::anyhow!("ACP prompt failed: {e:?}"));

    let full_text = accumulated.borrow().clone();

    let _ = event_tx.send(AgentEvent::TurnComplete {
        session_id,
        full_text: full_text.clone(),
        sender: None, // engine doesn't know about roster; registry fills this in
    });

    child.kill().await.ok();
    prompt_result.map(|_| full_text)
}

fn build_prompt_text(ctx: &AgentCtx) -> String {
    let mut parts = Vec::new();
    if !ctx.system_injection.is_empty() {
        parts.push(format!(
            "<system_context>\n{}\n</system_context>",
            ctx.system_injection
        ));
    }
    for msg in &ctx.history {
        parts.push(format!("[{}]: {}", msg.role, msg.content));
    }
    parts.push(ctx.user_text.clone());
    parts.join("\n\n")
}

fn build_mcp_servers(
    supports_sse: bool,
    url: Option<&str>,
) -> Vec<agent_client_protocol::McpServer> {
    use agent_client_protocol as acp;
    if supports_sse {
        if let Some(u) = url {
            if !u.is_empty() {
                return vec![acp::McpServer::Sse(acp::McpServerSse::new("team-tools", u))];
            }
        }
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol as acp;

    #[test]
    fn test_acp_engine_config_carries_workspace() {
        let cfg = AcpEngineConfig {
            command: "echo".to_string(),
            args: vec![],
            env: vec![],
            workspace_dir: Some(std::path::PathBuf::from("/tmp")),
        };
        assert_eq!(cfg.workspace_dir, Some(std::path::PathBuf::from("/tmp")));
    }

    #[test]
    fn test_mcp_servers_empty_when_no_url() {
        let servers = build_mcp_servers(true, None);
        assert!(servers.is_empty());
    }

    #[test]
    fn test_mcp_servers_empty_when_no_sse_capability() {
        let servers = build_mcp_servers(false, Some("http://127.0.0.1:9999"));
        assert!(servers.is_empty());
    }

    #[test]
    fn test_mcp_servers_empty_when_empty_url() {
        let servers = build_mcp_servers(true, Some(""));
        assert!(servers.is_empty());
    }

    #[test]
    fn test_mcp_servers_populated_when_url_and_capability() {
        let servers = build_mcp_servers(true, Some("http://127.0.0.1:9999"));
        assert_eq!(servers.len(), 1);
        if let acp::McpServer::Sse(ref h) = servers[0] {
            assert_eq!(h.name, "team-tools");
            assert_eq!(h.url, "http://127.0.0.1:9999");
        } else {
            panic!("expected Sse variant");
        }
    }
}
