use serde::{Deserialize, Serialize};

use crate::protocol::SessionKey;

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
    AcceptTask,
    ReopenTask,
    BlockTask,
    RequestHelp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TeamToolCall {
    CreateTask {
        id: Option<String>,
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
pub struct TeamToolRequest {
    pub session_key: SessionKey,
    pub call: TeamToolCall,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeamToolResponse {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

pub fn tool_for_call(call: &TeamToolCall) -> TeamTool {
    match call {
        TeamToolCall::CreateTask { .. } => TeamTool::CreateTask,
        TeamToolCall::StartExecution => TeamTool::StartExecution,
        TeamToolCall::RequestConfirmation { .. } => TeamTool::RequestConfirmation,
        TeamToolCall::PostUpdate { .. } => TeamTool::PostUpdate,
        TeamToolCall::GetTaskStatus => TeamTool::GetTaskStatus,
        TeamToolCall::AssignTask { .. } => TeamTool::AssignTask,
        TeamToolCall::CheckpointTask { .. } => TeamTool::CheckpointTask,
        TeamToolCall::SubmitTaskResult { .. } => TeamTool::SubmitTaskResult,
        TeamToolCall::AcceptTask { .. } => TeamTool::AcceptTask,
        TeamToolCall::ReopenTask { .. } => TeamTool::ReopenTask,
        TeamToolCall::BlockTask { .. } => TeamTool::BlockTask,
        TeamToolCall::RequestHelp { .. } => TeamTool::RequestHelp,
    }
}

pub fn is_legacy_alias(call: &TeamToolCall) -> bool {
    let _ = call;
    false
}

pub fn canonical_progress_tools() -> &'static [TeamTool] {
    &[
        TeamTool::CheckpointTask,
        TeamTool::SubmitTaskResult,
        TeamTool::RequestHelp,
        TeamTool::BlockTask,
    ]
}

pub fn canonical_terminal_tools() -> &'static [TeamTool] {
    &[
        TeamTool::SubmitTaskResult,
        TeamTool::AcceptTask,
        TeamTool::ReopenTask,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_verb_sets_include_expected_tools() {
        assert!(canonical_progress_tools().contains(&TeamTool::CheckpointTask));
        assert!(canonical_progress_tools().contains(&TeamTool::SubmitTaskResult));
        assert!(canonical_terminal_tools().contains(&TeamTool::SubmitTaskResult));
        assert!(canonical_terminal_tools().contains(&TeamTool::AcceptTask));
        assert!(canonical_terminal_tools().contains(&TeamTool::ReopenTask));
    }

    #[test]
    fn no_team_call_is_marked_as_legacy_alias() {
        assert!(!is_legacy_alias(&TeamToolCall::SubmitTaskResult {
            task_id: "T001".into(),
            summary: "done".into(),
            result_markdown: Some("final".into()),
            agent: None,
        }));
    }

    #[test]
    fn team_tool_request_round_trips_through_json() {
        let request = TeamToolRequest {
            session_key: SessionKey::new("lark", "group:oc_x"),
            call: TeamToolCall::GetTaskStatus,
        };
        let json = serde_json::to_string(&request).unwrap();
        let decoded: TeamToolRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, request);
    }
}
