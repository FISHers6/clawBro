//! RigEngine: rig-core LLM chat engine.
//! Supports Anthropic / OpenAI / DeepSeek via environment variables.
//! No Tauri dependencies — config comes from AgentConfig (env vars).

use anyhow::Result;
use futures::StreamExt as _;
use rig::{
    agent::AgentBuilder,
    agent::MultiTurnStreamItem,
    client::CompletionClient as _,
    completion::Chat,
    message::Message,
    providers::{anthropic, deepseek, openai},
    streaming::{StreamedAssistantContent, StreamingChat},
};
use std::sync::Arc;

use crate::bridge::{AgentEvent, AgentTurnRequest};
use crate::config::{AgentConfig, Provider};
use crate::tools::{
    register_runtime_tools, NoopToolAugmentor, RuntimeToolAugmentor, RuntimeToolRegistration,
    ToolProgressTracker,
};

pub struct RigEngine {
    config: AgentConfig,
}

const STREAMING_MAX_TOOL_TURNS: usize = 8;

impl RigEngine {
    pub fn new(config: AgentConfig) -> Self {
        Self { config }
    }

    fn should_use_native_deepseek_provider(&self) -> bool {
        match &self.config.provider {
            Provider::DeepSeek => true,
            Provider::OpenAI { base_url } => base_url
                .as_deref()
                .map(|url| url.to_ascii_lowercase().contains("deepseek"))
                .unwrap_or(false),
            Provider::Anthropic { .. } => false,
        }
    }

    async fn build_deepseek_agent<A: RuntimeToolAugmentor>(
        &self,
        session: &AgentTurnRequest,
        tracker: Option<ToolProgressTracker>,
        augmentor: &A,
    ) -> Result<RuntimeToolRegistration<deepseek::CompletionModel>> {
        let client = deepseek::Client::new(&self.config.api_key);
        register_runtime_tools(
            client
                .agent(&self.config.model)
                .preamble(&self.config.system_prompt),
            session,
            tracker,
            augmentor,
        )
        .await
    }

    async fn build_anthropic_agent<A: RuntimeToolAugmentor>(
        &self,
        session: &AgentTurnRequest,
        base_url: Option<&str>,
        tracker: Option<ToolProgressTracker>,
        augmentor: &A,
    ) -> Result<RuntimeToolRegistration<anthropic::completion::CompletionModel>> {
        let client = if let Some(url) = base_url {
            anthropic::Client::builder(&self.config.api_key)
                .base_url(url)
                .build()
                .map_err(|e| anyhow::anyhow!("Anthropic client build failed: {e}"))?
        } else {
            anthropic::Client::new(&self.config.api_key)
        };

        let builder = if base_url.is_some() {
            let mut model = anthropic::completion::CompletionModel::new(client, &self.config.model);
            if model.default_max_tokens.is_none() {
                model.default_max_tokens = Some(2048);
            }
            AgentBuilder::new(model).preamble(&self.config.system_prompt)
        } else {
            client
                .agent(&self.config.model)
                .preamble(&self.config.system_prompt)
        };

        register_runtime_tools(builder, session, tracker, augmentor).await
    }

    /// Non-streaming chat. Returns complete response text.
    /// Used when the provider does not need incremental display.
    pub async fn chat(
        &self,
        history: Vec<Message>,
        session: &AgentTurnRequest,
        user_message: &str,
    ) -> Result<String> {
        self.chat_with_augmentor(history, session, user_message, &NoopToolAugmentor)
            .await
    }

    pub async fn chat_with_augmentor<A: RuntimeToolAugmentor>(
        &self,
        history: Vec<Message>,
        session: &AgentTurnRequest,
        user_message: &str,
        augmentor: &A,
    ) -> Result<String> {
        if self.should_use_native_deepseek_provider() {
            let registration = self.build_deepseek_agent(session, None, augmentor).await?;
            let agent = registration.builder.build();
            let _external_mcp_clients = registration.external_mcp_clients;
            return agent
                .chat(user_message, history)
                .await
                .map_err(|e| anyhow::anyhow!("DeepSeek chat failed: {e}"));
        }

        match &self.config.provider {
            Provider::Anthropic { base_url } => {
                let registration = self
                    .build_anthropic_agent(session, base_url.as_deref(), None, augmentor)
                    .await?;
                let agent = registration.builder.build();
                let _external_mcp_clients = registration.external_mcp_clients;
                agent
                    .chat(user_message, history)
                    .await
                    .map_err(|e| anyhow::anyhow!("Anthropic chat failed: {e}"))
            }
            Provider::OpenAI { base_url } => {
                // rig-core 0.20 defaults to the new Responses API (POST /responses).
                // DeepSeek and other OpenAI-compatible providers only support Chat Completions.
                // Use completion_model(...).completions_api() to force the /chat/completions path.
                let client = if let Some(url) = base_url {
                    // Append /v1 if the base URL does not already end with it
                    let api_url = if url.ends_with("/v1") || url.ends_with("/v1/") {
                        url.clone()
                    } else {
                        format!("{}/v1", url.trim_end_matches('/'))
                    };
                    tracing::info!(
                        "OpenAI-compatible client base_url={api_url} model={}",
                        self.config.model
                    );
                    openai::Client::builder(&self.config.api_key)
                        .base_url(&api_url)
                        .build()
                        .map_err(|e| anyhow::anyhow!("OpenAI client build failed: {e}"))?
                } else {
                    tracing::info!(
                        "OpenAI client (default api.openai.com) model={}",
                        self.config.model
                    );
                    openai::Client::new(&self.config.api_key)
                };
                // Force Chat Completions API (not Responses API)
                let chat_model = client
                    .completion_model(&self.config.model)
                    .completions_api();
                let registration = register_runtime_tools(
                    AgentBuilder::new(chat_model).preamble(&self.config.system_prompt),
                    session,
                    None,
                    augmentor,
                )
                .await?;
                let agent = registration.builder.build();
                let _external_mcp_clients = registration.external_mcp_clients;
                agent
                    .chat(user_message, history)
                    .await
                    .map_err(|e| anyhow::anyhow!("OpenAI chat failed: {e}"))
            }
            Provider::DeepSeek => unreachable!("DeepSeek is handled by the native provider path"),
        }
    }

    /// Streaming chat (Anthropic only for now).
    /// Calls `on_delta` for each text token; returns the full response.
    /// For providers without streaming support, falls back to non-streaming.
    pub async fn chat_streaming<F>(
        &self,
        history: Vec<Message>,
        session: &AgentTurnRequest,
        user_message: &str,
        on_event: F,
    ) -> Result<String>
    where
        F: Fn(AgentEvent) + Send + Sync + 'static,
    {
        self.chat_streaming_with_augmentor(
            history,
            session,
            user_message,
            on_event,
            &NoopToolAugmentor,
        )
        .await
    }

    pub async fn chat_streaming_with_augmentor<A: RuntimeToolAugmentor, F>(
        &self,
        history: Vec<Message>,
        session: &AgentTurnRequest,
        user_message: &str,
        on_event: F,
        augmentor: &A,
    ) -> Result<String>
    where
        F: Fn(AgentEvent) + Send + Sync + 'static,
    {
        let on_event = Arc::new(on_event);
        let tracker = ToolProgressTracker::new(on_event.clone());
        if self.should_use_native_deepseek_provider() {
            let registration = self
                .build_deepseek_agent(session, Some(tracker), augmentor)
                .await?;
            let agent = registration.builder.build();
            let external_mcp_clients = registration.external_mcp_clients;
            return stream_agent_chat(agent, history, user_message, on_event, external_mcp_clients)
                .await;
        }

        match &self.config.provider {
            Provider::Anthropic { base_url } => {
                let registration = self
                    .build_anthropic_agent(
                        session,
                        base_url.as_deref(),
                        Some(tracker),
                        augmentor,
                    )
                    .await?;
                let agent = registration.builder.build();
                let external_mcp_clients = registration.external_mcp_clients;
                stream_agent_chat(agent, history, user_message, on_event, external_mcp_clients)
                    .await
            }
            // OpenAI (with or without custom base URL) falls back to non-streaming for now.
            // on_delta is NOT called here — the caller (agent.rs) handles delivery via send_chunk_await
            // to avoid double-sending the response.
            _ => self
                .chat_with_augmentor(history, session, user_message, augmentor)
                .await,
        }
    }
}

async fn stream_agent_chat<M, F>(
    agent: rig::agent::Agent<M>,
    history: Vec<Message>,
    user_message: &str,
    on_event: Arc<F>,
    _external_mcp_clients: Vec<
        rmcp::service::RunningService<rmcp::service::RoleClient, rmcp::model::ClientInfo>,
    >,
) -> Result<String>
where
    M: rig::completion::CompletionModel + 'static,
    M::StreamingResponse: rig::completion::GetTokenUsage,
    F: Fn(AgentEvent) + Send + Sync + 'static,
{
    let mut stream = agent
        .stream_chat(user_message, history)
        .multi_turn(STREAMING_MAX_TOOL_TURNS)
        .await;

    let mut full = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk.map_err(|e| anyhow::anyhow!("stream chunk error: {e}"))? {
            MultiTurnStreamItem::StreamItem(content) => match content {
                StreamedAssistantContent::Text(delta) => {
                    on_event(AgentEvent::TextDelta {
                        text: delta.text.clone(),
                    });
                    full.push_str(&delta.text);
                }
                StreamedAssistantContent::Final(_) => {}
                StreamedAssistantContent::ToolCall(_) => {}
                StreamedAssistantContent::Reasoning(_) => {}
            },
            MultiTurnStreamItem::FinalResponse(final_response) => {
                if full.trim().is_empty() {
                    full = final_response.response().to_string();
                }
            }
            _ => {}
        }
    }
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{ExecutionRole, RuntimeContext, ToolSurfaceSpec};
    use crate::config::{AgentConfig, Provider};

    fn test_session() -> AgentTurnRequest {
        AgentTurnRequest {
            participant_name: None,
            session_ref: "native:engine-test".into(),
            role: ExecutionRole::Solo,
            workspace_dir: None,
            prompt_text: "hello".into(),
            tool_surface: ToolSurfaceSpec::default(),
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            provider_profile: None,
            context: RuntimeContext::default(),
        }
    }

    #[test]
    fn test_engine_builds() {
        // Construction should always succeed (no API calls at construction time)
        let config = AgentConfig {
            provider: Provider::Anthropic { base_url: None },
            api_key: "sk-test-invalid".to_string(),
            model: "claude-opus-4-6".to_string(),
            system_prompt: "You are a helpful assistant.".to_string(),
        };
        let _engine = RigEngine::new(config);
    }

    #[test]
    fn test_engine_builds_openai() {
        let config = AgentConfig {
            provider: Provider::OpenAI { base_url: None },
            api_key: "sk-test".to_string(),
            model: "gpt-4o".to_string(),
            system_prompt: "test".to_string(),
        };
        let _engine = RigEngine::new(config);
    }

    #[test]
    fn test_engine_builds_openai_custom_base() {
        let config = AgentConfig {
            provider: Provider::OpenAI {
                base_url: Some("https://api.deepseek.com".to_string()),
            },
            api_key: "sk-ds-test".to_string(),
            model: "deepseek-chat".to_string(),
            system_prompt: "test".to_string(),
        };
        let _engine = RigEngine::new(config);
    }

    #[test]
    fn test_engine_builds_deepseek() {
        let config = AgentConfig {
            provider: Provider::DeepSeek,
            api_key: "ds-test".to_string(),
            model: "deepseek-chat".to_string(),
            system_prompt: "test".to_string(),
        };
        let _engine = RigEngine::new(config);
    }

    #[test]
    fn openai_custom_deepseek_base_uses_native_deepseek_provider() {
        let engine = RigEngine::new(AgentConfig {
            provider: Provider::OpenAI {
                base_url: Some("https://api.deepseek.com".to_string()),
            },
            api_key: "sk-ds-test".to_string(),
            model: "deepseek-chat".to_string(),
            system_prompt: "test".to_string(),
        });

        assert!(engine.should_use_native_deepseek_provider());
    }

    #[test]
    fn ordinary_openai_base_does_not_use_native_deepseek_provider() {
        let engine = RigEngine::new(AgentConfig {
            provider: Provider::OpenAI {
                base_url: Some("https://example.com".to_string()),
            },
            api_key: "sk-test".to_string(),
            model: "gpt-4o".to_string(),
            system_prompt: "test".to_string(),
        });

        assert!(!engine.should_use_native_deepseek_provider());
    }

    /// Requires OPENAI_API_KEY + OPENAI_API_BASE=https://api.deepseek.com — skipped in CI
    #[tokio::test]
    #[ignore = "requires OPENAI_API_KEY + OPENAI_API_BASE pointing to DeepSeek"]
    async fn test_chat_with_deepseek_direct() {
        let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY not set");
        let base_url = std::env::var("OPENAI_API_BASE").ok();
        let config = AgentConfig {
            provider: Provider::OpenAI { base_url },
            api_key: key,
            model: "deepseek-chat".to_string(),
            system_prompt: "Reply in one word only.".to_string(),
        };
        let engine = RigEngine::new(config);
        let result = engine.chat(vec![], &test_session(), "Reply PONG").await;
        eprintln!("DeepSeek result: {:?}", result);
        assert!(result.is_ok(), "DeepSeek chat failed: {:?}", result);
        assert!(!result.unwrap().is_empty());
    }

    /// Requires ANTHROPIC_API_KEY — skipped in CI
    #[tokio::test]
    #[ignore = "requires ANTHROPIC_API_KEY"]
    async fn test_chat_with_real_api() {
        let key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY not set");
        let config = AgentConfig {
            provider: Provider::Anthropic { base_url: None },
            api_key: key,
            model: "claude-haiku-4-5-20251001".to_string(),
            system_prompt: "Reply in one word only.".to_string(),
        };
        let engine = RigEngine::new(config);
        let result = engine.chat(vec![], &test_session(), "Say hello").await;
        assert!(result.is_ok(), "chat failed: {:?}", result);
        assert!(!result.unwrap().is_empty());
    }
}
