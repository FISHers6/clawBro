use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecialistActionKind {
    Submitted,
    Done,
    Checkpoint,
    HelpRequested,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecialistActionRecord {
    pub task_id: String,
    pub agent: String,
    pub kind: SpecialistActionKind,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialistTurnOutcome {
    TerminalSubmitted,
    TerminalDone,
    NonTerminalCheckpoint,
    NonTerminalHelpRequested,
    Blocked,
    MissingCompletion,
}

pub fn classify_specialist_turn(
    records: &[SpecialistActionRecord],
    task_id: &str,
    agent: &str,
    started_at: DateTime<Utc>,
) -> SpecialistTurnOutcome {
    let mut saw_checkpoint = false;
    let mut saw_help = false;
    let mut saw_blocked = false;

    for record in records {
        if record.task_id != task_id || record.agent != agent || record.at < started_at {
            continue;
        }
        match record.kind {
            SpecialistActionKind::Submitted => return SpecialistTurnOutcome::TerminalSubmitted,
            SpecialistActionKind::Done => return SpecialistTurnOutcome::TerminalDone,
            SpecialistActionKind::Blocked => saw_blocked = true,
            SpecialistActionKind::Checkpoint => saw_checkpoint = true,
            SpecialistActionKind::HelpRequested => saw_help = true,
        }
    }

    if saw_blocked {
        SpecialistTurnOutcome::Blocked
    } else if saw_checkpoint {
        SpecialistTurnOutcome::NonTerminalCheckpoint
    } else if saw_help {
        SpecialistTurnOutcome::NonTerminalHelpRequested
    } else {
        SpecialistTurnOutcome::MissingCompletion
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    fn record(
        task_id: &str,
        agent: &str,
        kind: SpecialistActionKind,
        at: DateTime<Utc>,
    ) -> SpecialistActionRecord {
        SpecialistActionRecord {
            task_id: task_id.to_string(),
            agent: agent.to_string(),
            kind,
            at,
        }
    }

    #[test]
    fn submitted_beats_checkpoint() {
        let started_at = Utc::now();
        let records = vec![
            record(
                "T1",
                "worker",
                SpecialistActionKind::Checkpoint,
                started_at + TimeDelta::milliseconds(1),
            ),
            record(
                "T1",
                "worker",
                SpecialistActionKind::Submitted,
                started_at + TimeDelta::milliseconds(2),
            ),
        ];
        assert_eq!(
            classify_specialist_turn(&records, "T1", "worker", started_at),
            SpecialistTurnOutcome::TerminalSubmitted
        );
    }

    #[test]
    fn done_beats_checkpoint() {
        let started_at = Utc::now();
        let records = vec![
            record(
                "T1",
                "worker",
                SpecialistActionKind::Checkpoint,
                started_at + TimeDelta::milliseconds(1),
            ),
            record(
                "T1",
                "worker",
                SpecialistActionKind::Done,
                started_at + TimeDelta::milliseconds(2),
            ),
        ];
        assert_eq!(
            classify_specialist_turn(&records, "T1", "worker", started_at),
            SpecialistTurnOutcome::TerminalDone
        );
    }

    #[test]
    fn blocked_is_distinct_from_nonterminal_progress() {
        let started_at = Utc::now();
        let records = vec![
            record(
                "T1",
                "worker",
                SpecialistActionKind::Checkpoint,
                started_at + TimeDelta::milliseconds(1),
            ),
            record(
                "T1",
                "worker",
                SpecialistActionKind::Blocked,
                started_at + TimeDelta::milliseconds(2),
            ),
        ];
        assert_eq!(
            classify_specialist_turn(&records, "T1", "worker", started_at),
            SpecialistTurnOutcome::Blocked
        );
    }

    #[test]
    fn no_matching_records_is_missing_completion() {
        let started_at = Utc::now();
        let records = vec![record(
            "T1",
            "worker",
            SpecialistActionKind::Checkpoint,
            started_at - TimeDelta::milliseconds(1),
        )];
        assert_eq!(
            classify_specialist_turn(&records, "T1", "worker", started_at),
            SpecialistTurnOutcome::MissingCompletion
        );
    }

    #[test]
    fn unrelated_task_and_agent_are_ignored() {
        let started_at = Utc::now();
        let records = vec![
            record(
                "OTHER",
                "worker",
                SpecialistActionKind::Submitted,
                started_at + TimeDelta::milliseconds(1),
            ),
            record(
                "T1",
                "other",
                SpecialistActionKind::Done,
                started_at + TimeDelta::milliseconds(2),
            ),
        ];
        assert_eq!(
            classify_specialist_turn(&records, "T1", "worker", started_at),
            SpecialistTurnOutcome::MissingCompletion
        );
    }
}
