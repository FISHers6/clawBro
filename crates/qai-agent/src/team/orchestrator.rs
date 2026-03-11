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
//! 注意：[DONE:]/[BLOCKED:] 文本标记已移除。当前 ACP-family 完成通知通过
//! `SharedTeamToolServer` 的 `complete_task` / `block_task` 工具实现；
//! canonical multi-backend semantics 将在 qai-runtime::tool_bridge 中升级为
//! submit/accept/reopen 流程。

use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use std::sync::Arc;
use tokio::task::JoinHandle;

use super::heartbeat::DispatchFn;
use super::registry::{Task, TaskRegistry};
use super::session::{TaskArtifactMeta, TeamSession};

// ─── TeamState ────────────────────────────────────────────────────────────────

/// Team Mode 执行状态机
#[derive(Debug, Clone, PartialEq, Serialize)]
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

#[derive(Debug, Clone, Default, Serialize)]
pub struct TeamTaskCounts {
    pub total: usize,
    pub pending: usize,
    pub claimed: usize,
    pub submitted: usize,
    pub accepted: usize,
    pub done: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TeamArtifactHealthSummary {
    pub root_present: bool,
    pub team_md_present: bool,
    pub context_md_present: bool,
    pub tasks_md_present: bool,
    pub task_artifacts_present: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TeamRuntimeSummary {
    pub team_id: String,
    pub state: TeamState,
    pub lead_session_key: Option<qai_protocol::SessionKey>,
    pub lead_agent_name: Option<String>,
    pub specialists: Vec<String>,
    pub tool_surface_ready: bool,
    pub mcp_port: Option<u16>,
    pub task_counts: TeamTaskCounts,
    pub artifact_health: TeamArtifactHealthSummary,
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
    heartbeat_handle: std::sync::Mutex<Option<JoinHandle<()>>>,
    dispatch_fn: DispatchFn,
    heartbeat_interval: std::time::Duration,
    max_parallel: std::sync::Mutex<usize>,
    /// IM scope to forward milestone notifications to (set at team-start time).
    scope: std::sync::OnceLock<qai_protocol::SessionKey>,
    /// Sends milestone message to the IM channel (injected from main.rs).
    notify_fn: std::sync::OnceLock<NotifyFn>,
    /// Unified MCP server handle (Lead + Specialist tools on one port, spawned at startup).
    /// Uses tokio::sync::Mutex because stop() is async.
    mcp_server_handle: tokio::sync::Mutex<Option<super::shared_mcp_server::SharedMcpServerHandle>>,
    /// Bound port of the unified MCP server (set once after spawn, used by all agents).
    pub mcp_server_port: std::sync::OnceLock<u16>,
    /// 当前 Team 执行状态（Planning / AwaitingConfirm / Running / Done）
    pub team_state_inner: std::sync::Mutex<TeamState>,
    /// Lead Agent 的 IM session key（设置后用于 TeamNotify 路由）
    pub lead_session_key: std::sync::OnceLock<qai_protocol::SessionKey>,
    /// Configured Lead agent name from `front_bot` in config.toml.
    pub lead_agent_name: std::sync::OnceLock<String>,
    /// List of Specialist agent names (from `team.roster` in config.toml).
    pub available_specialists: std::sync::OnceLock<Vec<String>>,
    /// TeamNotify MPSC sender — wired from main.rs after registry is ready.
    team_notify_tx: std::sync::OnceLock<tokio::sync::mpsc::Sender<qai_protocol::InboundMsg>>,
    #[cfg(test)]
    test_mcp_start_result: std::sync::Mutex<Option<std::result::Result<u16, String>>>,
}

impl TeamOrchestrator {
    pub fn new(
        registry: Arc<TaskRegistry>,
        session: Arc<TeamSession>,
        dispatch_fn: DispatchFn,
        heartbeat_interval: std::time::Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            registry,
            session,
            heartbeat_handle: std::sync::Mutex::new(None),
            dispatch_fn,
            heartbeat_interval,
            max_parallel: std::sync::Mutex::new(3),
            scope: std::sync::OnceLock::new(),
            notify_fn: std::sync::OnceLock::new(),
            mcp_server_handle: tokio::sync::Mutex::new(None),
            mcp_server_port: std::sync::OnceLock::new(),
            team_state_inner: std::sync::Mutex::new(TeamState::Planning),
            lead_session_key: std::sync::OnceLock::new(),
            lead_agent_name: std::sync::OnceLock::new(),
            available_specialists: std::sync::OnceLock::new(),
            team_notify_tx: std::sync::OnceLock::new(),
            #[cfg(test)]
            test_mcp_start_result: std::sync::Mutex::new(None),
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

    /// Set the list of available Specialist agents (from `team.roster` in config.toml).
    /// Called by main.rs during wiring so lead_layer_0 can list assignable agents.
    pub fn set_available_specialists(&self, agents: Vec<String>) {
        let _ = self.available_specialists.set(agents);
    }

    /// Configure the max number of concurrent specialist dispatches for the heartbeat.
    pub fn set_max_parallel(&self, max_parallel: usize) {
        *self.max_parallel.lock().unwrap() = max_parallel.max(1);
    }

    #[cfg(test)]
    pub fn set_test_mcp_start_result(&self, result: std::result::Result<u16, String>) {
        *self.test_mcp_start_result.lock().unwrap() = Some(result);
    }

    /// 向 IM 频道发布一条消息（Lead 调用 post_update 时使用）
    pub fn post_message(&self, message: &str) {
        if let (Some(f), Some(scope)) = (self.notify_fn.get(), self.scope.get()) {
            (f)(scope.clone(), message.to_string());
        }
    }

    pub fn status_snapshot(&self) -> TeamRuntimeSummary {
        let tasks = self.registry.all_tasks().unwrap_or_default();
        let mut counts = TeamTaskCounts {
            total: tasks.len(),
            ..TeamTaskCounts::default()
        };
        for task in &tasks {
            let status = task.status_raw.as_str();
            if status == "pending" {
                counts.pending += 1;
            } else if status.starts_with("claimed:") {
                counts.claimed += 1;
            } else if status.starts_with("submitted:") {
                counts.submitted += 1;
            } else if status.starts_with("accepted:") {
                counts.accepted += 1;
            } else if status == "done" {
                counts.done += 1;
            } else if status.starts_with("failed") {
                counts.failed += 1;
            }
        }
        let artifact_health = TeamArtifactHealthSummary {
            root_present: self.session.dir.is_dir(),
            team_md_present: self.session.dir.join("TEAM.md").is_file(),
            context_md_present: self.session.dir.join("CONTEXT.md").is_file(),
            tasks_md_present: self.session.dir.join("TASKS.md").is_file(),
            task_artifacts_present: self.session.dir.join("tasks").is_dir(),
        };

        TeamRuntimeSummary {
            team_id: self.session.team_id.clone(),
            state: self.team_state(),
            lead_session_key: self.lead_session_key.get().cloned(),
            lead_agent_name: self.lead_agent_name.get().cloned(),
            specialists: self
                .available_specialists
                .get()
                .cloned()
                .unwrap_or_default(),
            tool_surface_ready: self.mcp_server_port.get().is_some(),
            mcp_port: self.mcp_server_port.get().copied(),
            task_counts: counts,
            artifact_health,
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
        self.sync_task_artifacts(&id)?;
        Ok(format!("Task {} registered.", id))
    }

    // ── 激活执行（供 LeadMcpServer.start_execution 调用）──────────────────

    /// Eagerly start the unified SharedTeamMcpServer so both Lead and Specialist
    /// agents receive `mcp_server_url` from the very first turn.
    ///
    /// Called from `main.rs` immediately after creating the orchestrator, before any
    /// agent turn runs.  `activate()` (called later by the Lead via `start_execution`)
    /// skips the MCP spawn if the port is already set.
    pub async fn start_mcp_server(self: &Arc<Self>) -> Result<()> {
        if self.mcp_server_port.get().is_some() {
            return Ok(()); // already started
        }
        #[cfg(test)]
        if let Some(result) = self.test_mcp_start_result.lock().unwrap().take() {
            match result {
                Ok(port) => {
                    let _ = self.mcp_server_port.set(port);
                    tracing::info!(
                        team_id = %self.session.team_id,
                        mcp_port = ?self.mcp_server_port.get(),
                        "SharedTeamMcpServer test port injected"
                    );
                    return Ok(());
                }
                Err(message) => anyhow::bail!(message),
            }
        }
        let mcp_srv = super::shared_mcp_server::SharedTeamToolServer::new(Arc::clone(self));
        let mcp_handle = mcp_srv.spawn().await?;
        let _ = self.mcp_server_port.set(mcp_handle.port);
        *self.mcp_server_handle.lock().await = Some(mcp_handle);
        tracing::info!(
            team_id = %self.session.team_id,
            mcp_port = ?self.mcp_server_port.get(),
            "SharedTeamMcpServer started eagerly"
        );
        Ok(())
    }

    /// 启动 Heartbeat，设置 state → Running。
    /// MCP server is started eagerly via `start_mcp_server()`; this method only starts
    /// the heartbeat dispatch loop and writes team manifest files.
    pub async fn activate(self: &Arc<Self>) -> Result<String> {
        // Guard: already running?
        let already_activated = {
            let state = self.team_state_inner.lock().unwrap();
            *state == TeamState::Running || *state == TeamState::Done
        };
        if already_activated {
            anyhow::bail!("TeamOrchestrator::activate() called twice");
        }

        // Team execution must not transition to Running until the tool surface is reachable.
        if self.mcp_server_port.get().is_none() {
            self.start_mcp_server().await?;
        }

        // Write TEAM.md if not yet written
        let manifest = self.session.read_team_md();
        if manifest.is_empty() {
            let _ = self.session.write_team_md("Team execution started.");
        }

        // Write AGENTS.md — claude-code reads this automatically from workspace_dir,
        // providing true system-level context for both Lead and Specialists.
        let specialists_list = self
            .available_specialists
            .get()
            .map(|v| {
                v.iter()
                    .map(|s| format!("- **{}**", s))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|| "（未配置）".to_string());
        let lead_name = self
            .lead_agent_name
            .get()
            .map(|s| s.as_str())
            .unwrap_or("Lead");
        let agents_md = format!(
            "# Team Mode — Agent Roster\n\n\
             ## Lead Agent: {lead_name}\n\
             负责任务规划和协调。可用工具：\n\
             - `create_task(id, title, spec, deps, assignee, success_criteria)` — 注册子任务\n\
             - `start_execution()` — 启动所有 Ready 任务的并行执行\n\
             - `request_confirmation(plan_summary)` — 复杂任务执行前请求用户确认\n\
             - `post_update(message)` — 向用户播报进度\n\
             - `get_task_status()` — 查看全部任务状态\n\
             - `assign_task(task_id, agent)` — 重新分配任务给指定 Specialist\n\n\
             ## Specialist Agents\n\
             {specialists_list}\n\n\
             各 Specialist 独立执行分配的任务，完成后调用：\n\
             - `complete_task(task_id, note)` — 标记任务完成，note 为关键产出摘要\n\
             - `block_task(task_id, reason)` — 任务无法完成时上报阻塞原因\n\n\
             ## 工作流\n\
             1. Lead 调用 `create_task()` 定义任务图（含依赖关系）\n\
             2. Lead 调用 `start_execution()` 触发调度\n\
             3. Heartbeat 自动将 Ready 任务派发给对应 Specialist\n\
             4. Specialist 完成后调用 `complete_task()`，Lead 收到 `[团队通知]`\n\
             5. 所有任务完成后 Lead 合成最终结果并 `post_update()` 给用户\n",
            lead_name = lead_name,
            specialists_list = specialists_list,
        );
        let _ = self.session.write_agents_md(&agents_md);

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
                *self.max_parallel.lock().unwrap(),
            )
            .with_failure_notify(failure_notify),
        );
        let handle = tokio::spawn({
            let hb = std::sync::Arc::clone(&heartbeat);
            async move { hb.run().await }
        });
        *self.heartbeat_handle.lock().unwrap() = Some(handle);
        *self.team_state_inner.lock().unwrap() = TeamState::Running;

        tracing::info!(
            team_id = %self.session.team_id,
            mcp_port = ?self.mcp_server_port.get(),
            "Team activated"
        );
        Ok("Team execution started.".to_string())
    }

    // ── 启动 ──────────────────────────────────────────────────────────────────

    /// 应用 TeamPlan：写 TEAM.md / TASKS.md，注册任务，启动 Heartbeat
    pub async fn start(self: &Arc<Self>, plan: &TeamPlan) -> Result<()> {
        // Guard against double-start (use state, not mcp_server_port — port is now always set eagerly)
        let already_started = {
            let state = self.team_state_inner.lock().unwrap();
            *state == TeamState::Running || *state == TeamState::Done
        };
        if already_started {
            anyhow::bail!(
                "TeamOrchestrator::start() called twice for team '{}'",
                self.session.team_id
            );
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
            self.sync_task_artifacts(&task.id)?;
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
    pub fn handle_specialist_done(&self, task_id: &str, agent: &str, note: &str) -> Result<()> {
        // 1. 更新状态（校验认领者身份）
        self.registry.mark_done(task_id, agent, note)?;
        self.sync_task_artifacts(task_id)?;
        let _ = self.session.write_task_result(
            task_id,
            &format!(
                "# Result\n\nSubmitted by: {agent}\n\nFinal note:\n{note}\n"
            ),
        );

        // 2. 事件日志
        let event = serde_json::json!({
            "event": "DONE",
            "task": task_id,
            "agent": agent,
            "ts": Utc::now().to_rfc3339(),
        })
        .to_string();
        let _ = self.session.append_event(&event);

        // 3. 导出快照
        let _ = self.session.sync_tasks_md(&self.registry);

        // 4. 里程碑检查
        let all_done = self.registry.all_done()?;
        if all_done {
            *self.team_state_inner.lock().unwrap() = TeamState::Done;
            self.publish_milestone("all_done", "所有任务已完成 ✅")?;
        } else {
            // 当前任务完成通知（all_tasks() 只调用一次，task_title 从结果中提取）
            let tasks = self.registry.all_tasks().unwrap_or_default();
            let task_title = tasks
                .iter()
                .find(|t| t.id == task_id)
                .map(|t| t.title.clone())
                .unwrap_or_else(|| task_id.to_string());
            let done_count = tasks
                .iter()
                .filter(|t| t.status_raw == "done" || t.status_raw.starts_with("accepted:"))
                .count();
            let total = tasks.len();
            self.publish_milestone(
                "done",
                &format!(
                    "✅ 任务 {}「{}」@{} 已完成（{}/{}）",
                    task_id, task_title, agent, done_count, total
                ),
            )?;
            // 下游任务解锁通知
            let ready = self.registry.find_ready_tasks()?;
            if !ready.is_empty() {
                let ids: Vec<_> = ready.iter().map(|t| t.id.as_str()).collect();
                self.publish_milestone("unlocked", &format!("🔓 新任务已解锁：{}", ids.join(", ")))?;
            }
        }

        // 5. 推 TeamNotify 给 Lead
        self.dispatch_team_notify_done(task_id, agent, note, all_done);

        Ok(())
    }

    /// 处理 Specialist 提交结果（新语义：submitted，等待 Lead 验收）
    pub fn handle_specialist_submitted(
        &self,
        task_id: &str,
        agent: &str,
        summary: &str,
    ) -> Result<()> {
        self.registry.submit_task_result(task_id, agent, summary)?;
        self.sync_task_artifacts(task_id)?;
        let _ = self.session.write_task_result(
            task_id,
            &format!(
                "# Result\n\nSubmitted by: {agent}\n\nSummary:\n{summary}\n"
            ),
        );

        let event = serde_json::json!({
            "event": "SUBMITTED",
            "task": task_id,
            "agent": agent,
            "ts": Utc::now().to_rfc3339(),
        })
        .to_string();
        let _ = self.session.append_event(&event);
        let _ = self.session.sync_tasks_md(&self.registry);

        self.dispatch_team_notify_submitted(task_id, agent, summary);
        let task_title = self
            .registry
            .get_task(task_id)
            .ok()
            .flatten()
            .map(|t| t.title)
            .unwrap_or_else(|| task_id.to_string());
        let _ = self.publish_milestone(
            "submitted",
            &format!("📨 任务 {}「{}」@{} 已提交待验收", task_id, task_title, agent),
        );
        Ok(())
    }

    /// Lead 验收已提交任务（submitted -> accepted），并复用里程碑检查逻辑。
    pub fn accept_submitted_task(&self, task_id: &str, by: &str) -> Result<()> {
        self.registry.accept_task(task_id, by)?;
        self.sync_task_artifacts(task_id)?;
        let _ = self.session.append_task_progress(
            task_id,
            &format!(
                "[{}] {} accepted submission",
                Utc::now().to_rfc3339(),
                by
            ),
        );
        let previous = self
            .session
            .task_dir(task_id)
            .join("result.md");
        let acceptance_note = format!(
            "\n\n## Acceptance\n\nAccepted by: {by}\nTimestamp: {}\n",
            Utc::now().to_rfc3339()
        );
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(previous)
            .and_then(|mut file| {
                use std::io::Write;
                file.write_all(acceptance_note.as_bytes())
            });

        let event = serde_json::json!({
            "event": "ACCEPTED",
            "task": task_id,
            "by": by,
            "ts": Utc::now().to_rfc3339(),
        })
        .to_string();
        let _ = self.session.append_event(&event);
        let _ = self.session.sync_tasks_md(&self.registry);

        let all_done = self.registry.all_done()?;
        if all_done {
            *self.team_state_inner.lock().unwrap() = TeamState::Done;
            self.publish_milestone("all_done", "所有任务已完成 ✅")?;
        } else {
            let ready = self.registry.find_ready_tasks()?;
            if !ready.is_empty() {
                let ids: Vec<_> = ready.iter().map(|t| t.id.as_str()).collect();
                self.publish_milestone("checkpoint", &format!("新任务已解锁：{}", ids.join(", ")))?;
            }
        }

        self.dispatch_team_notify_accepted(task_id, by, all_done);
        Ok(())
    }

    /// Lead 重新打开已提交/已验收任务，退回 pending 并通知团队。
    pub fn reopen_submitted_task(&self, task_id: &str, reason: &str, by: &str) -> Result<()> {
        self.registry.reopen_task(task_id, reason)?;
        self.sync_task_artifacts(task_id)?;
        let _ = self.session.append_task_progress(
            task_id,
            &format!(
                "[{}] {} reopened task: {}",
                Utc::now().to_rfc3339(),
                by,
                reason
            ),
        );

        let event = serde_json::json!({
            "event": "REOPENED",
            "task": task_id,
            "by": by,
            "reason": reason,
            "ts": Utc::now().to_rfc3339(),
        })
        .to_string();
        let _ = self.session.append_event(&event);
        let _ = self.session.sync_tasks_md(&self.registry);

        self.dispatch_team_notify_reopened(task_id, by, reason);
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
            let summary = tasks
                .iter()
                .map(|t| {
                    format!(
                        "- {}（{}）：{}",
                        t.id,
                        t.assignee_hint.as_deref().unwrap_or("?"),
                        t.completion_note.as_deref().unwrap_or("完成")
                    )
                })
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
        // send().await via spawn: avoids blocking this sync fn while guaranteeing delivery.
        // try_send was silently dropping notifications when the channel was full under load.
        let task_id = task_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = tx.send(msg).await {
                tracing::warn!(task_id = %task_id, "TeamNotify dispatch failed: {e}");
            }
        });
    }

    fn dispatch_team_notify_submitted(&self, task_id: &str, agent: &str, summary: &str) {
        let lead_key = match self.lead_session_key.get().cloned() {
            Some(k) => k,
            None => return,
        };
        let tx = match self.team_notify_tx.get() {
            Some(t) => t.clone(),
            None => return,
        };
        let notify_content = format!(
            "[团队通知] 任务 {} 已提交待验收（执行者：{}）\n\n提交摘要：\n{}\n\n请检查结果，并决定 accept 或 reopen。",
            task_id, agent, summary
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
        let task_id = task_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = tx.send(msg).await {
                tracing::warn!(task_id = %task_id, "TeamNotify (submitted) dispatch failed: {e}");
            }
        });
    }

    fn dispatch_team_notify_accepted(&self, task_id: &str, by: &str, all_done: bool) {
        let lead_key = match self.lead_session_key.get().cloned() {
            Some(k) => k,
            None => return,
        };
        let tx = match self.team_notify_tx.get() {
            Some(t) => t.clone(),
            None => return,
        };
        let notify_content = if all_done {
            format!(
                "[团队通知] 任务 {} 已验收（验收者：{}）\n\n所有任务现已完成，请生成最终汇总并通过 post_update 发送给用户。",
                task_id, by
            )
        } else {
            format!(
                "[团队通知] 任务 {} 已验收（验收者：{}）\n\n如有新解锁任务，Heartbeat 将继续派发。",
                task_id, by
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
        let task_id = task_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = tx.send(msg).await {
                tracing::warn!(task_id = %task_id, "TeamNotify (accepted) dispatch failed: {e}");
            }
        });
    }

    fn dispatch_team_notify_reopened(&self, task_id: &str, by: &str, reason: &str) {
        let lead_key = match self.lead_session_key.get().cloned() {
            Some(k) => k,
            None => return,
        };
        let tx = match self.team_notify_tx.get() {
            Some(t) => t.clone(),
            None => return,
        };
        let notify_content = format!(
            "[团队通知] 任务 {} 已重新打开（操作者：{}）\n\n原因：{}\n\nHeartbeat 将在依赖满足时重新派发该任务。",
            task_id, by, reason
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
        let task_id = task_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = tx.send(msg).await {
                tracing::warn!(task_id = %task_id, "TeamNotify (reopened) dispatch failed: {e}");
            }
        });
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
        let task_id = task_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = tx.send(msg).await {
                tracing::warn!(task_id = %task_id, "TeamNotify (failed) dispatch failed: {e}");
            }
        });
    }

    /// 处理 Specialist 阻塞通知（Escalation → Lead via team_notify_tx）
    pub fn handle_specialist_blocked(
        &self,
        task_id: &str,
        agent: &str,
        reason: &str,
    ) -> Result<()> {
        // ── Identity check: only the agent that claimed this task can report it blocked ──
        anyhow::ensure!(
            self.registry.is_claimed_by(task_id, agent)?,
            "block_task: agent '{}' does not own task '{}'",
            agent,
            task_id,
        );
        // ──────────────────────────────────────────────────────────────────────────────────

        let event = serde_json::json!({
            "event": "BLOCKED",
            "task": task_id,
            "agent": agent,
            "reason": reason,
            "ts": Utc::now().to_rfc3339(),
        })
        .to_string();
        let _ = self.session.append_event(&event);
        self.registry.reset_claim(task_id)?;
        self.sync_task_artifacts(task_id)?;
        let _ = self.session.append_task_progress(
            task_id,
            &format!(
                "[{}] {} blocked task: {}",
                Utc::now().to_rfc3339(),
                agent,
                reason
            ),
        );

        // Escalation → Lead via team_notify_tx (same path as task completion)
        self.dispatch_team_notify_blocked(task_id, agent, reason);
        let task_title = self
            .registry
            .get_task(task_id)
            .ok()
            .flatten()
            .map(|t| t.title)
            .unwrap_or_else(|| task_id.to_string());
        let _ = self.publish_milestone(
            "blocked",
            &format!("🚧 任务 {}「{}」@{} 阻塞：{}", task_id, task_title, agent, reason),
        );

        Ok(())
    }

    /// 处理 Specialist 中间检查点，不改变任务状态，只通知 Lead 当前进展。
    pub fn handle_specialist_checkpoint(
        &self,
        task_id: &str,
        agent: &str,
        note: &str,
    ) -> Result<()> {
        let event = serde_json::json!({
            "event": "CHECKPOINT",
            "task": task_id,
            "agent": agent,
            "note": note,
            "ts": Utc::now().to_rfc3339(),
        })
        .to_string();
        let _ = self.session.append_event(&event);
        let _ = self.session.append_task_progress(
            task_id,
            &format!(
                "[{}] {} checkpoint: {}",
                Utc::now().to_rfc3339(),
                agent,
                note
            ),
        );
        self.dispatch_team_notify_checkpoint(task_id, agent, note);
        let _ = self.publish_milestone(
            "checkpoint",
            &format!("📍 [{}] @{} 进度：{}", task_id, agent, note),
        );
        Ok(())
    }

    /// 处理 Specialist 请求协助，不改变任务状态，也不释放 claim。
    pub fn handle_specialist_help_requested(
        &self,
        task_id: &str,
        agent: &str,
        message: &str,
    ) -> Result<()> {
        let event = serde_json::json!({
            "event": "HELP_REQUESTED",
            "task": task_id,
            "agent": agent,
            "message": message,
            "ts": Utc::now().to_rfc3339(),
        })
        .to_string();
        let _ = self.session.append_event(&event);
        let _ = self.session.append_task_progress(
            task_id,
            &format!(
                "[{}] {} requested help: {}",
                Utc::now().to_rfc3339(),
                agent,
                message
            ),
        );
        self.dispatch_team_notify_help(task_id, agent, message);
        Ok(())
    }

    /// 构建并发送 TeamNotify InboundMsg 给 Lead（task 阻塞）
    fn dispatch_team_notify_blocked(&self, task_id: &str, agent: &str, reason: &str) {
        let lead_key = match self.lead_session_key.get().cloned() {
            Some(k) => k,
            None => return,
        };
        let tx = match self.team_notify_tx.get() {
            Some(t) => t.clone(),
            None => return,
        };
        let notify_content = format!(
            "[团队通知] 任务 {} 已阻塞（执行者：{}）\n\n阻塞原因：{}\n\n请调用 assign_task() 重新分配或 post_update() 告知用户。",
            task_id, agent, reason
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
        let task_id = task_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = tx.send(msg).await {
                tracing::warn!(task_id = %task_id, "TeamNotify (blocked) dispatch failed: {e}");
            }
        });
    }

    fn dispatch_team_notify_checkpoint(&self, task_id: &str, agent: &str, note: &str) {
        let lead_key = match self.lead_session_key.get().cloned() {
            Some(k) => k,
            None => return,
        };
        let tx = match self.team_notify_tx.get() {
            Some(t) => t.clone(),
            None => return,
        };
        let notify_content = format!(
            "[团队通知] 任务 {} 已更新检查点（执行者：{}）\n\n进展：{}\n\n如有必要，可调用 post_update() 向用户同步阶段性进展。",
            task_id, agent, note
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
        let task_id = task_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = tx.send(msg).await {
                tracing::warn!(task_id = %task_id, "TeamNotify (checkpoint) dispatch failed: {e}");
            }
        });
    }

    fn dispatch_team_notify_help(&self, task_id: &str, agent: &str, message: &str) {
        let lead_key = match self.lead_session_key.get().cloned() {
            Some(k) => k,
            None => return,
        };
        let tx = match self.team_notify_tx.get() {
            Some(t) => t.clone(),
            None => return,
        };
        let notify_content = format!(
            "[团队通知] 任务 {} 请求协助（执行者：{}）\n\n请求内容：{}\n\n请决定是直接回复思路、重新分配，还是让其继续执行。",
            task_id, agent, message
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
        let task_id = task_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = tx.send(msg).await {
                tracing::warn!(task_id = %task_id, "TeamNotify (help) dispatch failed: {e}");
            }
        });
    }

    // ── 停止 ──────────────────────────────────────────────────────────────────

    /// 停止 Heartbeat、MCP Server 并归档 team-session
    pub async fn stop(&self) -> Result<()> {
        // Stop heartbeat
        if let Some(handle) = self.heartbeat_handle.lock().unwrap().take() {
            handle.abort();
        }
        // Stop unified MCP server
        if let Some(handle) = self.mcp_server_handle.lock().await.take() {
            handle.stop().await;
            tracing::info!(team_id = %self.session.team_id, "SharedTeamMcpServer stopped");
        }
        // Archive directory
        self.session.archive()?;
        tracing::info!(team_id = %self.session.team_id, "Team stopped and archived");
        Ok(())
    }

    // ── 里程碑 ────────────────────────────────────────────────────────────────

    fn publish_milestone(&self, kind: &str, message: &str) -> Result<()> {
        // Forward to IM channel via notify_fn (wired from main.rs at startup).
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

    fn sync_task_artifacts(&self, task_id: &str) -> Result<()> {
        let Some(task) = self.registry.get_task(task_id)? else {
            anyhow::bail!("task '{}' not found while syncing artifacts", task_id);
        };
        self.write_task_artifacts(&task)
    }

    fn write_task_artifacts(&self, task: &Task) -> Result<()> {
        self.session
            .write_task_meta(&task.id, &TaskArtifactMeta::from_task(task))?;
        self.session
            .write_task_spec(&task.id, &render_task_spec(task))?;
        Ok(())
    }
}

fn render_task_spec(task: &Task) -> String {
    let deps = task.deps();
    let deps_section = if deps.is_empty() {
        "None".to_string()
    } else {
        deps.join(", ")
    };
    format!(
        "# {title}\n\n## Task ID\n\n{id}\n\n## Description\n\n{spec}\n\n## Success Criteria\n\n{criteria}\n\n## Dependencies\n\n{deps}\n",
        title = task.title,
        id = task.id,
        spec = task.spec.as_deref().unwrap_or("No detailed spec provided."),
        criteria = task
            .success_criteria
            .as_deref()
            .unwrap_or("Complete the requested work."),
        deps = deps_section,
    )
}

// ─── 测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::registry::CreateTask;
    use tempfile::tempdir;

    fn make_orchestrator() -> (Arc<TeamOrchestrator>, tempfile::TempDir) {
        let tmp = tempdir().unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("test-team", tmp.path().to_path_buf()));
        let dispatch_fn: DispatchFn = Arc::new(|_agent, _task| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(3600), // 测试中不实际触发
        );
        (orch, tmp)
    }

    #[test]
    fn test_register_task_increments_registry() {
        let (orch, tmp) = make_orchestrator();
        let result = orch.register_task(CreateTask {
            id: "T001".into(),
            title: "Write DB schema".into(),
            spec: Some("Design the schema".into()),
            success_criteria: Some("Schema covers auth and billing".into()),
            ..Default::default()
        });
        assert!(result.is_ok());
        assert!(result.unwrap().contains("T001"));
        let task = orch.registry.get_task("T001").unwrap().unwrap();
        assert_eq!(task.title, "Write DB schema");
        let task_dir = tmp.path().join("tasks").join("T001");
        assert!(task_dir.join("meta.json").is_file());
        assert!(task_dir.join("spec.md").is_file());
        let spec = std::fs::read_to_string(task_dir.join("spec.md")).unwrap();
        assert!(spec.contains("Design the schema"));
        assert!(spec.contains("Schema covers auth and billing"));
    }

    #[test]
    fn test_team_state_starts_planning() {
        let (orch, _tmp) = make_orchestrator();
        assert!(matches!(orch.team_state(), TeamState::Planning));
    }

    #[tokio::test]
    async fn test_activate_starts_mcp_and_sets_running() {
        let (orch, _tmp) = make_orchestrator();
        orch.set_test_mcp_start_result(Ok(32123));
        orch.register_task(CreateTask {
            id: "T001".into(),
            title: "test".into(),
            ..Default::default()
        })
        .unwrap();
        orch.activate().await.unwrap();
        assert!(matches!(orch.team_state(), TeamState::Running));
        assert_eq!(orch.mcp_server_port.get().copied(), Some(32123));
    }

    #[tokio::test]
    async fn test_activate_fails_when_mcp_start_fails() {
        let (orch, _tmp) = make_orchestrator();
        orch.set_test_mcp_start_result(Err("synthetic mcp failure".to_string()));
        orch.register_task(CreateTask {
            id: "TFAIL".into(),
            title: "test".into(),
            ..Default::default()
        })
        .unwrap();

        let err = orch.activate().await.unwrap_err().to_string();
        assert!(err.contains("synthetic mcp failure"));
        assert!(matches!(orch.team_state(), TeamState::Planning));
        assert!(orch.mcp_server_port.get().is_none());
    }

    #[test]
    fn test_handle_specialist_done_updates_registry() {
        let (orch, tmp) = make_orchestrator();
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
        let result = std::fs::read_to_string(tmp.path().join("tasks").join("T003").join("result.md"))
            .unwrap();
        assert!(result.contains("created jwt.rs"));
        let meta = std::fs::read_to_string(tmp.path().join("tasks").join("T003").join("meta.json"))
            .unwrap();
        assert!(meta.contains("\"status\": \"done\""));
    }

    #[test]
    fn test_handle_specialist_submitted_updates_registry() {
        let (orch, tmp) = make_orchestrator();
        orch.registry
            .create_task(CreateTask {
                id: "T004".into(),
                title: "JWT impl".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T004", "codex").unwrap();

        orch.handle_specialist_submitted("T004", "codex", "ready for review")
            .unwrap();

        let task = orch.registry.get_task("T004").unwrap().unwrap();
        use crate::team::registry::TaskStatus;
        assert!(matches!(
            task.status_parsed(),
            TaskStatus::Submitted { ref agent, .. } if agent == "codex"
        ));
        assert_eq!(task.completion_note.as_deref(), Some("ready for review"));
        let result = std::fs::read_to_string(tmp.path().join("tasks").join("T004").join("result.md"))
            .unwrap();
        assert!(result.contains("ready for review"));
        let meta = std::fs::read_to_string(tmp.path().join("tasks").join("T004").join("meta.json"))
            .unwrap();
        assert!(meta.contains("submitted:codex:"));
    }

    #[tokio::test]
    async fn test_accept_submitted_task_triggers_all_done_milestone() {
        let (orch, tmp) = make_orchestrator();
        let received = Arc::new(std::sync::Mutex::new(vec![]));
        let received_clone = Arc::clone(&received);

        orch.set_notify_fn(Arc::new(move |_scope, msg| {
            received_clone.lock().unwrap().push(msg);
        }));
        orch.set_scope(qai_protocol::SessionKey::new("test", "test-scope"));

        orch.registry
            .create_task(CreateTask {
                id: "T005".into(),
                title: "only task".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T005", "codex").unwrap();
        orch.handle_specialist_submitted("T005", "codex", "ready")
            .unwrap();
        orch.accept_submitted_task("T005", "claude").unwrap();

        let task = orch.registry.get_task("T005").unwrap().unwrap();
        use crate::team::registry::TaskStatus;
        assert!(matches!(
            task.status_parsed(),
            TaskStatus::Accepted { ref by, .. } if by == "claude"
        ));

        let msgs = received.lock().unwrap();
        assert!(
            !msgs.is_empty(),
            "notify_fn should be called on acceptance milestone"
        );
        assert!(msgs.iter().any(|m| m.contains("所有任务已完成")));
        let result = std::fs::read_to_string(tmp.path().join("tasks").join("T005").join("result.md"))
            .unwrap();
        assert!(result.contains("Accepted by: claude"));
        let progress =
            std::fs::read_to_string(tmp.path().join("tasks").join("T005").join("progress.md"))
                .unwrap();
        assert!(progress.contains("claude accepted submission"));
    }

    #[tokio::test]
    async fn test_all_done_triggers_milestone_notify_fn() {
        let (orch, _tmp) = make_orchestrator();
        let received = Arc::new(std::sync::Mutex::new(vec![]));
        let received_clone = Arc::clone(&received);

        // Wire notify_fn to capture milestone messages
        orch.set_notify_fn(Arc::new(move |_scope, msg| {
            received_clone.lock().unwrap().push(msg);
        }));
        orch.set_scope(qai_protocol::SessionKey::new("test", "test-scope"));

        orch.registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "only task".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T001", "codex").unwrap();
        orch.handle_specialist_done("T001", "codex", "done")
            .unwrap();

        let msgs = received.lock().unwrap();
        assert!(!msgs.is_empty(), "notify_fn should be called on milestone");
        assert!(
            msgs[0].contains("所有任务已完成"),
            "unexpected: {}",
            msgs[0]
        );
    }

    #[tokio::test]
    async fn test_start_registers_tasks_and_writes_team_md() {
        let (orch, tmp) = make_orchestrator();
        orch.set_test_mcp_start_result(Ok(32124));

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
        let task_dir = tmp.path().join("tasks").join("T001");
        assert!(task_dir.join("meta.json").is_file());
        assert!(task_dir.join("spec.md").is_file());
    }

    #[test]
    fn block_task_rejects_non_owner() {
        use crate::team::registry::CreateTask;

        let (orch, _tmp) = make_orchestrator();

        orch.register_task(CreateTask {
            id: "T001".into(),
            title: "Test Task".into(),
            ..Default::default()
        })
        .unwrap();
        orch.registry.try_claim("T001", "codex").unwrap();

        // "gemini" tries to block "codex"'s task → should fail
        let result = orch.handle_specialist_blocked("T001", "gemini", "stuck");
        assert!(result.is_err(), "block_task should reject non-owner");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("gemini") || msg.contains("not own") || msg.contains("claimed"),
            "error message should mention agent or ownership: {msg}"
        );
    }

    #[test]
    fn block_task_accepts_owner() {
        use crate::team::registry::CreateTask;

        let (orch, tmp) = make_orchestrator();

        orch.register_task(CreateTask {
            id: "T002".into(),
            title: "Another Task".into(),
            ..Default::default()
        })
        .unwrap();
        orch.registry.try_claim("T002", "codex").unwrap();

        let result = orch.handle_specialist_blocked("T002", "codex", "stuck on auth");
        assert!(
            result.is_ok(),
            "block_task should accept owner: {:?}",
            result.err()
        );
        let task = orch.registry.get_task("T002").unwrap().unwrap();
        assert!(
            matches!(
                task.status_parsed(),
                crate::team::registry::TaskStatus::Pending
            ),
            "blocked task should release claim back to Pending, got {:?}",
            task.status_parsed()
        );
        let progress =
            std::fs::read_to_string(tmp.path().join("tasks").join("T002").join("progress.md"))
                .unwrap();
        assert!(progress.contains("blocked task"));
        assert!(progress.contains("stuck on auth"));
    }

    #[test]
    fn checkpoint_and_help_append_progress_artifacts() {
        let (orch, tmp) = make_orchestrator();

        orch.register_task(CreateTask {
            id: "T020".into(),
            title: "Need coordination".into(),
            ..Default::default()
        })
        .unwrap();

        orch.handle_specialist_checkpoint("T020", "codex", "halfway there")
            .unwrap();
        orch.handle_specialist_help_requested("T020", "codex", "need API guidance")
            .unwrap();

        let progress =
            std::fs::read_to_string(tmp.path().join("tasks").join("T020").join("progress.md"))
                .unwrap();
        assert!(progress.contains("checkpoint"));
        assert!(progress.contains("halfway there"));
        assert!(progress.contains("requested help"));
        assert!(progress.contains("need API guidance"));
    }

    #[test]
    fn test_checkpoint_publishes_milestone_to_im() {
        let (orch, _tmp) = make_orchestrator();
        let messages: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(vec![]));
        let msgs_clone = Arc::clone(&messages);
        orch.set_notify_fn(Arc::new(move |_scope, msg| {
            msgs_clone.lock().unwrap().push(msg);
        }));
        orch.set_scope(qai_protocol::SessionKey::new("lark", "group:test"));

        orch.registry
            .create_task(CreateTask {
                id: "T120".into(),
                title: "Design schema".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T120", "codex").unwrap();

        orch.handle_specialist_checkpoint("T120", "codex", "halfway there")
            .unwrap();

        let msgs = messages.lock().unwrap();
        assert!(
            msgs.iter().any(|m| m.contains("T120") && m.contains("codex") && m.contains("📍")),
            "checkpoint should publish IM milestone with task ID, agent, and 📍 emoji, got: {:?}",
            msgs
        );
        assert!(
            msgs.iter().any(|m| m.contains("halfway there")),
            "checkpoint message should include the note text, got: {:?}",
            msgs
        );
    }

    #[test]
    fn test_submit_publishes_milestone_to_im() {
        let (orch, _tmp) = make_orchestrator();
        let messages: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(vec![]));
        let msgs_clone = Arc::clone(&messages);
        orch.set_notify_fn(Arc::new(move |_scope, msg| {
            msgs_clone.lock().unwrap().push(msg);
        }));
        orch.set_scope(qai_protocol::SessionKey::new("lark", "group:test"));

        orch.registry
            .create_task(CreateTask {
                id: "T121".into(),
                title: "Implement JWT".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T121", "codex").unwrap();

        orch.handle_specialist_submitted("T121", "codex", "added jwt.rs")
            .unwrap();

        let msgs = messages.lock().unwrap();
        assert!(
            msgs.iter().any(|m| m.contains("T121") && m.contains("codex") && m.contains("📨")),
            "submit should publish IM milestone with task ID, agent, and 📨 emoji, got: {:?}",
            msgs
        );
        assert!(
            msgs.iter().any(|m| m.contains("Implement JWT")),
            "submit message should include the task title, got: {:?}",
            msgs
        );
    }

    #[test]
    fn test_blocked_publishes_milestone_to_im() {
        let (orch, _tmp) = make_orchestrator();
        let messages: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(vec![]));
        let msgs_clone = Arc::clone(&messages);
        orch.set_notify_fn(Arc::new(move |_scope, msg| {
            msgs_clone.lock().unwrap().push(msg);
        }));
        orch.set_scope(qai_protocol::SessionKey::new("lark", "group:test"));

        orch.registry
            .create_task(CreateTask {
                id: "T122".into(),
                title: "Write tests".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T122", "codex").unwrap();

        orch.handle_specialist_blocked("T122", "codex", "missing dep")
            .unwrap();

        let msgs = messages.lock().unwrap();
        assert!(
            msgs.iter().any(|m| m.contains("T122") && m.contains("codex") && m.contains("🚧")),
            "blocked should publish IM milestone with task ID, agent, and 🚧 emoji, got: {:?}",
            msgs
        );
        assert!(
            msgs.iter().any(|m| m.contains("missing dep")),
            "blocked message should include the reason text, got: {:?}",
            msgs
        );
    }

    #[test]
    fn test_done_individual_publishes_milestone_to_im() {
        let (orch, _tmp) = make_orchestrator();
        // Two tasks so all_done is false after first completes
        orch.registry
            .create_task(CreateTask {
                id: "T130".into(),
                title: "First task".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry
            .create_task(CreateTask {
                id: "T131".into(),
                title: "Second task".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T130", "codex").unwrap();

        let messages: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(vec![]));
        let msgs_clone = Arc::clone(&messages);
        orch.set_notify_fn(Arc::new(move |_scope, msg| {
            msgs_clone.lock().unwrap().push(msg);
        }));
        orch.set_scope(qai_protocol::SessionKey::new("lark", "group:test"));

        orch.handle_specialist_done("T130", "codex", "done").unwrap();

        let msgs = messages.lock().unwrap();
        assert!(
            msgs.iter().any(|m| m.contains("T130") && m.contains("codex") && m.contains("✅")),
            "individual done should publish IM milestone with task ID, agent, and ✅ emoji, got: {:?}",
            msgs
        );
        assert!(
            msgs.iter().any(|m| m.contains("First task")),
            "done message should include the task title, got: {:?}",
            msgs
        );
        // Progress counter: should show 1/2
        assert!(
            msgs.iter().any(|m| m.contains("1/2")),
            "done message should include progress counter 1/2, got: {:?}",
            msgs
        );
    }

    // ─── 功能测试：完整 Agent Swarm 生命周期 ──────────────────────────────────

    /// 验证 publish_milestone 在未注册 notify_fn/scope 时不 panic，直接返回 Ok
    #[test]
    fn test_publish_milestone_noop_without_notify_fn() {
        let (orch, _tmp) = make_orchestrator();
        // 故意不调用 set_notify_fn / set_scope

        orch.registry
            .create_task(CreateTask {
                id: "NOOP01".into(),
                title: "no-op test".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("NOOP01", "codex").unwrap();

        // 调用任意会触发 publish_milestone 的 handler，不应 panic
        let result = orch.handle_specialist_checkpoint("NOOP01", "codex", "halfway");
        assert!(
            result.is_ok(),
            "checkpoint without notify_fn should return Ok, not panic; got: {:?}",
            result
        );
    }

    /// 完整 Agent Swarm 生命周期：T_A（无依赖）→ T_B（依赖 T_A）
    /// 验证：T_A done → IM 包含 ✅ + 🔓 解锁通知；T_B done → 所有任务已完成 ✅
    #[test]
    fn test_full_swarm_lifecycle_dep_chain_im_milestones() {
        let (orch, _tmp) = make_orchestrator();
        let messages: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(vec![]));
        let msgs_clone = Arc::clone(&messages);
        orch.set_notify_fn(Arc::new(move |_scope, msg| {
            msgs_clone.lock().unwrap().push(msg);
        }));
        orch.set_scope(qai_protocol::SessionKey::new("lark", "group:swarm-test"));

        // 建立依赖链 T_A → T_B
        orch.registry
            .create_task(CreateTask {
                id: "T_A".into(),
                title: "Init DB".into(),
                assignee_hint: Some("codex".into()),
                ..Default::default()
            })
            .unwrap();
        orch.registry
            .create_task(CreateTask {
                id: "T_B".into(),
                title: "Seed Data".into(),
                assignee_hint: Some("claude".into()),
                deps: vec!["T_A".into()],
                ..Default::default()
            })
            .unwrap();

        // ── 阶段 1：T_A 完成 ────────────────────────────────────────────────
        orch.registry.try_claim("T_A", "codex").unwrap();
        orch.handle_specialist_done("T_A", "codex", "db schema created").unwrap();

        {
            let msgs = messages.lock().unwrap();
            // T_A done 通知：含 ✅、任务ID、agent
            assert!(
                msgs.iter().any(|m| m.contains("T_A") && m.contains("codex") && m.contains("✅")),
                "T_A done should emit IM milestone with ✅, task ID, agent; got: {:?}", msgs
            );
            // T_A done 进度计数 1/2
            assert!(
                msgs.iter().any(|m| m.contains("1/2")),
                "T_A done should show progress 1/2; got: {:?}", msgs
            );
            // T_B 解锁通知：含 🔓 和 T_B
            assert!(
                msgs.iter().any(|m| m.contains("🔓") && m.contains("T_B")),
                "T_A done should trigger 🔓 unlock notification for T_B; got: {:?}", msgs
            );
            // 全部完成通知不应出现（T_B 还未完成）
            assert!(
                !msgs.iter().any(|m| m.contains("所有任务已完成")),
                "all_done message must NOT fire before T_B completes; got: {:?}", msgs
            );
        }

        // ── 阶段 2：T_B 完成 ────────────────────────────────────────────────
        orch.registry.try_claim("T_B", "claude").unwrap();
        orch.handle_specialist_done("T_B", "claude", "data seeded").unwrap();

        {
            let msgs = messages.lock().unwrap();
            // 全部完成通知
            assert!(
                msgs.iter().any(|m| m.contains("所有任务已完成") && m.contains("✅")),
                "after T_B done, all_done milestone must fire; got: {:?}", msgs
            );
        }

        // TeamState 应变为 Done
        assert!(
            matches!(*orch.team_state_inner.lock().unwrap(), TeamState::Done),
            "team state must be Done after all tasks complete"
        );
    }

    /// 验证 checkpoint → submit → accept 完整提交验收路径中 IM 通知顺序正确
    #[test]
    fn test_submit_accept_flow_im_notifications() {
        let (orch, _tmp) = make_orchestrator();
        let messages: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(vec![]));
        let msgs_clone = Arc::clone(&messages);
        orch.set_notify_fn(Arc::new(move |_scope, msg| {
            msgs_clone.lock().unwrap().push(msg);
        }));
        orch.set_scope(qai_protocol::SessionKey::new("dingtalk", "group:qa"));

        orch.registry
            .create_task(CreateTask {
                id: "SA01".into(),
                title: "Write Auth API".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("SA01", "codex").unwrap();

        // Checkpoint 中间进度
        orch.handle_specialist_checkpoint("SA01", "codex", "50% done").unwrap();
        // Submit 提交待验收
        orch.handle_specialist_submitted("SA01", "codex", "auth.rs complete").unwrap();

        let msgs = messages.lock().unwrap();
        // checkpoint 在 submit 之前出现
        let cp_pos = msgs.iter().position(|m| m.contains("📍") && m.contains("SA01"));
        let sub_pos = msgs.iter().position(|m| m.contains("📨") && m.contains("SA01"));
        assert!(cp_pos.is_some(), "checkpoint IM message must exist; got: {:?}", msgs);
        assert!(sub_pos.is_some(), "submit IM message must exist; got: {:?}", msgs);
        assert!(
            cp_pos.unwrap() < sub_pos.unwrap(),
            "checkpoint IM must appear before submit IM in notification stream"
        );
        // submit 消息含任务标题
        assert!(
            msgs.iter().any(|m| m.contains("Write Auth API") && m.contains("📨")),
            "submit IM message must include task title; got: {:?}", msgs
        );
    }

    /// 验证 blocked 后 specialist 重新尝试 checkpoint 依然可以推送通知
    #[test]
    fn test_blocked_then_retry_checkpoint_both_notify() {
        let (orch, _tmp) = make_orchestrator();
        let messages: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(vec![]));
        let msgs_clone = Arc::clone(&messages);
        orch.set_notify_fn(Arc::new(move |_scope, msg| {
            msgs_clone.lock().unwrap().push(msg);
        }));
        orch.set_scope(qai_protocol::SessionKey::new("ws", "group:dev"));

        orch.registry
            .create_task(CreateTask {
                id: "BR01".into(),
                title: "Deploy service".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("BR01", "codex").unwrap();

        // 先 blocked
        orch.handle_specialist_blocked("BR01", "codex", "missing env vars").unwrap();
        // 再 checkpoint（模拟问题解决后继续）
        orch.handle_specialist_checkpoint("BR01", "codex", "env vars fixed, proceeding").unwrap();

        let msgs = messages.lock().unwrap();
        assert!(
            msgs.iter().any(|m| m.contains("🚧") && m.contains("BR01")),
            "blocked IM milestone must be present; got: {:?}", msgs
        );
        assert!(
            msgs.iter().any(|m| m.contains("📍") && m.contains("BR01")),
            "post-block checkpoint IM milestone must be present; got: {:?}", msgs
        );
        let blocked_pos = msgs.iter().position(|m| m.contains("🚧")).unwrap();
        let checkpoint_pos = msgs.iter().position(|m| m.contains("📍")).unwrap();
        assert!(blocked_pos < checkpoint_pos, "blocked IM must appear before checkpoint IM");
    }
}
