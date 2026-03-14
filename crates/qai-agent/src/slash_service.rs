use crate::approval::ApprovalDecision;
use crate::control_reply::ControlReply;
use crate::memory_service::{handle_memory_request, MemoryRequest};
use crate::registry::SlashControlContext;
use crate::slash::SlashCommand;
use anyhow::Result;
use qai_protocol::SessionKey;

pub(crate) struct SlashRequest<'a> {
    pub session_key: &'a SessionKey,
    pub command: &'a SlashCommand,
    pub target_agent: Option<&'a str>,
    pub control: SlashControlContext<'a>,
}

pub(crate) async fn execute_slash_request(req: SlashRequest<'_>) -> Result<ControlReply> {
    match req.command {
        SlashCommand::Remember(_)
        | SlashCommand::Memory(_)
        | SlashCommand::Forget(_)
        | SlashCommand::MemoryReset => {
            handle_memory_request(MemoryRequest {
                session_key: req.session_key,
                command: req.command,
                target_agent: req.target_agent,
                control: req.control.memory(),
            })
            .await
        }
        SlashCommand::SetBackend(name) => {
            let backend_id = req.control.resolve_backend_id(name);
            req.control.set_session_backend(req.session_key, backend_id);
            Ok(ControlReply::Final(req.command.confirmation_text()))
        }
        SlashCommand::Reset => {
            req.control.clear_session_history(req.session_key).await;
            Ok(ControlReply::Final(req.command.confirmation_text()))
        }
        SlashCommand::Help => Ok(ControlReply::Final(req.command.confirmation_text())),
        SlashCommand::Workspace(path_opt) => match path_opt {
            None => Ok(ControlReply::Final(
                req.control
                    .render_workspace_summary(req.session_key, req.target_agent),
            )),
            Some(path_str) => {
                let path = std::path::PathBuf::from(path_str);
                if !path.exists() {
                    return Ok(ControlReply::Final(format!(
                        "Directory does not exist: `{path_str}`"
                    )));
                }
                if !path.is_dir() {
                    return Ok(ControlReply::Final(format!(
                        "Path is not a directory: `{path_str}`"
                    )));
                }
                req.control.set_session_workspace(req.session_key, path);
                Ok(ControlReply::Final(format!(
                    "Workspace set to: `{path_str}`\nNew agent turns will run in this directory."
                )))
            }
        },
        SlashCommand::Approve {
            approval_id,
            decision,
        } => {
            let Some(parsed) = ApprovalDecision::parse(decision) else {
                return Ok(ControlReply::Final(
                    "❌ 无效审批决定。使用：allow-once / allow-always / deny".to_string(),
                ));
            };
            let Some(resolver) = req.control.approval_resolver() else {
                return Ok(ControlReply::Final(
                    "❌ 当前运行实例未启用审批解析器。".to_string(),
                ));
            };
            let resolved = resolver.resolve(approval_id, parsed).await?;
            if resolved {
                return Ok(ControlReply::Final(format!(
                    "✅ 已处理审批 `{}` -> `{}`",
                    approval_id,
                    parsed.as_str()
                )));
            }
            Ok(ControlReply::Final(format!(
                "⚠️ 审批 `{}` 不存在、已过期，或已被处理。",
                approval_id
            )))
        }
        SlashCommand::TeamStatus => Ok(ControlReply::Final(
            req.control.render_team_status(req.session_key),
        )),
        SlashCommand::Clear => {
            // Clear conversation history (same as /reset)
            req.control.clear_session_history(req.session_key).await;
            // Clear team workspace (tasks, events, jsonl files, reset state to Planning)
            req.control.clear_team_workspace(req.session_key).await;
            Ok(ControlReply::Final("✅ 对话历史与团队工作区已全部清除".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::{ApprovalDecision, ApprovalResolver};
    use crate::memory::{distiller::NoopDistiller, store::FileMemoryStore, MemorySystem};
    use crate::registry::SessionRegistry;
    use crate::slash::SlashCommand;
    use crate::team::heartbeat::DispatchFn;
    use crate::team::orchestrator::TeamOrchestrator;
    use crate::team::registry::{CreateTask, TaskRegistry};
    use crate::team::session::TeamSession;
    use anyhow::Result;
    use qai_protocol::AgentEvent;
    use qai_session::{SessionManager, SessionStorage};
    use std::sync::{Arc, Mutex};
    use tokio::sync::broadcast;

    #[derive(Default)]
    struct FakeApprovalResolver {
        decisions: Arc<Mutex<Vec<(String, ApprovalDecision)>>>,
        result: bool,
    }

    #[async_trait::async_trait]
    impl ApprovalResolver for FakeApprovalResolver {
        async fn resolve(&self, approval_id: &str, decision: ApprovalDecision) -> Result<bool> {
            self.decisions
                .lock()
                .unwrap()
                .push((approval_id.to_string(), decision));
            Ok(self.result)
        }
    }

    fn make_registry() -> (Arc<SessionRegistry>, broadcast::Receiver<AgentEvent>) {
        let dir = std::env::temp_dir().join(format!("test-slash-service-{}", uuid::Uuid::new_v4()));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        let store: Arc<dyn crate::memory::MemoryStore> = Arc::new(FileMemoryStore::new(
            std::env::temp_dir().join(uuid::Uuid::new_v4().to_string()),
        ));
        let distiller: Arc<dyn crate::memory::MemoryDistiller> = Arc::new(NoopDistiller);
        let memory_system = MemorySystem::new(vec![], store, distiller);
        SessionRegistry::new(
            None,
            session_manager,
            String::new(),
            None,
            Some(memory_system),
            None,
            None,
            vec![],
        )
    }

    fn scope() -> SessionKey {
        SessionKey::new("ws", "slash-scope")
    }

    #[tokio::test]
    async fn help_command_returns_help_text() {
        let (registry, _rx) = make_registry();
        let command = SlashCommand::Help;
        let reply = execute_slash_request(SlashRequest {
            session_key: &scope(),
            command: &command,
            target_agent: None,
            control: registry.slash_control(),
        })
        .await
        .unwrap();
        assert!(reply.final_text().unwrap().contains("/backend"));
    }

    #[tokio::test]
    async fn approve_command_uses_registered_resolver() {
        let (registry, _rx) = make_registry();
        let resolver = Arc::new(FakeApprovalResolver {
            decisions: Arc::new(Mutex::new(Vec::new())),
            result: true,
        });
        registry.set_approval_resolver(resolver.clone());
        let command = SlashCommand::Approve {
            approval_id: "approval-1".into(),
            decision: "allow-once".into(),
        };
        let reply = execute_slash_request(SlashRequest {
            session_key: &scope(),
            command: &command,
            target_agent: None,
            control: registry.slash_control(),
        })
        .await
        .unwrap();
        assert!(reply.final_text().unwrap().contains("approval-1"));
        let recorded = resolver.decisions.lock().unwrap();
        assert_eq!(recorded[0].1, ApprovalDecision::AllowOnce);
    }

    #[tokio::test]
    async fn workspace_without_override_reports_default_message() {
        let (registry, _rx) = make_registry();
        let command = SlashCommand::Workspace(None);
        let reply = execute_slash_request(SlashRequest {
            session_key: &scope(),
            command: &command,
            target_agent: None,
            control: registry.slash_control(),
        })
        .await
        .unwrap();
        assert!(reply.final_text().unwrap().contains("Current workspace"));
    }

    #[tokio::test]
    async fn team_status_without_orchestrator_reports_inactive_team() {
        let (registry, _rx) = make_registry();
        let command = SlashCommand::TeamStatus;
        let reply = execute_slash_request(SlashRequest {
            session_key: &scope(),
            command: &command,
            target_agent: None,
            control: registry.slash_control(),
        })
        .await
        .unwrap();
        assert!(reply.final_text().unwrap().contains("没有活跃的 Team"));
    }

    #[tokio::test]
    async fn team_status_with_orchestrator_renders_summary() {
        let (registry, _rx) = make_registry();
        let tmp = tempfile::tempdir().unwrap();
        let lead = SessionKey::new("ws", "lead-scope");
        let team_session = Arc::new(TeamSession::from_dir("team-1", tmp.path().to_path_buf()));
        team_session.write_team_md("- lead: planner\n").unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        task_registry
            .create_task(CreateTask {
                title: "sample task".to_string(),
                spec: Some("demo".to_string()),
                ..CreateTask::default()
            })
            .unwrap();
        team_session.sync_tasks_md(task_registry.as_ref()).unwrap();
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orchestrator = TeamOrchestrator::new(
            task_registry,
            team_session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        orchestrator.set_lead_session_key(lead.clone());
        registry.register_team_orchestrator("team-1".to_string(), orchestrator);
        let command = SlashCommand::TeamStatus;
        let reply = execute_slash_request(SlashRequest {
            session_key: &lead,
            command: &command,
            target_agent: None,
            control: registry.slash_control(),
        })
        .await
        .unwrap();
        let text = reply.final_text().unwrap();
        assert!(text.contains("Team 状态"));
        assert!(text.contains("sample task"));
    }
}
