use serde::{Deserialize, Serialize};

use super::milestone::TeamMilestoneEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TeamPublicUpdatesMode {
    #[default]
    Minimal,
    Normal,
    Verbose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamMilestoneVisibility {
    LeadOnly,
    Critical,
    Verbose,
}

pub fn milestone_visibility(event: &TeamMilestoneEvent) -> TeamMilestoneVisibility {
    match event {
        TeamMilestoneEvent::LeadMessage { .. } => TeamMilestoneVisibility::LeadOnly,
        TeamMilestoneEvent::TaskBlocked { .. }
        | TeamMilestoneEvent::TaskFailed { .. }
        | TeamMilestoneEvent::AllTasksDone => TeamMilestoneVisibility::Critical,
        TeamMilestoneEvent::TaskDispatched { .. }
        | TeamMilestoneEvent::TaskCheckpoint { .. }
        | TeamMilestoneEvent::TaskSubmitted { .. }
        | TeamMilestoneEvent::TaskDone { .. }
        | TeamMilestoneEvent::TasksUnlocked { .. } => TeamMilestoneVisibility::Verbose,
    }
}

pub fn milestone_is_public(event: &TeamMilestoneEvent, mode: TeamPublicUpdatesMode) -> bool {
    match milestone_visibility(event) {
        TeamMilestoneVisibility::LeadOnly => true,
        TeamMilestoneVisibility::Critical => {
            matches!(
                mode,
                TeamPublicUpdatesMode::Normal | TeamPublicUpdatesMode::Verbose
            )
        }
        TeamMilestoneVisibility::Verbose => matches!(mode, TeamPublicUpdatesMode::Verbose),
    }
}

pub fn milestone_dedupe_key(event: &TeamMilestoneEvent) -> Option<String> {
    Some(match event {
        TeamMilestoneEvent::LeadMessage { text } => format!("lead_message:{text}"),
        TeamMilestoneEvent::TaskDispatched { task_id, .. } => {
            format!("task_dispatched:{task_id}")
        }
        TeamMilestoneEvent::TaskCheckpoint {
            task_id,
            agent,
            note,
        } => format!("task_checkpoint:{task_id}:{agent}:{note}"),
        TeamMilestoneEvent::TaskSubmitted { task_id, agent, .. } => {
            format!("task_submitted:{task_id}:{agent}")
        }
        TeamMilestoneEvent::TaskBlocked {
            task_id,
            agent,
            reason,
            ..
        } => format!("task_blocked:{task_id}:{agent}:{reason}"),
        TeamMilestoneEvent::TaskFailed {
            task_id,
            agent,
            reason,
        } => {
            format!("task_failed:{task_id}:{agent}:{reason}")
        }
        TeamMilestoneEvent::TaskDone {
            task_id,
            agent,
            done_count,
            total,
            ..
        } => format!("task_done:{task_id}:{agent}:{done_count}:{total}"),
        TeamMilestoneEvent::TasksUnlocked { task_ids } => {
            format!("tasks_unlocked:{}", task_ids.join(","))
        }
        TeamMilestoneEvent::AllTasksDone => "all_tasks_done".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lead_message_is_public_in_all_modes() {
        let event = TeamMilestoneEvent::LeadMessage {
            text: "hello".to_string(),
        };
        assert!(milestone_is_public(&event, TeamPublicUpdatesMode::Minimal));
        assert!(milestone_is_public(&event, TeamPublicUpdatesMode::Normal));
        assert!(milestone_is_public(&event, TeamPublicUpdatesMode::Verbose));
    }

    #[test]
    fn all_tasks_done_is_hidden_in_minimal_and_public_in_normal() {
        let event = TeamMilestoneEvent::AllTasksDone;
        assert!(!milestone_is_public(&event, TeamPublicUpdatesMode::Minimal));
        assert!(milestone_is_public(&event, TeamPublicUpdatesMode::Normal));
        assert!(milestone_is_public(&event, TeamPublicUpdatesMode::Verbose));
    }

    #[test]
    fn task_failed_is_hidden_in_minimal_and_public_in_normal() {
        let event = TeamMilestoneEvent::TaskFailed {
            task_id: "T1".into(),
            agent: "agent-a".into(),
            reason: "boom".into(),
        };
        assert!(!milestone_is_public(&event, TeamPublicUpdatesMode::Minimal));
        assert!(milestone_is_public(&event, TeamPublicUpdatesMode::Normal));
        assert!(milestone_is_public(&event, TeamPublicUpdatesMode::Verbose));
    }

    #[test]
    fn task_checkpoint_is_public_only_in_verbose() {
        let event = TeamMilestoneEvent::TaskCheckpoint {
            task_id: "T1".into(),
            agent: "worker".into(),
            note: "half way".into(),
        };
        assert!(!milestone_is_public(&event, TeamPublicUpdatesMode::Minimal));
        assert!(!milestone_is_public(&event, TeamPublicUpdatesMode::Normal));
        assert!(milestone_is_public(&event, TeamPublicUpdatesMode::Verbose));
    }
}
