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

struct ApprovalFixtureAgent {
    action_tx: mpsc::UnboundedSender<AgentAction>,
    next_session_id: Cell<u64>,
}

impl ApprovalFixtureAgent {
    fn new(action_tx: mpsc::UnboundedSender<AgentAction>) -> Self {
        Self {
            action_tx,
            next_session_id: Cell::new(0),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for ApprovalFixtureAgent {
    async fn initialize(
        &self,
        arguments: acp::InitializeRequest,
    ) -> Result<acp::InitializeResponse, acp::Error> {
        Ok(
            acp::InitializeResponse::new(arguments.protocol_version).agent_info(
                acp::Implementation::new("qai-acp-approval-fixture", "0.1.0")
                    .title("QAI ACP Approval Fixture"),
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
        eprintln!(
            "fixture: prompt received for session {}",
            arguments.session_id.0
        );
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
    eprintln!("fixture: sending pre-approval text");
    send_text(conn, session_id.clone(), "fixture awaiting approval").await?;

    let tool_call_id = acp::ToolCallId::new("fixture-approval");
    let tool_update = acp::ToolCallUpdate::new(
        tool_call_id.clone(),
        acp::ToolCallUpdateFields::new()
            .title("fixture approval")
            .status(acp::ToolCallStatus::Pending),
    );
    eprintln!("fixture: requesting permission");
    let permission = conn
        .request_permission(acp::RequestPermissionRequest::new(
            session_id.clone(),
            tool_update,
            vec![
                acp::PermissionOption::new(
                    acp::PermissionOptionId::new("allow-once"),
                    "Allow once",
                    acp::PermissionOptionKind::AllowOnce,
                ),
                acp::PermissionOption::new(
                    acp::PermissionOptionId::new("reject-once"),
                    "Reject once",
                    acp::PermissionOptionKind::RejectOnce,
                ),
            ],
        ))
        .await?;
    eprintln!("fixture: permission resolved");

    let final_text = match permission.outcome {
        acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome {
            option_id,
            ..
        }) if option_id.0.as_ref() == "allow-once" => "approved via allow-once",
        acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome {
            option_id,
            ..
        }) if option_id.0.as_ref() == "reject-once" => "denied via reject-once",
        _ => "cancelled",
    };

    conn.session_notification(acp::SessionNotification::new(
        session_id.clone(),
        acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
            tool_call_id,
            acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
        )),
    ))
    .await?;
    eprintln!("fixture: sending final text {final_text}");
    send_text(conn, session_id, final_text).await?;

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
                ApprovalFixtureAgent::new(action_tx),
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
