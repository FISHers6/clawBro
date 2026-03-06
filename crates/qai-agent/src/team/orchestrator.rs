//! TeamOrchestrator — 团队生命周期管理
//!
//! 职责：
//!   - start()  : 写 TEAM.md + TASKS.md，启动 OrchestratorHeartbeat
//!   - stop()   : 归档 team-session 目录
//!   - handle_specialist_done() : 更新 SQLite，检查里程碑，推 TeamNotify 给 Lead
//!
//! 与 Heartbeat 分工：
//!   Heartbeat  = 定期调度（派发 Ready 任务、重置超时任务）
//!   Orchestrator = 事件响应（处理完成通知、检查里程碑、写文件）
//!
//! 注意：[DONE:]/[BLOCKED:] 文本标记已移除。完成通知通过 MCP complete_task 工具实现。

use anyhow::Result;
use chrono::Utc;
use std::sync::Arc;
use tokio::task::JoinHandle;

use super::bus::{InternalBus, InternalMsg, InternalMsgType};
use super::heartbeat::DispatchFn;
use super::registry::TaskRegistry;
use super::session::TeamSession;

// ─── TeamState ────────────────────────────────────────────────────────────────

/// Team Mode 执行状态机
#[derive(Debug, Clone, PartialEq)]
pub enum TeamState {
    /// Lead 正在通过 create_task() 建立任务图
    Planning,
    /// Lead 调用了 request_confirmation()，等待用户确认
    AwaitingConfirm,
    /// 任务执行中（Heartbeat 运行）
    Running,
    /// 所有任务已完成
    Done,
}

// ─── TeamPlan ─────────────────────────────────────────────────────────────────

/// Lead 规划产出的任务列表（在 /team start 之前由用户确认）
#[derive(Debug, Clone)]
pub struct TeamPlan {
    pub team_id: String,
    /// TEAM.md 内容（各 agent 职责说明）
    pub team_manifest: String,
    /// 任务列表（按依赖排序）
    pub tasks: Vec<PlannedTask>,
}

#[derive(Debug, Clone)]
pub struct PlannedTask {
    pub id: String,
    pub title: String,
    pub assignee: Option<String>,
    pub deps: Vec<String>,
    pub spec: Option<String>,
    pub success_criteria: Option<String>,
}

// ─── TeamOrchestrator ────────────────────────────────────────────────────────

/// 里程碑通知函数：(IM scope, message) → fire-and-forget to IM channel
pub type NotifyFn = Arc<dyn Fn(qai_protocol::SessionKey, String) + Send + Sync>;

pub struct TeamOrchestrator {
    pub registry: Arc<TaskRegistry>,
    pub session: Arc<TeamSession>,
    pub bus: Arc<InternalBus>,
    heartbeat_handle: std::sync::Mutex<Option<JoinHandle<()>>>,
    dispatch_fn: DispatchFn,
    heartbeat_interval: std::time::Duration,
    /// IM scope to forward milestone notifications to (set at team-start time).
    scope: std::sync::OnceLock<qai_protocol::SessionKey>,
    /// Sends milestone message to the IM channel (injected from main.rs).
    notify_fn: std::sync::OnceLock<NotifyFn>,
    /// Running HTTP MCP server handle (Some after start(), taken on stop()).
    /// Uses tokio::sync::Mutex because stop() is async.
    mcp_server_handle: tokio::sync::Mutex<Option<super::mcp_server::TeamMcpServerHandle>>,
    /// Bound port of the running MCP server (set once after start(), None until then).
    pub mcp_server_port: std::sync::OnceLock<u16>,
    /// Running Lead MCP server handle (Some after LeadMcpServer is spawned in main.rs).
    /// Kept alive here instead of leaking via mem::forget.
    lead_mcp_server_handle: tokio::sync::Mutex<Option<super::lead_mcp_server::LeadMcpServerHandle>>,
    /// 当前 Team 执行状态（Planning / AwaitingConfirm / Running / Done）
    pub team_state_inner: std::sync::Mutex<TeamState>,
    /// Lead Agent 的 IM session key（设置后用于 TeamNotify 路由）
    pub lead_session_key: std::sync::OnceLock<qai_protocol::SessionKey>,
    /// Configured Lead agent name from `front_bot` in config.toml.
    /// When set, registry uses this name to look up the roster engine for Lead turns
    /// that arrive without an explicit @mention.
    pub lead_agent_name: std::sync::OnceLock<String>,
    /// Bound port of the Lead MCP server (set after spawn in main.rs).
    pub lead_mcp_server_port: std::sync::OnceLock<u16>,
    /// TeamNotify MPSC sender — wired from main.rs after registry is ready.
    /// Used by handle_specialist_done() and failure handler to push notifications to Lead.
    team_notify_tx: std::sync::OnceLock<tokio::sync::mpsc::Sender<qai_protocol::InboundMsg>>,
}

impl TeamOrchestrator {
    pub fn new(
        registry: Arc<TaskRegistry>,
        session: Arc<TeamSession>,
        bus: Arc<InternalBus>,
        dispatch_fn: DispatchFn,
        heartbeat_interval: std::time::Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            registry,
            session,
            bus,
            heartbeat_handle: std::sync::Mutex::new(None),
            dispatch_fn,
            heartbeat_interval,
            scope: std::sync::OnceLock::new(),
            notify_fn: std::sync::OnceLock::new(),
            mcp_server_handle: tokio::sync::Mutex::new(None),
            mcp_server_port: std::sync::OnceLock::new(),
            lead_mcp_server_handle: tokio::sync::Mutex::new(None),
            team_state_inner: std::sync::Mutex::new(TeamState::Planning),
            lead_session_key: std::sync::OnceLock::new(),
            lead_agent_name: std::sync::OnceLock::new(),
            lead_mcp_server_port: std::sync::OnceLock::new(),
            team_notify_tx: std::sync::OnceLock::new(),
        })
    }

    // ── 里程碑通知接线 ────────────────────────────────────────────────────────

    /// 设置里程碑通知目标 IM scope（在 /team start 时调用）。
    pub fn set_scope(&self, scope: qai_protocol::SessionKey) {
        let _ = self.scope.set(scope);
    }

    /// 注入里程碑通知函数（main.rs 在启动时调用，提供 IM channel send 能力）。
    pub fn set_notify_fn(&self, f: NotifyFn) {
        let _ = self.notify_fn.set(f);
    }

    // ── Team 状态 ──────────────────────────────────────────────────────────────

    /// 获取当前 TeamState（克隆副本）
    pub fn team_state(&self) -> TeamState {
        self.team_state_inner.lock().unwrap().clone()
    }

    /// 设置 Lead 的 IM session key（由 main.rs 在启动时调用）
    pub fn set_lead_session_key(&self, key: qai_protocol::SessionKey) {
        let _ = self.lead_session_key.set(key);
    }

    /// 注入 TeamNotify MPSC sender（main.rs 在启动时调用）。
    /// handle_specialist_done() 和永久失败处理会用此 sender 推通知给 Lead。
    pub fn set_team_notify_tx(&self, tx: tokio::sync::mpsc::Sender<qai_protocol::InboundMsg>) {
        let _ = self.team_notify_tx.set(tx);
    }

    /// Set the Lead agent name (from `front_bot` in config.toml).
    /// Called by main.rs during wiring.
    pub fn set_lead_agent_name(&self, name: String) {
        let _ = self.lead_agent_name.set(name);
    }

    /// 存储 LeadMcpServer 句柄，防止其被 drop（替代 mem::forget）。
    /// 由 main.rs 在 LeadMcpServer 启动成功后调用。
    pub async fn store_lead_mcp_handle(&self, handle: super::lead_mcp_server::LeadMcpServerHandle) {
        *self.lead_mcp_server_handle.lock().await = Some(handle);
    }

    /// 向 IM 频道发布一条消息（Lead 调用 post_update 时使用）
    pub fn post_message(&self, message: &str) {
        if let (Some(f), Some(scope)) = (self.notify_fn.get(), self.scope.get()) {
            (f)(scope.clone(), message.to_string());
        }
    }

    // ── 增量任务注册（供 LeadMcpServer.create_task 调用）────────────────────

    /// 在 Planning 阶段注册单个任务。只能在 state == Planning 或 AwaitingConfirm 时调用。
    pub fn register_task(&self, task: super::registry::CreateTask) -> Result<String> {
        let state = self.team_state_inner.lock().unwrap().clone();
        if !matches!(state, TeamState::Planning | TeamState::AwaitingConfirm) {
            anyhow::bail!("Cannot register task: team is already {:?}", state);
        }
        let id = task.id.clone();
        self.registry.create_task(task)?;
        Ok(format!("Task {} registered.", id))
    }

    // ── 激活执行（供 LeadMcpServer.start_execution 调用）──────────────────

    /// 启动 Heartbeat + SpecialistMcpServer，设置 state → Running。
    /// 只允许调用一次（OnceLock guard）。
    pub async fn activate(self: &Arc<Self>) -> Result<String> {
        // Guard: already running?
        if self.mcp_server_port.get().is_some() {
            anyhow::bail!("TeamOrchestrator::activate() called twice");
        }
        // Transition state
        *self.team_state_inner.lock().unwrap() = TeamState::Running;

        // Write TEAM.md if not yet written
        let manifest = self.session.read_team_md();
        if manifest.is_empty() {
            let _ = self.session.write_team_md("Team execution started.");
        }

        // Sync TASKS.md snapshot
        self.session.sync_tasks_md(&self.registry)?;

        // Start Heartbeat (wire failure callback so permanent failures notify Lead)
        let self_for_failure = std::sync::Arc::clone(self);
        let failure_notify: super::heartbeat::FailureNotifyFn =
            std::sync::Arc::new(move |task_id: String, reason: String| {
                self_for_failure.dispatch_team_notify_failed(&task_id, &reason);
            });
        let heartbeat = std::sync::Arc::new(
            super::heartbeat::OrchestratorHeartbeat::new(
                std::sync::Arc::clone(&self.registry),
                std::sync::Arc::clone(&self.session),
                std::sync::Arc::clone(&self.dispatch_fn),
                self.heartbeat_interval,
            )
            .with_failure_notify(failure_notify),
        );
        let handle = tokio::spawn({
            let hb = std::sync::Arc::clone(&heartbeat);
            async move { hb.run().await }
        });
        *self.heartbeat_handle.lock().unwrap() = Some(handle);

        // Start SpecialistMcpServer
        let mcp_srv = super::mcp_server::TeamToolServer::new(
            std::sync::Arc::clone(&self.registry),
            std::sync::Arc::clone(self),
            self.session.team_id.clone(),
        );
        let mcp_handle = mcp_srv.spawn().await?;
        let _ = self.mcp_server_port.set(mcp_handle.port);
        *self.mcp_server_handle.lock().await = Some(mcp_handle);

        tracing::info!(
            team_id = %self.session.team_id,
            mcp_port = ?self.mcp_server_port.get(),
            "Team activated"
        );
        Ok("Team execution started.".to_string())
    }

    // ── 启动 ──────────────────────────────────────────────────────────────────

    /// 应用 TeamPlan：写 TEAM.md / TASKS.md，注册任务，启动 Heartbeat，启动 MCP Server
    pub async fn start(self: &Arc<Self>, plan: &TeamPlan) -> Result<()> {
        // Guard against double-start
        if self.mcp_server_port.get().is_some() {
            anyhow::bail!("TeamOrchestrator::start() called twice for team '{}'", self.session.team_id);
        }

        // 1. Write TEAM.md
        self.session.write_team_md(&plan.team_manifest)?;

        // 2. Register all tasks (sets state check, but start() bypasses it by writing directly to registry)
        for task in &plan.tasks {
            self.registry.create_task(super::registry::CreateTask {
                id: task.id.clone(),
                title: task.title.clone(),
                assignee_hint: task.assignee.clone(),
                deps: task.deps.clone(),
                timeout_secs: 1800,
                spec: task.spec.clone(),
                success_criteria: task.success_criteria.clone(),
            })?;
        }

        // 3. Activate (syncs TASKS.md, starts Heartbeat + MCP)
        self.activate().await?;

        tracing::info!(
            team_id = %self.session.team_id,
            tasks = plan.tasks.len(),
            "Team started via start()"
        );
        Ok(())
    }

    // ── 完成处理 ──────────────────────────────────────────────────────────────

    /// 处理 Specialist 完成通知（由 MCP complete_task 工具触发）（由 MCP complete_task 工具触发）
    ///
    /// 1. 更新 SQLite（mark_done）
    /// 2. 写事件日志
    /// 3. 导出 TASKS.md 快照
    /// 4. 检查里程碑（all_done 或新任务解锁）
    /// 5. 推 TeamNotify 给 Lead Agent
    pub fn handle_specialist_done(
        &self,
        task_id: &str,
        agent: &str,
        note: &str,
    ) -> Result<()> {
        // 1. 更新状态
        self.registry.mark_done(task_id, note)?;

        // 2. 事件日志
        let event = format!(
            r#"{{"event":"DONE","task":"{}","agent":"{}","ts":"{}"}}"#,
            task_id,
            agent,
            Utc::now().to_rfc3339()
        );
        let _ = self.session.append_event(&event);

        // 3. 导出快照
        let _ = self.session.sync_tasks_md(&self.registry);

        // 4. 里程碑检查
        let all_done = self.registry.all_done()?;
        if all_done {
            *self.team_state_inner.lock().unwrap() = TeamState::Done;
            self.publish_milestone("all_done", "所有任务已完成 ✅")?;
        } else {
            let ready = self.registry.find_ready_tasks()?;
            if !ready.is_empty() {
                let ids: Vec<_> = ready.iter().map(|t| t.id.as_str()).collect();
                self.publish_milestone(
                    "checkpoint",
                    &format!("新任务已解锁：{}", ids.join(", ")),
                )?;
            }
        }

        // 5. 推 TeamNotify 给 Lead
        self.dispatch_team_notify_done(task_id, agent, note, all_done);

        Ok(())
    }

    /// 构建并发送 TeamNotify InboundMsg 给 Lead（task 完成）
    fn dispatch_team_notify_done(&self, task_id: &str, agent: &str, note: &str, all_done: bool) {
        let lead_key = match self.lead_session_key.get().cloned() {
            Some(k) => k,
            None => return, // Lead key 未设置，静默跳过
        };
        let tx = match self.team_notify_tx.get() {
            Some(t) => t.clone(),
            None => return,
        };
        let tasks = self.registry.all_tasks().unwrap_or_default();
        let notify_content = if all_done {
            let summary = tasks.iter()
                .map(|t| format!(
                    "- {}（{}）：{}",
                    t.id,
                    t.assignee_hint.as_deref().unwrap_or("?"),
                    t.completion_note.as_deref().unwrap_or("完成")
                ))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "[团队通知] 所有任务已完成 ✅\n\n完成摘要：\n{}\n\n请生成最终汇总并通过 post_update 发送给用户。",
                summary
            )
        } else {
            let done_count = tasks.iter().filter(|t| t.status_raw == "done").count();
            let total = tasks.len();
            format!(
                "[团队通知] 任务 {} 已完成（执行者：{}）\n\n完成摘要：\n{}\n\n当前进度：{} / {} 完成",
                task_id, agent, note, done_count, total
            )
        };
        let lead_channel = lead_key.channel.clone();
        let msg = qai_protocol::InboundMsg {
            id: uuid::Uuid::new_v4().to_string(),
            session_key: lead_key,
            content: qai_protocol::MsgContent::text(notify_content),
            sender: "gateway".to_string(),
            channel: lead_channel,
            timestamp: Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: qai_protocol::MsgSource::TeamNotify,
        };
        // try_send avoids blocking in sync context; channel capacity 256 is safe for normal load
        if let Err(e) = tx.try_send(msg) {
            tracing::warn!(task_id = %task_id, "TeamNotify dispatch failed: {e}");
        }
    }

    /// 构建并发送 TeamNotify InboundMsg 给 Lead（task 永久失败）
    pub fn dispatch_team_notify_failed(&self, task_id: &str, reason: &str) {
        let lead_key = match self.lead_session_key.get().cloned() {
            Some(k) => k,
            None => return,
        };
        let tx = match self.team_notify_tx.get() {
            Some(t) => t.clone(),
            None => return,
        };
        let notify_content = format!(
            "[团队通知] 任务 {} 永久失败（已超过最大重试次数）\n\n原因：{}\n\n请调用 assign_task() 重新分配或调用 get_task_status() 查看全局状态。",
            task_id, reason
        );
        let lead_channel = lead_key.channel.clone();
        let msg = qai_protocol::InboundMsg {
            id: uuid::Uuid::new_v4().to_string(),
            session_key: lead_key,
            content: qai_protocol::MsgContent::text(notify_content),
            sender: "gateway".to_string(),
            channel: lead_channel,
            timestamp: Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: qai_protocol::MsgSource::TeamNotify,
        };
        if let Err(e) = tx.try_send(msg) {
            tracing::warn!(task_id = %task_id, "TeamNotify (failed) dispatch failed: {e}");
        }
    }

    /// 处理 Specialist 阻塞通知（Escalation → Lead）
    pub fn handle_specialist_blocked(
        &self,
        task_id: &str,
        agent: &str,
        reason: &str,
    ) -> Result<()> {
        let event = format!(
            r#"{{"event":"BLOCKED","task":"{}","agent":"{}","reason":"{}","ts":"{}"}}"#,
            task_id,
            agent,
            reason,
            Utc::now().to_rfc3339()
        );
        let _ = self.session.append_event(&event);

        // Escalation → Lead（通过 InternalBus）
        let msg = InternalMsg::new(
            format!("@{}", agent),
            "@lead",
            format!("Task {} blocked: {}", task_id, reason),
            InternalMsgType::Escalation,
            &self.session.team_id,
        );
        // 发送失败不中断流程（Lead 可能还未订阅）
        let _ = self.bus.send(msg);

        Ok(())
    }

    // ── 停止 ──────────────────────────────────────────────────────────────────

    /// 停止 Heartbeat、MCP Server 并归档 team-session
    pub async fn stop(&self) -> Result<()> {
        // Stop heartbeat
        if let Some(handle) = self.heartbeat_handle.lock().unwrap().take() {
            handle.abort();
        }
        // Stop Specialist MCP server
        if let Some(handle) = self.mcp_server_handle.lock().await.take() {
            handle.stop().await;
            tracing::info!(team_id = %self.session.team_id, "TeamMcpServer stopped");
        }
        // Stop Lead MCP server
        if let Some(handle) = self.lead_mcp_server_handle.lock().await.take() {
            handle.stop().await;
            tracing::info!(team_id = %self.session.team_id, "LeadMcpServer stopped");
        }
        // Cleanup InternalBus
        self.bus.cleanup_team(&self.session.team_id);
        // Archive directory
        self.session.archive()?;
        tracing::info!(team_id = %self.session.team_id, "Team stopped and archived");
        Ok(())
    }

    // ── 里程碑 ────────────────────────────────────────────────────────────────

    fn publish_milestone(&self, kind: &str, message: &str) -> Result<()> {
        let msg = InternalMsg::new(
            "@orchestrator",
            "broadcast",
            message,
            InternalMsgType::MilestoneUpdate,
            &self.session.team_id,
        );
        self.bus.broadcast_to_team(&self.session.team_id, msg);

        // Forward to IM channel if scope + notify_fn are wired (set at team-start time).
        if let (Some(f), Some(scope)) = (self.notify_fn.get(), self.scope.get()) {
            (f)(scope.clone(), message.to_string());
        }

        tracing::info!(
            team_id = %self.session.team_id,
            kind = %kind,
            "Milestone: {}", message
        );
        Ok(())
    }
}

// ─── 测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::registry::CreateTask;
    use std::sync::Mutex;
    use tempfile::tempdir;

    fn make_orchestrator() -> (Arc<TeamOrchestrator>, tempfile::TempDir) {
        let tmp = tempdir().unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("test-team", tmp.path().to_path_buf()));
        let bus = Arc::new(InternalBus::new());
        let dispatch_fn: DispatchFn = Arc::new(|_agent, _task, _session| {
            Box::pin(async { Ok(()) })
        });
        let orch = TeamOrchestrator::new(
            registry,
            session,
            bus,
            dispatch_fn,
            std::time::Duration::from_secs(3600), // 测试中不实际触发
        );
        (orch, tmp)
    }

    #[test]
    fn test_register_task_increments_registry() {
        let (orch, _tmp) = make_orchestrator();
        let result = orch.register_task(CreateTask {
            id: "T001".into(),
            title: "Write DB schema".into(),
            ..Default::default()
        });
        assert!(result.is_ok());
        assert!(result.unwrap().contains("T001"));
        let task = orch.registry.get_task("T001").unwrap().unwrap();
        assert_eq!(task.title, "Write DB schema");
    }

    #[test]
    fn test_team_state_starts_planning() {
        let (orch, _tmp) = make_orchestrator();
        assert!(matches!(orch.team_state(), TeamState::Planning));
    }

    #[tokio::test]
    async fn test_activate_starts_mcp_and_sets_running() {
        let (orch, _tmp) = make_orchestrator();
        orch.register_task(CreateTask {
            id: "T001".into(),
            title: "test".into(),
            ..Default::default()
        }).unwrap();
        orch.activate().await.unwrap();
        assert!(matches!(orch.team_state(), TeamState::Running));
        assert!(orch.mcp_server_port.get().is_some());
    }

    #[test]
    fn test_handle_specialist_done_updates_registry() {
        let (orch, _tmp) = make_orchestrator();
        orch.registry
            .create_task(CreateTask {
                id: "T003".into(),
                title: "JWT impl".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T003", "codex").unwrap();

        orch.handle_specialist_done("T003", "codex", "created jwt.rs")
            .unwrap();

        let task = orch.registry.get_task("T003").unwrap().unwrap();
        use crate::team::registry::TaskStatus;
        assert!(matches!(task.status_parsed(), TaskStatus::Done));
        assert_eq!(task.completion_note.as_deref(), Some("created jwt.rs"));
    }

    #[tokio::test]
    async fn test_all_done_triggers_milestone_broadcast() {
        let (orch, _tmp) = make_orchestrator();
        let received = Arc::new(Mutex::new(vec![]));
        let received_clone = Arc::clone(&received);
        let mut rx = orch.bus.subscribe("test-team", "@listener");

        // 在后台收消息
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                received_clone.lock().unwrap().push(msg.content.clone());
            }
        });

        orch.registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "only task".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T001", "codex").unwrap();
        orch.handle_specialist_done("T001", "codex", "done").unwrap();
        // publish_milestone 广播 → bus → @listener
    }

    #[tokio::test]
    async fn test_start_registers_tasks_and_writes_team_md() {
        let (orch, _tmp) = make_orchestrator();

        let plan = TeamPlan {
            team_id: "test-team".into(),
            team_manifest: "Claude: Lead\nCodex: Backend".into(),
            tasks: vec![PlannedTask {
                id: "T001".into(),
                title: "Setup".into(),
                assignee: Some("codex".into()),
                deps: vec![],
                spec: None,
                success_criteria: None,
            }],
        };

        orch.start(&plan).await.unwrap();

        let team_md = orch.session.read_team_md();
        assert!(team_md.contains("Claude: Lead"));

        let task = orch.registry.get_task("T001").unwrap().unwrap();
        assert_eq!(task.title, "Setup");
    }
}
