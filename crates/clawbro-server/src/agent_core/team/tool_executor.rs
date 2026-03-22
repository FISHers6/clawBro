use super::orchestrator::TeamOrchestrator;
use crate::protocol::SessionKey;
use crate::runtime::{RuntimeRole, TeamToolCall, TeamToolResponse};
use crate::team_contract::{ensure_team_call_allowed, execute_team_contract_call};
use anyhow::Result;
use std::sync::Arc;

pub fn resolve_team_tool_role(
    session_key: &SessionKey,
    team_orch: &Arc<TeamOrchestrator>,
) -> Result<RuntimeRole> {
    crate::team_contract::resolve_team_tool_role(session_key, team_orch)
}

pub fn resolve_claimed_agent(
    team_orch: &TeamOrchestrator,
    task_id: &str,
    explicit: Option<&str>,
) -> String {
    crate::team_contract::resolve_claimed_agent(team_orch, task_id, explicit)
}

pub fn ensure_team_tool_allowed(role: RuntimeRole, call: &TeamToolCall) -> Result<()> {
    ensure_team_call_allowed(role, call)
}

pub async fn execute_team_tool_call(
    team_orch: Arc<TeamOrchestrator>,
    role: RuntimeRole,
    call: TeamToolCall,
) -> Result<TeamToolResponse> {
    execute_team_contract_call(team_orch, role, call).await
}
