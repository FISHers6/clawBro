use crate::runtime::contract::{RuntimeEvent, TeamCallback};
use crate::runtime::helper_contract::ParsedTeamHelperResult;
use crate::team_contract::projection::openclaw::normalize_openclaw_helper_action;
use anyhow::anyhow;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenClawRunBinding {
    pub backend_id: String,
    pub participant_name: String,
    pub team_id: String,
    pub task_id: String,
    pub session_key: crate::protocol::SessionKey,
    pub openclaw_session_key: String,
    pub run_id: Option<String>,
    pub helper_invocation_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OpenClawTeamBridge {
    binding: OpenClawRunBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenClawTeamOutcome {
    Checkpoint {
        task_id: String,
        summary: String,
    },
    Submitted {
        task_id: String,
        summary: String,
        artifacts: Vec<String>,
        result_markdown: Option<String>,
    },
    Blocked {
        task_id: String,
        reason: String,
    },
    HelpRequested {
        task_id: String,
        reason: String,
    },
    CommandFailed {
        task_id: Option<String>,
        action: Option<String>,
        error: String,
    },
}

impl OpenClawTeamBridge {
    pub fn new(binding: OpenClawRunBinding) -> Self {
        Self { binding }
    }

    pub fn binding(&self) -> &OpenClawRunBinding {
        &self.binding
    }

    pub fn handle_helper_result(&self, value: &Value) -> anyhow::Result<RuntimeEvent> {
        let outcome = OpenClawTeamOutcome::from_helper_json(value)?;
        let callback = map_outcome_to_team_callback(&self.binding, outcome).map_err(|err| {
            anyhow!(
                "OpenClaw team bridge callback failed for backend `{}`: {err}",
                self.binding.backend_id
            )
        })?;
        Ok(RuntimeEvent::ToolCallback(callback))
    }
}

impl OpenClawTeamOutcome {
    pub fn from_helper_json(value: &Value) -> anyhow::Result<Self> {
        let parsed = ParsedTeamHelperResult::from_json(value)?;
        let normalized_action = normalize_openclaw_helper_action(parsed.action.as_str())?;
        if !parsed.ok {
            return Ok(Self::CommandFailed {
                task_id: parsed.task_id,
                action: Some(normalized_action.to_string()),
                error: value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("OpenClaw team helper failed")
                    .to_string(),
            });
        }

        match normalized_action {
            "checkpoint_task" => Ok(Self::Checkpoint {
                task_id: required_task_id(value)?,
                summary: required_string_field(value, "note")?,
            }),
            "submit_task_result" => Ok(Self::Submitted {
                task_id: required_task_id(value)?,
                summary: required_string_field(value, "summary")?,
                artifacts: value
                    .get("artifacts")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(ToOwned::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                result_markdown: value
                    .get("result_markdown")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            }),
            "block_task" => Ok(Self::Blocked {
                task_id: required_task_id(value)?,
                reason: required_string_field(value, "reason")?,
            }),
            "request_help" => Ok(Self::HelpRequested {
                task_id: required_task_id(value)?,
                reason: required_string_field(value, "message")?,
            }),
            other => Err(anyhow!("unsupported helper action: {other}")),
        }
    }
}

pub fn map_outcome_to_team_callback(
    binding: &OpenClawRunBinding,
    outcome: OpenClawTeamOutcome,
) -> anyhow::Result<TeamCallback> {
    match outcome {
        OpenClawTeamOutcome::Checkpoint { task_id, summary } => Ok(TeamCallback::TaskCheckpoint {
            task_id,
            note: summary,
            agent: binding.participant_name.clone(),
        }),
        OpenClawTeamOutcome::Submitted {
            task_id,
            summary,
            result_markdown,
            ..
        } => Ok(TeamCallback::TaskSubmitted {
            task_id,
            summary,
            result_markdown,
            agent: binding.participant_name.clone(),
        }),
        OpenClawTeamOutcome::Blocked { task_id, reason } => Ok(TeamCallback::TaskBlocked {
            task_id,
            reason,
            agent: binding.participant_name.clone(),
        }),
        OpenClawTeamOutcome::HelpRequested { task_id, reason } => {
            Ok(TeamCallback::TaskHelpRequested {
                task_id,
                message: reason,
                agent: binding.participant_name.clone(),
            })
        }
        OpenClawTeamOutcome::CommandFailed {
            task_id,
            action,
            error,
        } => Err(anyhow!(
            "openclaw helper command failed (task_id={}, action={}): {}",
            task_id.as_deref().unwrap_or("-"),
            action.as_deref().unwrap_or("-"),
            error
        )),
    }
}

fn required_task_id(value: &Value) -> anyhow::Result<String> {
    required_string_field(value, "task_id")
}

fn required_string_field(value: &Value, key: &str) -> anyhow::Result<String> {
    crate::runtime::helper_contract::required_string_field(value, key, "team helper result")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::render_team_helper_success;
    use serde_json::{Map, Value};

    fn binding(task_id: &str) -> OpenClawRunBinding {
        OpenClawRunBinding {
            backend_id: "openclaw-main".into(),
            participant_name: "worker".into(),
            team_id: "team-1".into(),
            task_id: task_id.into(),
            session_key: crate::protocol::SessionKey::new("specialist", "team:openclaw"),
            openclaw_session_key: "openclaw:team:openclaw".into(),
            run_id: Some("run-1".into()),
            helper_invocation_id: Some("helper-1".into()),
        }
    }

    #[test]
    fn parses_submitted_outcome() {
        let json = render_team_helper_success(
            "submit_task_result",
            Map::from_iter([
                ("task_id".into(), Value::String("T001".into())),
                ("summary".into(), Value::String("done".into())),
                (
                    "artifacts".into(),
                    Value::Array(vec![Value::String("src/lib.rs".into())]),
                ),
            ]),
        );
        let outcome = OpenClawTeamOutcome::from_helper_json(&json).unwrap();
        assert!(matches!(
            outcome,
            OpenClawTeamOutcome::Submitted { ref task_id, ref summary, ref artifacts, .. }
                if task_id == "T001" && summary == "done" && artifacts == &vec!["src/lib.rs".to_string()]
        ));
    }

    #[test]
    fn parses_checkpoint_outcome() {
        let json = render_team_helper_success(
            "checkpoint_task",
            Map::from_iter([
                ("task_id".into(), Value::String("T001".into())),
                ("note".into(), Value::String("halfway".into())),
            ]),
        );
        let outcome = OpenClawTeamOutcome::from_helper_json(&json).unwrap();
        assert!(matches!(
            outcome,
            OpenClawTeamOutcome::Checkpoint { ref task_id, ref summary }
                if task_id == "T001" && summary == "halfway"
        ));
    }

    #[test]
    fn helper_failure_becomes_command_failed() {
        let json = crate::runtime::render_team_helper_failure(
            "submit_task_result",
            Some("T001"),
            "missing summary",
        );
        let outcome = OpenClawTeamOutcome::from_helper_json(&json).unwrap();
        assert!(matches!(
            outcome,
            OpenClawTeamOutcome::CommandFailed { ref task_id, ref action, ref error }
                if task_id.as_deref() == Some("T001")
                    && action.as_deref() == Some("submit_task_result")
                    && error == "missing summary"
        ));
    }

    #[test]
    fn submitted_outcome_maps_to_team_callback() {
        let callback = map_outcome_to_team_callback(
            &binding("T001"),
            OpenClawTeamOutcome::Submitted {
                task_id: "T001".into(),
                summary: "done".into(),
                artifacts: vec!["src/lib.rs".into()],
                result_markdown: None,
            },
        )
        .unwrap();
        assert!(matches!(
            callback,
            TeamCallback::TaskSubmitted { ref task_id, ref summary, ref agent, .. }
                if task_id == "T001" && summary == "done" && agent == "worker"
        ));
    }

    #[test]
    fn bridge_normalizes_helper_result_into_runtime_event() {
        let bridge = OpenClawTeamBridge::new(binding("T001"));
        let event = bridge
            .handle_helper_result(&render_team_helper_success(
                "submit_task_result",
                Map::from_iter([
                    ("task_id".into(), Value::String("T001".into())),
                    ("summary".into(), Value::String("done".into())),
                ]),
            ))
            .unwrap();
        assert!(matches!(
            event,
            RuntimeEvent::ToolCallback(TeamCallback::TaskSubmitted {
                ref task_id,
                ref summary,
                ref agent,
                ..
            }) if task_id == "T001" && summary == "done" && agent == "worker"
        ));
    }

    #[test]
    fn unsupported_action_is_rejected() {
        let json = render_team_helper_success(
            "accept_task",
            Map::from_iter([("task_id".into(), Value::String("T001".into()))]),
        );
        let err = OpenClawTeamOutcome::from_helper_json(&json).unwrap_err();
        assert!(err.to_string().contains("unsupported helper action"));
    }
}
