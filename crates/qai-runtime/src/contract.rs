use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::backend::ApprovalMode;
use crate::provider_profiles::RuntimeProviderProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnMode {
    Solo,
    Relay,
    Team,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeRole {
    Solo,
    Leader,
    Specialist,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RuntimeContext {
    /// Derived system guidance for runtimes that need structured prompt text.
    /// This is a projection layer, not the durable source of persona identity.
    pub system_prompt: Option<String>,
    /// Canonical visible file contract for this turn.
    /// Entries are deterministic and role-aware projections of persona/workspace/team files.
    pub workspace_native_files: Vec<String>,
    /// Shared contextual memory projection for this turn.
    pub memory_summary: Option<String>,
    /// Private role-allowed memory projection for this turn.
    pub agent_memory: Option<String>,
    /// Projection of TEAM.md when team context is active.
    pub team_manifest: Option<String>,
    /// Derived task-local execution helper, not a durable context source.
    pub task_reminder: Option<String>,
    /// Canonical structured rolling history for this turn.
    #[serde(default)]
    pub history_messages: Vec<RuntimeHistoryMessage>,
    /// Compatibility projection of rolling history for prompt-based backends.
    #[serde(default)]
    pub history_lines: Vec<String>,
    /// Declares how runtimes should interpret raw transcript truth vs working-set transforms.
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
    /// Request-local context reduction only. Must not rewrite persisted transcript truth.
    pub pruning: TranscriptPruningMode,
    /// Policy parameters for request-local pruning when enabled.
    #[serde(default)]
    pub pruning_policy: RuntimePruningPolicy,
    /// Persisted/runtime-working-set relationship for this turn.
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
    /// No request-local transcript reduction is applied.
    #[default]
    Off,
    /// Reduce prompt bloat for one execution only; persisted transcript stays untouched.
    RequestLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TranscriptCompactionMode {
    /// Raw append-only transcript is both durable truth and runtime working set.
    #[default]
    RawTranscriptOnly,
    /// Raw transcript remains durable truth; runtime consumes a persisted compacted working set.
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
pub struct TurnIntent {
    pub session_key: qai_protocol::SessionKey,
    pub mode: TurnMode,
    pub leader_candidate: Option<String>,
    pub target_backend: Option<String>,
    pub user_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSessionSpec {
    pub backend_id: String,
    pub participant_name: Option<String>,
    pub session_key: qai_protocol::SessionKey,
    pub role: RuntimeRole,
    pub workspace_dir: Option<PathBuf>,
    pub prompt_text: String,
    pub tool_surface: ToolSurfaceSpec,
    #[serde(default)]
    pub approval_mode: ApprovalMode,
    /// ACP-family MCP bridge endpoint (typically SSE).
    pub tool_bridge_url: Option<String>,
    /// User-configured external MCP servers for backend families that support them.
    #[serde(default)]
    pub external_mcp_servers: Vec<ExternalMcpServerSpec>,
    /// Resolved host-owned provider profile for this turn, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_profile: Option<RuntimeProviderProfile>,
    /// Family-agnostic synchronous Team Tool RPC endpoint.
    pub team_tool_url: Option<String>,
    pub context: RuntimeContext,
    /// 上次该 session 在此 backend 使用的 ACP session ID（来自 SessionMeta）。
    /// ACP bridge-backed backend 用于 session/load resume；其他 backend 忽略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_session_id: Option<String>,
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
pub struct PermissionRequest {
    pub id: String,
    pub prompt: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub host: Option<String>,
    pub agent_id: Option<String>,
    pub expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TeamCallback {
    TaskCreated {
        task_id: String,
        title: String,
        assignee: String,
    },
    TaskAssigned {
        task_id: String,
        assignee: String,
    },
    ExecutionStarted,
    PublicUpdatePosted {
        message: String,
    },
    TaskCheckpoint {
        task_id: String,
        note: String,
        agent: String,
    },
    TaskSubmitted {
        task_id: String,
        summary: String,
        result_markdown: Option<String>,
        agent: String,
    },
    TaskAccepted {
        task_id: String,
        by: String,
    },
    TaskReopened {
        task_id: String,
        reason: String,
        by: String,
    },
    TaskBlocked {
        task_id: String,
        reason: String,
        agent: String,
    },
    TaskHelpRequested {
        task_id: String,
        message: String,
        agent: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeEvent {
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
    ApprovalRequest(PermissionRequest),
    ToolCallback(TeamCallback),
    TurnComplete {
        full_text: String,
    },
    TurnFailed {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnResult {
    pub full_text: String,
    pub events: Vec<RuntimeEvent>,
    /// 本次 turn 由 ACP backend 发出的新 session ID。
    /// new_session() 路径：Some(id)，需要持久化到 SessionMeta。
    /// load_session() 路径（resume）：None，prior_id 不变，无需写入。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emitted_backend_session_id: Option<String>,
    /// Deterministic fingerprint of the backend spec that produced/validated the
    /// current backend session handle. Used to decide whether a stored resume
    /// handle is still safe to reuse on a future turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_resume_fingerprint: Option<String>,
    /// 本次 turn 实际使用的 backend_id（由 run_dispatch_job 注入）。
    /// registry 层用此 key 调用 complete_turn()。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_backend_id: Option<String>,
    /// Runtime-discovered resume recovery that the host must persist after the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_recovery: Option<ResumeRecoveryAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResumeRecoveryAction {
    DropFailedLoadSessionHandle { stale_session_id: String },
}

pub fn render_history_lines(
    history_messages: &[RuntimeHistoryMessage],
    semantics: &RuntimeTranscriptSemantics,
) -> Vec<String> {
    let mut lines = Vec::new();
    let assistant_count = history_messages
        .iter()
        .filter(|msg| msg.role.eq_ignore_ascii_case("assistant"))
        .count();
    let protected_assistant_cutoff =
        assistant_count.saturating_sub(semantics.pruning_policy.keep_last_assistants);
    let mut assistant_seen = 0usize;

    for msg in history_messages {
        if msg.content.trim().is_empty() {
            if !msg.role.eq_ignore_ascii_case("assistant") {
                continue;
            }
        }
        let content = match msg.sender.as_deref() {
            Some(sender) if !sender.is_empty() => format!("[{sender}]: {}", msg.content),
            _ => msg.content.clone(),
        };
        if !content.trim().is_empty() {
            lines.push(format!("[{}]: {}", msg.role, content));
        }

        let should_prune_tool_results = if semantics.pruning == TranscriptPruningMode::RequestLocal
            && msg.role.eq_ignore_ascii_case("assistant")
        {
            let prune = assistant_seen < protected_assistant_cutoff;
            assistant_seen += 1;
            prune
        } else {
            if msg.role.eq_ignore_ascii_case("assistant") {
                assistant_seen += 1;
            }
            false
        };

        for call in &msg.tool_calls {
            let call_suffix = call
                .tool_call_id
                .as_deref()
                .map(|id| format!("#{id}"))
                .unwrap_or_default();
            lines.push(format!(
                "[tool_call:{}{}]: {}",
                call.name, call_suffix, call.input_json
            ));
            if let Some(output) = call
                .output
                .as_deref()
                .filter(|output| !output.trim().is_empty())
            {
                let rendered_output = if should_prune_tool_results {
                    soft_trim_tool_result(
                        output,
                        semantics.pruning_policy.min_prunable_tool_chars,
                        semantics.pruning_policy.soft_trim_head_chars,
                        semantics.pruning_policy.soft_trim_tail_chars,
                    )
                } else {
                    output.to_string()
                };
                lines.push(format!(
                    "[tool_result:{}{}]: {}",
                    call.name, call_suffix, rendered_output
                ));
            }
        }
    }
    lines
}

fn soft_trim_tool_result(
    output: &str,
    min_prunable_chars: usize,
    head_chars: usize,
    tail_chars: usize,
) -> String {
    let total_chars = output.chars().count();
    if total_chars < min_prunable_chars || total_chars <= head_chars + tail_chars + 32 {
        return output.to_string();
    }

    let head: String = output.chars().take(head_chars).collect();
    let tail: String = output
        .chars()
        .rev()
        .take(tail_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let omitted = total_chars.saturating_sub(head_chars + tail_chars);
    format!("{head}\n...[tool result pruned; omitted {omitted} chars]...\n{tail}")
}

pub fn render_runtime_prompt(session: &RuntimeSessionSpec) -> String {
    let mut parts = Vec::new();
    if let Some(system_prompt) = session.context.system_prompt.as_deref() {
        if !system_prompt.trim().is_empty() {
            parts.push(format!(
                "<system_context>\n{}\n</system_context>",
                system_prompt
            ));
        }
    }
    if let Some(task_reminder) = session.context.task_reminder.as_deref() {
        if !task_reminder.trim().is_empty() {
            parts.push(format!(
                "══════ 当前任务（自动注入，最高优先级）══════\n{}\n══════════════════════════════════════════",
                task_reminder
            ));
        }
    }
    if let Some(team_manifest) = session.context.team_manifest.as_deref() {
        if !team_manifest.trim().is_empty() {
            parts.push(format!("## 团队职责\n\n{}", team_manifest));
        }
    }
    if let Some(memory_summary) = session.context.memory_summary.as_deref() {
        if !memory_summary.trim().is_empty() {
            let label = match session.role {
                RuntimeRole::Specialist => "## 任务背景（团队上下文）",
                _ => "## 群组共享记忆",
            };
            parts.push(format!("{label}\n\n{}", cap_words(memory_summary, 300)));
        }
    }
    if !matches!(session.role, RuntimeRole::Specialist) {
        if let Some(agent_memory) = session.context.agent_memory.as_deref() {
            if !agent_memory.trim().is_empty() {
                parts.push(format!("## 长期记忆\n\n{}", cap_words(agent_memory, 500)));
            }
        }
    }
    if !session.context.workspace_native_files.is_empty() {
        parts.push(format!(
            "## 工作区原生上下文文件\n\n当前工作目录中已投影以下原生上下文文件，可按需直接读取：\n{}",
            session
                .context
                .workspace_native_files
                .iter()
                .map(|name| format!("- {name}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if !session.context.history_lines.is_empty() {
        parts.extend(
            session
                .context
                .history_lines
                .iter()
                .filter(|line| !line.trim().is_empty())
                .cloned(),
        );
    } else {
        parts.extend(render_history_lines(
            &session.context.history_messages,
            &session.context.transcript_semantics,
        ));
    }
    if let Some(user_input) = session.context.user_input.as_deref() {
        if !user_input.trim().is_empty() {
            parts.push(user_input.to_string());
        }
    }
    if parts.is_empty() {
        session.prompt_text.clone()
    } else {
        parts.join("\n\n")
    }
}

fn cap_words(text: &str, max_words: usize) -> String {
    let mut count = 0usize;
    let mut out = Vec::new();
    for word in text.split_whitespace() {
        if count >= max_words {
            break;
        }
        out.push(word);
        count += 1;
    }
    if out.is_empty() && !text.trim().is_empty() {
        text.to_string()
    } else if count < text.split_whitespace().count() {
        format!("{} ...", out.join(" "))
    } else {
        out.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_intent_captures_team_leader_candidate() {
        let intent = TurnIntent {
            session_key: qai_protocol::SessionKey::new("lark", "group:test"),
            mode: TurnMode::Team,
            leader_candidate: Some("codex".into()),
            target_backend: Some("codex".into()),
            user_text: "ship it".into(),
        };

        assert_eq!(intent.leader_candidate.as_deref(), Some("codex"));
        assert_eq!(intent.mode, TurnMode::Team);
    }

    #[test]
    fn runtime_session_spec_preserves_workspace_and_role() {
        let spec = RuntimeSessionSpec {
            backend_id: "openclaw-main".into(),
            participant_name: Some("leader".into()),
            session_key: qai_protocol::SessionKey::new("lark", "group:test"),
            role: RuntimeRole::Leader,
            workspace_dir: Some(PathBuf::from("/tmp/workspace")),
            prompt_text: "ship it".into(),
            tool_surface: ToolSurfaceSpec {
                team_tools: true,
                local_skills: true,
                external_mcp: false,
                backend_native_tools: true,
            },
            approval_mode: Default::default(),
            tool_bridge_url: Some("http://127.0.0.1:9999/sse".into()),
            external_mcp_servers: vec![
                ExternalMcpServerSpec {
                    name: "filesystem".into(),
                    transport: ExternalMcpTransport::Sse {
                        url: "http://127.0.0.1:3001/sse".into(),
                    },
                },
                ExternalMcpServerSpec {
                    name: "github".into(),
                    transport: ExternalMcpTransport::Sse {
                        url: "http://127.0.0.1:3002/sse".into(),
                    },
                },
            ],
            team_tool_url: Some("http://127.0.0.1:9999/runtime/team-tools".into()),
            provider_profile: None,
            backend_session_id: None,
            context: RuntimeContext::default(),
        };

        assert_eq!(spec.role, RuntimeRole::Leader);
        assert_eq!(spec.workspace_dir, Some(PathBuf::from("/tmp/workspace")));
        assert_eq!(spec.prompt_text, "ship it");
        assert!(spec.tool_surface.team_tools);
        assert_eq!(
            spec.tool_bridge_url.as_deref(),
            Some("http://127.0.0.1:9999/sse")
        );
        assert_eq!(spec.external_mcp_servers.len(), 2);
        assert_eq!(
            spec.team_tool_url.as_deref(),
            Some("http://127.0.0.1:9999/runtime/team-tools")
        );
    }

    #[test]
    fn render_runtime_prompt_prefers_structured_context() {
        let spec = RuntimeSessionSpec {
            backend_id: "native-main".into(),
            participant_name: Some("leader".into()),
            session_key: qai_protocol::SessionKey::new("lark", "group:test"),
            role: RuntimeRole::Leader,
            workspace_dir: None,
            prompt_text: "legacy raw prompt".into(),
            tool_surface: ToolSurfaceSpec::default(),
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: None,
            provider_profile: None,
            backend_session_id: None,
            context: RuntimeContext {
                system_prompt: Some("system rules".into()),
                history_messages: vec![RuntimeHistoryMessage {
                    role: "user".into(),
                    content: "hi".into(),
                    sender: None,
                    tool_calls: Vec::new(),
                }],
                history_lines: vec!["[user]: hi".into()],
                user_input: Some("ship it".into()),
                ..RuntimeContext::default()
            },
        };

        let prompt = render_runtime_prompt(&spec);
        assert!(prompt.contains("<system_context>"));
        assert!(prompt.contains("[user]: hi"));
        assert!(prompt.contains("ship it"));
        assert!(!prompt.contains("legacy raw prompt"));
    }

    #[test]
    fn render_runtime_prompt_falls_back_to_structured_history_messages() {
        let spec = RuntimeSessionSpec {
            backend_id: "native-main".into(),
            participant_name: None,
            session_key: qai_protocol::SessionKey::new("ws", "user:test"),
            role: RuntimeRole::Solo,
            workspace_dir: None,
            prompt_text: "legacy raw prompt".into(),
            tool_surface: ToolSurfaceSpec::default(),
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: None,
            provider_profile: None,
            backend_session_id: None,
            context: RuntimeContext {
                history_messages: vec![
                    RuntimeHistoryMessage {
                        role: "user".into(),
                        content: "first".into(),
                        sender: None,
                        tool_calls: Vec::new(),
                    },
                    RuntimeHistoryMessage {
                        role: "assistant".into(),
                        content: "second".into(),
                        sender: Some("@codex".into()),
                        tool_calls: vec![RuntimeToolCall {
                            tool_call_id: Some("call-1".into()),
                            name: "read".into(),
                            input_json: "{\"path\":\"README.md\"}".into(),
                            output: Some("ok".into()),
                        }],
                    },
                ],
                user_input: Some("third".into()),
                ..RuntimeContext::default()
            },
        };

        let prompt = render_runtime_prompt(&spec);
        assert!(prompt.contains("[user]: first"));
        assert!(prompt.contains("[assistant]: [@codex]: second"));
        assert!(prompt.contains("[tool_call:read#call-1]: {\"path\":\"README.md\"}"));
        assert!(prompt.contains("[tool_result:read#call-1]: ok"));
        assert!(prompt.contains("third"));
        assert!(!prompt.contains("legacy raw prompt"));
    }

    #[test]
    fn render_history_lines_keeps_tool_identity_visible_for_prompt_backends() {
        let lines = render_history_lines(
            &[RuntimeHistoryMessage {
                role: "assistant".into(),
                content: "checking".into(),
                sender: Some("@codex".into()),
                tool_calls: vec![RuntimeToolCall {
                    tool_call_id: Some("tool-42".into()),
                    name: "search".into(),
                    input_json: "{\"q\":\"history\"}".into(),
                    output: Some("done".into()),
                }],
            }],
            &RuntimeTranscriptSemantics {
                pruning: TranscriptPruningMode::Off,
                pruning_policy: RuntimePruningPolicy::default(),
                compaction: TranscriptCompactionMode::RawTranscriptOnly,
            },
        );

        assert_eq!(
            lines,
            vec![
                "[assistant]: [@codex]: checking".to_string(),
                "[tool_call:search#tool-42]: {\"q\":\"history\"}".to_string(),
                "[tool_result:search#tool-42]: done".to_string()
            ]
        );
    }

    #[test]
    fn render_history_lines_soft_trims_old_tool_results_but_keeps_recent_assistants() {
        let long_output = "x".repeat(5000);
        let lines = render_history_lines(
            &[
                RuntimeHistoryMessage {
                    role: "assistant".into(),
                    content: "older".into(),
                    sender: None,
                    tool_calls: vec![RuntimeToolCall {
                        tool_call_id: Some("old".into()),
                        name: "search".into(),
                        input_json: "{\"q\":\"old\"}".into(),
                        output: Some(long_output.clone()),
                    }],
                },
                RuntimeHistoryMessage {
                    role: "assistant".into(),
                    content: "recent-a".into(),
                    sender: None,
                    tool_calls: vec![RuntimeToolCall {
                        tool_call_id: Some("recent-a".into()),
                        name: "search".into(),
                        input_json: "{\"q\":\"recent-a\"}".into(),
                        output: Some(long_output.clone()),
                    }],
                },
                RuntimeHistoryMessage {
                    role: "assistant".into(),
                    content: "recent-b".into(),
                    sender: None,
                    tool_calls: vec![RuntimeToolCall {
                        tool_call_id: Some("recent-b".into()),
                        name: "search".into(),
                        input_json: "{\"q\":\"recent-b\"}".into(),
                        output: Some(long_output.clone()),
                    }],
                },
                RuntimeHistoryMessage {
                    role: "assistant".into(),
                    content: "recent-c".into(),
                    sender: None,
                    tool_calls: vec![RuntimeToolCall {
                        tool_call_id: Some("recent-c".into()),
                        name: "search".into(),
                        input_json: "{\"q\":\"recent-c\"}".into(),
                        output: Some(long_output.clone()),
                    }],
                },
            ],
            &RuntimeTranscriptSemantics {
                pruning: TranscriptPruningMode::RequestLocal,
                pruning_policy: RuntimePruningPolicy::default(),
                compaction: TranscriptCompactionMode::RawTranscriptOnly,
            },
        );

        let old_tool_result = lines
            .iter()
            .find(|line| line.starts_with("[tool_result:search#old]: "))
            .unwrap();
        assert!(old_tool_result.contains("[tool result pruned; omitted"));

        let recent_tool_result = lines
            .iter()
            .find(|line| line.starts_with("[tool_result:search#recent-c]: "))
            .unwrap();
        assert!(!recent_tool_result.contains("[tool result pruned; omitted"));
    }

    #[test]
    fn runtime_context_defaults_to_pruning_off_and_raw_transcript_only() {
        let context = RuntimeContext::default();
        assert_eq!(
            context.transcript_semantics.pruning,
            TranscriptPruningMode::Off
        );
        assert_eq!(
            context.transcript_semantics.pruning_policy,
            RuntimePruningPolicy::default()
        );
        assert_eq!(
            context.transcript_semantics.compaction,
            TranscriptCompactionMode::RawTranscriptOnly
        );
    }

    #[test]
    fn pruning_policy_serializes_as_contract_data() {
        let semantics = RuntimeTranscriptSemantics {
            pruning: TranscriptPruningMode::RequestLocal,
            pruning_policy: RuntimePruningPolicy {
                keep_last_assistants: 2,
                min_prunable_tool_chars: 1234,
                soft_trim_head_chars: 100,
                soft_trim_tail_chars: 200,
            },
            compaction: TranscriptCompactionMode::RawTranscriptOnly,
        };

        let json = serde_json::to_string(&semantics).unwrap();
        assert!(json.contains("\"keep_last_assistants\":2"));
        assert!(json.contains("\"min_prunable_tool_chars\":1234"));
    }

    #[test]
    fn runtime_session_spec_round_trips_external_mcp_servers() {
        let spec = RuntimeSessionSpec {
            backend_id: "native-main".into(),
            participant_name: None,
            session_key: qai_protocol::SessionKey::new("ws", "user:test"),
            role: RuntimeRole::Solo,
            workspace_dir: None,
            prompt_text: "hello".into(),
            tool_surface: ToolSurfaceSpec::default(),
            approval_mode: Default::default(),
            tool_bridge_url: Some("http://127.0.0.1:3000/sse".into()),
            external_mcp_servers: vec![
                ExternalMcpServerSpec {
                    name: "filesystem".into(),
                    transport: ExternalMcpTransport::Sse {
                        url: "http://127.0.0.1:3001/sse".into(),
                    },
                },
                ExternalMcpServerSpec {
                    name: "github".into(),
                    transport: ExternalMcpTransport::Sse {
                        url: "http://127.0.0.1:3002/sse".into(),
                    },
                },
            ],
            team_tool_url: Some("http://127.0.0.1:3000/runtime/team-tools".into()),
            provider_profile: None,
            backend_session_id: None,
            context: RuntimeContext::default(),
        };

        let json = serde_json::to_string(&spec).unwrap();
        let round_trip: RuntimeSessionSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(round_trip.external_mcp_servers, spec.external_mcp_servers);
        assert_eq!(round_trip.team_tool_url, spec.team_tool_url);
    }

    #[test]
    fn render_history_lines_keeps_full_tool_results_when_pruning_is_off() {
        let output = "z".repeat(5000);
        let lines = render_history_lines(
            &[RuntimeHistoryMessage {
                role: "assistant".into(),
                content: "older".into(),
                sender: None,
                tool_calls: vec![RuntimeToolCall {
                    tool_call_id: Some("call-7".into()),
                    name: "read".into(),
                    input_json: "{\"path\":\"big.txt\"}".into(),
                    output: Some(output.clone()),
                }],
            }],
            &RuntimeTranscriptSemantics {
                pruning: TranscriptPruningMode::Off,
                pruning_policy: RuntimePruningPolicy::default(),
                compaction: TranscriptCompactionMode::RawTranscriptOnly,
            },
        );

        let rendered = lines
            .iter()
            .find(|line| line.starts_with("[tool_result:read#call-7]: "))
            .unwrap();
        assert!(!rendered.contains("[tool result pruned; omitted"));
        assert!(rendered.ends_with(&output));
    }

    #[test]
    fn render_runtime_prompt_includes_task_team_and_memory_sections() {
        let spec = RuntimeSessionSpec {
            backend_id: "native-main".into(),
            participant_name: Some("worker".into()),
            session_key: qai_protocol::SessionKey::new("lark", "group:test"),
            role: RuntimeRole::Specialist,
            workspace_dir: None,
            prompt_text: String::new(),
            tool_surface: ToolSurfaceSpec::default(),
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: None,
            provider_profile: None,
            backend_session_id: None,
            context: RuntimeContext {
                task_reminder: Some("T1 implement jwt".into()),
                team_manifest: Some("Leader: claude\nSpecialist: codex".into()),
                memory_summary: Some("context summary".into()),
                agent_memory: Some("private specialist note".into()),
                workspace_native_files: vec!["AGENTS.md".into(), "TEAM.md".into()],
                user_input: Some("开始".into()),
                ..RuntimeContext::default()
            },
        };

        let prompt = render_runtime_prompt(&spec);
        assert!(prompt.contains("当前任务"));
        assert!(prompt.contains("团队职责"));
        assert!(prompt.contains("任务背景（团队上下文）"));
        assert!(prompt.contains("工作区原生上下文文件"));
        assert!(prompt.contains("AGENTS.md"));
        assert!(prompt.contains("开始"));
        assert!(!prompt.contains("长期记忆"));
    }

    #[test]
    fn render_runtime_prompt_includes_agent_memory_for_non_specialist() {
        let spec = RuntimeSessionSpec {
            backend_id: "native-main".into(),
            participant_name: Some("leader".into()),
            session_key: qai_protocol::SessionKey::new("lark", "group:test"),
            role: RuntimeRole::Leader,
            workspace_dir: None,
            prompt_text: String::new(),
            tool_surface: ToolSurfaceSpec::default(),
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: None,
            provider_profile: None,
            backend_session_id: None,
            context: RuntimeContext {
                agent_memory: Some("long term reviewer memory".into()),
                user_input: Some("继续".into()),
                ..RuntimeContext::default()
            },
        };

        let prompt = render_runtime_prompt(&spec);
        assert!(prompt.contains("长期记忆"));
        assert!(prompt.contains("long term reviewer memory"));
        assert!(prompt.contains("继续"));
    }
}
