use crate::agent_core::memory::MemorySystem;
use crate::agent_core::persona::AgentPersona;
use crate::agent_core::prompt_builder::SystemPromptBuilder;
use crate::agent_core::routing::RosterMatchData;
use crate::agent_core::skill_paths::{
    agent_scoped_skills_dir, project_universal_skills_dir, workspace_private_skills_dir,
};
use crate::agent_core::team::orchestrator::TeamOrchestrator;
use crate::agent_core::traits::{AgentCtx, AgentRole, HistoryMsg};
use crate::protocol::{InboundMsg, MsgSource, ScheduleTool, SessionKey, TeamTool};
use crate::runtime::{visible_schedule_tools_for_role, RuntimeRole};
use crate::session::StoredMessage;
use crate::skills_internal::{PersonaSkillData, SkillLoader};
use crate::team_contract::{render_canonical_team_skill_injection, render_team_host_contract};
use dashmap::DashSet;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) struct ContextAssemblyRequest<'a> {
    pub session_id: Uuid,
    pub session_key: &'a SessionKey,
    pub inbound: &'a InboundMsg,
    pub recent_messages: &'a [StoredMessage],
    pub roster_match: Option<&'a RosterMatchData>,
    pub agent_role: AgentRole,
    pub task_reminder: Option<String>,
    pub session_team_orch: Option<&'a Arc<TeamOrchestrator>>,
    pub system_injection: &'a str,
    pub memory_system: Option<&'a Arc<MemorySystem>>,
    pub default_persona_dir: Option<PathBuf>,
    pub default_workspace: Option<PathBuf>,
    pub session_workspace: Option<PathBuf>,
    pub skill_loader_dirs: &'a [PathBuf],
    pub inject_prompt_skills: bool,
    pub initialized_persona_dirs: &'a DashSet<PathBuf>,
    pub team_tool_url: Option<String>,
    pub allowed_team_tools: Vec<TeamTool>,
}

pub(crate) struct ContextAssemblyResult {
    pub ctx: AgentCtx,
    pub persona_prefix: Option<String>,
    pub resolved_persona_dir: Option<PathBuf>,
}

const SCHEDULER_HOST_GUIDANCE: &str = "## Host Scheduler Contract\n\n\
ClawBro exposes scheduler support at the host/runtime layer in addition to the builtin scheduler skill.\n\
When a task requires delayed execution, reminders, recurring follow-up, heartbeat checks, or any other time-based orchestration, prefer the scheduler tools instead of promising a manual follow-up in plain text.\n\
Treat the scheduler skill and the host scheduler contract as complementary: the skill explains the workflow, while the host scheduler tools perform the actual scheduled action.\n\
Do not claim that something has been scheduled unless the scheduler tool call succeeded.";

pub(crate) async fn assemble_context(request: ContextAssemblyRequest<'_>) -> ContextAssemblyResult {
    let history = build_history(request.recent_messages);
    let frontstage_human_turn = is_frontstage_human_turn(
        request.inbound.source.clone(),
        request.session_key.channel.as_str(),
        request.agent_role,
    );
    let workspace_dir_resolved = request
        .session_workspace
        .or_else(|| request.roster_match.and_then(|rm| rm.workspace_dir.clone()))
        .or(request.default_workspace);

    let (skill_injection, first_persona) = load_skill_injection(
        workspace_dir_resolved.as_ref(),
        request.roster_match,
        request.skill_loader_dirs,
        request.inject_prompt_skills,
    );

    let resolved_persona_dir = resolve_persona_dir(
        request.roster_match,
        request.default_persona_dir,
        request.session_key,
        request.initialized_persona_dirs,
    );

    let canonical_shared_memory = if request.agent_role == AgentRole::Specialist {
        request
            .session_team_orch
            .map(|o| o.session.read_context_md())
            .filter(|content| !content.trim().is_empty())
    } else if let Some(ms) = request.memory_system {
        ms.store()
            .load_shared_memory(request.session_key)
            .await
            .ok()
            .filter(|content| !content.trim().is_empty())
    } else {
        None
    };

    let canonical_team_manifest =
        if matches!(request.agent_role, AgentRole::Lead | AgentRole::Specialist) {
            request
                .session_team_orch
                .map(|o| o.session.read_team_md())
                .filter(|manifest| !manifest.trim().is_empty())
        } else {
            None
        };

    let mut canonical_agent_memory = None;
    let mut system_injection = if resolved_persona_dir.is_some() {
        let (soul_md, identity_raw, agent_memory) =
            load_persona_layers(resolved_persona_dir.as_ref(), request.session_key);
        canonical_agent_memory = (!agent_memory.trim().is_empty()).then_some(agent_memory);
        let gateway_and_workspace_skills =
            combine_gateway_and_workspace_injection(request.system_injection, &skill_injection);
        let combined_skills =
            combine_persona_skills(&gateway_and_workspace_skills, first_persona.as_ref());
        SystemPromptBuilder {
            persona: first_persona.as_ref(),
            soul_md: &soul_md,
            identity_raw: &identity_raw,
            skills_injection: &combined_skills,
            shared_memory: "",
            agent_memory: "",
            shared_max_words: 300,
            agent_max_words: 500,
            agent_role: request.agent_role,
            task_reminder: None,
            team_manifest: None,
        }
        .build()
    } else {
        if let Some(ref persona) = first_persona {
            tracing::debug!(
                persona = %persona.identity.name,
                "persona found in skill dirs but no persona root was resolved — \
                 only skill injection will be applied"
            );
        }
        combine_gateway_and_workspace_injection(request.system_injection, &skill_injection)
    };
    if let Some(roster_match) = request.roster_match {
        append_active_agent_identity_injection(
            &mut system_injection,
            &roster_match.agent_name,
            &roster_match.backend_id,
        );
    }
    append_scheduler_host_guidance(&mut system_injection);
    append_team_contract_guidance(&mut system_injection, request.agent_role);
    if matches!(request.agent_role, AgentRole::Lead | AgentRole::Specialist) {
        if let Some(team_workspace_guide) = request
            .session_team_orch
            .map(|o| o.session.read_agents_md())
            .filter(|content| !content.trim().is_empty())
        {
            append_team_workspace_guide_injection(&mut system_injection, &team_workspace_guide);
        }
    }

    let team_dir = if matches!(request.agent_role, AgentRole::Lead | AgentRole::Specialist) {
        request.session_team_orch.map(|o| o.session.dir.clone())
    } else {
        None
    };
    let effective_workspace = resolve_effective_workspace(
        request.agent_role,
        frontstage_human_turn,
        team_dir.clone(),
        workspace_dir_resolved.clone(),
    );
    let team_tool_url = request
        .session_team_orch
        .and_then(|_| request.team_tool_url.clone());
    let allowed_schedule_tools =
        resolve_schedule_tool_allowlist(request.inbound.source.clone(), request.agent_role);

    ContextAssemblyResult {
        persona_prefix: request
            .roster_match
            .and_then(|_| first_persona.as_ref().map(PersonaSkillData::display_prefix)),
        resolved_persona_dir: resolved_persona_dir.clone(),
        ctx: AgentCtx {
            session_id: request.session_id,
            session_key: request.inbound.session_key.clone(),
            participant_name: request.roster_match.map(|rm| rm.agent_name.clone()),
            user_text: request.inbound.content.as_text().unwrap_or("").to_string(),
            history,
            system_injection,
            persona_dir: resolved_persona_dir,
            workspace_root: workspace_dir_resolved,
            workspace_dir: effective_workspace,
            agent_role: request.agent_role,
            team_dir,
            task_reminder: request.task_reminder,
            team_tool_url,
            allowed_team_tools: request.allowed_team_tools,
            allowed_schedule_tools,
            shared_memory: canonical_shared_memory,
            agent_memory: canonical_agent_memory,
            team_manifest: canonical_team_manifest,
            frontstage_human_turn,
            backend_session_id: None, // populated by registry after context assembly
        },
    }
}

fn is_frontstage_human_turn(source: MsgSource, channel: &str, agent_role: AgentRole) -> bool {
    source == MsgSource::Human && channel == "lark" && !matches!(agent_role, AgentRole::Specialist)
}

fn resolve_schedule_tool_allowlist(source: MsgSource, agent_role: AgentRole) -> Vec<ScheduleTool> {
    if source != MsgSource::Human || agent_role == AgentRole::Specialist {
        return vec![];
    }

    let role = match agent_role {
        AgentRole::Solo => RuntimeRole::Solo,
        AgentRole::Lead => RuntimeRole::Leader,
        AgentRole::Specialist => RuntimeRole::Specialist,
    };
    visible_schedule_tools_for_role(role).visible
}

pub(crate) fn build_history(recent_messages: &[StoredMessage]) -> Vec<HistoryMsg> {
    recent_messages
        .iter()
        .filter(|message| !is_internal_gateway_team_notify(message))
        .map(|message| HistoryMsg {
            role: message.role.clone(),
            content: message.content.clone(),
            sender: message.sender.clone(),
            tool_calls: message.tool_calls.clone(),
        })
        .collect()
}

fn is_internal_gateway_team_notify(message: &StoredMessage) -> bool {
    message.sender.as_deref() == Some("gateway")
        && message.role == "user"
        && message.content.starts_with("[团队通知]")
}

fn append_active_agent_identity_injection(
    system_injection: &mut String,
    agent_name: &str,
    backend_id: &str,
) {
    let identity_block = format!(
        "## 当前执行身份\n\n你当前执行的 agent 是 `{agent_name}`，当前 runtime backend 是 `{backend_id}`。\n\
你必须以这个当前身份回答用户问题，不要把历史对话中其他 assistant 的自我介绍、旧 front-stage agent 名称或旧 backend 身份当成你当前的身份。\n\
历史 assistant 消息只用于保留会话语义，不用于覆盖你当前的执行身份。"
    );
    if system_injection.trim().is_empty() {
        *system_injection = identity_block;
    } else {
        system_injection.push_str("\n\n");
        system_injection.push_str(&identity_block);
    }
}

fn append_team_workspace_guide_injection(system_injection: &mut String, guide: &str) {
    let guide_block = format!("## Team Workspace Guide\n\n{guide}");
    if system_injection.trim().is_empty() {
        *system_injection = guide_block;
    } else {
        system_injection.push_str("\n\n");
        system_injection.push_str(&guide_block);
    }
}

fn append_scheduler_host_guidance(system_injection: &mut String) {
    if system_injection.contains("## Host Scheduler Contract") {
        return;
    }
    if system_injection.trim().is_empty() {
        *system_injection = SCHEDULER_HOST_GUIDANCE.to_string();
    } else {
        system_injection.push_str("\n\n");
        system_injection.push_str(SCHEDULER_HOST_GUIDANCE);
    }
}

fn append_team_contract_guidance(system_injection: &mut String, agent_role: AgentRole) {
    if !matches!(agent_role, AgentRole::Lead | AgentRole::Specialist) {
        return;
    }

    let host_contract = render_team_host_contract(agent_role);
    if !system_injection.contains("## Host Team Contract") {
        if system_injection.trim().is_empty() {
            *system_injection = host_contract.to_string();
        } else {
            system_injection.push_str("\n\n");
            system_injection.push_str(host_contract);
        }
    }

    let team_skill = render_canonical_team_skill_injection(agent_role);
    if !system_injection.contains("# Canonical Team Skill") {
        if system_injection.trim().is_empty() {
            *system_injection = team_skill.to_string();
        } else {
            system_injection.push_str("\n\n");
            system_injection.push_str(team_skill);
        }
    }
}

pub(crate) fn resolve_effective_workspace(
    agent_role: AgentRole,
    frontstage_human_turn: bool,
    team_dir: Option<PathBuf>,
    workspace_dir_resolved: Option<PathBuf>,
) -> Option<PathBuf> {
    // Specialist always works inside the team directory.
    // Lead processing a backend team notification (not a live human message) also
    // needs the team directory so that relative paths like `tasks/T006/result.md`
    // resolve correctly against the team workspace root.
    if agent_role == AgentRole::Specialist
        || (agent_role == AgentRole::Lead && !frontstage_human_turn)
    {
        team_dir.or(workspace_dir_resolved)
    } else {
        workspace_dir_resolved
    }
}

fn combine_gateway_and_workspace_injection(
    system_injection: &str,
    skill_injection: &str,
) -> String {
    if skill_injection.is_empty() {
        system_injection.to_string()
    } else if system_injection.is_empty() {
        skill_injection.to_string()
    } else {
        format!("{system_injection}\n\n{skill_injection}")
    }
}

fn combine_persona_skills(
    skill_injection: &str,
    first_persona: Option<&PersonaSkillData>,
) -> String {
    match first_persona {
        Some(persona) if !persona.capability_body.trim().is_empty() => {
            if skill_injection.is_empty() {
                persona.capability_body.clone()
            } else {
                format!("{}\n\n{}", persona.capability_body, skill_injection)
            }
        }
        _ => skill_injection.to_string(),
    }
}

fn load_persona_layers(
    resolved_persona_dir: Option<&PathBuf>,
    session_key: &SessionKey,
) -> (String, String, String) {
    if let Some(dir) = resolved_persona_dir {
        let persona = AgentPersona::load_from_dir_scoped(dir, session_key);
        (persona.soul, persona.identity, persona.memory)
    } else {
        (String::new(), String::new(), String::new())
    }
}

fn load_skill_injection(
    workspace_dir_resolved: Option<&PathBuf>,
    roster_match: Option<&RosterMatchData>,
    skill_loader_dirs: &[PathBuf],
    inject_prompt_skills: bool,
) -> (String, Option<PersonaSkillData>) {
    let mut agent_skill_dirs = Vec::new();
    if let Some(workspace) = workspace_dir_resolved {
        let project_universal = project_universal_skills_dir(workspace);
        if project_universal.exists() {
            agent_skill_dirs.push(project_universal);
        }

        let workspace_private = workspace_private_skills_dir(workspace);
        if workspace_private.exists() {
            agent_skill_dirs.push(workspace_private);
        }

        if let Some(rm) = roster_match {
            let agent_scoped = agent_scoped_skills_dir(workspace, &rm.agent_name);
            if agent_scoped.exists() {
                agent_skill_dirs.push(agent_scoped);
            }
        }
    }
    if let Some(rm) = roster_match {
        agent_skill_dirs.extend(rm.extra_skills_dirs.iter().cloned());
    }
    if agent_skill_dirs.is_empty() && skill_loader_dirs.is_empty() {
        return (String::new(), None);
    }

    let mut persona_dirs = agent_skill_dirs.clone();
    persona_dirs.extend(skill_loader_dirs.iter().cloned());
    let persona_loader = SkillLoader::with_dirs(persona_dirs);
    let personas = persona_loader.load_personas();

    let mut capability_dirs = agent_skill_dirs;
    if inject_prompt_skills {
        capability_dirs.extend(skill_loader_dirs.iter().cloned());
    }
    let skill_injection = if capability_dirs.is_empty() {
        String::new()
    } else {
        let capability_loader = SkillLoader::with_dirs(capability_dirs);
        let skills = capability_loader.load_all();
        capability_loader.build_system_injection(&skills)
    };
    (skill_injection, personas.into_iter().next())
}

fn resolve_persona_dir(
    roster_match: Option<&RosterMatchData>,
    default_persona_dir: Option<PathBuf>,
    session_key: &SessionKey,
    initialized_persona_dirs: &DashSet<PathBuf>,
) -> Option<PathBuf> {
    roster_match
        .and_then(|rm| rm.persona_dir.clone())
        .or(default_persona_dir)
        .or_else(|| {
            roster_match.map(|rm| {
                let dir = AgentPersona::default_dir_for(&rm.agent_name);
                if !initialized_persona_dirs.contains(&dir) {
                    if let Err(error) = AgentPersona::ensure_default_dir(&dir, &rm.agent_name) {
                        tracing::warn!(
                            agent = %rm.agent_name,
                            scope = %session_key.scope,
                            error = %error,
                            "Failed to create default persona dir"
                        );
                    } else {
                        initialized_persona_dirs.insert(dir.clone());
                    }
                }
                dir
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::skill_paths::sanitize_agent_name_for_path;
    use crate::protocol::SessionKey;
    use crate::protocol::{InboundMsg, MsgContent, MsgSource};
    use dashmap::DashSet;
    use uuid::Uuid;

    #[test]
    fn history_messages_preserve_sender_metadata_without_mutating_content() {
        let history = build_history(&[StoredMessage {
            id: Uuid::new_v4(),
            role: "assistant".to_string(),
            content: "done".to_string(),
            timestamp: chrono::Utc::now(),
            sender: Some("@codex".to_string()),
            tool_calls: None,
            fragment_event_ids: None,
            aggregation_mode: None,
        }]);

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "done");
        assert_eq!(history[0].sender.as_deref(), Some("@codex"));
    }

    #[test]
    fn build_history_excludes_internal_gateway_team_notify_messages() {
        let visible = StoredMessage {
            id: Uuid::new_v4(),
            role: "user".to_string(),
            content: "你好".to_string(),
            timestamp: chrono::Utc::now(),
            sender: Some("ou_1".to_string()),
            tool_calls: None,
            fragment_event_ids: None,
            aggregation_mode: None,
        };
        let internal = StoredMessage {
            id: Uuid::new_v4(),
            role: "user".to_string(),
            content: "[团队通知] 任务 T001 已验收".to_string(),
            timestamp: chrono::Utc::now(),
            sender: Some("gateway".to_string()),
            tool_calls: None,
            fragment_event_ids: None,
            aggregation_mode: None,
        };

        let history = build_history(&[visible.clone(), internal]);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, visible.content);
        assert_eq!(history[0].sender, visible.sender);
    }

    #[test]
    fn specialist_workspace_prefers_team_dir() {
        let resolved = resolve_effective_workspace(
            AgentRole::Specialist,
            false,
            Some(PathBuf::from("/tmp/team")),
            Some(PathBuf::from("/tmp/workspace")),
        );
        assert_eq!(resolved, Some(PathBuf::from("/tmp/team")));
    }

    // Regression: Lead processing a backend team-notify turn (e.g. acceptance
    // check after specialist submission) must also resolve to the team directory
    // so that relative paths like `tasks/T006/result.md` resolve correctly.
    #[test]
    fn lead_backend_team_notify_turn_uses_team_dir() {
        let resolved = resolve_effective_workspace(
            AgentRole::Lead,
            false, // backend turn — NOT a frontstage human message
            Some(PathBuf::from("/tmp/team")),
            Some(PathBuf::from("/tmp/workspace")),
        );
        assert_eq!(resolved, Some(PathBuf::from("/tmp/team")));
    }

    // Lead processing a live frontstage human message keeps the ordinary workspace
    // so the agent operates in the business workspace, not the team artefact dir.
    #[test]
    fn lead_frontstage_human_turn_keeps_workspace_root() {
        let resolved = resolve_effective_workspace(
            AgentRole::Lead,
            true, // frontstage human turn
            Some(PathBuf::from("/tmp/team")),
            Some(PathBuf::from("/tmp/workspace")),
        );
        assert_eq!(resolved, Some(PathBuf::from("/tmp/workspace")));
    }

    // Solo agents are unaffected by team_dir regardless of turn type.
    #[test]
    fn solo_agent_always_uses_workspace_root() {
        let resolved = resolve_effective_workspace(
            AgentRole::Solo,
            false,
            Some(PathBuf::from("/tmp/team")),
            Some(PathBuf::from("/tmp/workspace")),
        );
        assert_eq!(resolved, Some(PathBuf::from("/tmp/workspace")));
    }

    #[test]
    fn no_roster_match_keeps_persona_out_of_regular_skill_injection() {
        let tmp = tempfile::TempDir::new().unwrap();
        let persona_dir = tmp.path().join("rex-intj");
        std::fs::create_dir_all(&persona_dir).unwrap();
        std::fs::write(
            persona_dir.join("SKILL.md"),
            "---\nname: Rex\ntype: persona\n---\nRex capabilities.",
        )
        .unwrap();

        let (skill_injection, first_persona) =
            load_skill_injection(None, None, &[tmp.path().to_path_buf()], true);
        let combined = combine_gateway_and_workspace_injection("", &skill_injection);
        assert!(first_persona.is_some());
        assert!(combined.is_empty());
        assert!(first_persona
            .as_ref()
            .map(PersonaSkillData::display_prefix)
            .is_some());
    }

    #[test]
    fn resolve_persona_dir_prefers_roster_entry() {
        let roster_dir = tempfile::TempDir::new().unwrap();
        let default_dir = tempfile::TempDir::new().unwrap();
        let resolved = resolve_persona_dir(
            Some(&RosterMatchData {
                agent_name: "claude".into(),
                backend_id: "claude-main".into(),
                persona_dir: Some(roster_dir.path().to_path_buf()),
                workspace_dir: None,
                extra_skills_dirs: vec![],
            }),
            Some(default_dir.path().to_path_buf()),
            &SessionKey::new("ws", "ctx"),
            &DashSet::new(),
        );
        assert_eq!(resolved, Some(roster_dir.path().to_path_buf()));
    }

    fn inbound(session_key: SessionKey) -> InboundMsg {
        InboundMsg {
            id: "ctx-1".into(),
            session_key: session_key.clone(),
            content: MsgContent::text("hello"),
            sender: "user".into(),
            channel: session_key.channel.clone(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: MsgSource::Human,
        }
    }

    fn request_with_persona<'a>(
        session_key: &'a SessionKey,
        inbound: &'a InboundMsg,
        default_persona_dir: Option<PathBuf>,
        roster_match: Option<&'a RosterMatchData>,
        initialized_persona_dirs: &'a DashSet<PathBuf>,
    ) -> ContextAssemblyRequest<'a> {
        ContextAssemblyRequest {
            session_id: Uuid::new_v4(),
            session_key,
            inbound,
            recent_messages: &[],
            roster_match,
            agent_role: AgentRole::Solo,
            task_reminder: None,
            session_team_orch: None,
            system_injection: "",
            memory_system: None,
            default_persona_dir,
            default_workspace: None,
            session_workspace: None,
            skill_loader_dirs: &[],
            inject_prompt_skills: true,
            initialized_persona_dirs,
            team_tool_url: None,
            allowed_team_tools: vec![],
        }
    }

    #[tokio::test]
    async fn single_engine_default_persona_dir_loads_persona_layers() {
        let persona_dir = tempfile::TempDir::new().unwrap();
        std::fs::write(persona_dir.path().join("SOUL.md"), "SOUL layer").unwrap();
        std::fs::write(persona_dir.path().join("IDENTITY.md"), "IDENTITY layer").unwrap();
        std::fs::create_dir_all(persona_dir.path().join("memory")).unwrap();
        let session_key = SessionKey::new("lark", "group:test");
        let scope_key = crate::protocol::render_scope_storage_key(&session_key);
        std::fs::write(
            persona_dir
                .path()
                .join("memory")
                .join(format!("{scope_key}.md")),
            "scoped memory layer",
        )
        .unwrap();

        let inbound = inbound(session_key.clone());
        let initialized = DashSet::new();
        let result = assemble_context(request_with_persona(
            &session_key,
            &inbound,
            Some(persona_dir.path().to_path_buf()),
            None,
            &initialized,
        ))
        .await;

        assert!(result.ctx.system_injection.contains("SOUL layer"));
        assert!(result.ctx.system_injection.contains("IDENTITY layer"));
        assert_eq!(
            result.ctx.agent_memory.as_deref(),
            Some("scoped memory layer")
        );
        assert_eq!(
            result.resolved_persona_dir,
            Some(persona_dir.path().to_path_buf())
        );
    }

    #[tokio::test]
    async fn solo_single_engine_and_roster_paths_produce_same_persona_projection() {
        let persona_dir = tempfile::TempDir::new().unwrap();
        std::fs::write(persona_dir.path().join("SOUL.md"), "SOUL layer").unwrap();
        std::fs::write(persona_dir.path().join("IDENTITY.md"), "IDENTITY layer").unwrap();
        std::fs::create_dir_all(persona_dir.path().join("memory")).unwrap();
        let session_key = SessionKey::new("lark", "group:test");
        let scope_key = crate::protocol::render_scope_storage_key(&session_key);
        std::fs::write(
            persona_dir
                .path()
                .join("memory")
                .join(format!("{scope_key}.md")),
            "scoped memory layer",
        )
        .unwrap();
        let inbound = inbound(session_key.clone());
        let initialized = DashSet::new();
        let roster_match = RosterMatchData {
            agent_name: "claude".into(),
            backend_id: "claude-main".into(),
            persona_dir: Some(persona_dir.path().to_path_buf()),
            workspace_dir: None,
            extra_skills_dirs: vec![],
        };

        let single = assemble_context(request_with_persona(
            &session_key,
            &inbound,
            Some(persona_dir.path().to_path_buf()),
            None,
            &initialized,
        ))
        .await;
        let roster = assemble_context(request_with_persona(
            &session_key,
            &inbound,
            None,
            Some(&roster_match),
            &initialized,
        ))
        .await;

        assert_eq!(single.ctx.agent_memory, roster.ctx.agent_memory);
        assert_eq!(single.resolved_persona_dir, roster.resolved_persona_dir);
        assert!(roster
            .ctx
            .system_injection
            .contains("你当前执行的 agent 是 `claude`"));
        assert!(roster
            .ctx
            .system_injection
            .contains("当前 runtime backend 是 `claude-main`"));
        assert!(!single.ctx.system_injection.contains("当前执行身份"));
    }

    #[test]
    fn frontstage_human_turn_detected_for_human_lead_and_solo_only() {
        assert!(is_frontstage_human_turn(
            MsgSource::Human,
            "lark",
            AgentRole::Solo
        ));
        assert!(is_frontstage_human_turn(
            MsgSource::Human,
            "lark",
            AgentRole::Lead
        ));
        assert!(!is_frontstage_human_turn(
            MsgSource::Human,
            "lark",
            AgentRole::Specialist
        ));
        assert!(!is_frontstage_human_turn(
            MsgSource::Heartbeat,
            "lark",
            AgentRole::Lead
        ));
        assert!(!is_frontstage_human_turn(
            MsgSource::Human,
            "ws",
            AgentRole::Lead
        ));
    }

    #[test]
    fn frontstage_human_turn_still_loads_workspace_skill_injection() {
        let workspace = tempfile::TempDir::new().unwrap();
        let skill_dir = workspace.path().join(".agents").join("skills").join("demo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: demo\n---\nUse demo skill.",
        )
        .unwrap();

        let (skill_injection, _) =
            load_skill_injection(Some(&workspace.path().to_path_buf()), None, &[], true);
        assert!(skill_injection.contains("demo"));
    }

    #[test]
    fn workspace_private_skills_override_project_universal_skills() {
        let workspace = tempfile::TempDir::new().unwrap();

        let project_skill = workspace
            .path()
            .join(".agents")
            .join("skills")
            .join("shared");
        std::fs::create_dir_all(&project_skill).unwrap();
        std::fs::write(
            project_skill.join("SKILL.md"),
            "---\nname: shared\ndescription: project\n---\nProject universal body.",
        )
        .unwrap();

        let private_skill = workspace.path().join("skills").join("shared");
        std::fs::create_dir_all(&private_skill).unwrap();
        std::fs::write(
            private_skill.join("SKILL.md"),
            "---\nname: shared\ndescription: private\n---\nWorkspace private body.",
        )
        .unwrap();

        let (skill_injection, _) =
            load_skill_injection(Some(&workspace.path().to_path_buf()), None, &[], true);

        assert!(skill_injection.contains("Workspace private body."));
        assert!(!skill_injection.contains("Project universal body."));
    }

    #[test]
    fn agent_scoped_workspace_skills_override_workspace_private_skills() {
        let workspace = tempfile::TempDir::new().unwrap();

        let private_skill = workspace.path().join("skills").join("shared");
        std::fs::create_dir_all(&private_skill).unwrap();
        std::fs::write(
            private_skill.join("SKILL.md"),
            "---\nname: shared\ndescription: private\n---\nWorkspace private body.",
        )
        .unwrap();

        let agent_skill = workspace
            .path()
            .join(".agents")
            .join("agents")
            .join("alpha")
            .join("skills")
            .join("shared");
        std::fs::create_dir_all(&agent_skill).unwrap();
        std::fs::write(
            agent_skill.join("SKILL.md"),
            "---\nname: shared\ndescription: agent\n---\nAgent scoped body.",
        )
        .unwrap();

        let roster_match = RosterMatchData {
            agent_name: "alpha".into(),
            backend_id: "native-main".into(),
            persona_dir: None,
            workspace_dir: Some(workspace.path().to_path_buf()),
            extra_skills_dirs: vec![],
        };
        let (skill_injection, _) = load_skill_injection(
            Some(&workspace.path().to_path_buf()),
            Some(&roster_match),
            &[],
            true,
        );

        assert!(skill_injection.contains("Agent scoped body."));
        assert!(!skill_injection.contains("Workspace private body."));
    }

    #[test]
    fn sanitize_agent_name_for_path_replaces_path_traversal_chars() {
        assert_eq!(
            sanitize_agent_name_for_path("my agent/../../etc"),
            "my-agent-------etc"
        );
        assert_eq!(sanitize_agent_name_for_path(""), "unknown-agent");
    }

    #[test]
    fn gateway_skill_loader_dirs_do_not_reinject_capability_skills_when_system_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("gw-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: gw-skill\ndescription: demo\n---\nGateway skill body.",
        )
        .unwrap();

        let loader = SkillLoader::with_dirs(vec![tmp.path().to_path_buf()]);
        let static_injection = loader.build_system_injection(&loader.load_all());
        let (dynamic_injection, _) =
            load_skill_injection(None, None, &[tmp.path().to_path_buf()], false);
        let combined =
            combine_gateway_and_workspace_injection(&static_injection, &dynamic_injection);

        assert_eq!(combined.matches("Gateway skill body.").count(), 1);
    }

    #[test]
    fn non_native_backends_still_receive_loader_prompt_skills_with_builtin_static_injection() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("gw-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: gw-skill\ndescription: demo\n---\nGateway skill body.",
        )
        .unwrap();

        let (dynamic_injection, _) =
            load_skill_injection(None, None, &[tmp.path().to_path_buf()], true);
        let combined = combine_gateway_and_workspace_injection(
            "builtin scheduler skill marker",
            &dynamic_injection,
        );

        assert!(combined.contains("builtin scheduler skill marker"));
        assert!(combined.contains("Gateway skill body."));
    }

    #[test]
    fn gateway_skill_loader_dirs_still_load_personas_when_system_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let persona_dir = tmp.path().join("rex-intj");
        std::fs::create_dir_all(&persona_dir).unwrap();
        std::fs::write(
            persona_dir.join("SKILL.md"),
            "---\nname: Rex\ntype: persona\n---\nRex capabilities.",
        )
        .unwrap();

        let (dynamic_injection, first_persona) =
            load_skill_injection(None, None, &[tmp.path().to_path_buf()], false);
        assert!(dynamic_injection.trim().is_empty());
        assert!(first_persona.is_some());
    }

    #[tokio::test]
    async fn lead_context_includes_team_workspace_guide_from_team_session_agents_md() {
        use crate::agent_core::team::{
            heartbeat::DispatchFn, orchestrator::TeamOrchestrator, registry::TaskRegistry,
            session::TeamSession,
        };
        use std::sync::Arc;

        let tmp = tempfile::TempDir::new().unwrap();
        let session = Arc::new(TeamSession::from_dir("team-ctx", tmp.path().to_path_buf()));
        session
            .write_agents_md("# Team Workspace Operating Guide\n\nUse create_task first.")
            .unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(30),
        );

        let session_key = SessionKey::new("lark", "group:test");
        let inbound = inbound(session_key.clone());
        let initialized = DashSet::new();
        let result = assemble_context(ContextAssemblyRequest {
            session_id: Uuid::new_v4(),
            session_key: &session_key,
            inbound: &inbound,
            recent_messages: &[],
            roster_match: None,
            agent_role: AgentRole::Lead,
            task_reminder: None,
            session_team_orch: Some(&orch),
            system_injection: "",
            memory_system: None,
            default_persona_dir: None,
            default_workspace: None,
            session_workspace: None,
            skill_loader_dirs: &[],
            inject_prompt_skills: true,
            initialized_persona_dirs: &initialized,
            team_tool_url: None,
            allowed_team_tools: vec![],
        })
        .await;

        assert!(result.ctx.system_injection.contains("Team Workspace Guide"));
        assert!(result
            .ctx
            .system_injection
            .contains("Use create_task first."));
        assert!(result
            .ctx
            .system_injection
            .contains("## Host Team Contract"));
        assert!(result
            .ctx
            .system_injection
            .contains("# Canonical Team Skill"));
    }

    #[tokio::test]
    async fn default_agent_keeps_gateway_builtin_scheduler_injection() {
        let session_key = SessionKey::new("ws", "solo:test");
        let inbound = inbound(session_key.clone());
        let initialized = DashSet::new();
        let result = assemble_context(ContextAssemblyRequest {
            session_id: Uuid::new_v4(),
            session_key: &session_key,
            inbound: &inbound,
            recent_messages: &[],
            roster_match: None,
            agent_role: AgentRole::Solo,
            task_reminder: None,
            session_team_orch: None,
            system_injection: "builtin scheduler skill marker",
            memory_system: None,
            default_persona_dir: None,
            default_workspace: None,
            session_workspace: None,
            skill_loader_dirs: &[],
            inject_prompt_skills: true,
            initialized_persona_dirs: &initialized,
            team_tool_url: None,
            allowed_team_tools: vec![],
        })
        .await;

        assert!(result
            .ctx
            .system_injection
            .contains("builtin scheduler skill marker"));
        assert!(result
            .ctx
            .system_injection
            .contains("## Host Scheduler Contract"));
    }

    #[tokio::test]
    async fn roster_agent_keeps_gateway_builtin_scheduler_injection_alongside_workspace_skills() {
        let workspace = tempfile::TempDir::new().unwrap();
        let skill_dir = workspace.path().join(".agents").join("skills").join("demo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: demo\n---\nUse demo skill.",
        )
        .unwrap();

        let session_key = SessionKey::new("ws", "solo:test");
        let inbound = inbound(session_key.clone());
        let initialized = DashSet::new();
        let roster_match = RosterMatchData {
            agent_name: "alpha".into(),
            backend_id: "native-main".into(),
            persona_dir: None,
            workspace_dir: Some(workspace.path().to_path_buf()),
            extra_skills_dirs: vec![],
        };
        let result = assemble_context(ContextAssemblyRequest {
            session_id: Uuid::new_v4(),
            session_key: &session_key,
            inbound: &inbound,
            recent_messages: &[],
            roster_match: Some(&roster_match),
            agent_role: AgentRole::Solo,
            task_reminder: None,
            session_team_orch: None,
            system_injection: "builtin scheduler skill marker",
            memory_system: None,
            default_persona_dir: None,
            default_workspace: None,
            session_workspace: None,
            skill_loader_dirs: &[],
            inject_prompt_skills: true,
            initialized_persona_dirs: &initialized,
            team_tool_url: None,
            allowed_team_tools: vec![],
        })
        .await;

        assert!(result
            .ctx
            .system_injection
            .contains("builtin scheduler skill marker"));
        assert!(result
            .ctx
            .system_injection
            .contains("## Host Scheduler Contract"));
        assert!(result.ctx.system_injection.contains("demo"));
    }

    #[test]
    fn native_skill_backends_skip_loader_prompt_skills_but_keep_workspace_overlays_and_personas() {
        let workspace = tempfile::TempDir::new().unwrap();
        let skill_dir = workspace.path().join(".agents").join("skills").join("demo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: demo\n---\nUse demo skill.",
        )
        .unwrap();

        let persona_dir = workspace.path().join(".agents").join("skills").join("rex");
        std::fs::create_dir_all(&persona_dir).unwrap();
        std::fs::write(
            persona_dir.join("SKILL.md"),
            "---\nname: Rex\ntype: persona\n---\nRex capability body.",
        )
        .unwrap();

        let gateway = tempfile::TempDir::new().unwrap();
        let gateway_skill_dir = gateway.path().join("gw-skill");
        std::fs::create_dir_all(&gateway_skill_dir).unwrap();
        std::fs::write(
            gateway_skill_dir.join("SKILL.md"),
            "---\nname: gw-skill\ndescription: demo\n---\nGateway skill body.",
        )
        .unwrap();

        let (skill_injection, first_persona) = load_skill_injection(
            Some(&workspace.path().to_path_buf()),
            None,
            &[gateway.path().to_path_buf()],
            false,
        );

        assert!(skill_injection.contains("Use demo skill."));
        assert!(!skill_injection.contains("Gateway skill body."));
        assert_eq!(
            first_persona
                .as_ref()
                .map(|persona| persona.identity.name.as_str()),
            Some("Rex")
        );
    }
}
