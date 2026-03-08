use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    pub system_prompt: Option<String>,
    pub workspace_native_files: Vec<String>,
    pub memory_summary: Option<String>,
    pub agent_memory: Option<String>,
    pub team_manifest: Option<String>,
    pub task_reminder: Option<String>,
    pub history_lines: Vec<String>,
    pub user_input: Option<String>,
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
    /// ACP-family MCP bridge endpoint (typically SSE).
    pub tool_bridge_url: Option<String>,
    /// Family-agnostic synchronous Team Tool RPC endpoint.
    pub team_tool_url: Option<String>,
    pub context: RuntimeContext,
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
    TextDelta { text: String },
    ApprovalRequest(PermissionRequest),
    ToolCallback(TeamCallback),
    TurnComplete { full_text: String },
    TurnFailed { error: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnResult {
    pub full_text: String,
    pub events: Vec<RuntimeEvent>,
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
    parts.extend(
        session
            .context
            .history_lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .cloned(),
    );
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
            tool_bridge_url: Some("http://127.0.0.1:9999/sse".into()),
            team_tool_url: Some("http://127.0.0.1:9999/runtime/team-tools".into()),
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
            tool_bridge_url: None,
            team_tool_url: None,
            context: RuntimeContext {
                system_prompt: Some("system rules".into()),
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
    fn render_runtime_prompt_includes_task_team_and_memory_sections() {
        let spec = RuntimeSessionSpec {
            backend_id: "native-main".into(),
            participant_name: Some("worker".into()),
            session_key: qai_protocol::SessionKey::new("lark", "group:test"),
            role: RuntimeRole::Specialist,
            workspace_dir: None,
            prompt_text: String::new(),
            tool_surface: ToolSurfaceSpec::default(),
            tool_bridge_url: None,
            team_tool_url: None,
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
            tool_bridge_url: None,
            team_tool_url: None,
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
