use crate::team::orchestrator::TeamOrchestrator;
use dashmap::DashMap;
use qai_protocol::SessionKey;
use std::sync::Arc;

pub(crate) fn get_orchestrator_for_session(
    team_orchestrators: &DashMap<String, Arc<TeamOrchestrator>>,
    session_key: &SessionKey,
) -> Option<Arc<TeamOrchestrator>> {
    if session_key.channel.as_str() == "specialist" {
        let team_id = session_key.scope.splitn(2, ':').next()?;
        team_orchestrators
            .get(team_id)
            .map(|r| Arc::clone(r.value()))
    } else {
        team_orchestrators
            .iter()
            .find(|entry| entry.value().lead_session_key.get() == Some(session_key))
            .map(|entry| Arc::clone(entry.value()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::{
        orchestrator::TeamOrchestrator, registry::TaskRegistry, session::TeamSession,
    };
    use std::time::Duration;

    fn make_orchestrator() -> Arc<TeamOrchestrator> {
        let tmp = tempfile::tempdir().unwrap();
        let session = Arc::new(TeamSession::from_dir("team-001", tmp.path().to_path_buf()));
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let dispatch = Arc::new(move |_agent: String, _task: crate::team::registry::Task| {
            let fut: std::pin::Pin<
                Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>,
            > = Box::pin(async { Ok(()) });
            fut
        });
        TeamOrchestrator::new(registry, session, dispatch, Duration::from_secs(60))
    }

    #[test]
    fn specialist_session_routes_by_team_id_prefix() {
        let orchestrators = DashMap::new();
        let orch = make_orchestrator();
        orchestrators.insert("team-001".to_string(), Arc::clone(&orch));

        let key = SessionKey::new("specialist", "team-001:codex");
        let found = get_orchestrator_for_session(&orchestrators, &key);
        assert!(found.is_some());
    }
}
