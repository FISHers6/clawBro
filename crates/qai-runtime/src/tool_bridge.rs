use crate::contract::RuntimeRole;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TeamTool {
    CreateTask,
    StartExecution,
    RequestConfirmation,
    PostUpdate,
    GetTaskStatus,
    AssignTask,
    CheckpointTask,
    SubmitTaskResult,
    CompleteTask,
    AcceptTask,
    ReopenTask,
    BlockTask,
    RequestHelp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TeamToolCall {
    CreateTask {
        id: String,
        title: String,
        assignee: Option<String>,
        spec: Option<String>,
        deps: Vec<String>,
        success_criteria: Option<String>,
    },
    StartExecution,
    RequestConfirmation {
        plan_summary: String,
    },
    PostUpdate {
        message: String,
    },
    GetTaskStatus,
    AssignTask {
        task_id: String,
        new_assignee: String,
    },
    CheckpointTask {
        task_id: String,
        note: String,
        agent: Option<String>,
    },
    SubmitTaskResult {
        task_id: String,
        summary: String,
        result_markdown: Option<String>,
        agent: Option<String>,
    },
    AcceptTask {
        task_id: String,
        by: Option<String>,
    },
    ReopenTask {
        task_id: String,
        reason: String,
        by: Option<String>,
    },
    CompleteTask {
        task_id: String,
        note: String,
        result_markdown: Option<String>,
        agent: Option<String>,
    },
    BlockTask {
        task_id: String,
        reason: String,
        agent: Option<String>,
    },
    RequestHelp {
        task_id: String,
        message: String,
        agent: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamToolVisibility {
    pub role: RuntimeRole,
    pub visible: Vec<TeamTool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamToolRequest {
    pub session_key: qai_protocol::SessionKey,
    pub call: TeamToolCall,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeamToolResponse {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub payload: Option<Value>,
}

pub fn visible_team_tools_for_role(role: RuntimeRole) -> TeamToolVisibility {
    let visible = match role {
        RuntimeRole::Solo => vec![],
        RuntimeRole::Leader => vec![
            TeamTool::CreateTask,
            TeamTool::StartExecution,
            TeamTool::RequestConfirmation,
            TeamTool::PostUpdate,
            TeamTool::GetTaskStatus,
            TeamTool::AssignTask,
            TeamTool::AcceptTask,
            TeamTool::ReopenTask,
        ],
        RuntimeRole::Specialist => vec![
            TeamTool::CheckpointTask,
            TeamTool::SubmitTaskResult,
            TeamTool::CompleteTask,
            TeamTool::BlockTask,
            TeamTool::RequestHelp,
        ],
    };

    TeamToolVisibility { role, visible }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leader_visibility_contains_planning_and_acceptance_tools() {
        let policy = visible_team_tools_for_role(RuntimeRole::Leader);

        assert_eq!(policy.role, RuntimeRole::Leader);
        assert!(policy.visible.contains(&TeamTool::CreateTask));
        assert!(policy.visible.contains(&TeamTool::StartExecution));
        assert!(policy.visible.contains(&TeamTool::AcceptTask));
        assert!(policy.visible.contains(&TeamTool::ReopenTask));
        assert!(!policy.visible.contains(&TeamTool::SubmitTaskResult));
        assert!(!policy.visible.contains(&TeamTool::BlockTask));
    }

    #[test]
    fn specialist_visibility_contains_execution_tools_only() {
        let policy = visible_team_tools_for_role(RuntimeRole::Specialist);

        assert_eq!(policy.role, RuntimeRole::Specialist);
        assert!(policy.visible.contains(&TeamTool::CheckpointTask));
        assert!(policy.visible.contains(&TeamTool::SubmitTaskResult));
        assert!(policy.visible.contains(&TeamTool::CompleteTask));
        assert!(policy.visible.contains(&TeamTool::BlockTask));
        assert!(policy.visible.contains(&TeamTool::RequestHelp));
        assert!(!policy.visible.contains(&TeamTool::CreateTask));
        assert!(!policy.visible.contains(&TeamTool::AcceptTask));
    }

    #[test]
    fn solo_role_has_no_team_tools() {
        let policy = visible_team_tools_for_role(RuntimeRole::Solo);
        assert!(policy.visible.is_empty());
    }

    #[test]
    fn submit_task_result_call_preserves_agent_and_summary() {
        let call = TeamToolCall::SubmitTaskResult {
            task_id: "T001".into(),
            summary: "Implemented JWT middleware".into(),
            result_markdown: Some("# Result\n\nImplemented JWT middleware".into()),
            agent: Some("codex".into()),
        };

        match call {
            TeamToolCall::SubmitTaskResult {
                task_id,
                summary,
                result_markdown,
                agent,
            } => {
                assert_eq!(task_id, "T001");
                assert_eq!(summary, "Implemented JWT middleware");
                assert_eq!(
                    result_markdown.as_deref(),
                    Some("# Result\n\nImplemented JWT middleware")
                );
                assert_eq!(agent.as_deref(), Some("codex"));
            }
            other => panic!("unexpected call: {other:?}"),
        }
    }
}
