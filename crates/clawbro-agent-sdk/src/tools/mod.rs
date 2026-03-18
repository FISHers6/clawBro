//! Built-in tools for the ClawBro Agent.
//! All tools implement rig-core's `Tool` trait directly (no Tauri dependencies).

use crate::bridge::{
    AgentEvent, AgentTurnRequest, ApprovalMode, ExternalMcpServerSpec, ExternalMcpTransport,
};
use anyhow::Result;
use rig::{
    agent::AgentBuilder,
    completion::ToolDefinition,
    tool::{Tool, ToolError},
};
use rmcp::{
    model::{ClientCapabilities, ClientInfo, Implementation},
    service::{RoleClient, RunningService},
    transport::SseClientTransport,
    ServiceExt,
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
pub mod bash;
pub mod fileio;
pub mod search;

pub use bash::BashTool;
pub use fileio::{EditFileTool, ViewFileTool, WriteFileTool};
pub use search::{GlobTool, GrepTool, LsTool};

#[derive(Clone)]
pub struct ToolProgressTracker {
    emit: Arc<dyn Fn(AgentEvent) + Send + Sync>,
}

impl ToolProgressTracker {
    pub fn new(emit: Arc<dyn Fn(AgentEvent) + Send + Sync>) -> Self {
        Self { emit }
    }

    pub fn tool_started(&self, tool_name: &str, call_id: &str, input_summary: Option<String>) {
        (self.emit)(AgentEvent::ToolCallStarted {
            tool_name: tool_name.to_string(),
            call_id: call_id.to_string(),
            input_summary,
        });
    }

    pub fn tool_completed<T: serde::Serialize>(&self, tool_name: &str, call_id: &str, output: &T) {
        let result = serde_json::to_string(output)
            .unwrap_or_else(|_| "\"<unserializable tool result>\"".to_string());
        (self.emit)(AgentEvent::ToolCallCompleted {
            tool_name: tool_name.to_string(),
            call_id: call_id.to_string(),
            result,
        });
    }

    pub fn tool_failed(&self, tool_name: &str, call_id: &str, error: &dyn std::error::Error) {
        (self.emit)(AgentEvent::ToolCallFailed {
            tool_name: tool_name.to_string(),
            call_id: call_id.to_string(),
            error: error.to_string(),
        });
    }
}

static TOOL_CALL_SEQ: AtomicU64 = AtomicU64::new(1);

fn next_tool_call_id(tool_name: &str) -> String {
    let seq = TOOL_CALL_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("clawbro-tool:{tool_name}:{seq}")
}

pub struct EventedTool<T> {
    inner: T,
    tracker: Option<ToolProgressTracker>,
    approval_mode: ApprovalMode,
}

type ExternalMcpClient = RunningService<RoleClient, ClientInfo>;

pub struct RuntimeToolRegistration<M>
where
    M: rig::completion::CompletionModel,
{
    pub builder: AgentBuilder<M>,
    pub external_mcp_clients: Vec<ExternalMcpClient>,
}

impl<T> EventedTool<T> {
    pub fn new(
        inner: T,
        tracker: Option<ToolProgressTracker>,
        approval_mode: ApprovalMode,
    ) -> Self {
        Self {
            inner,
            tracker,
            approval_mode,
        }
    }
}

pub trait RuntimeToolAugmentor {
    fn augment<M: rig::completion::CompletionModel>(
        &self,
        builder: AgentBuilder<M>,
        session: &AgentTurnRequest,
        tracker: Option<ToolProgressTracker>,
        approval_mode: ApprovalMode,
    ) -> AgentBuilder<M>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoopToolAugmentor;

impl RuntimeToolAugmentor for NoopToolAugmentor {
    fn augment<M: rig::completion::CompletionModel>(
        &self,
        builder: AgentBuilder<M>,
        _session: &AgentTurnRequest,
        _tracker: Option<ToolProgressTracker>,
        _approval_mode: ApprovalMode,
    ) -> AgentBuilder<M> {
        builder
    }
}

impl<T> Tool for EventedTool<T>
where
    T: Tool<Error = ToolError>,
{
    const NAME: &'static str = T::NAME;
    type Error = ToolError;
    type Args = T::Args;
    type Output = T::Output;

    async fn definition(&self, prompt: String) -> ToolDefinition {
        <T as Tool>::definition(&self.inner, prompt).await
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_call_id(T::NAME);
        if let Some(tracker) = &self.tracker {
            tracker.tool_started(T::NAME, &call_id, None);
        }
        let denial_message = match self.approval_mode {
            ApprovalMode::Manual => Some(
                "Tool execution requires manual approval, but quick_ai_native does not support host-mediated manual approvals over the current runtime bridge. Use backend approval mode auto_allow or auto_deny.".to_string(),
            ),
            ApprovalMode::AutoDeny => Some(
                "Tool execution denied by backend approval policy (auto_deny).".to_string(),
            ),
            ApprovalMode::AutoAllow => None,
        };
        if let Some(message) = denial_message {
            let err = ToolError::ToolCallError(message.clone().into());
            if let Some(tracker) = &self.tracker {
                tracker.tool_failed(T::NAME, &call_id, &err);
            }
            return Err(err);
        }
        match <T as Tool>::call(&self.inner, args).await {
            Ok(output) => {
                if let Some(tracker) = &self.tracker {
                    tracker.tool_completed(T::NAME, &call_id, &output);
                }
                Ok(output)
            }
            Err(err) => {
                if let Some(tracker) = &self.tracker {
                    tracker.tool_failed(T::NAME, &call_id, &err);
                }
                Err(err)
            }
        }
    }
}

/// Register all built-in tools onto an AgentBuilder.
/// Accepts any builder whose model implements `CompletionModel`.
/// Usage:
///   ```ignore
///   let builder = register_tools(client.agent(model).preamble(system_prompt));
///   let agent = builder.build();
///   ```
pub fn register_tools<M: rig::completion::CompletionModel>(
    builder: rig::agent::AgentBuilder<M>,
) -> rig::agent::AgentBuilder<M> {
    builder
        .tool(BashTool)
        .tool(ViewFileTool)
        .tool(WriteFileTool)
        .tool(EditFileTool)
        .tool(GlobTool)
        .tool(GrepTool)
        .tool(LsTool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{
        AgentTurnRequest, ExecutionRole, ExternalMcpServerSpec, ExternalMcpTransport,
        RuntimeContext, ToolSurfaceSpec,
    };
    use rig::{
        agent::AgentBuilder,
        completion::{
            CompletionError, CompletionModel, CompletionRequest, CompletionResponse, Prompt, Usage,
        },
        message::{AssistantContent, Text, ToolCall, ToolFunction},
        streaming::{StreamingCompletionResponse, StreamingResult},
        OneOrMany,
    };
    use rmcp::{
        handler::server::{router::tool::ToolRouter, wrapper::Parameters},
        model::{
            CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities,
            ServerInfo,
        },
        tool, tool_handler, tool_router,
        transport::{sse_server::SseServerConfig, SseServer},
        ErrorData as McpError, ServerHandler,
    };
    use schemars::JsonSchema;
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use tokio::net::TcpListener;
    use tokio_util::sync::CancellationToken;

    #[derive(Clone)]
    struct OkTool;

    impl Tool for OkTool {
        const NAME: &'static str = "ok_tool";
        type Error = ToolError;
        type Args = ();
        type Output = String;

        async fn definition(&self, _prompt: String) -> ToolDefinition {
            ToolDefinition {
                name: Self::NAME.to_string(),
                description: "test tool".to_string(),
                parameters: serde_json::json!({"type":"object"}),
            }
        }

        async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
            Ok("ok".to_string())
        }
    }

    #[derive(Clone)]
    struct FailingTool;

    impl Tool for FailingTool {
        const NAME: &'static str = "failing_tool";
        type Error = ToolError;
        type Args = ();
        type Output = String;

        async fn definition(&self, _prompt: String) -> ToolDefinition {
            ToolDefinition {
                name: Self::NAME.to_string(),
                description: "failing tool".to_string(),
                parameters: serde_json::json!({"type":"object"}),
            }
        }

        async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
            Err(ToolError::ToolCallError("boom".into()))
        }
    }

    #[tokio::test]
    async fn evented_tool_emits_matching_start_and_result_call_ids() {
        let events = Arc::new(Mutex::new(Vec::<AgentEvent>::new()));
        let sink = events.clone();
        let tracker = ToolProgressTracker::new(Arc::new(move |event| {
            sink.lock().unwrap().push(event);
        }));
        let tool = EventedTool::new(OkTool, Some(tracker), ApprovalMode::AutoAllow);

        let output = tool.call(()).await.unwrap();
        assert_eq!(output, "ok");

        let events = events.lock().unwrap().clone();
        assert_eq!(events.len(), 2);
        let start_id = match &events[0] {
            AgentEvent::ToolCallStarted { call_id, .. } => call_id.clone(),
            other => panic!("expected ToolCallStarted, got {other:?}"),
        };
        let result_id = match &events[1] {
            AgentEvent::ToolCallCompleted { call_id, .. } => call_id.clone(),
            other => panic!("expected ToolCallCompleted, got {other:?}"),
        };
        assert_eq!(start_id, result_id);
    }

    #[tokio::test]
    async fn evented_tool_emits_matching_start_and_failed_call_ids() {
        let events = Arc::new(Mutex::new(Vec::<AgentEvent>::new()));
        let sink = events.clone();
        let tracker = ToolProgressTracker::new(Arc::new(move |event| {
            sink.lock().unwrap().push(event);
        }));
        let tool = EventedTool::new(FailingTool, Some(tracker), ApprovalMode::AutoAllow);

        let err = tool.call(()).await.unwrap_err();
        assert_eq!(err.to_string(), "ToolCallError: boom");

        let events = events.lock().unwrap().clone();
        assert_eq!(events.len(), 2);
        let start_id = match &events[0] {
            AgentEvent::ToolCallStarted { call_id, .. } => call_id.clone(),
            other => panic!("expected ToolCallStarted, got {other:?}"),
        };
        let failed_id = match &events[1] {
            AgentEvent::ToolCallFailed { call_id, .. } => call_id.clone(),
            other => panic!("expected ToolCallFailed, got {other:?}"),
        };
        assert_eq!(start_id, failed_id);
    }

    #[derive(Clone)]
    struct ToolThenTextModel {
        turn: Arc<AtomicUsize>,
    }

    impl ToolThenTextModel {
        fn new() -> Self {
            Self {
                turn: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[allow(refining_impl_trait)]
    impl CompletionModel for ToolThenTextModel {
        type Response = ();
        type StreamingResponse = ();

        async fn completion(
            &self,
            _request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let turn = self.turn.fetch_add(1, Ordering::SeqCst);
            if turn == 0 {
                Ok(CompletionResponse {
                    choice: OneOrMany::one(AssistantContent::ToolCall(ToolCall {
                        id: "tc_external_1".to_string(),
                        call_id: Some("tc_external_1".to_string()),
                        function: ToolFunction {
                            name: "EchoExternal".to_string(),
                            arguments: serde_json::json!({"text": "hello from external mcp"}),
                        },
                    })),
                    usage: Usage::new(),
                    raw_response: (),
                })
            } else {
                Ok(CompletionResponse {
                    choice: OneOrMany::one(AssistantContent::Text(Text {
                        text: "external mcp tool completed".to_string(),
                    })),
                    usage: Usage::new(),
                    raw_response: (),
                })
            }
        }

        async fn stream(
            &self,
            _request: CompletionRequest,
        ) -> Result<StreamingCompletionResponse<Self::StreamingResponse>, CompletionError> {
            let stream: StreamingResult<()> = Box::pin(futures::stream::empty());
            Ok(StreamingCompletionResponse::stream(stream))
        }
    }

    #[derive(Clone)]
    struct EchoExternalServer {
        tool_router: ToolRouter<Self>,
        calls: Arc<AtomicUsize>,
    }

    #[derive(Debug, serde::Deserialize, JsonSchema)]
    struct EchoArgs {
        text: String,
    }

    #[tool_router]
    impl EchoExternalServer {
        fn new(calls: Arc<AtomicUsize>) -> Self {
            Self {
                tool_router: Self::tool_router(),
                calls,
            }
        }

        #[tool(name = "EchoExternal", description = "Echo back the provided text")]
        async fn echo_external(
            &self,
            Parameters(args): Parameters<EchoArgs>,
        ) -> Result<CallToolResult, McpError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(CallToolResult::success(vec![Content::text(args.text)]))
        }
    }

    #[tool_handler]
    impl ServerHandler for EchoExternalServer {
        fn get_info(&self) -> ServerInfo {
            let mut implementation = Implementation::from_build_env();
            implementation.name = "test-external-mcp".into();
            implementation.title = Some("Test External MCP".into());

            ServerInfo {
                protocol_version: ProtocolVersion::V_2024_11_05,
                server_info: implementation,
                capabilities: ServerCapabilities::builder().enable_tools().build(),
                instructions: None,
            }
        }
    }

    async fn spawn_echo_external_server(
        calls: Arc<AtomicUsize>,
    ) -> anyhow::Result<(String, CancellationToken)> {
        let listener =
            TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))).await?;
        let addr = listener.local_addr()?;
        let ct = CancellationToken::new();
        let (sse_server, sse_router) = SseServer::new(SseServerConfig {
            bind: addr,
            sse_path: "/sse".to_string(),
            post_path: "/message".to_string(),
            ct: ct.clone(),
            sse_keep_alive: None,
        });
        let server_ct = sse_server.with_service(move || EchoExternalServer::new(calls.clone()));
        let shutdown_ct = server_ct.clone();
        tokio::spawn(async move {
            let server = axum::serve(listener, sse_router).with_graceful_shutdown(async move {
                shutdown_ct.cancelled().await;
            });
            let _ = server.await;
        });
        Ok((format!("http://{addr}/sse"), server_ct))
    }

    fn runtime_session_with_external_mcp(url: String) -> AgentTurnRequest {
        AgentTurnRequest {
            participant_name: None,
            workspace_dir: None,
            session_ref: "test:external-mcp".to_string(),
            role: ExecutionRole::Solo,
            prompt_text: "use external mcp".to_string(),
            tool_surface: ToolSurfaceSpec {
                team_tools: false,
                local_skills: false,
                external_mcp: true,
                backend_native_tools: true,
            },
            approval_mode: ApprovalMode::AutoAllow,
            tool_bridge_url: None,
            context: RuntimeContext::default(),
            external_mcp_servers: vec![ExternalMcpServerSpec {
                name: "test-external".to_string(),
                transport: ExternalMcpTransport::Sse { url },
            }],
            provider_profile: None,
        }
    }

    #[tokio::test]
    async fn register_runtime_tools_connects_and_uses_external_sse_mcp_server() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (url, server_ct) = spawn_echo_external_server(calls.clone()).await.unwrap();

        let session = runtime_session_with_external_mcp(url);
        let registration = register_runtime_tools(
            AgentBuilder::new(ToolThenTextModel::new()),
            &session,
            None,
            &NoopToolAugmentor,
        )
        .await
        .unwrap();

        assert_eq!(registration.external_mcp_clients.len(), 1);

        let agent = registration.builder.build();
        let result: String = agent
            .prompt("please use the external tool")
            .multi_turn(2)
            .await
            .unwrap();

        assert_eq!(result, "external mcp tool completed");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        server_ct.cancel();
    }

    #[tokio::test]
    async fn register_runtime_tools_skips_broken_external_mcp_server() {
        let session = runtime_session_with_external_mcp("http://127.0.0.1:9/sse".to_string());
        let registration = register_runtime_tools(
            AgentBuilder::new(ToolThenTextModel::new()),
            &session,
            None,
            &NoopToolAugmentor,
        )
        .await
        .unwrap();

        assert!(registration.external_mcp_clients.is_empty());
    }
}

pub async fn register_runtime_tools<M: rig::completion::CompletionModel>(
    builder: AgentBuilder<M>,
    session: &AgentTurnRequest,
    tracker: Option<ToolProgressTracker>,
    augmentor: &impl RuntimeToolAugmentor,
) -> Result<RuntimeToolRegistration<M>> {
    let approval_mode = session.approval_mode;
    let builder = builder
        .tool(EventedTool::new(BashTool, tracker.clone(), approval_mode))
        .tool(EventedTool::new(
            ViewFileTool,
            tracker.clone(),
            approval_mode,
        ))
        .tool(EventedTool::new(
            WriteFileTool,
            tracker.clone(),
            approval_mode,
        ))
        .tool(EventedTool::new(
            EditFileTool,
            tracker.clone(),
            approval_mode,
        ))
        .tool(EventedTool::new(GlobTool, tracker.clone(), approval_mode))
        .tool(EventedTool::new(GrepTool, tracker.clone(), approval_mode))
        .tool(EventedTool::new(LsTool, tracker.clone(), approval_mode));
    let builder = augmentor.augment(builder, session, tracker.clone(), approval_mode);

    let (builder, external_mcp_clients) =
        register_external_mcp_tools(builder, &session.external_mcp_servers, approval_mode).await?;
    Ok(RuntimeToolRegistration {
        builder,
        external_mcp_clients,
    })
}

async fn register_external_mcp_tools<M: rig::completion::CompletionModel>(
    builder: AgentBuilder<M>,
    servers: &[ExternalMcpServerSpec],
    approval_mode: ApprovalMode,
) -> Result<(AgentBuilder<M>, Vec<ExternalMcpClient>)> {
    if !matches!(approval_mode, ApprovalMode::AutoAllow) {
        tracing::warn!(
            ?approval_mode,
            "skipping external MCP registration for quick_ai_native because backend approval mode does not permit autonomous tool execution"
        );
        return Ok((builder, Vec::new()));
    }
    let mut builder = builder;
    let mut clients = Vec::new();

    for server in servers {
        let client = match connect_external_mcp_server(server).await {
            Ok(client) => client,
            Err(err) => {
                tracing::warn!(
                    server = %server.name,
                    error = %err,
                    "failed to connect external MCP server; skipping"
                );
                continue;
            }
        };
        let tools = match client.list_tools(Default::default()).await {
            Ok(result) => result.tools,
            Err(err) => {
                tracing::warn!(
                    server = %server.name,
                    error = %err,
                    "failed to list tools from external MCP server; skipping"
                );
                continue;
            }
        };
        let peer = client.peer().clone();
        builder = tools.into_iter().fold(builder, |builder, tool| {
            builder.rmcp_tool(tool, peer.clone())
        });
        clients.push(client);
    }

    Ok((builder, clients))
}

async fn connect_external_mcp_server(server: &ExternalMcpServerSpec) -> Result<ExternalMcpClient> {
    match &server.transport {
        ExternalMcpTransport::Sse { url } => {
            let transport = SseClientTransport::start(url.clone()).await?;
            let client_info = ClientInfo {
                protocol_version: Default::default(),
                capabilities: ClientCapabilities::default(),
                client_info: Implementation::from_build_env(),
            };
            let client = client_info.serve(transport).await?;
            tracing::info!(server = %server.name, url = %url, "connected external MCP server");
            Ok(client)
        }
    }
}
