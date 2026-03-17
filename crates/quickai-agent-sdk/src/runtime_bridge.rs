use anyhow::Result;
use rig::{
    message::{AssistantContent, Message, ToolResultContent, UserContent},
    OneOrMany,
};

use crate::{
    bridge::{AgentEvent, AgentTurnRequest, RuntimeHistoryMessage, RuntimeToolCall},
    config::AgentConfig,
    engine::RigEngine,
    tools::{NoopToolAugmentor, RuntimeToolAugmentor},
};

pub struct QuickAiRuntimeBridge {
    config: AgentConfig,
}

impl QuickAiRuntimeBridge {
    pub fn new(config: AgentConfig) -> Self {
        Self { config }
    }

    pub async fn execute<F>(&self, session: &AgentTurnRequest, on_event: F) -> Result<String>
    where
        F: Fn(AgentEvent) + Send + Sync + 'static,
    {
        self.execute_with_augmentor(session, on_event, &NoopToolAugmentor)
            .await
    }

    pub async fn execute_with_augmentor<A: RuntimeToolAugmentor, F>(
        &self,
        session: &AgentTurnRequest,
        on_event: F,
        augmentor: &A,
    ) -> Result<String>
    where
        F: Fn(AgentEvent) + Send + Sync + 'static,
    {
        let mut config = self
            .config
            .with_runtime_provider_profile(session.provider_profile.as_ref())?;
        if let Some(system_prompt) = &session.context.system_prompt {
            config.system_prompt = system_prompt.clone();
        }

        let engine = RigEngine::new(config);
        let user_input = session
            .context
            .user_input
            .as_deref()
            .unwrap_or(&session.prompt_text);
        let history = history_from_turn_request(session);
        engine
            .chat_streaming_with_augmentor(history, session, user_input, on_event, augmentor)
            .await
    }
}

fn history_from_turn_request(session: &AgentTurnRequest) -> Vec<Message> {
    let mut history = history_from_structured_context(&session.context.history_messages);
    if history.is_empty() {
        history = history_from_legacy_lines(&session.context.history_lines);
    }
    history
}

fn history_from_structured_context(history_messages: &[RuntimeHistoryMessage]) -> Vec<Message> {
    history_messages
        .iter()
        .enumerate()
        .flat_map(|(msg_idx, msg)| messages_from_runtime_history(msg_idx, msg))
        .collect()
}

fn messages_from_runtime_history(msg_idx: usize, msg: &RuntimeHistoryMessage) -> Vec<Message> {
    let mut messages = Vec::new();
    if let Some(base) = message_from_role_and_content(&msg.role, &msg.content) {
        messages.push(base);
    }

    if !msg.role.eq_ignore_ascii_case("assistant") {
        return messages;
    }

    for (call_idx, call) in msg.tool_calls.iter().enumerate() {
        messages.extend(messages_from_tool_call(msg_idx, call_idx, call));
    }

    messages
}

fn messages_from_tool_call(
    msg_idx: usize,
    call_idx: usize,
    call: &RuntimeToolCall,
) -> Vec<Message> {
    let mut messages = Vec::new();
    let tool_call_id = call
        .tool_call_id
        .clone()
        .unwrap_or_else(|| format!("qai-tool-{msg_idx}-{call_idx}"));

    let assistant_tool_call = if call.tool_call_id.is_some() {
        Message::Assistant {
            id: None,
            content: OneOrMany::one(AssistantContent::tool_call_with_call_id(
                tool_call_id.clone(),
                tool_call_id.clone(),
                call.name.clone(),
                parse_tool_arguments(&call.input_json),
            )),
        }
    } else {
        Message::Assistant {
            id: None,
            content: OneOrMany::one(AssistantContent::tool_call(
                tool_call_id.clone(),
                call.name.clone(),
                parse_tool_arguments(&call.input_json),
            )),
        }
    };
    messages.push(assistant_tool_call);

    if let Some(output) = call
        .output
        .as_deref()
        .filter(|output| !output.trim().is_empty())
    {
        let tool_result = if call.tool_call_id.is_some() {
            Message::User {
                content: OneOrMany::one(UserContent::tool_result_with_call_id(
                    tool_call_id.clone(),
                    tool_call_id.clone(),
                    OneOrMany::one(ToolResultContent::text(output)),
                )),
            }
        } else {
            Message::User {
                content: OneOrMany::one(UserContent::tool_result(
                    tool_call_id,
                    OneOrMany::one(ToolResultContent::text(output)),
                )),
            }
        };
        messages.push(tool_result);
    }

    messages
}

fn parse_tool_arguments(input_json: &str) -> serde_json::Value {
    serde_json::from_str(input_json)
        .unwrap_or_else(|_| serde_json::Value::String(input_json.to_string()))
}

fn history_from_legacy_lines(history_lines: &[String]) -> Vec<Message> {
    history_lines
        .iter()
        .filter_map(|line| parse_legacy_history_line(line))
        .collect()
}

fn parse_legacy_history_line(line: &str) -> Option<Message> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix('[')?;
    let (role, content) = rest.split_once("]: ")?;
    message_from_role_and_content(role, content)
}

fn message_from_role_and_content(role: &str, content: &str) -> Option<Message> {
    let content = content.trim();
    if content.is_empty() {
        return None;
    }
    match role.trim().to_ascii_lowercase().as_str() {
        "user" => Some(Message::user(content)),
        "assistant" => Some(Message::assistant(content)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bridge::{
            AgentTurnRequest, ExecutionRole, ExternalMcpServerSpec, ExternalMcpTransport,
            RuntimeContext, RuntimeHistoryMessage, RuntimeToolCall, ToolSurfaceSpec,
        },
        config::{AgentConfig, Provider},
    };
    use rig::message::{AssistantContent, Message, UserContent};

    fn test_config() -> AgentConfig {
        AgentConfig {
            provider: Provider::OpenAI { base_url: None },
            api_key: "sk-test".into(),
            model: "gpt-4o".into(),
            system_prompt: "base prompt".into(),
        }
    }

    fn test_session(prompt_text: &str, system_prompt: Option<&str>) -> AgentTurnRequest {
        AgentTurnRequest {
            participant_name: None,
            session_ref: "native:test".into(),
            role: ExecutionRole::Solo,
            workspace_dir: None,
            prompt_text: prompt_text.into(),
            tool_surface: ToolSurfaceSpec::default(),
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            provider_profile: None,
            context: RuntimeContext {
                system_prompt: system_prompt.map(ToOwned::to_owned),
                ..RuntimeContext::default()
            },
        }
    }

    #[test]
    fn runtime_bridge_constructs() {
        let bridge = QuickAiRuntimeBridge::new(test_config());
        let _ = bridge;
    }

    #[test]
    fn runtime_bridge_session_contract_can_override_system_prompt() {
        let session = test_session("hello", Some("override prompt"));
        assert_eq!(session.prompt_text, "hello");
        assert_eq!(
            session.context.system_prompt.as_deref(),
            Some("override prompt")
        );
    }

    fn extract_text(message: &Message) -> String {
        match message {
            Message::User { content } => content
                .iter()
                .find_map(|item| match item {
                    UserContent::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
            Message::Assistant { content, .. } => content
                .iter()
                .find_map(|item| match item {
                    AssistantContent::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
        }
    }

    #[test]
    fn structured_history_is_preferred_over_legacy_lines() {
        let session = AgentTurnRequest {
            participant_name: None,
            session_ref: "native:test".into(),
            role: ExecutionRole::Solo,
            workspace_dir: None,
            prompt_text: "hello".into(),
            tool_surface: ToolSurfaceSpec::default(),
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            provider_profile: None,
            context: RuntimeContext {
                history_messages: vec![
                    RuntimeHistoryMessage {
                        role: "user".into(),
                        content: "first".into(),
                        sender: Some("alice".into()),
                        tool_calls: Vec::new(),
                    },
                    RuntimeHistoryMessage {
                        role: "assistant".into(),
                        content: "second".into(),
                        sender: Some("agent".into()),
                        tool_calls: Vec::new(),
                    },
                ],
                history_lines: vec!["[user]: ignored".into(), "[assistant]: ignored too".into()],
                ..RuntimeContext::default()
            },
        };

        let history = history_from_turn_request(&session);
        assert_eq!(history.len(), 2);
        assert_eq!(extract_text(&history[0]), "first");
        assert_eq!(extract_text(&history[1]), "second");
    }

    #[test]
    fn tool_calls_are_rehydrated_from_structured_history() {
        let session = AgentTurnRequest {
            participant_name: None,
            session_ref: "native:test".into(),
            role: ExecutionRole::Solo,
            workspace_dir: None,
            prompt_text: "hello".into(),
            tool_surface: ToolSurfaceSpec::default(),
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![ExternalMcpServerSpec {
                name: "filesystem".into(),
                transport: ExternalMcpTransport::Sse {
                    url: "http://127.0.0.1:3001/sse".into(),
                },
            }],
            provider_profile: None,
            context: RuntimeContext {
                history_messages: vec![RuntimeHistoryMessage {
                    role: "assistant".into(),
                    content: "called tool".into(),
                    sender: Some("agent".into()),
                    tool_calls: vec![RuntimeToolCall {
                        name: "read_file".into(),
                        input_json: "{\"path\":\"README.md\"}".into(),
                        output: Some("file contents".into()),
                        tool_call_id: Some("call-1".into()),
                    }],
                }],
                ..RuntimeContext::default()
            },
        };

        let history = history_from_turn_request(&session);
        assert_eq!(history.len(), 3);
    }
}
