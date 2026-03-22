use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::runtime::contract::RuntimeRole;

use super::schema::{tool_for_call, TeamTool, TeamToolCall};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamToolVisibility {
    pub role: RuntimeRole,
    pub visible: Vec<TeamTool>,
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
            TeamTool::BlockTask,
            TeamTool::RequestHelp,
        ],
    };

    TeamToolVisibility { role, visible }
}

pub fn ensure_team_call_allowed(role: RuntimeRole, call: &TeamToolCall) -> Result<()> {
    let tool = tool_for_call(call);
    let visibility = visible_team_tools_for_role(role);
    anyhow::ensure!(
        visibility.visible.contains(&tool),
        "team tool '{tool:?}' is not allowed for role '{role:?}'"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lead_visibility_contains_coordination_tools() {
        let visible = visible_team_tools_for_role(RuntimeRole::Leader).visible;
        assert!(visible.contains(&TeamTool::CreateTask));
        assert!(visible.contains(&TeamTool::AssignTask));
        assert!(visible.contains(&TeamTool::AcceptTask));
    }

    #[test]
    fn specialist_visibility_contains_execution_tools() {
        let visible = visible_team_tools_for_role(RuntimeRole::Specialist).visible;
        assert!(visible.contains(&TeamTool::CheckpointTask));
        assert!(visible.contains(&TeamTool::SubmitTaskResult));
        assert!(visible.contains(&TeamTool::RequestHelp));
    }

    #[test]
    fn unauthorized_call_is_rejected() {
        let err = ensure_team_call_allowed(
            RuntimeRole::Specialist,
            &TeamToolCall::CreateTask {
                id: Some("T001".into()),
                title: "illegal".into(),
                assignee: None,
                spec: None,
                deps: vec![],
                success_criteria: None,
            },
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("CreateTask"));
        assert!(err.contains("Specialist"));
    }
}
