use crate::memory::{MemoryDistiller, MemoryStore};
use crate::memory::event::MemoryEvent;
use crate::memory::trigger::MemoryTrigger;
use async_trait::async_trait;
use std::sync::Arc;

pub struct IdleDistillTrigger;

#[async_trait]
impl MemoryTrigger for IdleDistillTrigger {
    fn name(&self) -> &str { "idle_distill" }

    fn matches(&self, event: &MemoryEvent) -> bool {
        matches!(event, MemoryEvent::SessionIdle { .. })
    }

    async fn fire(
        &self,
        event: MemoryEvent,
        store: Arc<dyn MemoryStore>,
        distiller: Arc<dyn MemoryDistiller>,
    ) -> anyhow::Result<()> {
        if let MemoryEvent::SessionIdle { scope, persona_dir, .. } = event {
            let logs = store.load_recent_logs(&persona_dir, &scope, 7).await?;
            let current = store.load_agent_memory(&persona_dir, &scope).await?;
            if logs.is_empty() && current.is_empty() { return Ok(()); }
            let new_memory = distiller.distill(&logs, &current).await?;
            store.overwrite_agent_memory(&persona_dir, &scope, &new_memory).await?;
        }
        Ok(())
    }
}
