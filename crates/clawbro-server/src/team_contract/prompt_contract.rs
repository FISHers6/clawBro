use crate::agent_core::traits::AgentRole;

const CANONICAL_TEAM_SKILL_LEAD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/host-skills/team-lead/SKILL.md"
));

const CANONICAL_TEAM_SKILL_SPECIALIST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/host-skills/team-specialist/SKILL.md"
));

pub fn render_team_host_contract(agent_role: AgentRole) -> &'static str {
    match agent_role {
        AgentRole::Lead => {
            "## Host Team Contract\n\n\
ClawBro owns team coordination semantics at the host/runtime layer.\n\
Use canonical team tools and TEAM.md / AGENTS.md instructions as the source of truth for coordination behavior.\n\
Lead agents own delegation, execution start, acceptance, reopen decisions, and user-facing coordination.\n\
Before ending a team turn, record coordination progress or a terminal outcome with a canonical lead action.\n\
\n\
## Team Helper Contract\n\n\
For Claude Code / Codex / ACP-style backends, lead coordination is executed through the CLI bridge:\n\
- `clawbro team-helper create-task --title \"...\" --assignee claw`\n\
- `clawbro team-helper assign-task --task-id T001 --assignee claw`\n\
- `clawbro team-helper start-execution`\n\
- `clawbro team-helper get-task-status`\n\
- `clawbro team-helper post-update --message \"...\"`\n\
- `clawbro team-helper request-confirmation --plan-summary \"...\"`\n\
- `clawbro team-helper accept-task --task-id T001`\n\
- `clawbro team-helper reopen-task --task-id T001 --reason \"...\"`\n\
\n\
`clawbro team-helper` reads `CLAWBRO_TEAM_TOOL_URL` and `CLAWBRO_SESSION_REF` from the runtime environment automatically.\n\
Do not search for tokens, URLs, or hidden endpoints manually."
        }
        AgentRole::Specialist => {
            "## Host Team Contract\n\n\
ClawBro owns team coordination semantics at the host/runtime layer.\n\
Use canonical team tools and TEAM.md / AGENTS.md instructions as the source of truth for execution behavior.\n\
Specialist agents own task execution, checkpointing, help requests, blocking, and final submission.\n\
Before ending a team turn, record execution progress or a terminal outcome with a canonical specialist action.\n\
\n\
## Team Helper Contract\n\n\
For Claude Code / Codex / ACP-style backends, specialist coordination is executed through the CLI bridge:\n\
- `clawbro team-helper checkpoint-task --task-id T001 --note \"...\"`\n\
- `clawbro team-helper request-help --task-id T001 --message \"...\"`\n\
- `clawbro team-helper block-task --task-id T001 --reason \"...\"`\n\
- `clawbro team-helper submit-task-result --task-id T001 --summary \"...\" --result-markdown \"...\"`\n\
\n\
`clawbro team-helper` reads `CLAWBRO_TEAM_TOOL_URL` and `CLAWBRO_SESSION_REF` from the runtime environment automatically.\n\
Do not search for tokens, URLs, or hidden endpoints manually."
        }
        AgentRole::Solo => "",
    }
}

pub fn render_canonical_team_skill_injection(agent_role: AgentRole) -> &'static str {
    match agent_role {
        AgentRole::Lead => CANONICAL_TEAM_SKILL_LEAD,
        AgentRole::Specialist => CANONICAL_TEAM_SKILL_SPECIALIST,
        AgentRole::Solo => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lead_contract_does_not_expose_specialist_submit_flow() {
        let rendered = render_team_host_contract(AgentRole::Lead);
        assert!(rendered.contains("create-task"));
        assert!(rendered.contains("accept-task"));
        assert!(rendered.contains("request-confirmation"));
        assert!(!rendered.contains("submit-task-result"));
        assert!(!rendered.contains("checkpoint-task"));
    }

    #[test]
    fn specialist_contract_does_not_expose_lead_planning_flow() {
        let rendered = render_team_host_contract(AgentRole::Specialist);
        assert!(rendered.contains("submit-task-result"));
        assert!(rendered.contains("checkpoint-task"));
        assert!(!rendered.contains("create-task"));
        assert!(!rendered.contains("accept-task"));
        assert!(!rendered.contains("request-confirmation"));
    }
}
