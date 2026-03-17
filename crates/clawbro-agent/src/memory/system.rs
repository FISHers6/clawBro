use crate::memory::event::MemoryEvent;
use crate::memory::trigger::MemoryTrigger;
use crate::memory::{MemoryDistiller, MemoryStore};
use std::sync::Arc;

pub struct MemorySystem {
    triggers: Vec<Arc<dyn MemoryTrigger>>,
    store: Arc<dyn MemoryStore>,
    distiller: Arc<dyn MemoryDistiller>,
}

impl MemorySystem {
    pub fn new(
        triggers: Vec<Arc<dyn MemoryTrigger>>,
        store: Arc<dyn MemoryStore>,
        distiller: Arc<dyn MemoryDistiller>,
    ) -> Arc<Self> {
        Arc::new(Self {
            triggers,
            store,
            distiller,
        })
    }

    /// 发射事件到所有匹配触发器，全部在后台 spawn（不阻塞调用方）
    pub fn emit(&self, event: MemoryEvent) {
        for trigger in &self.triggers {
            if trigger.matches(&event) {
                let trigger = Arc::clone(trigger);
                let store = Arc::clone(&self.store);
                let distiller = Arc::clone(&self.distiller);
                let ev = event.clone();
                tokio::spawn(async move {
                    if let Err(e) = trigger.fire(ev, store, distiller).await {
                        tracing::warn!("[memory] trigger '{}' error: {e}", trigger.name());
                    }
                });
            }
        }
    }

    pub fn store(&self) -> Arc<dyn MemoryStore> {
        Arc::clone(&self.store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::trigger::MemoryTrigger;
    use crate::memory::{
        distiller::NoopDistiller,
        event::{MemoryEvent, MemoryTarget},
        store::FileMemoryStore,
    };
    use clawbro_protocol::SessionKey;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tempfile::tempdir;

    struct CountingTrigger(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl MemoryTrigger for CountingTrigger {
        fn name(&self) -> &str {
            "counter"
        }
        fn matches(&self, _: &MemoryEvent) -> bool {
            true
        }
        async fn fire(
            &self,
            _: MemoryEvent,
            _: Arc<dyn MemoryStore>,
            _: Arc<dyn MemoryDistiller>,
        ) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct NeverMatch;
    #[async_trait::async_trait]
    impl MemoryTrigger for NeverMatch {
        fn name(&self) -> &str {
            "never"
        }
        fn matches(&self, _: &MemoryEvent) -> bool {
            false
        }
        async fn fire(
            &self,
            _: MemoryEvent,
            _: Arc<dyn MemoryStore>,
            _: Arc<dyn MemoryDistiller>,
        ) -> anyhow::Result<()> {
            panic!("should never be called");
        }
    }

    fn make_event() -> MemoryEvent {
        MemoryEvent::UserRemember {
            scope: SessionKey::new("ws", "u1"),
            target: MemoryTarget::Shared,
            content: "test".to_string(),
        }
    }

    #[tokio::test]
    async fn test_emit_calls_matching_trigger() {
        let counter = Arc::new(AtomicUsize::new(0));
        let dir = tempdir().unwrap();
        let store: Arc<dyn MemoryStore> = Arc::new(FileMemoryStore::new(dir.path().to_path_buf()));
        let distiller: Arc<dyn MemoryDistiller> = Arc::new(NoopDistiller);
        let triggers: Vec<Arc<dyn MemoryTrigger>> =
            vec![Arc::new(CountingTrigger(counter.clone()))];
        let system = MemorySystem::new(triggers, store, distiller);
        system.emit(make_event());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_emit_skips_non_matching_trigger() {
        let dir = tempdir().unwrap();
        let store: Arc<dyn MemoryStore> = Arc::new(FileMemoryStore::new(dir.path().to_path_buf()));
        let distiller: Arc<dyn MemoryDistiller> = Arc::new(NoopDistiller);
        let triggers: Vec<Arc<dyn MemoryTrigger>> = vec![Arc::new(NeverMatch)];
        let system = MemorySystem::new(triggers, store, distiller);
        system.emit(make_event()); // NeverMatch.matches() = false, so fire() never called
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // no panic = pass
    }
}
