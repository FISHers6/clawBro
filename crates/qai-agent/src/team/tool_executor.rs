use super::orchestrator::{TeamOrchestrator, TeamState};
use super::registry::CreateTask;
use anyhow::Result;
use qai_protocol::SessionKey;
use qai_runtime::{
    visible_team_tools_for_role, RuntimeRole, TeamTool, TeamToolCall, TeamToolResponse,
};
use std::sync::Arc;

pub fn resolve_team_tool_role(
    session_key: &SessionKey,
    team_orch: &Arc<TeamOrchestrator>,
) -> Result<RuntimeRole> {
    if team_orch.lead_session_key.get() == Some(session_key) {
        return Ok(RuntimeRole::Leader);
    }
    if session_key.channel == "specialist"
        && session_key
            .scope
            .starts_with(&format!("{}:", team_orch.session.team_id))
    {
        return Ok(RuntimeRole::Specialist);
    }
    anyhow::bail!(
        "session '{}' is not authorized for team tool access in team '{}'",
        session_key.scope,
        team_orch.session.team_id
    );
}

pub fn resolve_claimed_agent(
    team_orch: &TeamOrchestrator,
    task_id: &str,
    explicit: Option<&str>,
) -> String {
    explicit
        .map(ToOwned::to_owned)
        .or_else(|| {
            team_orch
                .registry
                .get_task(task_id)
                .ok()
                .flatten()
                .and_then(|t| {
                    t.status_raw
                        .strip_prefix("claimed:")
                        .and_then(|s| s.split(':').next())
                        .map(|s| s.to_string())
                })
        })
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn ensure_team_tool_allowed(role: RuntimeRole, call: &TeamToolCall) -> Result<()> {
    let tool = match call {
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
    };
    let visibility = visible_team_tools_for_role(role);
    anyhow::ensure!(
        visibility.visible.contains(&tool),
        "team tool '{tool:?}' is not allowed for role '{role:?}'"
    );
    Ok(())
}

pub async fn execute_team_tool_call(
    team_orch: Arc<TeamOrchestrator>,
    role: RuntimeRole,
    call: TeamToolCall,
) -> Result<TeamToolResponse> {
    ensure_team_tool_allowed(role, &call)?;

    let response = match call {
        TeamToolCall::CreateTask {
            id,
            title,
            assignee,
            spec,
            deps,
            success_criteria,
        } => TeamToolResponse {
            ok: true,
            message: team_orch.register_task(CreateTask {
                id,
                title,
                assignee_hint: assignee,
                deps,
                timeout_secs: 1800,
                spec,
                success_criteria,
            })?,
            payload: None,
        },
        TeamToolCall::StartExecution => TeamToolResponse {
            ok: true,
            message: team_orch.activate().await?,
            payload: None,
        },
        TeamToolCall::RequestConfirmation { plan_summary } => {
            let formatted = format!("**Plan for confirmation:**\n\n{}", plan_summary);
            team_orch.post_message(&formatted);
            *team_orch.team_state_inner.lock().unwrap() = TeamState::AwaitingConfirm;
            TeamToolResponse {
                ok: true,
                message: "Confirmation requested. Waiting for user reply.".to_string(),
                payload: None,
            }
        }
        TeamToolCall::PostUpdate { message } => {
            team_orch.post_message(&message);
            TeamToolResponse {
                ok: true,
                message: "Posted.".to_string(),
                payload: None,
            }
        }
        TeamToolCall::GetTaskStatus => {
            let tasks = team_orch.registry.all_tasks()?;
            let arr: Vec<serde_json::Value> = tasks
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "id": t.id,
                        "title": t.title,
                        "status": t.status_raw,
                        "assignee": t.assignee_hint,
                        "deps": t.deps(),
                        "retry_count": t.retry_count,
                        "completion_note": t.completion_note,
                    })
                })
                .collect();
            let payload = serde_json::Value::Array(arr.clone());
            TeamToolResponse {
                ok: true,
                message: serde_json::to_string_pretty(&arr)?,
                payload: Some(payload),
            }
        }
        TeamToolCall::AssignTask {
            task_id,
            new_assignee,
        } => {
            team_orch.registry.reassign_task(&task_id, &new_assignee)?;
            TeamToolResponse {
                ok: true,
                message: format!("Task {} reassigned to {}.", task_id, new_assignee),
                payload: None,
            }
        }
        TeamToolCall::CheckpointTask {
            task_id,
            note,
            agent,
        } => {
            let agent = resolve_claimed_agent(&team_orch, &task_id, agent.as_deref());
            team_orch.handle_specialist_checkpoint(&task_id, &agent, &note)?;
            TeamToolResponse {
                ok: true,
                message: format!("Checkpoint recorded for task {}.", task_id),
                payload: None,
            }
        }
        TeamToolCall::SubmitTaskResult {
            task_id,
            summary,
            agent,
        } => {
            let agent = resolve_claimed_agent(&team_orch, &task_id, agent.as_deref());
            team_orch.handle_specialist_submitted(&task_id, &agent, &summary)?;
            TeamToolResponse {
                ok: true,
                message: format!("Task {} submitted for review.", task_id),
                payload: None,
            }
        }
        TeamToolCall::AcceptTask { task_id, by } => {
            let by = by.as_deref().unwrap_or("leader");
            team_orch.accept_submitted_task(&task_id, by)?;
            TeamToolResponse {
                ok: true,
                message: format!("Task {} accepted by {}.", task_id, by),
                payload: None,
            }
        }
        TeamToolCall::ReopenTask {
            task_id,
            reason,
            by,
        } => {
            let by = by.as_deref().unwrap_or("leader");
            team_orch.reopen_submitted_task(&task_id, &reason, by)?;
            TeamToolResponse {
                ok: true,
                message: format!("Task {} reopened by {}.", task_id, by),
                payload: None,
            }
        }
        TeamToolCall::BlockTask {
            task_id,
            reason,
            agent,
        } => {
            let agent = resolve_claimed_agent(&team_orch, &task_id, agent.as_deref());
            if !team_orch
                .registry
                .is_claimed_by(&task_id, &agent)
                .unwrap_or(false)
            {
                anyhow::bail!("task '{}' is not currently claimed by '{}'", task_id, agent);
            }
            team_orch.handle_specialist_blocked(&task_id, &agent, &reason)?;
            TeamToolResponse {
                ok: true,
                message: format!("Task {} reported as blocked: {}", task_id, reason),
                payload: None,
            }
        }
        TeamToolCall::RequestHelp {
            task_id,
            message,
            agent,
        } => {
            let agent = resolve_claimed_agent(&team_orch, &task_id, agent.as_deref());
            if !team_orch
                .registry
                .is_claimed_by(&task_id, &agent)
                .unwrap_or(false)
            {
                anyhow::bail!("task '{}' is not currently claimed by '{}'", task_id, agent);
            }
            team_orch.handle_specialist_help_requested(&task_id, &agent, &message)?;
            TeamToolResponse {
                ok: true,
                message: format!("Help request sent for task {}.", task_id),
                payload: None,
            }
        }
    };

    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::{heartbeat::DispatchFn, registry::TaskRegistry, session::TeamSession};
    use tempfile::tempdir;

    fn make_orchestrator() -> Arc<TeamOrchestrator> {
        let tmp = tempdir().unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir(
            "team-tool-exec",
            tmp.path().to_path_buf(),
        ));
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        orch.set_lead_session_key(SessionKey::new("lark", "group:team-tool-exec"));
        orch
    }

    #[tokio::test]
    async fn executor_registers_task_through_canonical_business_path() {
        let orch = make_orchestrator();

        let response = execute_team_tool_call(
            Arc::clone(&orch),
            RuntimeRole::Leader,
            TeamToolCall::CreateTask {
                id: "T500".into(),
                title: "wire adapter".into(),
                assignee: Some("codex".into()),
                spec: None,
                deps: vec![],
                success_criteria: None,
            },
        )
        .await
        .unwrap();

        assert!(response.ok);
        assert!(response.message.contains("T500"));
        let task = orch.registry.get_task("T500").unwrap().unwrap();
        assert_eq!(task.title, "wire adapter");
    }

    #[test]
    fn executor_rejects_tool_not_visible_for_role() {
        let err = ensure_team_tool_allowed(
            RuntimeRole::Specialist,
            &TeamToolCall::CreateTask {
                id: "T501".into(),
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
