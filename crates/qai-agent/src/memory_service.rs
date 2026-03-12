use crate::control_reply::ControlReply;
use crate::memory::cap_to_words;
use crate::memory::MemoryEvent;
use crate::registry::MemoryControlContext;
use crate::slash::SlashCommand;
use anyhow::{bail, Result};
use qai_protocol::SessionKey;

pub(crate) struct MemoryRequest<'a> {
    pub session_key: &'a SessionKey,
    pub command: &'a SlashCommand,
    pub target_agent: Option<&'a str>,
    pub control: MemoryControlContext<'a>,
}

pub(crate) async fn handle_memory_request(req: MemoryRequest<'_>) -> Result<ControlReply> {
    if !req.control.is_enabled() {
        return Ok(ControlReply::Final(
            "❌ 当前运行实例未启用记忆系统。".to_string(),
        ));
    }

    match req.command {
        SlashCommand::Remember(content) => {
            let memory_target = req.control.resolve_memory_target(req.target_agent);
            if let Some(ms) = req.control.memory_system() {
                ms.emit(MemoryEvent::UserRemember {
                    scope: req.session_key.clone(),
                    target: memory_target,
                    content: content.clone(),
                });
            }
            Ok(ControlReply::Final(req.command.confirmation_text()))
        }
        SlashCommand::Memory(agent_opt) => match agent_opt {
            Some(agent_name) => Ok(ControlReply::Final(
                req.control
                    .read_agent_memory(agent_name, req.session_key)
                    .await?
                    .unwrap_or_else(|| format!("No memory found for agent @{agent_name}")),
            )),
            None => Ok(ControlReply::Final(
                render_shared_memory(req.control, req.session_key).await?,
            )),
        },
        SlashCommand::Forget(keyword) => {
            if let Some(ms) = req.control.memory_system() {
                let store = ms.store();
                let shared = store
                    .load_shared_memory(req.session_key)
                    .await
                    .unwrap_or_default();
                let keyword = keyword.to_lowercase();
                let filtered: String = shared
                    .lines()
                    .filter(|line| !line.to_lowercase().contains(&keyword))
                    .map(|line| format!("{line}\n"))
                    .collect();
                store
                    .overwrite_shared(req.session_key, &filtered)
                    .await
                    .ok();
            }
            Ok(ControlReply::Final(req.command.confirmation_text()))
        }
        SlashCommand::MemoryReset => handle_memory_reset(req.control, req.session_key).await,
        _ => bail!("non-memory slash command routed to memory_service"),
    }
}

async fn render_shared_memory(
    control: MemoryControlContext<'_>,
    session_key: &SessionKey,
) -> Result<String> {
    let Some(ms) = control.memory_system() else {
        return Ok("❌ 当前运行实例未启用记忆系统。".to_string());
    };
    let store = ms.store();
    let shared = store
        .load_shared_memory(session_key)
        .await
        .unwrap_or_default();
    let scope_display = &session_key.scope;
    if shared.is_empty() {
        return Ok(format!(
            "📭 当前还没有关于「{scope_display}」的记忆。\n\n可以告诉我一些背景，比如：\n- 团队用什么技术栈？\n- 有哪些编码规范？\n- 当前在做什么项目？\n\n或者直接 /remember <内容> 手动添加。"
        ));
    }
    let ts = store
        .shared_last_modified(session_key)
        .await
        .ok()
        .flatten()
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "未知".to_string());
    Ok(format!(
        "📚 当前记忆（{scope_display}）\n最后更新：{ts}\n\n{}\n\n输入 /remember <内容> 添加新记忆，/forget <关键词> 删除。",
        cap_to_words(&shared, 500)
    ))
}

async fn handle_memory_reset(
    control: MemoryControlContext<'_>,
    session_key: &SessionKey,
) -> Result<ControlReply> {
    let now = std::time::Instant::now();
    if control.consume_pending_reset_confirmation(session_key, now) {
        if let Some(ms) = control.memory_system() {
            ms.store().overwrite_shared(session_key, "").await.ok();
        }
        return Ok(ControlReply::Final("✅ 记忆已清空。".to_string()));
    }
    control.arm_pending_reset(session_key, now);
    Ok(ControlReply::Final(
        "⚠️ 你确定要清空当前 scope 的共享记忆吗？此操作不可撤销。\n再次发送 /memory reset 以确认（60 秒内有效）。"
            .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{
        distiller::NoopDistiller, store::FileMemoryStore, triggers::UserRememberTrigger,
        MemorySystem, MemoryTrigger,
    };
    use crate::registry::SessionRegistry;
    use crate::roster::{AgentEntry, AgentRoster};
    use crate::slash::SlashCommand;
    use qai_protocol::AgentEvent;
    use qai_session::{SessionManager, SessionStorage};
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::broadcast;

    fn make_registry_with_persona(
        default_persona_dir: Option<std::path::PathBuf>,
    ) -> (Arc<SessionRegistry>, broadcast::Receiver<AgentEvent>) {
        make_registry_with_roster(None, default_persona_dir)
    }

    fn make_registry_with_roster(
        roster: Option<AgentRoster>,
        default_persona_dir: Option<std::path::PathBuf>,
    ) -> (Arc<SessionRegistry>, broadcast::Receiver<AgentEvent>) {
        let dir =
            std::env::temp_dir().join(format!("test-memory-service-{}", uuid::Uuid::new_v4()));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        let store: Arc<dyn crate::memory::MemoryStore> = Arc::new(FileMemoryStore::new(
            std::env::temp_dir().join(uuid::Uuid::new_v4().to_string()),
        ));
        let distiller: Arc<dyn crate::memory::MemoryDistiller> = Arc::new(NoopDistiller);
        let triggers: Vec<Arc<dyn MemoryTrigger>> = vec![Arc::new(UserRememberTrigger)];
        let memory_system = MemorySystem::new(triggers, store, distiller);
        SessionRegistry::new(
            None,
            session_manager,
            String::new(),
            roster,
            Some(memory_system),
            default_persona_dir,
            None,
            vec![],
        )
    }

    fn make_registry() -> (Arc<SessionRegistry>, broadcast::Receiver<AgentEvent>) {
        make_registry_with_persona(None)
    }

    fn scope() -> SessionKey {
        SessionKey::new("ws", "memory-scope")
    }

    #[tokio::test]
    async fn empty_shared_memory_returns_guidance() {
        let (registry, _rx) = make_registry();
        let command = SlashCommand::Memory(None);
        let reply = handle_memory_request(MemoryRequest {
            session_key: &scope(),
            command: &command,
            target_agent: None,
            control: registry.memory_control(),
        })
        .await
        .unwrap();
        let text = reply.final_text().unwrap();
        assert!(text.contains("技术栈"));
        assert!(text.contains("编码规范"));
        assert!(text.contains("项目"));
    }

    #[tokio::test]
    async fn remember_and_forget_update_shared_memory() {
        let (registry, _rx) = make_registry();
        let session_key = scope();
        let remember = SlashCommand::Remember("我们用 Rust".to_string());
        handle_memory_request(MemoryRequest {
            session_key: &session_key,
            command: &remember,
            target_agent: None,
            control: registry.memory_control(),
        })
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let memory = SlashCommand::Memory(None);
        let reply = handle_memory_request(MemoryRequest {
            session_key: &session_key,
            command: &memory,
            target_agent: None,
            control: registry.memory_control(),
        })
        .await
        .unwrap();
        assert!(reply.final_text().unwrap().contains("我们用 Rust"));

        let forget = SlashCommand::Forget("rust".to_string());
        handle_memory_request(MemoryRequest {
            session_key: &session_key,
            command: &forget,
            target_agent: None,
            control: registry.memory_control(),
        })
        .await
        .unwrap();
        let reply = handle_memory_request(MemoryRequest {
            session_key: &session_key,
            command: &memory,
            target_agent: None,
            control: registry.memory_control(),
        })
        .await
        .unwrap();
        assert!(!reply.final_text().unwrap().contains("我们用 Rust"));
    }

    #[tokio::test]
    async fn memory_reset_requires_confirmation_then_clears() {
        let (registry, _rx) = make_registry();
        let session_key = scope();
        let remember = SlashCommand::Remember("短期记忆".to_string());
        handle_memory_request(MemoryRequest {
            session_key: &session_key,
            command: &remember,
            target_agent: None,
            control: registry.memory_control(),
        })
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let reset = SlashCommand::MemoryReset;
        let first = handle_memory_request(MemoryRequest {
            session_key: &session_key,
            command: &reset,
            target_agent: None,
            control: registry.memory_control(),
        })
        .await
        .unwrap();
        assert!(first
            .final_text()
            .unwrap()
            .contains("再次发送 /memory reset"));

        let second = handle_memory_request(MemoryRequest {
            session_key: &session_key,
            command: &reset,
            target_agent: None,
            control: registry.memory_control(),
        })
        .await
        .unwrap();
        assert_eq!(second.final_text(), Some("✅ 记忆已清空。"));
    }

    #[tokio::test]
    async fn memory_reset_expired_pending_rewarns() {
        let (registry, _rx) = make_registry();
        let session_key = scope();
        let expired = std::time::Instant::now() - std::time::Duration::from_secs(61);
        registry.inject_pending_reset_at(session_key.clone(), expired);

        let reset = SlashCommand::MemoryReset;
        let reply = handle_memory_request(MemoryRequest {
            session_key: &session_key,
            command: &reset,
            target_agent: None,
            control: registry.memory_control(),
        })
        .await
        .unwrap();
        assert!(reply
            .final_text()
            .unwrap()
            .contains("再次发送 /memory reset"));
    }

    #[tokio::test]
    async fn agent_memory_returns_missing_message_when_file_absent() {
        let persona_dir = tempdir().unwrap();
        let (registry, _rx) = make_registry_with_persona(Some(persona_dir.path().to_path_buf()));
        let session_key = scope();
        let command = SlashCommand::Memory(Some("reviewer".to_string()));
        let reply = handle_memory_request(MemoryRequest {
            session_key: &session_key,
            command: &command,
            target_agent: None,
            control: registry.memory_control(),
        })
        .await
        .unwrap();
        assert_eq!(
            reply.final_text(),
            Some("No memory found for agent @reviewer")
        );
    }

    #[tokio::test]
    async fn agent_memory_reads_persona_file_when_present() {
        let persona_dir = tempdir().unwrap();
        let roster = AgentRoster::new(vec![AgentEntry {
            name: "reviewer".to_string(),
            mentions: vec!["@reviewer".to_string()],
            backend_id: "reviewer-backend".to_string(),
            persona_dir: Some(persona_dir.path().to_path_buf()),
            workspace_dir: None,
            extra_skills_dirs: vec![],
        }]);
        let (registry, _rx) = make_registry_with_roster(Some(roster), None);
        let session_key = scope();
        registry
            .memory_control()
            .memory_system()
            .unwrap()
            .store()
            .append_to_agent_memory(persona_dir.path(), &session_key, "reviewer memory content")
            .await
            .unwrap();
        let command = SlashCommand::Memory(Some("reviewer".to_string()));
        let reply = handle_memory_request(MemoryRequest {
            session_key: &session_key,
            command: &command,
            target_agent: None,
            control: registry.memory_control(),
        })
        .await
        .unwrap();
        assert!(reply
            .final_text()
            .unwrap()
            .contains("reviewer memory content"));
    }

    #[tokio::test]
    async fn agent_memory_does_not_guess_from_default_persona_dir() {
        let persona_dir = tempdir().unwrap();
        let (registry, _rx) = make_registry_with_persona(Some(persona_dir.path().to_path_buf()));
        let session_key = scope();
        registry
            .memory_control()
            .memory_system()
            .unwrap()
            .store()
            .append_to_agent_memory(persona_dir.path(), &session_key, "single-engine memory")
            .await
            .unwrap();
        let command = SlashCommand::Memory(Some("reviewer".to_string()));
        let reply = handle_memory_request(MemoryRequest {
            session_key: &session_key,
            command: &command,
            target_agent: None,
            control: registry.memory_control(),
        })
        .await
        .unwrap();
        assert_eq!(
            reply.final_text(),
            Some("No memory found for agent @reviewer")
        );
    }

    #[tokio::test]
    async fn memory_commands_report_disabled_when_memory_system_missing() {
        let dir = std::env::temp_dir().join(format!(
            "test-memory-service-disabled-{}",
            uuid::Uuid::new_v4()
        ));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        let (registry, _rx) = SessionRegistry::new(
            None,
            session_manager,
            String::new(),
            None,
            None,
            None,
            None,
            vec![],
        );
        let session_key = scope();
        let command = SlashCommand::Memory(None);
        let reply = handle_memory_request(MemoryRequest {
            session_key: &session_key,
            command: &command,
            target_agent: None,
            control: registry.memory_control(),
        })
        .await
        .unwrap();
        assert_eq!(reply.final_text(), Some("❌ 当前运行实例未启用记忆系统。"));
    }
}
