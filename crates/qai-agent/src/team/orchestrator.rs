//! TeamOrchestrator — 团队生命周期管理
//!
//! 职责：
//!   - start()  : 写 TEAM.md + TASKS.md，启动 OrchestratorHeartbeat
//!   - stop()   : 归档 team-session 目录
//!   - parse_done_marker() : 从 Specialist 输出中提取 [DONE: Txxx]
//!   - handle_specialist_done() : 更新 SQLite，检查里程碑
//!
//! 与 Heartbeat 分工：
//!   Heartbeat  = 定期调度（派发 Ready 任务、重置超时任务）
//!   Orchestrator = 事件响应（处理完成通知、检查里程碑、写文件）

use anyhow::Result;
use chrono::Utc;
use std::sync::Arc;
use tokio::task::JoinHandle;

use super::bus::{InternalBus, InternalMsg, InternalMsgType};
use super::heartbeat::{DispatchFn, OrchestratorHeartbeat};
use super::registry::TaskRegistry;
use super::session::TeamSession;

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

    // ── 启动 ──────────────────────────────────────────────────────────────────

    /// 应用 TeamPlan：写 TEAM.md / TASKS.md，注册任务，启动 Heartbeat，启动 MCP Server
    pub async fn start(self: &Arc<Self>, plan: &TeamPlan) -> Result<()> {
        // 1. 写 TEAM.md
        self.session.write_team_md(&plan.team_manifest)?;

        // 2. 注册所有任务到 TaskRegistry
        for task in &plan.tasks {
            use super::registry::CreateTask;
            self.registry.create_task(CreateTask {
                id: task.id.clone(),
                title: task.title.clone(),
                assignee_hint: task.assignee.clone(),
                deps: task.deps.clone(),
                timeout_secs: 1800,
                spec: task.spec.clone(),
                success_criteria: task.success_criteria.clone(),
            })?;
        }

        // 3. 导出初始 TASKS.md 快照
        self.session.sync_tasks_md(&self.registry)?;

        // 4. 启动 OrchestratorHeartbeat
        let heartbeat = Arc::new(OrchestratorHeartbeat::new(
            Arc::clone(&self.registry),
            Arc::clone(&self.session),
            Arc::clone(&self.dispatch_fn),
            self.heartbeat_interval,
        ));
        let handle = tokio::spawn({
            let hb = Arc::clone(&heartbeat);
            async move { hb.run().await }
        });
        *self.heartbeat_handle.lock().unwrap() = Some(handle);

        // 5. 启动 per-team MCP Server
        let mcp_srv = super::mcp_server::TeamToolServer::new(
            Arc::clone(&self.registry),
            Arc::clone(self),
            self.session.team_id.clone(),
        );
        let handle = mcp_srv.spawn().await?;
        let _ = self.mcp_server_port.set(handle.port);
        *self.mcp_server_handle.lock().await = Some(handle);

        tracing::info!(
            team_id = %self.session.team_id,
            tasks = plan.tasks.len(),
            mcp_port = ?self.mcp_server_port.get(),
            "Team started"
        );
        Ok(())
    }

    // ── 完成处理 ──────────────────────────────────────────────────────────────

    /// 从 Specialist 输出中提取 `[DONE: T003]` 标记（纯函数）
    pub fn parse_done_marker(output: &str) -> Option<String> {
        // 手动解析，避免 regex 依赖
        if let Some(start) = output.find("[DONE:") {
            let rest = &output[start + 6..]; // 跳过 "[DONE:"
            let rest = rest.trim_start();
            if let Some(end) = rest.find(']') {
                let id = rest[..end].trim();
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }
        None
    }

    /// 从 Specialist 输出中提取 `[BLOCKED: <原因>]` 标记（纯函数）
    pub fn parse_blocked_marker(output: &str) -> Option<String> {
        if let Some(start) = output.find("[BLOCKED:") {
            let rest = &output[start + 9..];
            let rest = rest.trim_start();
            if let Some(end) = rest.find(']') {
                let reason = rest[..end].trim();
                if !reason.is_empty() {
                    return Some(reason.to_string());
                }
            }
        }
        None
    }

    /// 处理 Specialist 完成通知
    ///
    /// 1. 更新 SQLite（mark_done）
    /// 2. 写事件日志
    /// 3. 导出 TASKS.md 快照
    /// 4. 检查里程碑（all_done 或新任务解锁）
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
        if self.registry.all_done()? {
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

        Ok(())
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
        // Stop MCP server
        if let Some(handle) = self.mcp_server_handle.lock().await.take() {
            handle.stop().await;
            tracing::info!(team_id = %self.session.team_id, "TeamMcpServer stopped");
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
    fn test_parse_done_marker() {
        assert_eq!(
            TeamOrchestrator::parse_done_marker("工作完成。[DONE: T003] 谢谢。"),
            Some("T003".to_string())
        );
        assert_eq!(
            TeamOrchestrator::parse_done_marker("[DONE: T001]"),
            Some("T001".to_string())
        );
        assert_eq!(
            TeamOrchestrator::parse_done_marker("没有标记的文本"),
            None
        );
    }

    #[test]
    fn test_parse_blocked_marker() {
        assert_eq!(
            TeamOrchestrator::parse_blocked_marker("[BLOCKED: 需要数据库连接串]"),
            Some("需要数据库连接串".to_string())
        );
        assert_eq!(TeamOrchestrator::parse_blocked_marker("正常文本"), None);
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
