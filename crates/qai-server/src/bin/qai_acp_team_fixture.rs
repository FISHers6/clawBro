use agent_client_protocol::{self as acp, Client as _};
use anyhow::{anyhow, Context, Result};
use rmcp::{model::CallToolRequestParam, service::ServiceExt, transport::SseClientTransport};
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

enum AgentAction {
    Prompt {
        session_id: acp::SessionId,
        prompt_text: String,
        reply: oneshot::Sender<acp::PromptResponse>,
    },
}

struct TeamFixtureAgent {
    action_tx: mpsc::UnboundedSender<AgentAction>,
    next_session_id: Cell<u64>,
    session_mcp_urls: Rc<RefCell<HashMap<String, String>>>,
}

impl TeamFixtureAgent {
    fn new(
        action_tx: mpsc::UnboundedSender<AgentAction>,
        session_mcp_urls: Rc<RefCell<HashMap<String, String>>>,
    ) -> Self {
        Self {
            action_tx,
            next_session_id: Cell::new(0),
            session_mcp_urls,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for TeamFixtureAgent {
    async fn initialize(
        &self,
        arguments: acp::InitializeRequest,
    ) -> Result<acp::InitializeResponse, acp::Error> {
        Ok(acp::InitializeResponse::new(arguments.protocol_version)
            .agent_info(
                acp::Implementation::new("qai-acp-team-fixture", "0.1.0")
                    .title("QAI ACP Team Fixture"),
            )
            .agent_capabilities(
                acp::AgentCapabilities::default()
                    .mcp_capabilities(acp::McpCapabilities::new().sse(true)),
            ))
    }

    async fn authenticate(
        &self,
        _arguments: acp::AuthenticateRequest,
    ) -> Result<acp::AuthenticateResponse, acp::Error> {
        Ok(acp::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        arguments: acp::NewSessionRequest,
    ) -> Result<acp::NewSessionResponse, acp::Error> {
        let session_id = self.next_session_id.get();
        self.next_session_id.set(session_id + 1);
        let session_id = session_id.to_string();
        if let Some(url) = arguments.mcp_servers.iter().find_map(extract_sse_url) {
            self.session_mcp_urls
                .borrow_mut()
                .insert(session_id.clone(), url.to_string());
        }
        Ok(acp::NewSessionResponse::new(session_id))
    }

    async fn load_session(
        &self,
        _arguments: acp::LoadSessionRequest,
    ) -> Result<acp::LoadSessionResponse, acp::Error> {
        Ok(acp::LoadSessionResponse::default())
    }

    async fn prompt(
        &self,
        arguments: acp::PromptRequest,
    ) -> Result<acp::PromptResponse, acp::Error> {
        let prompt_text = extract_prompt_text(&arguments.prompt);
        let (reply_tx, reply_rx) = oneshot::channel();
        self.action_tx
            .send(AgentAction::Prompt {
                session_id: arguments.session_id,
                prompt_text,
                reply: reply_tx,
            })
            .map_err(|_| acp::Error::internal_error())?;
        reply_rx.await.map_err(|_| acp::Error::internal_error())
    }

    async fn cancel(&self, _args: acp::CancelNotification) -> Result<(), acp::Error> {
        Ok(())
    }

    async fn set_session_mode(
        &self,
        _args: acp::SetSessionModeRequest,
    ) -> Result<acp::SetSessionModeResponse, acp::Error> {
        Ok(acp::SetSessionModeResponse::default())
    }

    async fn set_session_config_option(
        &self,
        _args: acp::SetSessionConfigOptionRequest,
    ) -> Result<acp::SetSessionConfigOptionResponse, acp::Error> {
        Ok(acp::SetSessionConfigOptionResponse::new(vec![]))
    }

    async fn ext_method(&self, _args: acp::ExtRequest) -> Result<acp::ExtResponse, acp::Error> {
        Err(acp::Error::method_not_found())
    }

    async fn ext_notification(&self, _args: acp::ExtNotification) -> Result<(), acp::Error> {
        Ok(())
    }
}

async fn send_text(
    conn: &acp::AgentSideConnection,
    session_id: acp::SessionId,
    text: impl Into<String>,
) -> Result<(), acp::Error> {
    conn.session_notification(acp::SessionNotification::new(
        session_id,
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new(text),
        ))),
    ))
    .await
}

async fn run_prompt_action(
    conn: &acp::AgentSideConnection,
    session_id: acp::SessionId,
    prompt_text: String,
    session_mcp_urls: Rc<RefCell<HashMap<String, String>>>,
) -> Result<acp::PromptResponse, acp::Error> {
    let session_key = session_id.0.to_string();
    let mcp_url = session_mcp_urls
        .borrow()
        .get(&session_key)
        .cloned()
        .ok_or_else(acp::Error::internal_error)?;

    let task_id = extract_task_id(&prompt_text).unwrap_or_else(|| "T001".to_string());
    call_submit_task_result(&mcp_url, &task_id)
        .await
        .map_err(|_| acp::Error::internal_error())?;

    send_text(conn, session_id, format!("acp-worker:submitted:{task_id}")).await?;
    Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
}

async fn call_submit_task_result(mcp_url: &str, task_id: &str) -> Result<()> {
    let transport = SseClientTransport::start(mcp_url.to_string())
        .await
        .context("failed to connect to injected MCP SSE server")?;
    let client = ().serve(transport).await.context("failed to initialize MCP client")?;

    let arguments = json!({
        "task_id": task_id,
        "summary": "acp worker fixture result",
        "agent": "worker",
    });

    let result = client
        .call_tool(CallToolRequestParam {
            name: "submit_task_result".into(),
            arguments: arguments.as_object().cloned(),
        })
        .await
        .context("submit_task_result tool call failed")?;
    client.cancel().await.ok();

    if result.is_error.unwrap_or(false) {
        return Err(anyhow!("submit_task_result returned MCP error"));
    }
    Ok(())
}

fn extract_sse_url(server: &acp::McpServer) -> Option<&str> {
    match server {
        acp::McpServer::Sse(sse) => Some(sse.url.as_str()),
        _ => None,
    }
}

fn extract_prompt_text(blocks: &[acp::ContentBlock]) -> String {
    let mut text = String::new();
    for block in blocks {
        if let acp::ContentBlock::Text(content) = block {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(content.text.as_ref());
        }
    }
    text
}

fn extract_task_id(text: &str) -> Option<String> {
    for token in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-') {
        if token.starts_with('T') && token.len() > 1 {
            return Some(token.to_string());
        }
    }
    None
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> acp::Result<()> {
    let outgoing = tokio::io::stdout().compat_write();
    let incoming = tokio::io::stdin().compat();

    let local_set = tokio::task::LocalSet::new();
    local_set
        .run_until(async move {
            let (action_tx, mut action_rx) = mpsc::unbounded_channel();
            let session_mcp_urls = Rc::new(RefCell::new(HashMap::new()));
            let (conn, handle_io) = acp::AgentSideConnection::new(
                TeamFixtureAgent::new(action_tx, session_mcp_urls.clone()),
                outgoing,
                incoming,
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );

            tokio::task::spawn_local(async move {
                while let Some(action) = action_rx.recv().await {
                    match action {
                        AgentAction::Prompt {
                            session_id,
                            prompt_text,
                            reply,
                        } => {
                            let result = run_prompt_action(
                                &conn,
                                session_id,
                                prompt_text,
                                session_mcp_urls.clone(),
                            )
                            .await;
                            let response = result.unwrap_or_else(|_| {
                                acp::PromptResponse::new(acp::StopReason::EndTurn)
                            });
                            let _ = reply.send(response);
                        }
                    }
                }
            });

            handle_io.await
        })
        .await
}
