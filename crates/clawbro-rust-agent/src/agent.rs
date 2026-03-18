//! ClawBro ACP Agent
//! Implements the ACP Agent trait over stdio.
//!
//! Modes:
//! - Stub mode (no config): echoes prompt content back
//! - Engine mode (config present): calls rig-core LLM with streaming

use agent_client_protocol as acp;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};

use crate::config::AgentConfig;
use crate::runtime_bridge::ClawBroRuntimeBridge;
use crate::team::ClawBroTeamToolAugmentor;
use clawbro_agent_sdk::bridge::{
    AgentEvent, AgentTurnRequest, ExecutionRole, RuntimeContext, RuntimeHistoryMessage,
    ToolSurfaceSpec,
};

pub type NotifTx = mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)>;

pub struct ClawBroAgent {
    next_session_id: Cell<u64>,
    notif_tx: NotifTx,
    config: Option<AgentConfig>,
    /// session_id → conversation history
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

    /// Send a text chunk and wait for the background task to acknowledge.
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
            crate::config::Provider::Anthropic { .. } => false,
            crate::config::Provider::DeepSeek => false,
            crate::config::Provider::OpenAI { base_url } => !base_url
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
                acp::Implementation::new("clawbro-rust-agent", env!("CARGO_PKG_VERSION")),
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
        Ok(acp::NewSessionResponse::new(acp::SessionId::new(
            session_id,
        )))
    }

    async fn prompt(&self, args: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
        // Extract user text from all text content blocks
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
                // Engine mode: stream LLM response via rig-core
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
                                // Fire-and-forget streaming delta (no await in sync closure)
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

                // Send a final awaited chunk if we got a non-streaming fallback
                // (in streaming mode the chunks are already sent above)
                // For Anthropic streaming, `response` is the accumulated text,
                // but chunks were already sent via on_delta.
                // For non-streaming fallback providers, we send once here.
                if response.is_empty() {
                    // Nothing to do
                } else if Self::should_send_final_chunk(config) {
                    // Non-streaming providers: send the full response as one chunk
                    self.send_chunk_await(&args.session_id, response.clone())
                        .await?;
                }
                // (Anthropic: chunks already sent via on_delta above)

                // Update session history
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
                // Stub mode: echo back with await for reliable delivery
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Provider;
    use tokio::sync::mpsc;

    #[test]
    fn test_agent_new_stub_mode() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let agent = ClawBroAgent::new(tx, None);
        assert_eq!(agent.next_session_id.get(), 0);
        assert!(agent.config.is_none());
        assert!(agent.sessions.borrow().is_empty());
    }

    #[test]
    fn test_agent_new_engine_mode() {
        use crate::config::{AgentConfig, Provider};
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = AgentConfig {
            provider: Provider::Anthropic { base_url: None },
            api_key: "sk-test".to_string(),
            model: "claude-opus-4-6".to_string(),
            system_prompt: "test".to_string(),
        };
        let agent = ClawBroAgent::new(tx, Some(config));
        assert!(agent.config.is_some());
    }

    #[test]
    fn final_chunk_only_for_non_streaming_openai() {
        let openai_default = AgentConfig {
            provider: Provider::OpenAI { base_url: None },
            api_key: "sk-test".to_string(),
            model: "gpt-4o".to_string(),
            system_prompt: "test".to_string(),
        };
        assert!(ClawBroAgent::should_send_final_chunk(&openai_default));

        let openai_deepseek = AgentConfig {
            provider: Provider::OpenAI {
                base_url: Some("https://api.deepseek.com/v1".to_string()),
            },
            api_key: "sk-test".to_string(),
            model: "deepseek-chat".to_string(),
            system_prompt: "test".to_string(),
        };
        assert!(!ClawBroAgent::should_send_final_chunk(&openai_deepseek));

        let native_deepseek = AgentConfig {
            provider: Provider::DeepSeek,
            api_key: "sk-test".to_string(),
            model: "deepseek-chat".to_string(),
            system_prompt: "test".to_string(),
        };
        assert!(!ClawBroAgent::should_send_final_chunk(&native_deepseek));
    }
}
