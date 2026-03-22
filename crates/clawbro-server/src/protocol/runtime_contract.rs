use serde::{Deserialize, Serialize};

pub use crate::team_contract::{TeamTool, TeamToolCall, TeamToolRequest, TeamToolResponse};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScheduleTool {
    CreateDelayReminder,
    CreateAtReminder,
    CreateEveryReminder,
    CreateCronReminder,
    CreateDelayAgentSchedule,
    CreateAtAgentSchedule,
    CreateEveryAgentSchedule,
    CreateCronAgentSchedule,
    ListSchedules,
    ListCurrentSessionSchedules,
    PauseSchedule,
    ResumeSchedule,
    DeleteSchedule,
    DeleteScheduleByName,
    ClearCurrentSessionSchedules,
    RunScheduleNow,
    ScheduleHistory,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::SessionKey;

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
