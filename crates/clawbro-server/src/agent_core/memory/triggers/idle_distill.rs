use crate::agent_core::memory::event::MemoryEvent;
use crate::agent_core::memory::trigger::MemoryTrigger;
use crate::agent_core::memory::{MemoryDistiller, MemoryStore};
use async_trait::async_trait;
use std::sync::Arc;

pub struct IdleDistillTrigger;

#[async_trait]
impl MemoryTrigger for IdleDistillTrigger {
    fn name(&self) -> &str {
        "idle_distill"
    }

    fn matches(&self, event: &MemoryEvent) -> bool {
        matches!(event, MemoryEvent::SessionIdle { .. })
    }

    async fn fire(
        &self,
        event: MemoryEvent,
        store: Arc<dyn MemoryStore>,
        distiller: Arc<dyn MemoryDistiller>,
    ) -> anyhow::Result<()> {
        if let MemoryEvent::SessionIdle {
            scope, persona_dir, ..
        } = event
        {
            let logs = store.load_recent_logs(&persona_dir, &scope, 7).await?;
            let current = store.load_agent_memory(&persona_dir, &scope).await?;
            if logs.is_empty() && current.is_empty() {
                return Ok(());
            }
            let new_memory = distiller.distill(&logs, &current).await?;
            store
                .overwrite_agent_memory(&persona_dir, &scope, &new_memory)
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::memory::{
        distiller::NoopDistiller, store::FileMemoryStore, trigger::MemoryTrigger,
    };
    use crate::protocol::SessionKey;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_idle_distills_and_overwrites_agent_memory() {
        let shared = tempdir().unwrap();
        let persona = tempdir().unwrap();
        let store: Arc<dyn MemoryStore> =
            Arc::new(FileMemoryStore::new(shared.path().to_path_buf()));
        let distiller: Arc<dyn MemoryDistiller> = Arc::new(NoopDistiller);
        store
            .append_daily_log(
                persona.path(),
                &SessionKey::new("lark", "g2"),
                "user: test\nbot: ok",
            )
            .await
            .unwrap();
        let trigger = IdleDistillTrigger;
        let event = MemoryEvent::SessionIdle {
            scope: SessionKey::new("lark", "g2"),
            agent: "bot".to_string(),
            persona_dir: persona.path().to_path_buf(),
        };
        assert!(trigger.matches(&event));
        trigger.fire(event, store.clone(), distiller).await.unwrap();
        let mem = store
            .load_agent_memory(persona.path(), &SessionKey::new("lark", "g2"))
            .await
            .unwrap();
        assert!(
            !mem.is_empty(),
            "memory should be written after idle distillation"
        );
    }
}
