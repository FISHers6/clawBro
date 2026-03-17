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
//! canonical multi-backend semantics 将在 clawbro-runtime::tool_bridge 中升级为
//! submit/accept/reopen 流程。

use anyhow::Result;
use chrono::Utc;
use clawbro_protocol::SessionKey;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::task::JoinHandle;

use super::completion_routing::{
    RoutingDeliveryStatus, TeamNotifyRequest, TeamRoutingEnvelope, TeamRoutingEvent,
};
use super::heartbeat::DispatchFn;
use super::milestone::TeamMilestoneEvent;
use super::registry::{Task, TaskRegistry};
use super::session::{LeaderUpdateKind, TaskArtifactMeta, TeamSession};
use super::specialist_turn::{
    classify_specialist_turn, SpecialistActionKind, SpecialistActionRecord, SpecialistTurnOutcome,
};
use crate::turn_context::TurnDeliverySource;

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

#[derive(Debug, Clone, Default, Serialize)]
pub struct TeamRoutingStats {
    pub direct_delivered: usize,
    pub queued_delivered: usize,
    pub fallback_redirected: usize,
    pub pending_count: usize,
    pub missing_delivery_target: usize,
    pub delivery_dedupe_ledger_size: usize,
    pub delivery_dedupe_hits: usize,
    pub failed_terminal: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct TeamRuntimeSummary {
    pub team_id: String,
    pub state: TeamState,
    pub lead_session_key: Option<clawbro_protocol::SessionKey>,
    pub lead_agent_name: Option<String>,
    pub latest_leader_update: Option<crate::team::session::LeaderUpdateRecord>,
    pub latest_channel_send: Option<crate::team::session::ChannelSendRecord>,
    pub specialists: Vec<String>,
    pub tool_surface_ready: bool,
    pub mcp_port: Option<u16>,
    pub task_counts: TeamTaskCounts,
    pub artifact_health: TeamArtifactHealthSummary,
    pub routing_stats: TeamRoutingStats,
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

/// 里程碑事件回调：(IM scope, event) → fire-and-forget
///
/// 生产端：将 event 渲染为字符串后推送到 IM channel。
/// 测试端：收集事件到 Vec 供断言，不涉及任何字符串操作。
pub type MilestoneFn = Arc<dyn Fn(clawbro_protocol::SessionKey, TeamMilestoneEvent) + Send + Sync>;

pub struct TeamOrchestrator {
    pub registry: Arc<TaskRegistry>,
    pub session: Arc<TeamSession>,
    heartbeat_handle: std::sync::Mutex<Option<JoinHandle<()>>>,
    dispatch_fn: DispatchFn,
    heartbeat_interval: std::time::Duration,
    max_parallel: std::sync::Mutex<usize>,
    /// IM scope to forward milestone notifications to (set at team-start time).
    scope: std::sync::OnceLock<clawbro_protocol::SessionKey>,
    /// Typed milestone event callback (injected from main.rs at startup).
    /// Production: renders event → IM channel string.
    /// Tests: collects events into Vec for typed assertions.
    milestone_fn: std::sync::OnceLock<MilestoneFn>,
    /// Unified MCP server handle (Lead + Specialist tools on one port, spawned at startup).
    /// Uses tokio::sync::Mutex because stop() is async.
    mcp_server_handle: tokio::sync::Mutex<Option<super::shared_mcp_server::SharedMcpServerHandle>>,
    /// Bound port of the unified MCP server (set once after spawn, used by all agents).
    pub mcp_server_port: std::sync::OnceLock<u16>,
    /// 当前 Team 执行状态（Planning / AwaitingConfirm / Running / Done）
    pub team_state_inner: std::sync::Mutex<TeamState>,
    /// Lead Agent 的最近一次真实 ingress session key 诊断快照。
    lead_session_key: std::sync::Mutex<Option<clawbro_protocol::SessionKey>>,
    /// Lead Agent 的最近一次真实 turn delivery source。
    lead_delivery_source: std::sync::Mutex<Option<TurnDeliverySource>>,
    /// Configured Lead agent name from `front_bot` in config.toml.
    pub lead_agent_name: std::sync::OnceLock<String>,
    /// List of Specialist agent names (from `team.roster` in config.toml).
    pub available_specialists: std::sync::OnceLock<Vec<String>>,
    /// TeamNotify MPSC sender — wired from main.rs after registry is ready.
    team_notify_tx: std::sync::OnceLock<tokio::sync::mpsc::Sender<TeamNotifyRequest>>,
    pending_store_lock: std::sync::Mutex<()>,
    /// Recent canonical specialist actions for dispatch-window outcome classification.
    recent_specialist_actions: std::sync::Mutex<VecDeque<SpecialistActionRecord>>,
    dispatch_contexts: std::sync::Mutex<HashMap<(String, String), DispatchContextRecord>>,
    pending_lead_fragments: std::sync::Mutex<Vec<PendingLeadFragment>>,
    done_final_posted: std::sync::Mutex<bool>,
    #[cfg(test)]
    test_mcp_start_result: std::sync::Mutex<Option<std::result::Result<u16, String>>>,
}

#[derive(Debug, Clone)]
struct DispatchContextRecord {
    run_id: String,
    requester_session_key: SessionKey,
    parent_run_id: Option<String>,
    delivery_source: Option<TurnDeliverySource>,
}

#[derive(Debug, Clone)]
pub struct PendingLeadFragment {
    pub event_id: String,
    pub text: String,
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
            milestone_fn: std::sync::OnceLock::new(),
            mcp_server_handle: tokio::sync::Mutex::new(None),
            mcp_server_port: std::sync::OnceLock::new(),
            team_state_inner: std::sync::Mutex::new(TeamState::Planning),
            lead_session_key: std::sync::Mutex::new(None),
            lead_delivery_source: std::sync::Mutex::new(None),
            lead_agent_name: std::sync::OnceLock::new(),
            available_specialists: std::sync::OnceLock::new(),
            team_notify_tx: std::sync::OnceLock::new(),
            pending_store_lock: std::sync::Mutex::new(()),
            recent_specialist_actions: std::sync::Mutex::new(VecDeque::new()),
            dispatch_contexts: std::sync::Mutex::new(HashMap::new()),
            pending_lead_fragments: std::sync::Mutex::new(Vec::new()),
            done_final_posted: std::sync::Mutex::new(false),
            #[cfg(test)]
            test_mcp_start_result: std::sync::Mutex::new(None),
        })
    }

    fn task_is_terminal(task: &Task) -> bool {
        matches!(
            task.status_parsed(),
            super::registry::TaskStatus::Accepted { .. }
                | super::registry::TaskStatus::Done
                | super::registry::TaskStatus::Failed(_)
        )
    }

    fn archive_completed_cycle_if_needed(&self) -> Result<bool> {
        let tasks = self.registry.all_tasks()?;
        if tasks.is_empty() || !tasks.iter().all(Self::task_is_terminal) {
            return Ok(false);
        }

        let archive_path = self.session.archive_completed_cycle(&tasks)?;
        self.registry.clear_all_tasks()?;
        self.session.sync_tasks_md(&self.registry)?;
        self.session.clear_delivery_dedupe_ledgers()?;
        *self.done_final_posted.lock().unwrap() = false;
        *self.team_state_inner.lock().unwrap() = TeamState::Planning;

        let event = serde_json::json!({
            "event": "ARCHIVED_COMPLETED_CYCLE",
            "ts": Utc::now().to_rfc3339(),
            "task_count": tasks.len(),
            "archive_path": archive_path,
        })
        .to_string();
        let _ = self.session.append_event(&event);
        Ok(true)
    }

    // ── 里程碑通知接线 ────────────────────────────────────────────────────────

    /// 设置里程碑通知目标 IM scope（在 /team start 时调用）。
    pub fn set_scope(&self, scope: clawbro_protocol::SessionKey) {
        let _ = self.scope.set(scope);
    }

    /// 注入里程碑事件回调（main.rs 在启动时调用）。
    ///
    /// 生产端将 event 传给 render_for_im() 后推送到 IM channel。
    /// 测试端收集类型化事件供 matches! 断言。
    pub fn set_milestone_fn(&self, f: MilestoneFn) {
        let _ = self.milestone_fn.set(f);
    }

    // ── Team 状态 ──────────────────────────────────────────────────────────────

    /// 获取当前 TeamState（克隆副本）
    pub fn team_state(&self) -> TeamState {
        self.team_state_inner.lock().unwrap().clone()
    }

    /// Reopen a completed team when a new human lead turn starts a fresh planning cycle.
    /// This is intentionally non-destructive: task ledgers and artifacts are preserved.
    pub fn reopen_for_new_planning_cycle_if_done(&self) -> bool {
        let mut state = self.team_state_inner.lock().unwrap();
        if *state != TeamState::Done {
            return false;
        }
        *state = TeamState::Planning;
        *self.done_final_posted.lock().unwrap() = false;
        let event = serde_json::json!({
            "event": "REOPENED_FOR_PLANNING",
            "ts": Utc::now().to_rfc3339(),
        })
        .to_string();
        let _ = self.session.append_event(&event);
        true
    }

    /// 设置 Lead 的 IM session key（由 main.rs 在启动时调用）
    pub fn set_lead_session_key(&self, key: clawbro_protocol::SessionKey) {
        *self.lead_session_key.lock().unwrap() = Some(key);
        self.flush_pending_routing_events();
    }

    pub fn lead_session_key(&self) -> Option<clawbro_protocol::SessionKey> {
        self.lead_session_key.lock().unwrap().clone()
    }

    pub fn update_lead_delivery_source(&self, source: TurnDeliverySource) {
        let lead_key = source.session_key();
        *self.lead_delivery_source.lock().unwrap() = Some(source);
        self.set_lead_session_key(lead_key);
    }

    pub fn lead_delivery_source(&self) -> Option<TurnDeliverySource> {
        self.lead_delivery_source.lock().unwrap().clone()
    }

    /// 注入 TeamNotify MPSC sender（main.rs 在启动时调用）。
    /// handle_specialist_done() 和永久失败处理会用此 sender 推通知给 Lead。
    pub fn set_team_notify_tx(&self, tx: tokio::sync::mpsc::Sender<TeamNotifyRequest>) {
        let _ = self.team_notify_tx.set(tx);
        self.flush_pending_routing_events();
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

    /// 向 IM 频道发布 Lead 的任意文字更新（post_update 工具调用时使用）
    pub fn post_message(&self, message: &str) -> bool {
        if !self.reserve_done_cycle_post_update() {
            tracing::info!(
                team_id = %self.session.team_id,
                "Suppressing duplicate post_update after completed team cycle"
            );
            return false;
        }
        self.record_leader_fragment(LeaderUpdateKind::PostUpdate, message);
        let event = TeamMilestoneEvent::LeadMessage {
            text: message.to_string(),
        };
        let _ = self.emit_milestone(event);
        true
    }

    fn reserve_done_cycle_post_update(&self) -> bool {
        if self.team_state() != TeamState::Done {
            return true;
        }
        let mut posted = self.done_final_posted.lock().unwrap();
        if *posted {
            return false;
        }
        *posted = true;
        true
    }

    pub fn record_leader_fragment(&self, kind: LeaderUpdateKind, text: &str) {
        let source_agent = self
            .lead_agent_name
            .get()
            .cloned()
            .unwrap_or_else(|| "leader".to_string());
        match self.session.record_leader_update(
            self.lead_session_key().as_ref(),
            self.lead_delivery_source().as_ref(),
            &source_agent,
            kind,
            text,
            None,
        ) {
            Ok(event_id) => self
                .pending_lead_fragments
                .lock()
                .unwrap()
                .push(PendingLeadFragment {
                    event_id,
                    text: text.to_string(),
                }),
            Err(err) => {
                tracing::warn!(
                    team_id = %self.session.team_id,
                    error = %err,
                    "Failed to record leader update ledger entry"
                );
            }
        }
    }

    pub fn take_pending_lead_fragments(&self) -> Vec<PendingLeadFragment> {
        let mut refs = self.pending_lead_fragments.lock().unwrap();
        std::mem::take(&mut *refs)
    }

    pub fn clear_pending_lead_fragments(&self) {
        self.pending_lead_fragments.lock().unwrap().clear();
    }

    pub fn notify_task_dispatched(&self, task_id: &str, task_title: &str, agent: &str) {
        let _ = self.emit_milestone(TeamMilestoneEvent::TaskDispatched {
            task_id: task_id.to_string(),
            task_title: task_title.to_string(),
            agent: agent.to_string(),
        });
    }

    pub fn record_dispatch_start(
        &self,
        task_id: &str,
        agent: &str,
        requester_session_key: SessionKey,
        parent_run_id: Option<String>,
        delivery_source: Option<TurnDeliverySource>,
    ) -> String {
        let run_id = uuid::Uuid::new_v4().to_string();
        let mut contexts = self.dispatch_contexts.lock().unwrap();
        contexts.insert(
            (task_id.to_string(), agent.to_string()),
            DispatchContextRecord {
                run_id: run_id.clone(),
                requester_session_key,
                parent_run_id,
                delivery_source,
            },
        );
        run_id
    }

    pub fn status_snapshot(&self) -> TeamRuntimeSummary {
        let tasks = self.registry.all_tasks().unwrap_or_default();
        let mut counts = TeamTaskCounts {
            total: tasks.len(),
            ..TeamTaskCounts::default()
        };
        for task in &tasks {
            let status = task.status_raw.as_str();
            if status == "pending" || status.starts_with("hold:") {
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
        let routing_stats = self.routing_stats();
        let latest_leader_update = self.session.load_latest_leader_update().unwrap_or(None);
        let latest_channel_send = self.session.load_latest_channel_send().unwrap_or(None);

        TeamRuntimeSummary {
            team_id: self.session.team_id.clone(),
            state: self.team_state(),
            lead_session_key: self.lead_session_key(),
            lead_agent_name: self.lead_agent_name.get().cloned(),
            latest_leader_update,
            latest_channel_send,
            specialists: self
                .available_specialists
                .get()
                .cloned()
                .unwrap_or_default(),
            tool_surface_ready: self.mcp_server_port.get().is_some(),
            mcp_port: self.mcp_server_port.get().copied(),
            task_counts: counts,
            artifact_health,
            routing_stats,
        }
    }

    fn routing_stats(&self) -> TeamRoutingStats {
        let outcomes = self.session.load_routing_outcomes().unwrap_or_default();
        // Hold the store lock while reading pending completions to get a consistent snapshot.
        let pending = {
            let _guard = self.pending_store_lock.lock().unwrap();
            match self.session.load_pending_completions() {
                Ok(records) => records,
                Err(err) => {
                    tracing::warn!(
                        team_id = %self.session.team_id,
                        error = %err,
                        "Failed to load pending completions for routing stats"
                    );
                    vec![]
                }
            }
        };
        let delivery_dedupe_ledger_size = self.session.delivery_dedupe_ledger_size().unwrap_or(0);
        let delivery_dedupe_hits = self.session.delivery_dedupe_hit_count().unwrap_or(0);
        let mut stats = TeamRoutingStats {
            pending_count: pending.len(),
            delivery_dedupe_ledger_size,
            delivery_dedupe_hits,
            ..TeamRoutingStats::default()
        };
        for envelope in &pending {
            if envelope.requester_session_key.is_none() && envelope.fallback_session_keys.is_empty()
            {
                stats.missing_delivery_target += 1;
            }
        }
        for envelope in outcomes {
            match envelope.delivery_status {
                RoutingDeliveryStatus::DirectDelivered => stats.direct_delivered += 1,
                RoutingDeliveryStatus::QueuedDelivered => stats.queued_delivered += 1,
                RoutingDeliveryStatus::FallbackRedirected => stats.fallback_redirected += 1,
                RoutingDeliveryStatus::FailedTerminal => stats.failed_terminal += 1,
                RoutingDeliveryStatus::PersistedPending => stats.pending_count += 1,
                RoutingDeliveryStatus::NotRouted => {}
            }
        }
        stats
    }

    // ── 增量任务注册（供 LeadMcpServer.create_task 调用）────────────────────

    /// 在 Planning 阶段注册单个任务。只能在 state == Planning 或 AwaitingConfirm 时调用。
    pub fn register_task(&self, task: super::registry::CreateTask) -> Result<String> {
        self.archive_completed_cycle_if_needed()?;
        let state = self.team_state_inner.lock().unwrap().clone();
        if !matches!(state, TeamState::Planning | TeamState::AwaitingConfirm) {
            anyhow::bail!("Cannot register task: team is already {:?}", state);
        }
        let id = task.id.clone();
        self.registry.create_task(task)?;
        self.sync_task_artifacts(&id)?;
        Ok(format!("Task {} registered.", id))
    }

    pub fn allocate_task_id(&self) -> Result<String> {
        self.archive_completed_cycle_if_needed()?;
        self.registry.next_task_id()
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
            let lead_name = self
                .lead_agent_name
                .get()
                .map(|s| s.as_str())
                .unwrap_or("Lead");
            let specialists = self
                .available_specialists
                .get()
                .cloned()
                .unwrap_or_default();
            let _ = self.session.write_team_md(&render_team_manifest_document(
                lead_name,
                &specialists,
                None,
            ));
        }

        let lead_name = self
            .lead_agent_name
            .get()
            .map(|s| s.as_str())
            .unwrap_or("Lead");
        let specialists = self
            .available_specialists
            .get()
            .cloned()
            .unwrap_or_default();
        let agents_md = render_team_agents_guide(lead_name, &specialists);
        let _ = self.session.write_agents_md(&agents_md);

        // Sync TASKS.md snapshot
        self.session.sync_tasks_md(&self.registry)?;

        // Start Heartbeat (wire failure callback so permanent failures notify Lead)
        let self_for_failure = std::sync::Arc::clone(self);
        let failure_notify: super::heartbeat::FailureNotifyFn =
            std::sync::Arc::new(move |task_id: String, agent: String, reason: String| {
                self_for_failure.dispatch_team_notify_failed(&task_id, &agent, &reason);
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
        *self.done_final_posted.lock().unwrap() = false;
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
        let lead_name = self
            .lead_agent_name
            .get()
            .map(|s| s.as_str())
            .unwrap_or("Lead");
        let specialists = self
            .available_specialists
            .get()
            .cloned()
            .unwrap_or_default();
        self.session.write_team_md(&render_team_manifest_document(
            lead_name,
            &specialists,
            Some(plan.team_manifest.as_str()),
        ))?;

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

    fn resolve_result_body(
        &self,
        agent: &str,
        label: &str,
        summary_or_note: &str,
        result_markdown: Option<&str>,
    ) -> String {
        if let Some(body) = result_markdown
            .map(str::trim)
            .filter(|body| !body.is_empty())
        {
            return body.to_string();
        }
        format!("# Result\n\nSubmitted by: {agent}\n\n{label}:\n{summary_or_note}\n")
    }

    /// 处理 Specialist 完成通知（由 MCP complete_task 工具触发）
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
        result_markdown: Option<&str>,
    ) -> Result<()> {
        // 1. 更新状态（校验认领者身份）
        self.registry.mark_done(task_id, agent, note)?;
        self.record_specialist_action(task_id, agent, SpecialistActionKind::Done);
        self.sync_task_artifacts(task_id)?;
        let result_artifact_path = format!("tasks/{task_id}/result.md");
        let result_body = self.resolve_result_body(agent, "Final note", note, result_markdown);
        let _ = self.session.write_task_result(task_id, &result_body);

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
            self.emit_milestone(TeamMilestoneEvent::AllTasksDone)?;
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
            self.emit_milestone(TeamMilestoneEvent::TaskDone {
                task_id: task_id.to_string(),
                task_title,
                agent: agent.to_string(),
                done_count,
                total,
            })?;
            // 下游任务解锁通知
            let ready = self.registry.find_ready_tasks()?;
            if !ready.is_empty() {
                let task_ids = ready.iter().map(|t| t.id.clone()).collect();
                self.emit_milestone(TeamMilestoneEvent::TasksUnlocked { task_ids })?;
            }
        }

        // 5. 推 TeamNotify 给 Lead
        let tasks = self.registry.all_tasks().unwrap_or_default();
        let mut routing_event = if result_markdown
            .map(str::trim)
            .filter(|body| !body.is_empty())
            .is_some()
        {
            TeamRoutingEvent::completed(task_id, agent, note, all_done)
                .with_result_payload(result_body, result_artifact_path)
        } else {
            // summary-only fallback keeps payload out of transcript to avoid duplicate injection
            TeamRoutingEvent::completed(task_id, agent, note, all_done)
                .with_result_artifact_path(result_artifact_path)
        };
        if all_done {
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
            routing_event.detail = format!(
                "[团队通知] 所有任务已完成 ✅\n\n完成摘要：\n{}\n\n请生成最终汇总并通过 post_update 发送给用户。",
                summary
            );
        } else {
            let done_count = tasks
                .iter()
                .filter(|t| t.status_raw == "done" || t.status_raw.starts_with("accepted:"))
                .count();
            let total = tasks.len();
            routing_event.detail = format!(
                "{}\n\n当前进度：{} / {} 完成",
                routing_event.detail, done_count, total
            );
        }
        self.dispatch_team_routing_event(self.build_routing_envelope(
            task_id,
            agent,
            routing_event,
        ));

        Ok(())
    }

    /// 处理 Specialist 提交结果（新语义：submitted，等待 Lead 验收）
    pub fn handle_specialist_submitted(
        &self,
        task_id: &str,
        agent: &str,
        summary: &str,
        result_markdown: Option<&str>,
    ) -> Result<()> {
        self.registry.submit_task_result(task_id, agent, summary)?;
        self.record_specialist_action(task_id, agent, SpecialistActionKind::Submitted);
        self.sync_task_artifacts(task_id)?;
        let result_artifact_path = format!("tasks/{task_id}/result.md");
        let result_body = self.resolve_result_body(agent, "Summary", summary, result_markdown);
        let _ = self.session.write_task_result(task_id, &result_body);

        let event = serde_json::json!({
            "event": "SUBMITTED",
            "task": task_id,
            "agent": agent,
            "ts": Utc::now().to_rfc3339(),
        })
        .to_string();
        let _ = self.session.append_event(&event);
        let _ = self.session.sync_tasks_md(&self.registry);

        self.dispatch_team_routing_event(
            self.build_routing_envelope(
                task_id,
                agent,
                if result_markdown
                    .map(str::trim)
                    .filter(|body| !body.is_empty())
                    .is_some()
                {
                    TeamRoutingEvent::submitted(task_id, agent, summary)
                        .with_result_payload(result_body, result_artifact_path)
                } else {
                    TeamRoutingEvent::submitted(task_id, agent, summary)
                        .with_result_artifact_path(result_artifact_path)
                },
            ),
        );
        let task_title = self
            .registry
            .get_task(task_id)
            .ok()
            .flatten()
            .map(|t| t.title)
            .unwrap_or_else(|| task_id.to_string());
        let _ = self.emit_milestone(TeamMilestoneEvent::TaskSubmitted {
            task_id: task_id.to_string(),
            task_title,
            agent: agent.to_string(),
        });
        Ok(())
    }

    /// Lead 验收已提交任务（submitted -> accepted），并复用里程碑检查逻辑。
    pub fn accept_submitted_task(&self, task_id: &str, by: &str) -> Result<()> {
        let submitted_task = self.registry.get_task(task_id)?;
        let (completed_agent, task_title) = submitted_task
            .as_ref()
            .map(|task| {
                let completed_agent = match task.status_parsed() {
                    super::registry::TaskStatus::Submitted { ref agent, .. } => agent.clone(),
                    _ => task
                        .assignee_hint
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                };
                (completed_agent, task.title.clone())
            })
            .unwrap_or_else(|| ("unknown".to_string(), task_id.to_string()));
        self.registry.accept_task(task_id, by)?;
        self.sync_task_artifacts(task_id)?;
        let _ = self.session.append_task_progress(
            task_id,
            &format!("[{}] {} accepted submission", Utc::now().to_rfc3339(), by),
        );
        let previous = self.session.task_dir(task_id).join("result.md");
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

        let tasks = self.registry.all_tasks().unwrap_or_default();
        let done_count = tasks
            .iter()
            .filter(|t| t.status_raw == "done" || t.status_raw.starts_with("accepted:"))
            .count();
        let total = tasks.len();
        self.emit_milestone(TeamMilestoneEvent::TaskDone {
            task_id: task_id.to_string(),
            task_title,
            agent: completed_agent.clone(),
            done_count,
            total,
        })?;

        let all_done = self.registry.all_done()?;
        if all_done {
            *self.team_state_inner.lock().unwrap() = TeamState::Done;
            self.emit_milestone(TeamMilestoneEvent::AllTasksDone)?;
        } else {
            let ready = self.registry.find_ready_tasks()?;
            if !ready.is_empty() {
                let task_ids = ready.iter().map(|t| t.id.clone()).collect();
                self.emit_milestone(TeamMilestoneEvent::TasksUnlocked { task_ids })?;
            }
        }

        self.dispatch_team_routing_event(self.build_routing_envelope(
            task_id,
            by,
            TeamRoutingEvent::accepted(task_id, by, all_done),
        ));
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

        self.dispatch_team_routing_event(self.build_routing_envelope(
            task_id,
            by,
            TeamRoutingEvent::reopened(task_id, by, reason),
        ));
        Ok(())
    }

    fn build_routing_envelope(
        &self,
        task_id: &str,
        agent: &str,
        event: TeamRoutingEvent,
    ) -> TeamRoutingEnvelope {
        let context = self.latest_dispatch_context(task_id, agent);
        let requester_session_key = context
            .as_ref()
            .map(|record| record.requester_session_key.clone())
            .or_else(|| self.lead_session_key())
            .or_else(|| self.scope.get().cloned());
        let delivery_source = context
            .as_ref()
            .and_then(|record| record.delivery_source.clone())
            .or_else(|| self.lead_delivery_source());
        let mut fallback_session_keys = Vec::new();
        if let Some(lead_key) = self.lead_session_key() {
            if requester_session_key.as_ref() != Some(&lead_key)
                && !fallback_session_keys.contains(&lead_key)
            {
                fallback_session_keys.push(lead_key.clone());
            }
        }
        if let Some(scope_key) = self.scope.get() {
            if requester_session_key.as_ref() != Some(scope_key)
                && !fallback_session_keys.contains(scope_key)
            {
                fallback_session_keys.push(scope_key.clone());
            }
        }
        TeamRoutingEnvelope {
            run_id: context
                .as_ref()
                .map(|record| record.run_id.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            parent_run_id: context
                .as_ref()
                .and_then(|record| record.parent_run_id.clone()),
            requester_session_key,
            fallback_session_keys,
            delivery_source,
            team_id: self.session.team_id.clone(),
            delivery_status: RoutingDeliveryStatus::NotRouted,
            event,
        }
    }

    fn latest_dispatch_context(&self, task_id: &str, agent: &str) -> Option<DispatchContextRecord> {
        self.dispatch_contexts
            .lock()
            .unwrap()
            .get(&(task_id.to_string(), agent.to_string()))
            .cloned()
    }

    fn dispatch_team_routing_event(&self, envelope: TeamRoutingEnvelope) {
        // Hold pending_store_lock for the entire flush→send→persist sequence to prevent
        // interleaving with concurrent dispatches that could cause event reordering.
        let _guard = self.pending_store_lock.lock().unwrap();

        let Some(tx) = self.team_notify_tx.get().cloned() else {
            if let Err(err) = self.session.append_pending_completion(
                &envelope
                    .clone()
                    .with_delivery_status(RoutingDeliveryStatus::PersistedPending),
            ) {
                tracing::error!(
                    team_id = %self.session.team_id,
                    task_id = %envelope.event.task_id,
                    error = %err,
                    "Failed to persist pending team routing event (no tx)"
                );
            }
            return;
        };

        // Flush existing pending events first (inline, lock already held).
        self.flush_pending_routing_events_locked(&tx);

        let pending_on_error = envelope
            .clone()
            .with_delivery_status(RoutingDeliveryStatus::PersistedPending);
        if let Err(err) = tx.try_send(TeamNotifyRequest {
            envelope: envelope.clone(),
        }) {
            tracing::warn!(
                team_id = %self.session.team_id,
                task_id = %envelope.event.task_id,
                kind = ?envelope.event.kind,
                "TeamNotify dispatch deferred: {err}"
            );
            if let Err(persist_err) = self.session.append_pending_completion(&pending_on_error) {
                tracing::error!(
                    team_id = %self.session.team_id,
                    task_id = %envelope.event.task_id,
                    error = %persist_err,
                    "Failed to persist pending team routing event"
                );
            }
        }
    }

    /// 构建并发送 TeamNotify InboundMsg 给 Lead（task 永久失败）
    pub fn dispatch_team_notify_failed(&self, task_id: &str, agent: &str, reason: &str) {
        let _ = self.emit_milestone(TeamMilestoneEvent::TaskFailed {
            task_id: task_id.to_string(),
            agent: agent.to_string(),
            reason: reason.to_string(),
        });
        self.dispatch_team_routing_event(self.build_routing_envelope(
            task_id,
            agent,
            TeamRoutingEvent::failed(task_id, reason),
        ));
    }

    fn dispatch_team_notify_missing_completion(&self, task_id: &str, agent: &str) {
        self.dispatch_team_routing_event(self.build_routing_envelope(
            task_id,
            agent,
            TeamRoutingEvent::missing_completion(task_id, agent),
        ));
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

        self.record_specialist_action(task_id, agent, SpecialistActionKind::Blocked);
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
        self.dispatch_team_routing_event(self.build_routing_envelope(
            task_id,
            agent,
            TeamRoutingEvent::blocked(task_id, agent, reason),
        ));
        let task_title = self
            .registry
            .get_task(task_id)
            .ok()
            .flatten()
            .map(|t| t.title)
            .unwrap_or_else(|| task_id.to_string());
        let _ = self.emit_milestone(TeamMilestoneEvent::TaskBlocked {
            task_id: task_id.to_string(),
            task_title,
            agent: agent.to_string(),
            reason: reason.to_string(),
        });

        Ok(())
    }

    /// 处理 Specialist 中间检查点，不改变任务状态，只通知 Lead 当前进展。
    pub fn handle_specialist_checkpoint(
        &self,
        task_id: &str,
        agent: &str,
        note: &str,
    ) -> Result<()> {
        self.record_specialist_action(task_id, agent, SpecialistActionKind::Checkpoint);
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
        self.dispatch_team_routing_event(self.build_routing_envelope(
            task_id,
            agent,
            TeamRoutingEvent::checkpoint(task_id, agent, note),
        ));
        let _ = self.emit_milestone(TeamMilestoneEvent::TaskCheckpoint {
            task_id: task_id.to_string(),
            agent: agent.to_string(),
            note: note.to_string(),
        });
        Ok(())
    }

    /// 处理 Specialist 请求协助，不改变任务状态，也不释放 claim。
    pub fn handle_specialist_help_requested(
        &self,
        task_id: &str,
        agent: &str,
        message: &str,
    ) -> Result<()> {
        self.record_specialist_action(task_id, agent, SpecialistActionKind::HelpRequested);
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
        self.dispatch_team_routing_event(self.build_routing_envelope(
            task_id,
            agent,
            TeamRoutingEvent::help_requested(task_id, agent, message),
        ));
        Ok(())
    }

    pub fn specialist_actions_since(
        &self,
        task_id: &str,
        agent: &str,
        started_at: chrono::DateTime<Utc>,
    ) -> Vec<SpecialistActionRecord> {
        self.recent_specialist_actions
            .lock()
            .unwrap()
            .iter()
            .filter(|record| {
                record.task_id == task_id && record.agent == agent && record.at >= started_at
            })
            .cloned()
            .collect()
    }

    pub fn classify_specialist_turn(
        &self,
        task_id: &str,
        agent: &str,
        started_at: chrono::DateTime<Utc>,
    ) -> SpecialistTurnOutcome {
        let records = self.specialist_actions_since(task_id, agent, started_at);
        classify_specialist_turn(&records, task_id, agent, started_at)
    }

    pub fn handle_specialist_missing_completion(
        &self,
        task_id: &str,
        agent: &str,
        reply_excerpt: Option<&str>,
    ) -> Result<()> {
        if self.registry.is_claimed_by(task_id, agent)? {
            self.registry
                .hold_claim(task_id, agent, "missing_completion")?;
        }
        self.sync_task_artifacts(task_id)?;
        let excerpt = reply_excerpt.unwrap_or("(no reply text captured)");
        let event = serde_json::json!({
            "event": "MISSING_COMPLETION",
            "task": task_id,
            "agent": agent,
            "reply_excerpt": excerpt,
            "ts": Utc::now().to_rfc3339(),
        })
        .to_string();
        let _ = self.session.append_event(&event);
        let _ = self.session.append_task_progress(
            task_id,
            &format!(
                "[{}] {} ended turn without canonical completion/progress tool. Reply excerpt: {}",
                Utc::now().to_rfc3339(),
                agent,
                excerpt
            ),
        );
        let _ = self.session.sync_tasks_md(&self.registry);
        self.dispatch_team_notify_missing_completion(task_id, agent);
        Ok(())
    }

    // ── 停止 ──────────────────────────────────────────────────────────────────

    /// 清除团队工作区：停止 Heartbeat、清空 tasks.db + 所有 jsonl 文件、重置内存状态回 Planning。
    /// 用于 /clear 命令，不归档目录（保留目录结构供下次复用）。
    pub async fn clear_workspace(&self) -> Result<()> {
        // Stop heartbeat
        if let Some(handle) = self.heartbeat_handle.lock().unwrap().take() {
            handle.abort();
        }
        // Keep the shared MCP server alive across /clear so the next lead turn
        // does not inherit a stale cached port with no listener behind it.
        // Clear tasks.db
        self.registry.clear_all_tasks()?;
        // Clear all jsonl files
        let dir = &self.session.dir;
        for filename in &[
            "events.jsonl",
            "routing-events.jsonl",
            "pending-completions.jsonl",
            "delivered-milestones.jsonl",
            "delivery-dedupe-hits.jsonl",
            "leader-updates.jsonl",
            "channel-sends.jsonl",
        ] {
            let path = dir.join(filename);
            if path.exists() {
                std::fs::write(&path, b"")?;
            }
        }
        // Clear task artifacts directory
        let tasks_dir = dir.join("tasks");
        if tasks_dir.exists() {
            std::fs::remove_dir_all(&tasks_dir)?;
            std::fs::create_dir_all(&tasks_dir)?;
        }
        // Reset in-memory state to Planning
        *self.team_state_inner.lock().unwrap() = TeamState::Planning;
        *self.done_final_posted.lock().unwrap() = false;
        self.pending_lead_fragments.lock().unwrap().clear();
        tracing::info!(team_id = %self.session.team_id, "Team workspace cleared");
        Ok(())
    }

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

    fn emit_milestone(&self, event: TeamMilestoneEvent) -> Result<()> {
        if let (Some(f), Some(scope)) = (self.milestone_fn.get(), self.scope.get()) {
            (f)(scope.clone(), event.clone());
        }
        tracing::info!(
            team_id = %self.session.team_id,
            kind = %event.kind_str(),
            "Milestone: {:?}", event
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
        self.session
            .ensure_task_plan(&task.id, &render_task_plan(task))?;
        Ok(())
    }

    fn record_specialist_action(&self, task_id: &str, agent: &str, kind: SpecialistActionKind) {
        const MAX_RECENT_SPECIALIST_ACTIONS: usize = 256;
        let mut actions = self.recent_specialist_actions.lock().unwrap();
        actions.push_back(SpecialistActionRecord {
            task_id: task_id.to_string(),
            agent: agent.to_string(),
            kind,
            at: Utc::now(),
        });
        while actions.len() > MAX_RECENT_SPECIALIST_ACTIONS {
            actions.pop_front();
        }
    }

    /// Called from setters (set_lead_session_key / set_team_notify_tx) where no lock is held yet.
    fn flush_pending_routing_events(&self) {
        let Some(tx) = self.team_notify_tx.get().cloned() else {
            return;
        };
        let _guard = self.pending_store_lock.lock().unwrap();
        self.flush_pending_routing_events_locked(&tx);
    }

    /// Core flush logic. Caller MUST already hold `pending_store_lock`.
    fn flush_pending_routing_events_locked(
        &self,
        tx: &tokio::sync::mpsc::Sender<TeamNotifyRequest>,
    ) {
        let pending = match self.session.load_pending_completions() {
            Ok(records) => records,
            Err(err) => {
                tracing::warn!(
                    team_id = %self.session.team_id,
                    error = %err,
                    "Failed to load pending team routing events"
                );
                return;
            }
        };
        if pending.is_empty() {
            return;
        }
        let mut remaining = Vec::new();
        for envelope in pending {
            let pending_on_error = envelope
                .clone()
                .with_delivery_status(RoutingDeliveryStatus::PersistedPending);
            if let Err(err) = tx.try_send(TeamNotifyRequest {
                envelope: envelope.clone(),
            }) {
                tracing::warn!(
                    team_id = %self.session.team_id,
                    task_id = %envelope.event.task_id,
                    "Failed to replay pending team routing event: {err}"
                );
                remaining.push(pending_on_error);
            }
        }
        if let Err(err) = self.session.replace_pending_completions(&remaining) {
            tracing::error!(
                team_id = %self.session.team_id,
                error = %err,
                "Failed to rewrite pending team routing events after replay"
            );
        }
    }

    pub fn persist_pending_routing_event(&self, envelope: TeamRoutingEnvelope) {
        let _guard = self.pending_store_lock.lock().unwrap();
        if let Err(err) = self.session.append_pending_completion(&envelope) {
            tracing::error!(
                team_id = %self.session.team_id,
                task_id = %envelope.event.task_id,
                error = %err,
                "Failed to persist pending team routing event"
            );
        }
    }

    pub fn mark_routing_event_delivered(&self, delivered: &TeamRoutingEnvelope) {
        let _guard = self.pending_store_lock.lock().unwrap();
        if let Err(err) = self
            .session
            .remove_pending_completion_by_run_id(&delivered.run_id)
        {
            tracing::error!(
                team_id = %self.session.team_id,
                run_id = %delivered.run_id,
                error = %err,
                "Failed to clear delivered pending routing event"
            );
        }
        if let Err(err) = self.session.append_routing_outcome(delivered) {
            tracing::error!(
                team_id = %self.session.team_id,
                run_id = %delivered.run_id,
                error = %err,
                "Failed to append routing outcome"
            );
        }
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

fn render_task_plan(task: &Task) -> String {
    let deps = task.deps();
    let deps_section = if deps.is_empty() {
        "None".to_string()
    } else {
        deps.join(", ")
    };
    format!(
        "# Task Plan: {title}\n\n**Task ID**: {id}\n**Assignee Hint**: {assignee}\n**Status**: {status}\n**Dependencies**: {deps}\n\n## Steps\n\n- [ ] Read `spec.md`, `TASKS.md`, and relevant team context\n- [ ] Break the work into concrete execution steps\n- [ ] Execute the task and capture meaningful checkpoints in `progress.md`\n- [ ] Write final deliverables to `result.md`\n\n## Notes\n\n- Success criteria: {criteria}\n- Update this file instead of keeping a private scratch plan.\n",
        title = task.title,
        id = task.id,
        assignee = task.assignee_hint.as_deref().unwrap_or("unassigned"),
        status = task.status_raw,
        deps = deps_section,
        criteria = task
            .success_criteria
            .as_deref()
            .unwrap_or("Complete the requested work."),
    )
}

fn render_team_manifest_document(
    lead_name: &str,
    specialists: &[String],
    extra_manifest: Option<&str>,
) -> String {
    let specialists_list = if specialists.is_empty() {
        "- none configured".to_string()
    } else {
        specialists
            .iter()
            .map(|name| format!("- `{name}`: specialist execution lane"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let extra_section = extra_manifest
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| format!("\n## Team-Specific Notes\n\n{text}\n"))
        .unwrap_or_default();
    format!(
        "# Team Constitution\n\n\
## Mission\n\n\
This team exists to coordinate user-visible work through one lead and zero or more specialists.\
\nThe lead owns delegation, acceptance, and final user-facing synthesis. Specialists own execution inside their assigned tasks.\n\n\
## Roles\n\n\
- Lead: `{lead_name}`\n\
- Specialists:\n{specialists_list}\n\n\
## Shared Sources Of Truth\n\n\
- `TEAM.md`: team contract and collaboration rules\n\
- `AGENTS.md`: runtime operating guide for this team workspace\n\
- `CONTEXT.md`: shared task background curated by the lead\n\
- `TASKS.md`: current task snapshot\n\
- `tasks/<task-id>/meta.json`: machine-readable task state\n\
- `tasks/<task-id>/spec.md`: lead-authored task definition\n\
- `tasks/<task-id>/plan.md`: specialist execution plan\n\
- `tasks/<task-id>/progress.md`: append-only progress and escalation trail\n\
- `tasks/<task-id>/result.md`: final specialist result plus lead acceptance notes\n\n\
## Coordination Precedence\n\n\
If the user is asking to delegate, split work, assign another bot, coordinate specialists, or manage team execution, the lead must enter team coordination first.\
\nGeneric repo workflow skills are for performing work inside a task, not for replacing task creation or assignment.\n\n\
## Speaking Rules\n\n\
- Only the lead speaks to the user-facing channel.\n\
- Specialists report upward through canonical team coordination actions.\n\
- Internal coordination messages are not user-visible until the lead summarizes them.\n\n\
## Task Lifecycle\n\n\
1. Lead creates tasks with explicit specs and success criteria.\n\
2. Team execution assigns ready tasks to specialists.\n\
3. Specialists keep `plan.md`, `progress.md`, and `result.md` current.\n\
4. Lead reviews submitted work, accepts or reopens it, then synthesizes a user-visible update.\n\n\
## Transport Note\n\n\
This team currently exposes coordination through the runtime team tool surface.\
\nThat transport may change later, but this collaboration contract stays the same.\n\
{extra_section}",
        lead_name = lead_name,
        specialists_list = specialists_list,
        extra_section = extra_section,
    )
}

fn render_team_agents_guide(lead_name: &str, specialists: &[String]) -> String {
    let specialists_list = if specialists.is_empty() {
        "- none configured".to_string()
    } else {
        specialists
            .iter()
            .map(|name| format!("- `{name}`"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "# Team Workspace Operating Guide\n\n\
This workspace is for coordinated team execution.\n\
Read `TEAM.md`, `TASKS.md`, and task-local artifacts before acting.\n\n\
## Current Lead\n\n\
- `{lead_name}` is the front-stage lead for this team cycle.\n\
- Available specialists:\n{specialists_list}\n\n\
## Lead Turn Rules\n\n\
Use team coordination first when the user intent is any of:\n\
- delegate work to another bot or specialist\n\
- split a request into sub-tasks\n\
- track specialist progress or review specialist results\n\
- ask for team execution, handoff, assignment, or re-assignment\n\n\
For those requests, do not begin with generic repo workflow chatter like \"check skills\", \"brainstorm first\", or \"write a plan first\".\
\nFirst decide the task graph and use the team coordination surface:\n\
- `create_task(...)`\n\
- `assign_task(...)`\n\
- `start_execution()`\n\
- `get_task_status()`\n\
- `post_update(...)`\n\n\
Generic repo workflow skills remain available, but they are for doing work inside a task or for explicit design/implementation requests from the user.\n\n\
## Specialist Turn Rules\n\n\
- Treat the assigned task and `tasks/<task-id>/...` artifacts as the working surface.\n\
- Update `plan.md` before substantive execution if it still contains only the default scaffold.\n\
- Use canonical team coordination actions for checkpoints, help requests, submission, completion, blocking, and reopen handling.\n\
- Do not rely on plain natural-language replies as the only record of progress.\n\n\
## Artifact Discipline\n\n\
Each task directory should remain readable without replaying the whole transcript:\n\
- `spec.md` explains the assignment\n\
- `plan.md` explains the execution approach\n\
- `progress.md` explains what happened during execution\n\
- `result.md` contains the deliverable and review outcome\n\n\
## User-Facing Rule\n\n\
Only the lead owns direct user-facing synthesis. Specialists report through the team contract, even if the underlying transport changes in the future.\n",
        lead_name = lead_name,
        specialists_list = specialists_list,
    )
}

// ─── 测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::registry::CreateTask;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::{timeout, Duration};

    async fn post_initialize_to_mcp(port: u16) -> String {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect to shared team MCP server");
        let body = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test-client","version":"0.0.0"}}}"#;
        let request = format!(
            concat!(
                "POST /mcp HTTP/1.1\r\n",
                "Host: 127.0.0.1:{port}\r\n",
                "Accept: application/json, text/event-stream\r\n",
                "Content-Type: application/json\r\n",
                "Content-Length: {content_length}\r\n",
                "Connection: close\r\n",
                "\r\n",
                "{body}"
            ),
            port = port,
            content_length = body.len(),
            body = body,
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write initialize request");

        let mut response = Vec::new();
        timeout(Duration::from_secs(2), stream.read_to_end(&mut response))
            .await
            .expect("read initialize response timed out")
            .expect("read initialize response");
        String::from_utf8(response).expect("initialize response is utf-8")
    }

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

    #[test]
    fn test_reopen_for_new_planning_cycle_allows_new_tasks_after_done() {
        let (orch, _tmp) = make_orchestrator();
        *orch.team_state_inner.lock().unwrap() = TeamState::Done;

        assert!(
            orch.reopen_for_new_planning_cycle_if_done(),
            "Done team should reopen for a new planning cycle"
        );
        assert!(matches!(orch.team_state(), TeamState::Planning));

        let result = orch.register_task(CreateTask {
            id: "T002".into(),
            title: "Plan follow-up work".into(),
            ..Default::default()
        });
        assert!(result.is_ok(), "reopened team must accept new tasks");
    }

    #[test]
    fn test_register_task_archives_completed_cycle_before_new_cycle() {
        let (orch, tmp) = make_orchestrator();
        orch.register_task(CreateTask {
            id: "T001".into(),
            title: "finished".into(),
            ..Default::default()
        })
        .unwrap();
        orch.registry.try_claim("T001", "codex-beta").unwrap();
        orch.registry
            .submit_task_result("T001", "codex-beta", "done")
            .unwrap();
        orch.accept_submitted_task("T001", "leader").unwrap();
        orch.session
            .mark_delivery_dedupe("group:test", "all_tasks_done")
            .unwrap();

        orch.register_task(CreateTask {
            id: "T001".into(),
            title: "fresh cycle task".into(),
            ..Default::default()
        })
        .unwrap();

        let tasks = orch.registry.all_tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "fresh cycle task");
        assert!(
            tmp.path().join("cycles").exists(),
            "completed cycle artifacts should be archived"
        );
        assert_eq!(
            orch.session.delivery_dedupe_ledger_size().unwrap(),
            0,
            "new cycle should reset milestone dedupe ledger"
        );
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

    #[tokio::test]
    async fn test_clear_workspace_keeps_shared_mcp_server_reachable() {
        let (orch, _tmp) = make_orchestrator();
        orch.start_mcp_server().await.unwrap();
        let port = orch
            .mcp_server_port
            .get()
            .copied()
            .expect("shared MCP server port");

        orch.clear_workspace().await.unwrap();

        let response = post_initialize_to_mcp(port).await;
        assert!(
            response.starts_with("HTTP/1.1 200 OK"),
            "unexpected initialize status: {response}"
        );
        assert!(
            response
                .to_ascii_lowercase()
                .contains("content-type: text/event-stream"),
            "missing SSE content type after /clear: {response}"
        );
        assert!(matches!(orch.team_state(), TeamState::Planning));
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

        orch.handle_specialist_done("T003", "codex", "created jwt.rs", None)
            .unwrap();

        let task = orch.registry.get_task("T003").unwrap().unwrap();
        use crate::team::registry::TaskStatus;
        assert!(matches!(task.status_parsed(), TaskStatus::Done));
        assert_eq!(task.completion_note.as_deref(), Some("created jwt.rs"));
        let result =
            std::fs::read_to_string(tmp.path().join("tasks").join("T003").join("result.md"))
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

        orch.handle_specialist_submitted("T004", "codex", "ready for review", None)
            .unwrap();

        let task = orch.registry.get_task("T004").unwrap().unwrap();
        use crate::team::registry::TaskStatus;
        assert!(matches!(
            task.status_parsed(),
            TaskStatus::Submitted { ref agent, .. } if agent == "codex"
        ));
        assert_eq!(task.completion_note.as_deref(), Some("ready for review"));
        let result =
            std::fs::read_to_string(tmp.path().join("tasks").join("T004").join("result.md"))
                .unwrap();
        assert!(result.contains("ready for review"));
        let meta = std::fs::read_to_string(tmp.path().join("tasks").join("T004").join("meta.json"))
            .unwrap();
        assert!(meta.contains("submitted:codex:"));
    }

    #[test]
    fn test_handle_specialist_done_routes_result_payload_and_artifact_ref() {
        let (orch, _tmp) = make_orchestrator();
        orch.set_scope(SessionKey::new("ws", "group:test-team"));
        orch.registry
            .create_task(CreateTask {
                id: "T003A".into(),
                title: "JWT impl".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T003A", "codex").unwrap();

        orch.handle_specialist_done("T003A", "codex", "created jwt.rs", None)
            .unwrap();

        let pending = orch.session.load_pending_completions().unwrap();
        assert_eq!(pending.len(), 1);
        // Only artifact path is set; inline payload is intentionally omitted to avoid
        // duplicate injection (detail already contains the note).
        assert_eq!(
            pending[0].event.result_artifact_path.as_deref(),
            Some("tasks/T003A/result.md")
        );
        assert!(
            pending[0].event.result_payload.is_none(),
            "done event should NOT carry inline payload (detail already contains the note)"
        );
    }

    #[test]
    fn test_handle_specialist_done_uses_explicit_result_markdown_for_payload_and_artifact() {
        let (orch, tmp) = make_orchestrator();
        orch.set_scope(SessionKey::new("ws", "group:test-team"));
        orch.registry
            .create_task(CreateTask {
                id: "T003B".into(),
                title: "JWT impl".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T003B", "codex").unwrap();

        let result_markdown = "# Result\n\nImplemented middleware\n\n```rust\nfn auth() {}\n```";
        orch.handle_specialist_done("T003B", "codex", "created jwt.rs", Some(result_markdown))
            .unwrap();

        let result =
            std::fs::read_to_string(tmp.path().join("tasks").join("T003B").join("result.md"))
                .unwrap();
        assert_eq!(result, result_markdown);

        let pending = orch.session.load_pending_completions().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].event.result_payload.as_deref(),
            Some(result_markdown)
        );
        assert_eq!(
            pending[0].event.result_artifact_path.as_deref(),
            Some("tasks/T003B/result.md")
        );
    }

    #[test]
    fn test_handle_specialist_submitted_routes_result_payload_and_artifact_ref() {
        let (orch, _tmp) = make_orchestrator();
        orch.set_scope(SessionKey::new("ws", "group:test-team"));
        orch.registry
            .create_task(CreateTask {
                id: "T004A".into(),
                title: "JWT impl".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T004A", "codex").unwrap();

        orch.handle_specialist_submitted("T004A", "codex", "ready for review", None)
            .unwrap();

        let pending = orch.session.load_pending_completions().unwrap();
        assert_eq!(pending.len(), 1);
        // Only artifact path is set; inline payload is intentionally omitted.
        assert_eq!(
            pending[0].event.result_artifact_path.as_deref(),
            Some("tasks/T004A/result.md")
        );
        assert!(
            pending[0].event.result_payload.is_none(),
            "submitted event should NOT carry inline payload (detail already contains the summary)"
        );
    }

    #[test]
    fn test_handle_specialist_submitted_uses_explicit_result_markdown_for_payload_and_artifact() {
        let (orch, tmp) = make_orchestrator();
        orch.set_scope(SessionKey::new("ws", "group:test-team"));
        orch.registry
            .create_task(CreateTask {
                id: "T004B".into(),
                title: "JWT impl".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T004B", "codex").unwrap();

        let result_markdown = "# Result\n\nReady for review\n\n- added jwt.rs\n- added tests";
        orch.handle_specialist_submitted(
            "T004B",
            "codex",
            "ready for review",
            Some(result_markdown),
        )
        .unwrap();

        let result =
            std::fs::read_to_string(tmp.path().join("tasks").join("T004B").join("result.md"))
                .unwrap();
        assert_eq!(result, result_markdown);

        let pending = orch.session.load_pending_completions().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].event.result_payload.as_deref(),
            Some(result_markdown)
        );
        assert_eq!(
            pending[0].event.result_artifact_path.as_deref(),
            Some("tasks/T004B/result.md")
        );
    }

    #[test]
    fn test_classify_specialist_turn_uses_recorded_actions() {
        let (orch, _tmp) = make_orchestrator();
        orch.registry
            .create_task(CreateTask {
                id: "T900".into(),
                title: "record actions".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T900", "worker").unwrap();
        let started_at = chrono::Utc::now();
        orch.handle_specialist_checkpoint("T900", "worker", "halfway")
            .unwrap();
        orch.handle_specialist_submitted("T900", "worker", "done", None)
            .unwrap();

        assert!(matches!(
            orch.classify_specialist_turn("T900", "worker", started_at),
            SpecialistTurnOutcome::TerminalSubmitted
        ));
    }

    #[test]
    fn test_handle_specialist_missing_completion_holds_claim_and_logs_diagnostic() {
        let (orch, tmp) = make_orchestrator();
        orch.registry
            .create_task(CreateTask {
                id: "T901".into(),
                title: "missing completion".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T901", "worker").unwrap();

        orch.handle_specialist_missing_completion("T901", "worker", Some("WORKER_OK"))
            .unwrap();

        let task = orch.registry.get_task("T901").unwrap().unwrap();
        assert!(matches!(
            task.status_parsed(),
            crate::team::registry::TaskStatus::Held {
                ref reason,
                ref agent,
                ..
            } if reason == "missing_completion" && agent == "worker"
        ));
        let progress =
            std::fs::read_to_string(tmp.path().join("tasks").join("T901").join("progress.md"))
                .unwrap();
        assert!(progress.contains("without canonical completion/progress tool"));
        assert!(progress.contains("WORKER_OK"));
        let events = std::fs::read_to_string(tmp.path().join("events.jsonl")).unwrap();
        assert!(events.contains("MISSING_COMPLETION"));
    }

    #[tokio::test]
    async fn pending_routing_event_replays_once_notify_path_is_available() {
        let (orch, _tmp) = make_orchestrator();
        orch.set_scope(SessionKey::new("ws", "group:test-team"));
        orch.dispatch_team_notify_failed("T404", "codex", "no lead yet");

        let pending = orch.session.load_pending_completions().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].requester_session_key.as_ref().unwrap().scope,
            "group:test-team"
        );

        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        orch.set_lead_session_key(SessionKey::new("ws", "group:test-team"));
        orch.set_team_notify_tx(tx);

        let replayed = timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("pending replay timed out")
            .expect("pending replay channel closed");
        assert_eq!(
            replayed
                .envelope
                .requester_session_key
                .as_ref()
                .unwrap()
                .scope,
            "group:test-team"
        );
        assert!(replayed
            .envelope
            .event
            .render_for_parent()
            .contains("永久失败"));
    }

    #[test]
    fn mark_routing_event_delivered_clears_pending_entry() {
        let (orch, _tmp) = make_orchestrator();
        let pending = TeamRoutingEnvelope {
            run_id: "run-pending".into(),
            parent_run_id: None,
            requester_session_key: Some(SessionKey::new("ws", "group:test-team")),
            fallback_session_keys: vec![],
            delivery_source: None,
            team_id: orch.session.team_id.clone(),
            delivery_status: RoutingDeliveryStatus::PersistedPending,
            event: TeamRoutingEvent::failed("T404", "boom"),
        };
        orch.session.append_pending_completion(&pending).unwrap();

        let delivered = pending
            .clone()
            .with_delivery_status(RoutingDeliveryStatus::DirectDelivered);
        orch.mark_routing_event_delivered(&delivered);

        assert!(orch.session.load_pending_completions().unwrap().is_empty());
        let outcomes = orch.session.load_routing_outcomes().unwrap();
        assert_eq!(outcomes, vec![delivered]);
    }

    #[test]
    fn build_routing_envelope_includes_distinct_fallback_targets() {
        let (orch, _tmp) = make_orchestrator();
        orch.set_lead_session_key(SessionKey::new("ws", "group:lead"));
        orch.set_scope(SessionKey::new("ws", "group:scope"));

        orch.dispatch_team_notify_failed("T777", "codex", "boom");

        let pending = orch.session.load_pending_completions().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].requester_session_key.as_ref().unwrap().scope,
            "group:lead"
        );
        assert_eq!(pending[0].fallback_session_keys.len(), 1);
        assert_eq!(pending[0].fallback_session_keys[0].scope, "group:scope");
    }

    #[test]
    fn routing_envelope_without_live_target_stays_pending_and_reports_missing_target() {
        let (orch, _tmp) = make_orchestrator();

        orch.dispatch_team_notify_failed("T999", "codex", "no routing target");

        let pending = orch.session.load_pending_completions().unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].requester_session_key.is_none());
        assert!(pending[0].fallback_session_keys.is_empty());

        let stats = orch.status_snapshot().routing_stats;
        assert_eq!(stats.pending_count, 1);
        assert_eq!(stats.missing_delivery_target, 1);
    }

    #[test]
    fn flush_pending_routing_events_preserves_unsent_records_when_channel_is_full() {
        let (orch, _tmp) = make_orchestrator();
        orch.set_scope(SessionKey::new("ws", "group:test-team"));
        orch.dispatch_team_notify_failed("T001", "codex", "one");
        orch.dispatch_team_notify_failed("T002", "codex", "two");

        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        orch.set_lead_session_key(SessionKey::new("ws", "group:test-team"));
        orch.set_team_notify_tx(tx);

        let pending = orch.session.load_pending_completions().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].event.task_id, "T002");
    }

    #[tokio::test]
    async fn test_accept_submitted_task_triggers_all_done_milestone() {
        let (orch, tmp) = make_orchestrator();
        let events: Arc<std::sync::Mutex<Vec<TeamMilestoneEvent>>> =
            Arc::new(std::sync::Mutex::new(vec![]));
        let evs_clone = Arc::clone(&events);

        orch.set_milestone_fn(Arc::new(move |_scope, ev| {
            evs_clone.lock().unwrap().push(ev);
        }));
        orch.set_scope(clawbro_protocol::SessionKey::new("test", "test-scope"));

        orch.registry
            .create_task(CreateTask {
                id: "T005".into(),
                title: "only task".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T005", "codex").unwrap();
        orch.handle_specialist_submitted("T005", "codex", "ready", None)
            .unwrap();
        orch.accept_submitted_task("T005", "claude").unwrap();

        let task = orch.registry.get_task("T005").unwrap().unwrap();
        use crate::team::registry::TaskStatus;
        assert!(matches!(
            task.status_parsed(),
            TaskStatus::Accepted { ref by, .. } if by == "claude"
        ));

        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| matches!(
                e,
                TeamMilestoneEvent::TaskDone { task_id, agent, done_count, total, .. }
                if task_id == "T005" && agent == "codex" && *done_count == 1 && *total == 1
            )),
            "TaskDone event must fire after accept; got: {:?}",
            evs
        );
        assert!(
            evs.iter()
                .any(|e| matches!(e, TeamMilestoneEvent::AllTasksDone)),
            "AllTasksDone event must fire after accept; got: {:?}",
            evs
        );
        let result =
            std::fs::read_to_string(tmp.path().join("tasks").join("T005").join("result.md"))
                .unwrap();
        assert!(result.contains("Accepted by: claude"));
        let progress =
            std::fs::read_to_string(tmp.path().join("tasks").join("T005").join("progress.md"))
                .unwrap();
        assert!(progress.contains("claude accepted submission"));
    }

    #[test]
    fn post_message_suppresses_duplicate_done_cycle_update() {
        let (orch, _tmp) = make_orchestrator();
        *orch.team_state_inner.lock().unwrap() = TeamState::Done;

        assert!(orch.post_message("final one"));
        assert!(!orch.post_message("final two"));

        let updates =
            std::fs::read_to_string(orch.session.dir.join("leader-updates.jsonl")).unwrap();
        assert!(updates.contains("final one"));
        assert!(!updates.contains("final two"));
    }

    #[tokio::test]
    async fn test_all_done_triggers_milestone_event() {
        let (orch, _tmp) = make_orchestrator();
        let events: Arc<std::sync::Mutex<Vec<TeamMilestoneEvent>>> =
            Arc::new(std::sync::Mutex::new(vec![]));
        let evs_clone = Arc::clone(&events);

        orch.set_milestone_fn(Arc::new(move |_scope, ev| {
            evs_clone.lock().unwrap().push(ev);
        }));
        orch.set_scope(clawbro_protocol::SessionKey::new("test", "test-scope"));

        orch.registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "only task".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T001", "codex").unwrap();
        orch.handle_specialist_done("T001", "codex", "done", None)
            .unwrap();

        let evs = events.lock().unwrap();
        assert!(
            evs.iter()
                .any(|e| matches!(e, TeamMilestoneEvent::AllTasksDone)),
            "AllTasksDone event must fire; got: {:?}",
            evs
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
        assert!(team_md.contains("Team Constitution"));
        assert!(team_md.contains("Coordination Precedence"));
        assert!(team_md.contains("tasks/<task-id>/plan.md"));

        let agents_md = orch.session.read_agents_md();
        assert!(agents_md.contains("Lead Turn Rules"));
        assert!(agents_md.contains("delegate work to another bot"));
        assert!(agents_md.contains("Generic repo workflow skills remain available"));

        let task = orch.registry.get_task("T001").unwrap().unwrap();
        assert_eq!(task.title, "Setup");
        let task_dir = tmp.path().join("tasks").join("T001");
        assert!(task_dir.join("meta.json").is_file());
        assert!(task_dir.join("spec.md").is_file());
        assert!(task_dir.join("plan.md").is_file());
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

    // ─── 里程碑事件类型化测试 ──────────────────────────────────────────────────
    // 测试仅断言 TeamMilestoneEvent 枚举变体和字段，不依赖 emoji/文案字符串。
    // IM 渲染逻辑由 milestone::render_for_im() 独立测试。

    fn collect_events(
        orch: &Arc<TeamOrchestrator>,
    ) -> Arc<std::sync::Mutex<Vec<TeamMilestoneEvent>>> {
        let events: Arc<std::sync::Mutex<Vec<TeamMilestoneEvent>>> =
            Arc::new(std::sync::Mutex::new(vec![]));
        let evs_clone = Arc::clone(&events);
        orch.set_milestone_fn(Arc::new(move |_scope, ev| {
            evs_clone.lock().unwrap().push(ev);
        }));
        orch.set_scope(clawbro_protocol::SessionKey::new("lark", "group:test"));
        events
    }

    #[test]
    fn test_checkpoint_emits_typed_milestone_event() {
        let (orch, _tmp) = make_orchestrator();
        let events = collect_events(&orch);

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

        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| matches!(
                e,
                TeamMilestoneEvent::TaskCheckpoint { task_id, agent, note }
                if task_id == "T120" && agent == "codex" && note == "halfway there"
            )),
            "checkpoint must emit TaskCheckpoint {{ task_id, agent, note }}; got: {:?}",
            evs
        );
    }

    #[test]
    fn test_submit_emits_typed_milestone_event() {
        let (orch, _tmp) = make_orchestrator();
        let events = collect_events(&orch);

        orch.registry
            .create_task(CreateTask {
                id: "T121".into(),
                title: "Implement JWT".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("T121", "codex").unwrap();
        orch.handle_specialist_submitted("T121", "codex", "added jwt.rs", None)
            .unwrap();

        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| matches!(
                e,
                TeamMilestoneEvent::TaskSubmitted { task_id, task_title, agent }
                if task_id == "T121" && task_title == "Implement JWT" && agent == "codex"
            )),
            "submit must emit TaskSubmitted {{ task_id, task_title, agent }}; got: {:?}",
            evs
        );
    }

    #[test]
    fn test_failed_emits_typed_milestone_event() {
        let (orch, _tmp) = make_orchestrator();
        let events = collect_events(&orch);

        orch.dispatch_team_notify_failed("T404", "codex", "max retries exceeded");

        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| matches!(
                e,
                TeamMilestoneEvent::TaskFailed { task_id, agent, reason }
                if task_id == "T404" && agent == "codex" && reason == "max retries exceeded"
            )),
            "failed must emit TaskFailed {{ task_id, agent, reason }}; got: {:?}",
            evs
        );
    }

    #[test]
    fn test_blocked_emits_typed_milestone_event() {
        let (orch, _tmp) = make_orchestrator();
        let events = collect_events(&orch);

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

        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| matches!(
                e,
                TeamMilestoneEvent::TaskBlocked { task_id, agent, reason, .. }
                if task_id == "T122" && agent == "codex" && reason == "missing dep"
            )),
            "blocked must emit TaskBlocked {{ task_id, agent, reason }}; got: {:?}",
            evs
        );
    }

    #[test]
    fn test_done_individual_emits_typed_milestone_with_progress() {
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
        let events = collect_events(&orch);

        orch.handle_specialist_done("T130", "codex", "done", None)
            .unwrap();

        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| matches!(
                e,
                TeamMilestoneEvent::TaskDone {
                    task_id, task_title, agent, done_count, total
                }
                if task_id == "T130"
                    && task_title == "First task"
                    && agent == "codex"
                    && *done_count == 1
                    && *total == 2
            )),
            "individual done must emit TaskDone with correct progress counts; got: {:?}",
            evs
        );
    }

    // ─── 功能测试：完整 Agent Swarm 生命周期 ──────────────────────────────────

    /// emit_milestone 在未注册 milestone_fn/scope 时不 panic，直接返回 Ok
    #[test]
    fn test_emit_milestone_noop_without_milestone_fn() {
        let (orch, _tmp) = make_orchestrator();
        // 故意不调用 set_milestone_fn / set_scope

        orch.registry
            .create_task(CreateTask {
                id: "NOOP01".into(),
                title: "no-op test".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("NOOP01", "codex").unwrap();

        let result = orch.handle_specialist_checkpoint("NOOP01", "codex", "halfway");
        assert!(
            result.is_ok(),
            "checkpoint without milestone_fn must return Ok, not panic; got: {:?}",
            result
        );
    }

    /// 完整 Agent Swarm 生命周期：T_A（无依赖）→ T_B（依赖 T_A）
    /// 断言：TaskDone(T_A) + TasksUnlocked([T_B]) + AllTasksDone — 全类型化
    #[test]
    fn test_full_swarm_lifecycle_dep_chain_typed_events() {
        let (orch, _tmp) = make_orchestrator();
        let events = collect_events(&orch);

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
        orch.handle_specialist_done("T_A", "codex", "db schema created", None)
            .unwrap();

        {
            let evs = events.lock().unwrap();
            assert!(
                evs.iter().any(|e| matches!(
                    e,
                    TeamMilestoneEvent::TaskDone { task_id, agent, done_count, total, .. }
                    if task_id == "T_A" && agent == "codex" && *done_count == 1 && *total == 2
                )),
                "T_A done must emit TaskDone(1/2); got: {:?}",
                evs
            );
            assert!(
                evs.iter().any(|e| matches!(
                    e,
                    TeamMilestoneEvent::TasksUnlocked { task_ids }
                    if task_ids.contains(&"T_B".to_string())
                )),
                "T_A done must emit TasksUnlocked([T_B]); got: {:?}",
                evs
            );
            assert!(
                !evs.iter()
                    .any(|e| matches!(e, TeamMilestoneEvent::AllTasksDone)),
                "AllTasksDone must NOT fire before T_B completes; got: {:?}",
                evs
            );
        }

        // ── 阶段 2：T_B 完成 ────────────────────────────────────────────────
        orch.registry.try_claim("T_B", "claude").unwrap();
        orch.handle_specialist_done("T_B", "claude", "data seeded", None)
            .unwrap();

        {
            let evs = events.lock().unwrap();
            assert!(
                evs.iter()
                    .any(|e| matches!(e, TeamMilestoneEvent::AllTasksDone)),
                "AllTasksDone must fire after T_B done; got: {:?}",
                evs
            );
        }

        assert!(
            matches!(*orch.team_state_inner.lock().unwrap(), TeamState::Done),
            "TeamState must be Done after all tasks complete"
        );
    }

    /// checkpoint → submit 事件顺序：TaskCheckpoint 先于 TaskSubmitted
    #[test]
    fn test_submit_flow_checkpoint_precedes_submit_event() {
        let (orch, _tmp) = make_orchestrator();
        let events = collect_events(&orch);

        orch.registry
            .create_task(CreateTask {
                id: "SA01".into(),
                title: "Write Auth API".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("SA01", "codex").unwrap();

        orch.handle_specialist_checkpoint("SA01", "codex", "50% done")
            .unwrap();
        orch.handle_specialist_submitted("SA01", "codex", "auth.rs complete", None)
            .unwrap();

        let evs = events.lock().unwrap();
        let cp_pos = evs.iter().position(|e| {
            matches!(
                e, TeamMilestoneEvent::TaskCheckpoint { task_id, .. } if task_id == "SA01"
            )
        });
        let sub_pos = evs.iter().position(|e| {
            matches!(
                e, TeamMilestoneEvent::TaskSubmitted { task_id, task_title, .. }
                if task_id == "SA01" && task_title == "Write Auth API"
            )
        });
        assert!(
            cp_pos.is_some(),
            "TaskCheckpoint event must exist; got: {:?}",
            evs
        );
        assert!(
            sub_pos.is_some(),
            "TaskSubmitted event must exist; got: {:?}",
            evs
        );
        assert!(
            cp_pos.unwrap() < sub_pos.unwrap(),
            "TaskCheckpoint must precede TaskSubmitted in event stream"
        );
    }

    /// blocked 后继续 checkpoint：TaskBlocked 先于 TaskCheckpoint
    #[test]
    fn test_blocked_then_retry_checkpoint_event_order() {
        let (orch, _tmp) = make_orchestrator();
        let events = collect_events(&orch);

        orch.registry
            .create_task(CreateTask {
                id: "BR01".into(),
                title: "Deploy service".into(),
                ..Default::default()
            })
            .unwrap();
        orch.registry.try_claim("BR01", "codex").unwrap();

        orch.handle_specialist_blocked("BR01", "codex", "missing env vars")
            .unwrap();
        orch.handle_specialist_checkpoint("BR01", "codex", "env vars fixed")
            .unwrap();

        let evs = events.lock().unwrap();
        let blocked_pos = evs.iter().position(|e| {
            matches!(
                e, TeamMilestoneEvent::TaskBlocked { task_id, reason, .. }
                if task_id == "BR01" && reason == "missing env vars"
            )
        });
        let cp_pos = evs.iter().position(|e| {
            matches!(
                e, TeamMilestoneEvent::TaskCheckpoint { task_id, note, .. }
                if task_id == "BR01" && note == "env vars fixed"
            )
        });
        assert!(
            blocked_pos.is_some(),
            "TaskBlocked event must exist; got: {:?}",
            evs
        );
        assert!(
            cp_pos.is_some(),
            "TaskCheckpoint event must exist after blocked; got: {:?}",
            evs
        );
        assert!(
            blocked_pos.unwrap() < cp_pos.unwrap(),
            "TaskBlocked must precede TaskCheckpoint in event stream"
        );
    }
}
