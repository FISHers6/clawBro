use crate::memory::{MemoryDistiller, MemoryStore};
use crate::memory::event::MemoryEvent;
use crate::memory::trigger::MemoryTrigger;
use async_trait::async_trait;
use std::sync::Arc;

pub struct NTurnDistillTrigger { n: u64 }

impl NTurnDistillTrigger {
    pub fn new(n: u64) -> Self {
        assert!(n > 0, "distill_every_n must be > 0");
        Self { n }
    }
    pub fn should_fire_for(&self, turn_count: u64) -> bool {
        turn_count > 0 && turn_count % self.n == 0
    }
}

#[async_trait]
impl MemoryTrigger for NTurnDistillTrigger {
    fn name(&self) -> &str { "nturn_distill" }

    fn matches(&self, event: &MemoryEvent) -> bool {
        if let MemoryEvent::TurnCompleted { turn_count, .. } = event {
            self.should_fire_for(*turn_count)
        } else { false }
    }

    async fn fire(
        &self,
        event: MemoryEvent,
        store: Arc<dyn MemoryStore>,
        distiller: Arc<dyn MemoryDistiller>,
    ) -> anyhow::Result<()> {
        if let MemoryEvent::TurnCompleted { scope, persona_dir, .. } = event {
            let logs = store.load_recent_logs(&persona_dir, &scope, 7).await?;
            let current = store.load_agent_memory(&persona_dir, &scope).await?;
            if logs.is_empty() && current.is_empty() { return Ok(()); }
            let new_memory = distiller.distill(&logs, &current).await?;
            store.overwrite_agent_memory(&persona_dir, &scope, &new_memory).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{distiller::NoopDistiller, store::FileMemoryStore, trigger::MemoryTrigger};
    use qai_protocol::SessionKey;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn turn_event(turn_count: u64, persona_dir: PathBuf) -> MemoryEvent {
        MemoryEvent::TurnCompleted {
            scope: SessionKey::new("dingtalk", "g1"),
            agent: "bot".to_string(),
            persona_dir,
            turn_count,
        }
    }

    #[test]
    fn test_matches_turn_completed_only() {
        let t = NTurnDistillTrigger::new(20);
        assert!(t.matches(&turn_event(20, PathBuf::from("/tmp"))));
        assert!(!t.matches(&MemoryEvent::SessionIdle {
            scope: SessionKey::new("ws", "u"), agent: "x".to_string(),
            persona_dir: PathBuf::from("/tmp"),
        }));
    }

    #[test]
    fn test_fires_only_at_threshold() {
        let t = NTurnDistillTrigger::new(20);
        assert!(t.should_fire_for(20));
        assert!(t.should_fire_for(40));
        assert!(!t.should_fire_for(19));
        assert!(!t.should_fire_for(21));
        assert!(!t.should_fire_for(1));
    }

    #[tokio::test]
    async fn test_distills_and_overwrites_agent_memory() {
        let shared = tempdir().unwrap();
        let persona = tempdir().unwrap();
        let store: Arc<dyn MemoryStore> = Arc::new(FileMemoryStore::new(shared.path().to_path_buf()));
        let distiller: Arc<dyn MemoryDistiller> = Arc::new(NoopDistiller);
        store.append_daily_log(persona.path(), &SessionKey::new("dingtalk", "g1"), "user: hello\nbot: hi").await.unwrap();
        let trigger = NTurnDistillTrigger::new(20);
        let event = turn_event(20, persona.path().to_path_buf());
        trigger.fire(event, store.clone(), distiller).await.unwrap();
        let mem = store.load_agent_memory(persona.path(), &SessionKey::new("dingtalk", "g1")).await.unwrap();
        assert!(!mem.is_empty(), "memory should be written after distillation");
    }
}
