use crate::memory::event::MemoryEvent;
use crate::memory::trigger::MemoryTrigger;
use crate::memory::{MemoryDistiller, MemoryStore};
use async_trait::async_trait;
use std::sync::Arc;
use tracing;

pub struct NightlyConsolidationTrigger;

#[async_trait]
impl MemoryTrigger for NightlyConsolidationTrigger {
    fn name(&self) -> &str {
        "nightly_consolidation"
    }

    fn matches(&self, event: &MemoryEvent) -> bool {
        matches!(event, MemoryEvent::NightlyConsolidation { .. })
    }

    async fn fire(
        &self,
        event: MemoryEvent,
        store: Arc<dyn MemoryStore>,
        distiller: Arc<dyn MemoryDistiller>,
    ) -> anyhow::Result<()> {
        if let MemoryEvent::NightlyConsolidation { scope, agent_dirs } = event {
            let mut all_memories = String::new();
            for (name, dir) in &agent_dirs {
                let mem = store
                    .load_agent_memory(dir, &scope)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!("nightly: failed to load agent memory for {name}: {e}");
                        String::new()
                    });
                if !mem.is_empty() {
                    all_memories.push_str(&format!("### {name}\n{mem}\n\n"));
                }
            }
            if all_memories.is_empty() {
                return Ok(());
            }
            let current_shared = store.load_shared_memory(&scope).await.unwrap_or_else(|e| {
                tracing::warn!("nightly: failed to load shared memory: {e}");
                String::new()
            });
            let new_shared = distiller.distill(&all_memories, &current_shared).await?;
            store.overwrite_shared(&scope, &new_shared).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{distiller::NoopDistiller, store::FileMemoryStore, trigger::MemoryTrigger};
    use qai_protocol::SessionKey;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_nightly_consolidation_reads_all_agents_and_writes_shared() {
        let shared = tempdir().unwrap();
        let agent_a = tempdir().unwrap();
        let agent_b = tempdir().unwrap();
        let scope = SessionKey::new("dingtalk", "g1");
        let store: Arc<dyn MemoryStore> =
            Arc::new(FileMemoryStore::new(shared.path().to_path_buf()));
        let distiller: Arc<dyn MemoryDistiller> = Arc::new(NoopDistiller);
        store
            .overwrite_agent_memory(agent_a.path(), &scope, "agent_a知道：Go + PG")
            .await
            .unwrap();
        store
            .overwrite_agent_memory(agent_b.path(), &scope, "agent_b知道：Redis lock")
            .await
            .unwrap();
        let trigger = NightlyConsolidationTrigger;
        let event = MemoryEvent::NightlyConsolidation {
            scope: scope.clone(),
            agent_dirs: vec![
                ("a".to_string(), agent_a.path().to_path_buf()),
                ("b".to_string(), agent_b.path().to_path_buf()),
            ],
        };
        assert!(trigger.matches(&event));
        trigger.fire(event, store.clone(), distiller).await.unwrap();
        let shared_mem = store.load_shared_memory(&scope).await.unwrap();
        assert!(!shared_mem.is_empty());
    }
}
