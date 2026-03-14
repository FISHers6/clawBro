use crate::bindings::{resolve_binding, BindingRule};
use crate::control::role_resolver::is_front_bot_turn;
use crate::control::session_router::get_orchestrator_for_session as route_orchestrator_for_session;
use crate::control::turn_intent::build_turn_intent;
use crate::roster::{AgentEntry, AgentRoster};
use crate::team::orchestrator::{TeamOrchestrator, TeamState};
use crate::traits::AgentRole;
use dashmap::{DashMap, DashSet};
use qai_protocol::{InboundMsg, MsgSource, SessionKey};
use qai_runtime::contract::{TurnIntent, TurnMode};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RosterMatchData {
    pub agent_name: String,
    pub backend_id: String,
    pub persona_dir: Option<PathBuf>,
    pub workspace_dir: Option<PathBuf>,
    pub extra_skills_dirs: Vec<PathBuf>,
}

#[derive(Clone)]
pub(crate) struct RoutingDecision {
    pub team_orchestrator: Option<Arc<TeamOrchestrator>>,
    pub roster_match: Option<RosterMatchData>,
    pub intent: TurnIntent,
    pub fallback_backend_id: Option<String>,
    pub sender_name: Option<String>,
    pub agent_role: AgentRole,
    pub is_lead: bool,
    pub task_reminder: Option<String>,
}

pub(crate) fn resolve_turn_routing(
    inbound: &InboundMsg,
    roster: Option<&AgentRoster>,
    bindings: &[BindingRule],
    team_orchestrators: &DashMap<String, Arc<TeamOrchestrator>>,
    auto_promote_scopes: &DashSet<String>,
    session_backend_id: Option<String>,
    specialist_task_reminder: Option<String>,
) -> RoutingDecision {
    let session_key = &inbound.session_key;
    let session_team_orch = route_orchestrator_for_session(team_orchestrators, session_key);
    let early_is_specialist = inbound.source == MsgSource::Heartbeat;
    let auto_promote_orch = resolve_auto_promote_orchestrator(
        inbound,
        team_orchestrators,
        auto_promote_scopes,
        session_team_orch.is_none(),
    );
    let session_team_orch = session_team_orch.or(auto_promote_orch);
    let early_is_lead = !early_is_specialist
        && session_team_orch.is_some()
        && is_front_bot_turn(inbound, &session_team_orch, roster);
    let specialist_binding_match = if early_is_specialist {
        resolve_binding_match(roster, inbound, session_key, bindings)
    } else {
        None
    };

    let roster_match = if specialist_binding_match.is_some() {
        specialist_binding_match
    } else {
        inbound
            .target_agent
            .as_deref()
            .and_then(|mention| resolve_roster_match_by_mention(roster, mention))
            .or_else(|| {
                if early_is_lead {
                    session_team_orch
                        .as_ref()
                        .and_then(|o| o.lead_agent_name.get())
                        .and_then(|name| resolve_roster_match_by_name(roster, name))
                } else {
                    None
                }
            })
            .or_else(|| {
                if inbound.target_agent.is_none() && session_backend_id.is_none() {
                    resolve_binding_match(roster, inbound, session_key, bindings)
                        .or_else(|| resolve_default_roster_match(roster))
                } else {
                    None
                }
            })
    };

    let turn_mode = if early_is_specialist || early_is_lead {
        TurnMode::Team
    } else if inbound.source == MsgSource::Relay {
        TurnMode::Relay
    } else {
        TurnMode::Solo
    };

    let leader_candidate = session_team_orch
        .as_ref()
        .and_then(|o| o.lead_agent_name.get())
        .and_then(|name| {
            roster
                .and_then(|r| r.find_by_name(name))
                .map(|entry| entry.runtime_backend_id().to_string())
                .or_else(|| Some(name.clone()))
        });

    let intent = build_turn_intent(
        inbound,
        turn_mode,
        leader_candidate.as_deref(),
        roster_match
            .as_ref()
            .map(|rm| rm.backend_id.as_str())
            .or(session_backend_id.as_deref()),
    );

    let (fallback_backend_id, sender_name) = if let Some(rm) = &roster_match {
        (
            Some(rm.backend_id.clone()),
            Some(format!("@{}", rm.agent_name)),
        )
    } else {
        (session_backend_id, None)
    };

    let agent_role = if early_is_specialist {
        AgentRole::Specialist
    } else if early_is_lead {
        AgentRole::Lead
    } else {
        AgentRole::Solo
    };

    let task_reminder = if early_is_lead {
        build_lead_task_reminder(session_team_orch.as_ref())
    } else {
        specialist_task_reminder
    };

    RoutingDecision {
        team_orchestrator: session_team_orch,
        roster_match,
        intent,
        fallback_backend_id,
        sender_name,
        agent_role,
        is_lead: early_is_lead,
        task_reminder,
    }
}

pub(crate) fn resolve_default_roster_match(
    roster: Option<&AgentRoster>,
) -> Option<RosterMatchData> {
    roster
        .and_then(|r| r.default_agent())
        .map(roster_match_from_entry)
}

pub(crate) fn resolve_binding_match(
    roster: Option<&AgentRoster>,
    inbound: &InboundMsg,
    session_key: &SessionKey,
    bindings: &[BindingRule],
) -> Option<RosterMatchData> {
    let binding = resolve_binding(inbound, session_key, bindings)?;
    resolve_roster_match_by_name(roster, binding.agent_name())
}

pub(crate) fn resolve_roster_match_by_name(
    roster: Option<&AgentRoster>,
    name: &str,
) -> Option<RosterMatchData> {
    roster
        .and_then(|r| r.find_by_name(name))
        .map(roster_match_from_entry)
}

pub(crate) fn resolve_roster_match_by_mention(
    roster: Option<&AgentRoster>,
    mention: &str,
) -> Option<RosterMatchData> {
    roster
        .and_then(|r| r.find_by_mention(mention))
        .map(roster_match_from_entry)
}

fn roster_match_from_entry(entry: &AgentEntry) -> RosterMatchData {
    RosterMatchData {
        agent_name: entry.name.clone(),
        backend_id: entry.runtime_backend_id().to_string(),
        persona_dir: entry.persona_dir.clone(),
        workspace_dir: entry.workspace_dir.clone(),
        extra_skills_dirs: entry.extra_skills_dirs.clone(),
    }
}

fn resolve_auto_promote_orchestrator(
    inbound: &InboundMsg,
    team_orchestrators: &DashMap<String, Arc<TeamOrchestrator>>,
    auto_promote_scopes: &DashSet<String>,
    no_session_orchestrator: bool,
) -> Option<Arc<TeamOrchestrator>> {
    if inbound.source == MsgSource::Heartbeat
        || !no_session_orchestrator
        || inbound.source != MsgSource::Human
        || !auto_promote_scopes.contains(&inbound.session_key.scope)
        || !crate::mode_selector::is_team_trigger(inbound.content.as_text().unwrap_or(""))
    {
        return None;
    }
    let found = team_orchestrators
        .iter()
        .find(|entry| {
            entry
                .value()
                .lead_session_key
                .get()
                .map(|key| key.scope == inbound.session_key.scope)
                .unwrap_or(false)
        })
        .map(|entry| Arc::clone(entry.value()));
    if found.is_none() {
        tracing::warn!(
            scope = %inbound.session_key.scope,
            "auto_promote triggered but no orchestrator found for this scope — falling back to Solo"
        );
    }
    found
}

fn build_lead_task_reminder(team_orch: Option<&Arc<TeamOrchestrator>>) -> Option<String> {
    let state = team_orch
        .map(|o| o.team_state())
        .unwrap_or(TeamState::Planning);
    let specialists_list = team_orch
        .and_then(|o| o.available_specialists.get())
        .map(|v| v.join(", "))
        .unwrap_or_else(|| "（未配置）".to_string());
    Some(match state {
        TeamState::Planning | TeamState::AwaitingConfirm => {
            format!(
                "你是团队协调者。用户的请求需要多个 Agent 协作完成。\n\n\
                 可分配的 Specialist：{specialists_list}\n\n\
                 步骤：\n\
                 1. 分析任务，调用 create_task() 定义所有子任务和依赖关系（assignee 填 Specialist 名称）\n\
                 2. 简单任务（≤3个、无复杂依赖）直接调用 start_execution()\n\
                 3. 复杂任务先调用 request_confirmation(plan_summary)，等用户确认后再执行\n\
                 4. Specialist 完成后通常会先提交结果；你收到待验收通知后，用 accept_task() 验收或 reopen_task() 打回\n\
                 5. 任务执行中你会收到 [团队通知] 消息，用 post_update() 向用户播报关键进度\n\
                 6. 收到\"所有任务已完成\"通知后，合成最终结果并调用 post_update() 发给用户\n\n\
                 可用工具：create_task, start_execution, request_confirmation, post_update, get_task_status, assign_task, accept_task, reopen_task",
            )
        }
        TeamState::Running | TeamState::Done => {
            format!(
                "团队任务执行中。可分配的 Specialist：{specialists_list}\n\n\
                 你会收到 [团队通知] 消息：\n\
                 - 用 post_update(message) 向用户播报进度\n\
                 - 用 get_task_status() 查看全局状态\n\
                 - 用 assign_task(task_id, agent) 重新分配卡住的任务（agent 填 Specialist 名称）\n\
                 - 对 submitted 结果用 accept_task(task_id) 验收，或用 reopen_task(task_id, reason) 打回\n\
                 - 收到\"所有任务已完成\"通知后，合成最终汇总并 post_update",
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::AgentEntry;
    use crate::team::{heartbeat::DispatchFn, registry::TaskRegistry, session::TeamSession};
    use qai_protocol::{MsgContent, SessionKey};
    use std::time::Duration;

    fn make_roster() -> AgentRoster {
        AgentRoster::new(vec![
            AgentEntry {
                name: "claude".into(),
                mentions: vec!["@claude".into()],
                backend_id: "claude-main".into(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
            AgentEntry {
                name: "codex".into(),
                mentions: vec!["@codex".into()],
                backend_id: "codex-main".into(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
        ])
    }

    fn make_orchestrator() -> Arc<TeamOrchestrator> {
        let tmp = tempfile::tempdir().unwrap();
        let session = Arc::new(TeamSession::from_dir(
            "team-routing",
            tmp.path().to_path_buf(),
        ));
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let dispatch: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(registry, session, dispatch, Duration::from_secs(60));
        orch.set_lead_session_key(SessionKey::new("lark", "group:route"));
        orch.set_lead_agent_name("claude".to_string());
        orch
    }

    fn inbound(scope: &str, target_agent: Option<&str>) -> InboundMsg {
        InboundMsg {
            id: "route-1".into(),
            session_key: SessionKey::new("lark", scope),
            content: MsgContent::text("hello team"),
            sender: "user".into(),
            channel: "lark".into(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: target_agent.map(str::to_string),
            source: MsgSource::Human,
        }
    }

    #[test]
    fn scope_binding_only_applies_when_session_backend_unset() {
        let bindings = vec![BindingRule::scope("group:route", "claude")];
        let roster = make_roster();

        let bound = resolve_turn_routing(
            &inbound("group:route", None),
            Some(&roster),
            &bindings,
            &DashMap::new(),
            &DashSet::new(),
            None,
            None,
        );
        assert_eq!(bound.intent.target_backend.as_deref(), Some("claude-main"));

        let manual = resolve_turn_routing(
            &inbound("group:route", None),
            Some(&roster),
            &bindings,
            &DashMap::new(),
            &DashSet::new(),
            Some("manual-backend".to_string()),
            None,
        );
        assert_eq!(
            manual.intent.target_backend.as_deref(),
            Some("manual-backend")
        );
    }

    #[test]
    fn lead_without_explicit_mention_falls_back_to_front_bot_backend() {
        let roster = make_roster();
        let orch = make_orchestrator();
        let orchestrators = DashMap::new();
        orchestrators.insert("team-routing".to_string(), orch);

        let decision = resolve_turn_routing(
            &inbound("group:route", None),
            Some(&roster),
            &[],
            &orchestrators,
            &DashSet::new(),
            None,
            None,
        );

        assert!(decision.is_lead);
        assert_eq!(decision.agent_role, AgentRole::Lead);
        assert_eq!(
            decision.intent.target_backend.as_deref(),
            Some("claude-main")
        );
        assert!(decision
            .task_reminder
            .as_deref()
            .is_some_and(|text| text.contains("create_task()")));
    }

    #[test]
    fn specialist_team_binding_beats_generated_target_agent_hint() {
        let roster = AgentRoster::new(vec![
            AgentEntry {
                name: "codex".into(),
                mentions: vec!["@codex".into()],
                backend_id: "codex-main".into(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
            AgentEntry {
                name: "openclaw".into(),
                mentions: vec!["@openclaw".into()],
                backend_id: "openclaw-main".into(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
        ]);
        let inbound = InboundMsg {
            id: "route-specialist-team".into(),
            session_key: SessionKey::new("specialist", "team-123:codex"),
            content: MsgContent::text("do task"),
            sender: "orchestrator".into(),
            channel: "specialist".into(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: Some("@codex".into()),
            source: MsgSource::Heartbeat,
        };

        let decision = resolve_turn_routing(
            &inbound,
            Some(&roster),
            &[BindingRule::Team {
                team_id: "team-123".into(),
                agent_name: "openclaw".into(),
            }],
            &DashMap::new(),
            &DashSet::new(),
            None,
            Some("task reminder".into()),
        );

        assert_eq!(
            decision.intent.target_backend.as_deref(),
            Some("openclaw-main")
        );
        assert_eq!(decision.agent_role, AgentRole::Specialist);
        assert_eq!(decision.task_reminder.as_deref(), Some("task reminder"));
    }

    // ── resolve_auto_promote_orchestrator 专项测试 ────────────────────────

    fn make_orchestrator_for_scope(scope: &str) -> Arc<TeamOrchestrator> {
        let tmp = tempfile::tempdir().unwrap();
        let session = Arc::new(TeamSession::from_dir("team-auto", tmp.path().to_path_buf()));
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let dispatch: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(registry, session, dispatch, Duration::from_secs(60));
        orch.set_lead_session_key(SessionKey::new("lark", scope));
        orch.set_lead_agent_name("claude".to_string());
        orch
    }

    fn team_trigger_inbound(scope: &str) -> InboundMsg {
        InboundMsg {
            id: "auto-1".into(),
            session_key: SessionKey::new("lark", scope),
            // "多agent" is a registered team trigger keyword
            content: MsgContent::text("请帮我多agent完成这个任务"),
            sender: "user".into(),
            channel: "lark".into(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: MsgSource::Human,
        }
    }

    #[test]
    fn auto_promote_returns_orchestrator_when_keyword_matches_registered_scope() {
        let scope = "group:trigger-test";
        let orch = make_orchestrator_for_scope(scope);

        let orchestrators: DashMap<String, Arc<TeamOrchestrator>> = DashMap::new();
        orchestrators.insert("team-auto".to_string(), Arc::clone(&orch));

        let auto_promote_scopes = DashSet::new();
        auto_promote_scopes.insert(scope.to_string());

        let result = resolve_auto_promote_orchestrator(
            &team_trigger_inbound(scope),
            &orchestrators,
            &auto_promote_scopes,
            true, // no_session_orchestrator = true (none found yet)
        );
        assert!(
            result.is_some(),
            "should find orchestrator for registered scope with trigger keyword"
        );
    }

    #[test]
    fn auto_promote_returns_none_for_non_trigger_text() {
        let scope = "group:no-trigger";
        let orch = make_orchestrator_for_scope(scope);

        let orchestrators: DashMap<String, Arc<TeamOrchestrator>> = DashMap::new();
        orchestrators.insert("team-auto-2".to_string(), Arc::clone(&orch));

        let auto_promote_scopes = DashSet::new();
        auto_promote_scopes.insert(scope.to_string());

        let plain_inbound = InboundMsg {
            id: "auto-2".into(),
            session_key: SessionKey::new("lark", scope),
            content: MsgContent::text("今天天气不错"), // not a team trigger
            sender: "user".into(),
            channel: "lark".into(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: MsgSource::Human,
        };

        let result = resolve_auto_promote_orchestrator(
            &plain_inbound,
            &orchestrators,
            &auto_promote_scopes,
            true,
        );
        assert!(result.is_none(), "non-trigger text should not auto-promote");
    }

    #[test]
    fn auto_promote_returns_none_when_scope_has_no_orchestrator() {
        // Scope is registered for auto-promote but no orchestrator covers it
        let scope = "group:orphan-scope";
        let auto_promote_scopes = DashSet::new();
        auto_promote_scopes.insert(scope.to_string());

        // Different orchestrator registered for a different scope
        let other_orch = make_orchestrator_for_scope("group:other");
        let orchestrators: DashMap<String, Arc<TeamOrchestrator>> = DashMap::new();
        orchestrators.insert("team-other".to_string(), other_orch);

        let result = resolve_auto_promote_orchestrator(
            &team_trigger_inbound(scope),
            &orchestrators,
            &auto_promote_scopes,
            true,
        );
        assert!(
            result.is_none(),
            "should return None and warn when no orchestrator covers the scope"
        );
    }
}
