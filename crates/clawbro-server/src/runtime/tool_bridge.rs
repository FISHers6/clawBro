use serde::{Deserialize, Serialize};

use crate::runtime::contract::RuntimeRole;

pub use crate::protocol::{
    ScheduleTool, TeamTool, TeamToolCall, TeamToolRequest, TeamToolResponse,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamToolVisibility {
    pub role: RuntimeRole,
    pub visible: Vec<TeamTool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleToolVisibility {
    pub role: RuntimeRole,
    pub visible: Vec<ScheduleTool>,
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

pub fn visible_schedule_tools_for_role(role: RuntimeRole) -> ScheduleToolVisibility {
    let visible = match role {
        RuntimeRole::Solo | RuntimeRole::Leader => vec![
            ScheduleTool::CreateDelayReminder,
            ScheduleTool::CreateAtReminder,
            ScheduleTool::CreateEveryReminder,
            ScheduleTool::CreateCronReminder,
            ScheduleTool::CreateDelayAgentSchedule,
            ScheduleTool::CreateAtAgentSchedule,
            ScheduleTool::CreateEveryAgentSchedule,
            ScheduleTool::CreateCronAgentSchedule,
            ScheduleTool::ListSchedules,
            ScheduleTool::ListCurrentSessionSchedules,
            ScheduleTool::PauseSchedule,
            ScheduleTool::ResumeSchedule,
            ScheduleTool::DeleteSchedule,
            ScheduleTool::DeleteScheduleByName,
            ScheduleTool::ClearCurrentSessionSchedules,
            ScheduleTool::RunScheduleNow,
            ScheduleTool::ScheduleHistory,
        ],
        RuntimeRole::Specialist => vec![],
    };
    ScheduleToolVisibility { role, visible }
}
