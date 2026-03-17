use agent_client_protocol::{self as acp, Client as _};
use std::cell::Cell;
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

enum AgentAction {
    Prompt {
        session_id: acp::SessionId,
        reply: oneshot::Sender<acp::PromptResponse>,
    },
}

struct EchoFixtureAgent {
    action_tx: mpsc::UnboundedSender<AgentAction>,
    next_session_id: Cell<u64>,
}

impl EchoFixtureAgent {
    fn new(action_tx: mpsc::UnboundedSender<AgentAction>) -> Self {
        Self {
            action_tx,
            next_session_id: Cell::new(0),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for EchoFixtureAgent {
    async fn initialize(
        &self,
        arguments: acp::InitializeRequest,
    ) -> Result<acp::InitializeResponse, acp::Error> {
        Ok(
            acp::InitializeResponse::new(arguments.protocol_version).agent_info(
                acp::Implementation::new("clawbro-acp-echo-fixture", "0.1.0")
                    .title("QAI ACP Echo Fixture"),
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
        Ok(acp::NewSessionResponse::new(session_id.to_string()))
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
        let (reply_tx, reply_rx) = oneshot::channel();
        self.action_tx
            .send(AgentAction::Prompt {
                session_id: arguments.session_id,
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
) -> Result<acp::PromptResponse, acp::Error> {
    // Emit usage_update and session_info_update before the text chunk.
    // These are additive ACP protocol variants; the client must not fail on them.
    conn.session_notification(acp::SessionNotification::new(
        session_id.clone(),
        acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(10, 100)),
    ))
    .await?;
    conn.session_notification(acp::SessionNotification::new(
        session_id.clone(),
        acp::SessionUpdate::SessionInfoUpdate(acp::SessionInfoUpdate::new()),
    ))
    .await?;
    send_text(conn, session_id.clone(), "acp:fixture").await?;
    Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
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
                EchoFixtureAgent::new(action_tx),
                outgoing,
                incoming,
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );

            tokio::task::spawn_local(async move {
                while let Some(action) = action_rx.recv().await {
                    match action {
                        AgentAction::Prompt { session_id, reply } => {
                            let result = run_prompt_action(&conn, session_id).await;
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
