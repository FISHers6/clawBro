use serde::{Deserialize, Serialize};

use crate::contract::RuntimeRole;

pub use qai_protocol::{TeamTool, TeamToolCall, TeamToolRequest, TeamToolResponse};

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
            TeamTool::CompleteTask,
            TeamTool::BlockTask,
            TeamTool::RequestHelp,
        ],
    };

    TeamToolVisibility { role, visible }
}
