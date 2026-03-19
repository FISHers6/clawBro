use crate::runtime::contract::{RuntimeEvent, TeamCallback};
use crate::runtime::helper_contract::ParsedTeamHelperResult;
use anyhow::anyhow;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenClawLeadRunBinding {
    pub backend_id: String,
    pub participant_name: String,
    pub session_key: crate::protocol::SessionKey,
    pub team_id: String,
    pub turn_id: Option<String>,
    pub openclaw_session_key: String,
    pub run_id: Option<String>,
    pub helper_invocation_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OpenClawLeadBridge {
    binding: OpenClawLeadRunBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenClawLeadOutcome {
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
    TaskAccepted {
        task_id: String,
        by: String,
    },
    TaskReopened {
        task_id: String,
        reason: String,
        by: String,
    },
    PublicUpdatePosted {
        message: String,
    },
    CommandFailed {
        action: Option<String>,
        task_id: Option<String>,
        error: String,
    },
}

impl OpenClawLeadBridge {
    pub fn new(binding: OpenClawLeadRunBinding) -> Self {
        Self { binding }
    }

    pub fn binding(&self) -> &OpenClawLeadRunBinding {
        &self.binding
    }

    pub fn handle_helper_result(&self, value: &Value) -> anyhow::Result<RuntimeEvent> {
        let outcome = OpenClawLeadOutcome::from_helper_json(value)?;
        let callback = map_lead_outcome_to_team_callback(&self.binding, outcome)?;
        Ok(RuntimeEvent::ToolCallback(callback))
    }
}

impl OpenClawLeadOutcome {
    pub fn from_helper_json(value: &Value) -> anyhow::Result<Self> {
        let parsed = ParsedTeamHelperResult::from_json(value)?;
        let normalized_action = normalize_lead_action(parsed.action.as_str())?;
        if !parsed.ok {
            return Ok(Self::CommandFailed {
                action: Some(normalized_action.to_string()),
                task_id: parsed.task_id,
                error: value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("OpenClaw lead helper failed")
                    .to_string(),
            });
        }

        match normalized_action {
            "create_task" => Ok(Self::TaskCreated {
                task_id: required_string_field(value, "task_id")?,
                title: required_string_field(value, "title")?,
                assignee: required_string_field(value, "assignee")?,
            }),
            "assign_task" => Ok(Self::TaskAssigned {
                task_id: required_string_field(value, "task_id")?,
                assignee: required_string_field(value, "assignee")?,
            }),
            "start_execution" => Ok(Self::ExecutionStarted),
            "accept_task" => Ok(Self::TaskAccepted {
                task_id: required_string_field(value, "task_id")?,
                by: optional_string_field(value, "by").unwrap_or_else(|| "leader".to_string()),
            }),
            "reopen_task" => Ok(Self::TaskReopened {
                task_id: required_string_field(value, "task_id")?,
                reason: required_string_field(value, "reason")?,
                by: optional_string_field(value, "by").unwrap_or_else(|| "leader".to_string()),
            }),
            "post_update" => Ok(Self::PublicUpdatePosted {
                message: required_string_field(value, "message")?,
            }),
            other => Err(anyhow!("unsupported lead helper action: {other}")),
        }
    }
}

fn normalize_lead_action(action: &str) -> anyhow::Result<&'static str> {
    match action {
        "create_task" => Ok("create_task"),
        "assign_task" => Ok("assign_task"),
        "start_execution" => Ok("start_execution"),
        "accept_task" => Ok("accept_task"),
        "reopen_task" => Ok("reopen_task"),
        "post_update" => Ok("post_update"),
        other => Err(anyhow!("unsupported lead helper action: {other}")),
    }
}

fn required_string_field(value: &Value, key: &str) -> anyhow::Result<String> {
    crate::runtime::helper_contract::required_string_field(value, key, "lead helper result")
}

fn optional_string_field(value: &Value, key: &str) -> Option<String> {
    crate::runtime::helper_contract::optional_string_field(value, key)
}

fn map_lead_outcome_to_team_callback(
    binding: &OpenClawLeadRunBinding,
    outcome: OpenClawLeadOutcome,
) -> anyhow::Result<TeamCallback> {
    match outcome {
        OpenClawLeadOutcome::TaskCreated {
            task_id,
            title,
            assignee,
        } => Ok(TeamCallback::TaskCreated {
            task_id,
            title,
            assignee,
        }),
        OpenClawLeadOutcome::TaskAssigned { task_id, assignee } => {
            Ok(TeamCallback::TaskAssigned { task_id, assignee })
        }
        OpenClawLeadOutcome::ExecutionStarted => Ok(TeamCallback::ExecutionStarted),
        OpenClawLeadOutcome::TaskAccepted { task_id, by } => {
            Ok(TeamCallback::TaskAccepted { task_id, by })
        }
        OpenClawLeadOutcome::TaskReopened {
            task_id,
            reason,
            by,
        } => Ok(TeamCallback::TaskReopened {
            task_id,
            reason,
            by,
        }),
        OpenClawLeadOutcome::PublicUpdatePosted { message } => {
            Ok(TeamCallback::PublicUpdatePosted { message })
        }
        OpenClawLeadOutcome::CommandFailed {
            action,
            task_id,
            error,
        } => Err(anyhow!(
            "openclaw lead helper command failed (participant={}, task_id={}, action={}): {}",
            binding.participant_name,
            task_id.as_deref().unwrap_or("-"),
            action.as_deref().unwrap_or("-"),
            error
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::render_team_helper_success;
    use serde_json::{Map, Value};

    #[test]
    fn parses_create_task_outcome() {
        let json = render_team_helper_success(
            "create_task",
            Map::from_iter([
                ("task_id".into(), Value::String("T001".into())),
                ("title".into(), Value::String("Implement JWT".into())),
                ("assignee".into(), Value::String("worker".into())),
            ]),
        );

        let outcome = OpenClawLeadOutcome::from_helper_json(&json).unwrap();
        assert!(matches!(
            outcome,
            OpenClawLeadOutcome::TaskCreated { ref task_id, ref title, ref assignee }
                if task_id == "T001" && title == "Implement JWT" && assignee == "worker"
        ));
    }

    #[test]
    fn parses_reopen_task_outcome() {
        let json = render_team_helper_success(
            "reopen_task",
            Map::from_iter([
                ("task_id".into(), Value::String("T001".into())),
                ("reason".into(), Value::String("tests missing".into())),
                ("by".into(), Value::String("leader".into())),
            ]),
        );

        let outcome = OpenClawLeadOutcome::from_helper_json(&json).unwrap();
        assert!(matches!(
            outcome,
            OpenClawLeadOutcome::TaskReopened { ref task_id, ref reason, ref by }
                if task_id == "T001" && reason == "tests missing" && by == "leader"
        ));
    }

    #[test]
    fn lead_helper_failure_becomes_command_failed() {
        let json = crate::runtime::render_team_helper_failure(
            "assign_task",
            Some("T001"),
            "missing assignee",
        );

        let outcome = OpenClawLeadOutcome::from_helper_json(&json).unwrap();
        assert!(matches!(
            outcome,
            OpenClawLeadOutcome::CommandFailed { ref action, ref task_id, ref error }
                if action.as_deref() == Some("assign_task")
                    && task_id.as_deref() == Some("T001")
                    && error == "missing assignee"
        ));
    }

    #[test]
    fn rejects_unsupported_action() {
        let json = render_team_helper_success("request_confirmation", Map::new());

        let err = OpenClawLeadOutcome::from_helper_json(&json).unwrap_err();
        assert!(err.to_string().contains("unsupported lead helper action"));
    }

    fn binding() -> OpenClawLeadRunBinding {
        OpenClawLeadRunBinding {
            backend_id: "openclaw-lead".into(),
            participant_name: "leader".into(),
            session_key: crate::protocol::SessionKey::new("ws", "group:test"),
            team_id: "group".into(),
            turn_id: Some("turn-1".into()),
            openclaw_session_key: "openclaw:group:test".into(),
            run_id: Some("run-1".into()),
            helper_invocation_id: Some("helper-1".into()),
        }
    }

    #[test]
    fn create_task_outcome_maps_to_tool_callback() {
        let bridge = OpenClawLeadBridge::new(binding());
        let value = render_team_helper_success(
            "create_task",
            Map::from_iter([
                ("task_id".into(), Value::String("T001".into())),
                ("title".into(), Value::String("Implement JWT".into())),
                ("assignee".into(), Value::String("worker".into())),
            ]),
        );

        let event = bridge.handle_helper_result(&value).unwrap();
        assert!(matches!(
            event,
            RuntimeEvent::ToolCallback(TeamCallback::TaskCreated {
                ref task_id,
                ref title,
                ref assignee,
            }) if task_id == "T001" && title == "Implement JWT" && assignee == "worker"
        ));
    }
}
