use crate::memory::event::{MemoryEvent, MemoryTarget};
use crate::memory::trigger::MemoryTrigger;
use crate::memory::{MemoryDistiller, MemoryStore};
use async_trait::async_trait;
use std::sync::Arc;

pub struct UserRememberTrigger;

#[async_trait]
impl MemoryTrigger for UserRememberTrigger {
    fn name(&self) -> &str {
        "user_remember"
    }

    fn matches(&self, event: &MemoryEvent) -> bool {
        matches!(event, MemoryEvent::UserRemember { .. })
    }

    async fn fire(
        &self,
        event: MemoryEvent,
        store: Arc<dyn MemoryStore>,
        _distiller: Arc<dyn MemoryDistiller>,
    ) -> anyhow::Result<()> {
        if let MemoryEvent::UserRemember {
            scope,
            target,
            content,
        } = event
        {
            let entry = format!("- {content}\n");
            match target {
                MemoryTarget::Shared => store.append_shared(&scope, &entry).await?,
                MemoryTarget::Agent { persona_dir } => {
                    store
                        .append_to_agent_memory(&persona_dir, &scope, &entry)
                        .await?
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::event::{MemoryEvent, MemoryTarget};
    use crate::memory::{distiller::NoopDistiller, store::FileMemoryStore, trigger::MemoryTrigger};
    use clawbro_protocol::SessionKey;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_user_remember_shared_appends_to_shared() {
        let dir = tempdir().unwrap();
        let store: Arc<dyn MemoryStore> = Arc::new(FileMemoryStore::new(dir.path().to_path_buf()));
        let distiller: Arc<dyn MemoryDistiller> = Arc::new(NoopDistiller);
        let trigger = UserRememberTrigger;
        let event = MemoryEvent::UserRemember {
            scope: SessionKey::new("dingtalk", "g1"),
            target: MemoryTarget::Shared,
            content: "我们用 Redis".to_string(),
        };
        assert!(trigger.matches(&event));
        trigger
            .fire(event, Arc::clone(&store), distiller)
            .await
            .unwrap();
        let mem = store
            .load_shared_memory(&SessionKey::new("dingtalk", "g1"))
            .await
            .unwrap();
        assert!(mem.contains("我们用 Redis"));
    }

    #[tokio::test]
    async fn test_user_remember_agent_appends_to_agent() {
        let shared_dir = tempdir().unwrap();
        let persona_dir = tempdir().unwrap();
        let store: Arc<dyn MemoryStore> =
            Arc::new(FileMemoryStore::new(shared_dir.path().to_path_buf()));
        let distiller: Arc<dyn MemoryDistiller> = Arc::new(NoopDistiller);
        let trigger = UserRememberTrigger;
        let event = MemoryEvent::UserRemember {
            scope: SessionKey::new("dingtalk", "g1"),
            target: MemoryTarget::Agent {
                persona_dir: persona_dir.path().to_path_buf(),
            },
            content: "Alice 喜欢 Python".to_string(),
        };
        trigger
            .fire(event, Arc::clone(&store), distiller)
            .await
            .unwrap();
        let mem = store
            .load_agent_memory(persona_dir.path(), &SessionKey::new("dingtalk", "g1"))
            .await
            .unwrap();
        assert!(mem.contains("Alice 喜欢 Python"));
    }

    #[test]
    fn test_user_remember_trigger_does_not_match_other_events() {
        let trigger = UserRememberTrigger;
        let e = MemoryEvent::SessionIdle {
            scope: SessionKey::new("ws", "u"),
            agent: "bot".to_string(),
            persona_dir: std::path::PathBuf::from("/tmp"),
        };
        assert!(!trigger.matches(&e));
    }
}
