use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    #[default]
    Manual,
    AutoAllow,
    AutoDeny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionRole {
    Solo,
    Leader,
    Specialist,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RuntimeContext {
    pub system_prompt: Option<String>,
    pub workspace_native_files: Vec<String>,
    pub memory_summary: Option<String>,
    pub agent_memory: Option<String>,
    pub team_manifest: Option<String>,
    pub task_reminder: Option<String>,
    #[serde(default)]
    pub history_messages: Vec<RuntimeHistoryMessage>,
    #[serde(default)]
    pub history_lines: Vec<String>,
    #[serde(default)]
    pub transcript_semantics: RuntimeTranscriptSemantics,
    pub user_input: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RuntimeHistoryMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<RuntimeToolCall>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeToolCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub name: String,
    pub input_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTranscriptSemantics {
    pub pruning: TranscriptPruningMode,
    #[serde(default)]
    pub pruning_policy: RuntimePruningPolicy,
    pub compaction: TranscriptCompactionMode,
}

impl Default for RuntimeTranscriptSemantics {
    fn default() -> Self {
        Self {
            pruning: TranscriptPruningMode::Off,
            pruning_policy: RuntimePruningPolicy::default(),
            compaction: TranscriptCompactionMode::RawTranscriptOnly,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePruningPolicy {
    pub keep_last_assistants: usize,
    pub min_prunable_tool_chars: usize,
    pub soft_trim_head_chars: usize,
    pub soft_trim_tail_chars: usize,
}

impl Default for RuntimePruningPolicy {
    fn default() -> Self {
        Self {
            keep_last_assistants: 3,
            min_prunable_tool_chars: 4_000,
            soft_trim_head_chars: 800,
            soft_trim_tail_chars: 800,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TranscriptPruningMode {
    #[default]
    Off,
    RequestLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TranscriptCompactionMode {
    #[default]
    RawTranscriptOnly,
    WorkingSetProjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolSurfaceSpec {
    pub team_tools: bool,
    pub local_skills: bool,
    pub external_mcp: bool,
    pub backend_native_tools: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalMcpServerSpec {
    pub name: String,
    pub transport: ExternalMcpTransport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExternalMcpTransport {
    Sse { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProviderProfile {
    pub id: String,
    pub protocol: RuntimeProviderProtocol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum RuntimeProviderProtocol {
    OfficialSession,
    AnthropicCompatible {
        base_url: String,
        auth_token: String,
        default_model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        small_fast_model: Option<String>,
    },
    OpenaiCompatible {
        base_url: String,
        api_key: String,
        default_model: String,
    },
}

impl RuntimeProviderProfile {
    pub fn is_official_session(&self) -> bool {
        matches!(self.protocol, RuntimeProviderProtocol::OfficialSession)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTurnRequest {
    pub participant_name: Option<String>,
    pub session_ref: String,
    pub role: ExecutionRole,
    pub workspace_dir: Option<PathBuf>,
    pub prompt_text: String,
    pub tool_surface: ToolSurfaceSpec,
    #[serde(default)]
    pub approval_mode: ApprovalMode,
    pub tool_bridge_url: Option<String>,
    #[serde(default)]
    pub external_mcp_servers: Vec<ExternalMcpServerSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_profile: Option<RuntimeProviderProfile>,
    pub context: RuntimeContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentEvent {
    TextDelta {
        text: String,
    },
    ToolCallStarted {
        tool_name: String,
        call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_summary: Option<String>,
    },
    ToolCallCompleted {
        tool_name: String,
        call_id: String,
        result: String,
    },
    ToolCallFailed {
        tool_name: String,
        call_id: String,
        error: String,
    },
    TurnComplete {
        full_text: String,
    },
    TurnFailed {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentResult {
    pub full_text: String,
    pub events: Vec<AgentEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_turn_request_round_trips_through_json() {
        let request = AgentTurnRequest {
            participant_name: Some("alpha".into()),
            session_ref: "lark:user:ou_1".into(),
            role: ExecutionRole::Solo,
            workspace_dir: None,
            prompt_text: "hello".into(),
            tool_surface: ToolSurfaceSpec::default(),
            approval_mode: ApprovalMode::AutoAllow,
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            provider_profile: None,
            context: RuntimeContext::default(),
        };
        let json = serde_json::to_string(&request).unwrap();
        let decoded: AgentTurnRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.prompt_text, "hello");
        assert_eq!(decoded.approval_mode, ApprovalMode::AutoAllow);
    }

    #[test]
    fn agent_event_round_trips_through_json() {
        let event = AgentEvent::ToolCallCompleted {
            tool_name: "read_file".into(),
            call_id: "call-1".into(),
            result: "{\"ok\":true}".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: AgentEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn agent_result_round_trips_through_json() {
        let result = AgentResult {
            full_text: "done".into(),
            events: vec![
                AgentEvent::TextDelta {
                    text: "partial".into(),
                },
                AgentEvent::TurnComplete {
                    full_text: "done".into(),
                },
            ],
        };
        let json = serde_json::to_string(&result).unwrap();
        let decoded: AgentResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, result);
    }
}
