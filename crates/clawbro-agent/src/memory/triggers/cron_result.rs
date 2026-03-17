use crate::memory::event::MemoryEvent;
use crate::memory::trigger::MemoryTrigger;
use crate::memory::{MemoryDistiller, MemoryStore};
use async_trait::async_trait;
use std::sync::Arc;

pub struct CronResultTrigger;

#[async_trait]
impl MemoryTrigger for CronResultTrigger {
    fn name(&self) -> &str {
        "cron_result"
    }

    fn matches(&self, event: &MemoryEvent) -> bool {
        matches!(event, MemoryEvent::CronJobCompleted { .. })
    }

    async fn fire(
        &self,
        event: MemoryEvent,
        store: Arc<dyn MemoryStore>,
        _: Arc<dyn MemoryDistiller>,
    ) -> anyhow::Result<()> {
        if let MemoryEvent::CronJobCompleted {
            scope,
            result_summary,
            ..
        } = event
        {
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
            store
                .append_shared(&scope, &format!("- [{now}] {result_summary}\n"))
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{distiller::NoopDistiller, store::FileMemoryStore, trigger::MemoryTrigger};
    use clawbro_protocol::SessionKey;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_cron_result_writes_to_shared() {
        let shared = tempdir().unwrap();
        let persona = tempdir().unwrap();
        let store: Arc<dyn MemoryStore> =
            Arc::new(FileMemoryStore::new(shared.path().to_path_buf()));
        let distiller: Arc<dyn MemoryDistiller> = Arc::new(NoopDistiller);
        let trigger = CronResultTrigger;
        let event = MemoryEvent::CronJobCompleted {
            scope: SessionKey::new("dingtalk", "g1"),
            agent: "standup".to_string(),
            persona_dir: persona.path().to_path_buf(),
            result_summary: "站会：支付模块进行中".to_string(),
        };
        assert!(trigger.matches(&event));
        trigger.fire(event, store.clone(), distiller).await.unwrap();
        let shared_mem = store
            .load_shared_memory(&SessionKey::new("dingtalk", "g1"))
            .await
            .unwrap();
        assert!(shared_mem.contains("支付模块进行中"));
    }
}
