use crate::agent_sdk_internal::{
    bridge::{
        AgentEvent, AgentTurnRequest, ExecutionRole, RuntimeContext, RuntimeHistoryMessage,
        ToolSurfaceSpec,
    },
    config::{AgentConfig, Provider},
    runtime_bridge::ClawBroRuntimeBridge,
};
use crate::embedded_agent::team::ClawBroTeamToolAugmentor;
use agent_client_protocol as acp;
use agent_client_protocol::Client as _;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

pub type NotifTx = mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)>;

pub async fn run_stdio_agent() -> anyhow::Result<()> {
    let agent_config = AgentConfig::from_env().ok();

    if agent_config.is_some() {
        tracing::info!("Starting embedded clawbro ACP agent with rig-core LLM engine");
    } else {
        tracing::info!("Starting embedded clawbro ACP agent in echo stub mode (no API key set)");
    }

    let outgoing = tokio::io::stdout().compat_write();
    let incoming = tokio::io::stdin().compat();

    let local_set = tokio::task::LocalSet::new();
    local_set
        .run_until(async move {
            let (notif_tx, mut notif_rx) = tokio::sync::mpsc::unbounded_channel();

            let (conn, handle_io) = acp::AgentSideConnection::new(
                ClawBroAgent::new(notif_tx, agent_config),
                outgoing,
                incoming,
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );

            tokio::task::spawn_local(async move {
                while let Some((notification, reply_tx)) = notif_rx.recv().await {
                    if let Err(e) = conn.session_notification(notification).await {
                        tracing::error!("session_notification failed: {e}");
                        break;
                    }
                    reply_tx.send(()).ok();
                }
            });

            handle_io.await?;
            Ok(())
        })
        .await
}

pub struct ClawBroAgent {
    next_session_id: Cell<u64>,
    notif_tx: NotifTx,
    config: Option<AgentConfig>,
    sessions: RefCell<HashMap<String, Vec<RuntimeHistoryMessage>>>,
}

impl ClawBroAgent {
    pub fn new(notif_tx: NotifTx, config: Option<AgentConfig>) -> Self {
        Self {
            next_session_id: Cell::new(0),
            notif_tx,
            config,
            sessions: RefCell::new(HashMap::new()),
        }
    }

    async fn send_chunk_await(&self, session_id: &acp::SessionId, text: String) -> acp::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.notif_tx
            .send((
                acp::SessionNotification::new(
                    session_id.clone(),
                    acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                        acp::ContentBlock::Text(acp::TextContent::new(text)),
                    )),
                ),
                tx,
            ))
            .map_err(|_| acp::Error::internal_error())?;
        rx.await.map_err(|_| acp::Error::internal_error())?;
        Ok(())
    }

    fn should_send_final_chunk(config: &AgentConfig) -> bool {
        match &config.provider {
            Provider::Anthropic { .. } => false,
            Provider::DeepSeek => false,
            Provider::OpenAI { base_url } => !base_url
                .as_deref()
                .map(|url| url.to_ascii_lowercase().contains("deepseek"))
                .unwrap_or(false),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for ClawBroAgent {
    async fn initialize(
        &self,
        _args: acp::InitializeRequest,
    ) -> acp::Result<acp::InitializeResponse> {
        Ok(
            acp::InitializeResponse::new(acp::ProtocolVersion::V1).agent_info(
                acp::Implementation::new("clawbro", env!("CARGO_PKG_VERSION")),
            ),
        )
    }

    async fn authenticate(
        &self,
        _args: acp::AuthenticateRequest,
    ) -> acp::Result<acp::AuthenticateResponse> {
        Ok(acp::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        _args: acp::NewSessionRequest,
    ) -> acp::Result<acp::NewSessionResponse> {
        let id = self.next_session_id.get();
        self.next_session_id.set(id + 1);
        let session_id = id.to_string();
        self.sessions
            .borrow_mut()
            .insert(session_id.clone(), vec![]);
        Ok(acp::NewSessionResponse::new(acp::SessionId::new(session_id)))
    }

    async fn prompt(&self, args: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
        let user_text: String = args
            .prompt
            .iter()
            .filter_map(|c| {
                if let acp::ContentBlock::Text(t) = c {
                    Some(t.text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" ");

        if user_text.is_empty() {
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }

        let session_id_str = args.session_id.0.to_string();

        match &self.config {
            Some(config) => {
                let history = self
                    .sessions
                    .borrow()
                    .get(&session_id_str)
                    .cloned()
                    .unwrap_or_default();

                let bridge = ClawBroRuntimeBridge::new(config.clone());
                let team_tools = ClawBroTeamToolAugmentor::from_env();
                let notif_tx = self.notif_tx.clone();
                let session_id_for_delta = args.session_id.clone();

                let response = bridge
                    .execute_with_augmentor(
                        &AgentTurnRequest {
                            participant_name: None,
                            session_ref: format!("acp:{session_id_str}"),
                            role: ExecutionRole::Solo,
                            workspace_dir: None,
                            prompt_text: user_text.clone(),
                            tool_surface: ToolSurfaceSpec::default(),
                            approval_mode: Default::default(),
                            tool_bridge_url: None,
                            external_mcp_servers: vec![],
                            provider_profile: None,
                            context: RuntimeContext {
                                system_prompt: Some(config.system_prompt.clone()),
                                history_messages: history,
                                ..RuntimeContext::default()
                            },
                        },
                        move |event| {
                            if let AgentEvent::TextDelta { text } = event {
                                let (tx, _rx) = oneshot::channel();
                                let _ = notif_tx.send((
                                    acp::SessionNotification::new(
                                        session_id_for_delta.clone(),
                                        acp::SessionUpdate::AgentMessageChunk(
                                            acp::ContentChunk::new(acp::ContentBlock::Text(
                                                acp::TextContent::new(text),
                                            )),
                                        ),
                                    ),
                                    tx,
                                ));
                            }
                        },
                        &team_tools,
                    )
                    .await
                    .map_err(|e| {
                        tracing::error!("rig engine error: {e}");
                        acp::Error::internal_error()
                    })?;

                if !response.is_empty() && Self::should_send_final_chunk(config) {
                    self.send_chunk_await(&args.session_id, response.clone())
                        .await?;
                }

                {
                    let mut sessions = self.sessions.borrow_mut();
                    let h = sessions.entry(session_id_str).or_default();
                    h.push(RuntimeHistoryMessage {
                        role: "user".into(),
                        content: user_text,
                        sender: None,
                        tool_calls: Vec::new(),
                    });
                    h.push(RuntimeHistoryMessage {
                        role: "assistant".into(),
                        content: response,
                        sender: None,
                        tool_calls: Vec::new(),
                    });
                }
            }
            None => {
                let reply = format!("Echo: {user_text}");
                self.send_chunk_await(&args.session_id, reply).await?;
            }
        }

        Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
    }

    async fn cancel(&self, _args: acp::CancelNotification) -> acp::Result<()> {
        Ok(())
    }
}
