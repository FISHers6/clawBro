use crate::roster::AgentRoster;
use crate::team::orchestrator::TeamOrchestrator;
use clawbro_protocol::InboundMsg;
use std::sync::Arc;

/// Returns `true` if the current inbound turn should be handled by the Lead agent.
///
/// A turn is a "Lead turn" when:
/// - There is no `@mention`
/// - The `@mention` explicitly targets the configured `front_bot`
pub(crate) fn is_front_bot_turn(
    inbound: &InboundMsg,
    orch: &Option<Arc<TeamOrchestrator>>,
    roster: Option<&AgentRoster>,
) -> bool {
    let orch = match orch {
        Some(o) => o,
        None => return false,
    };
    let front_bot = match orch.lead_agent_name.get() {
        Some(n) => n.as_str(),
        None => return true,
    };

    match &inbound.target_agent {
        None => true,
        Some(mention) => {
            let mention_bare = mention.trim_start_matches('@');
            mention_bare.eq_ignore_ascii_case(front_bot)
                || roster
                    .and_then(|r| r.find_by_mention(mention))
                    .map(|e| e.name.eq_ignore_ascii_case(front_bot))
                    .unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::AgentEntry;
    use crate::team::{
        orchestrator::TeamOrchestrator, registry::TaskRegistry, session::TeamSession,
    };
    use clawbro_protocol::{MsgContent, MsgSource, SessionKey};
    use std::sync::Arc;
    use std::time::Duration;

    fn make_orchestrator() -> Arc<TeamOrchestrator> {
        let tmp = tempfile::tempdir().unwrap();
        let session = Arc::new(TeamSession::from_dir("team-role", tmp.path().to_path_buf()));
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let dispatch = Arc::new(move |_agent: String, _task: crate::team::registry::Task| {
            let fut: std::pin::Pin<
                Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>,
            > = Box::pin(async { Ok(()) });
            fut
        });
        let orch = TeamOrchestrator::new(registry, session, dispatch, Duration::from_secs(60));
        orch.set_lead_agent_name("claude".to_string());
        orch
    }

    fn make_inbound(target_agent: Option<&str>) -> InboundMsg {
        InboundMsg {
            id: "1".to_string(),
            session_key: SessionKey::new("lark", "group:test"),
            content: MsgContent::text("hello"),
            sender: "user".to_string(),
            channel: "lark".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: target_agent.map(str::to_string),
            source: MsgSource::Human,
        }
    }

    #[test]
    fn non_front_bot_mention_in_team_scope_is_not_lead() {
        let orch = Some(make_orchestrator());
        let roster = AgentRoster::new(vec![
            AgentEntry {
                name: "claude".to_string(),
                mentions: vec!["@claude".to_string()],
                backend_id: "claude-main".to_string(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
            AgentEntry {
                name: "codex".to_string(),
                mentions: vec!["@codex".to_string()],
                backend_id: "codex-main".to_string(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
        ]);

        let inbound = make_inbound(Some("@codex"));
        assert!(!is_front_bot_turn(&inbound, &orch, Some(&roster)));
    }
}
