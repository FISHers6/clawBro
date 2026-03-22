use agent_client_protocol::{self as acp, Client as _};
use anyhow::{Context, Result};
use std::cell::Cell;
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
}

impl TeamFixtureAgent {
    fn new(action_tx: mpsc::UnboundedSender<AgentAction>) -> Self {
        Self {
            action_tx,
            next_session_id: Cell::new(0),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for TeamFixtureAgent {
    async fn initialize(
        &self,
        arguments: acp::InitializeRequest,
    ) -> Result<acp::InitializeResponse, acp::Error> {
        Ok(
            acp::InitializeResponse::new(arguments.protocol_version).agent_info(
                acp::Implementation::new("clawbro-acp-team-fixture", "0.1.0")
                    .title("ClawBro ACP Team Fixture"),
            ),
        )
    }

    async fn authenticate(
        &self,
        _arguments: acp::AuthenticateRequest,
    ) -> Result<acp::AuthenticateResponse, acp::Error> {
        Ok(acp::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        _arguments: acp::NewSessionRequest,
    ) -> Result<acp::NewSessionResponse, acp::Error> {
        let session_id = self.next_session_id.get();
        self.next_session_id.set(session_id + 1);
        let session_id = session_id.to_string();
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
) -> Result<acp::PromptResponse, acp::Error> {
    let task_id = extract_task_id(&prompt_text).unwrap_or_else(|| "T001".to_string());
    if let Err(err) = call_submit_task_result(&task_id).await {
        eprintln!("clawbro-acp-team-fixture submit_task_result failed: {err:#}");
        return Err(acp::Error::internal_error());
    }

    send_text(conn, session_id, format!("acp-worker:submitted:{task_id}")).await?;
    Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
}

async fn call_submit_task_result(task_id: &str) -> Result<()> {
    let url = std::env::var("CLAWBRO_TEAM_TOOL_URL")
        .context("missing CLAWBRO_TEAM_TOOL_URL for ACP specialist turn")?;
    let session_ref = std::env::var("CLAWBRO_SESSION_REF")
        .context("missing CLAWBRO_SESSION_REF for ACP specialist turn")?;
    let session_key = clawbro::protocol::parse_session_key_text(&session_ref)
        .map_err(|err| anyhow::anyhow!("invalid CLAWBRO_SESSION_REF: {err}"))?;

    let response = reqwest::Client::new()
        .post(url)
        .json(&clawbro::runtime::TeamToolRequest {
            session_key,
            call: clawbro::runtime::TeamToolCall::SubmitTaskResult {
                task_id: task_id.to_string(),
                summary: "acp worker fixture result".to_string(),
                result_markdown: Some(
                    "# ACP Worker Result\n\nImplemented the fixture task and prepared the final deliverable body for lead review."
                        .to_string(),
                ),
                agent: Some("worker".to_string()),
            },
        })
        .send()
        .await
        .context("failed to invoke team tool endpoint")?;
    let status = response.status();
    let body: clawbro::runtime::TeamToolResponse = response
        .json()
        .await
        .context("failed to decode team tool response")?;
    if !status.is_success() || !body.ok {
        anyhow::bail!("submit_task_result failed: {}", body.message);
    }
    Ok(())
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
        if token.starts_with('T')
            && token.len() > 1
            && token[1..].chars().all(|c| c.is_ascii_digit())
        {
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
            let (conn, handle_io) = acp::AgentSideConnection::new(
                TeamFixtureAgent::new(action_tx),
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
                            let result = run_prompt_action(&conn, session_id, prompt_text).await;
                            let response = result.unwrap_or_else(|err| {
                                eprintln!("clawbro-acp-team-fixture prompt failed: {err:#}");
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
