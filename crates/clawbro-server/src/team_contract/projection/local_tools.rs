use crate::runtime::RuntimeRole;
use crate::team_contract::{visible_team_tools_for_role, TeamTool};

pub fn project_local_team_tools(
    role: RuntimeRole,
    allowed_team_tools: &[TeamTool],
) -> Vec<TeamTool> {
    let default_tools = visible_team_tools_for_role(role).visible;
    if allowed_team_tools.is_empty() {
        return default_tools;
    }
    default_tools
        .into_iter()
        .filter(|tool| allowed_team_tools.contains(tool))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_returns_default_role_tools_when_allowlist_is_empty() {
        let tools = project_local_team_tools(RuntimeRole::Leader, &[]);
        assert!(tools.contains(&TeamTool::CreateTask));
        assert!(tools.contains(&TeamTool::AcceptTask));
    }

    #[test]
    fn projection_filters_to_allowlist() {
        let tools = project_local_team_tools(
            RuntimeRole::Leader,
            &[TeamTool::PostUpdate, TeamTool::AcceptTask],
        );
        assert_eq!(tools, vec![TeamTool::PostUpdate, TeamTool::AcceptTask]);
    }
}
