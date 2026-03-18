use crate::agent_core::memory::event::MemoryEvent;
use crate::agent_core::memory::{MemoryDistiller, MemoryStore};
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait MemoryTrigger: Send + Sync {
    fn name(&self) -> &str;
    fn matches(&self, event: &MemoryEvent) -> bool;
    async fn fire(
        &self,
        event: MemoryEvent,
        store: Arc<dyn MemoryStore>,
        distiller: Arc<dyn MemoryDistiller>,
    ) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::memory::event::{MemoryEvent, MemoryTarget};
    use crate::protocol::SessionKey;
    use std::sync::Arc;

    struct AlwaysMatch;
    #[async_trait::async_trait]
    impl MemoryTrigger for AlwaysMatch {
        fn name(&self) -> &str {
            "always"
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
            Ok(())
        }
    }

    #[test]
    fn test_trigger_matches_all() {
        let t = AlwaysMatch;
        let e = MemoryEvent::UserRemember {
            scope: SessionKey::new("ws", "u1"),
            target: MemoryTarget::Shared,
            content: "x".to_string(),
        };
        assert!(t.matches(&e));
    }
}
