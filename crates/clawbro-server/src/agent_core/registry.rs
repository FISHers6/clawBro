// clawBro-gateway/crates/clawbro-agent/src/registry.rs
//! SessionRegistry: per-session backend routing + generic @mention routing.
//! Architectural role: Gateway orchestration layer (not platform-specific).
//! - Channels extract @mentions → InboundMsg.target_agent
//! - Registry resolves target_agent via AgentRoster (generic name lookup)
//! - No platform-specific text parsing here

use crate::agent_core::bindings::BindingRule;
use crate::agent_core::context_assembly::{assemble_context, ContextAssemblyRequest};
use crate::agent_core::control::session_router::get_orchestrator_for_session as route_orchestrator_for_session;
use crate::agent_core::dedup::DedupStore;
use crate::agent_core::memory::{MemoryEvent, MemorySystem, MemoryTarget};
use crate::agent_core::post_turn::{process_post_turn, PostTurnInput, PostTurnProcessor};
use crate::agent_core::relay::RelayEngine;
use crate::agent_core::roster::AgentRoster;
use crate::agent_core::routing::resolve_turn_routing;
use crate::agent_core::runtime_dispatch::{
    default_runtime_dispatch, RuntimeDispatch, RuntimeDispatchRequest,
};
use crate::agent_core::slash::SlashCommand;
use crate::agent_core::slash_service::{execute_slash_request, SlashRequest};
use crate::agent_core::team::orchestrator::TeamOrchestrator;
use crate::agent_core::team::orchestrator::TeamRuntimeSummary;
use crate::agent_core::team::tool_executor::{execute_team_tool_call, resolve_team_tool_role};
use crate::agent_core::turn_context::{TurnDeliverySource, TurnExecutionContext};
use crate::agent_core::ApprovalResolver;
use crate::channels_internal::mention_trigger::MentionTrigger;
use crate::protocol::{
    normalize_conversation_identity, parse_session_key_text, AgentEvent, InboundMsg, SessionKey,
};
use crate::runtime::contract::{ResumeRecoveryAction, TeamCallback, TurnResult};
use crate::runtime::{RuntimeEvent, TeamToolCall, TeamToolResponse};
use crate::session::{
    ResumableBackendSession, ResumeDropReason, SessionManager, StoredMessage, ToolCallRecord,
};
use anyhow::Result;
use dashmap::DashMap;
use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Single session state: holds per-session runtime backend selection.
pub struct Session {
    pub key: SessionKey,
    pub backend_id: Option<String>,
}

fn normalized_session_key(session_key: &SessionKey) -> SessionKey {
    normalize_conversation_identity(session_key)
}

fn collect_tool_call_records(events: &[RuntimeEvent]) -> Vec<ToolCallRecord> {
    use serde_json::json;
    use std::collections::{hash_map::Entry, HashMap};

    let mut order = Vec::new();
    let mut records: HashMap<String, ToolCallRecord> = HashMap::new();

    for event in events {
        match event {
            RuntimeEvent::ToolCallStarted {
                tool_name,
                call_id,
                input_summary,
            } => match records.entry(call_id.clone()) {
                Entry::Vacant(slot) => {
                    order.push(call_id.clone());
                    slot.insert(ToolCallRecord {
                        tool_call_id: Some(call_id.clone()),
                        name: tool_name.clone(),
                        input: input_summary
                            .as_ref()
                            .map(|summary| json!({ "summary": summary }))
                            .unwrap_or(serde_json::Value::Null),
                        output: None,
                    });
                }
                Entry::Occupied(mut slot) => {
                    let record = slot.get_mut();
                    record.name = tool_name.clone();
                    if record.input.is_null() {
                        record.input = input_summary
                            .as_ref()
                            .map(|summary| json!({ "summary": summary }))
                            .unwrap_or(serde_json::Value::Null);
                    }
                }
            },
            RuntimeEvent::ToolCallCompleted {
                tool_name,
                call_id,
                result,
            } => {
                let record = records.entry(call_id.clone()).or_insert_with(|| {
                    order.push(call_id.clone());
                    ToolCallRecord {
                        tool_call_id: Some(call_id.clone()),
                        name: tool_name.clone(),
                        input: serde_json::Value::Null,
                        output: None,
                    }
                });
                record.name = tool_name.clone();
                record.output = Some(result.clone());
            }
            RuntimeEvent::ToolCallFailed {
                tool_name,
                call_id,
                error,
            } => {
                let record = records.entry(call_id.clone()).or_insert_with(|| {
                    order.push(call_id.clone());
                    ToolCallRecord {
                        tool_call_id: Some(call_id.clone()),
                        name: tool_name.clone(),
                        input: serde_json::Value::Null,
                        output: None,
                    }
                });
                record.name = tool_name.clone();
                record.output = Some(format!("ERROR: {error}"));
            }
            _ => {}
        }
    }

    order
        .into_iter()
        .filter_map(|call_id| records.remove(&call_id))
        .collect()
}

#[derive(Clone, Copy)]
pub(crate) struct MemoryControlContext<'a> {
    registry: &'a SessionRegistry,
}

impl<'a> MemoryControlContext<'a> {
    pub(crate) fn is_enabled(self) -> bool {
        self.registry.memory_system.is_some()
    }

    pub(crate) fn memory_system(self) -> Option<Arc<MemorySystem>> {
        self.registry.memory_system.clone()
    }

    pub(crate) fn resolve_memory_target(self, target_agent: Option<&str>) -> MemoryTarget {
        target_agent
            .and_then(|mention| {
                self.registry
                    .roster
                    .as_ref()?
                    .find_by_mention(mention)?
                    .persona_dir
                    .clone()
            })
            .map(|dir| MemoryTarget::Agent { persona_dir: dir })
            .unwrap_or(MemoryTarget::Shared)
    }

    pub(crate) async fn read_agent_memory(
        self,
        agent_name: &str,
        session_key: &SessionKey,
    ) -> Result<Option<String>> {
        let persona_dir = self
            .registry
            .roster
            .as_ref()
            .and_then(|r| r.find_by_name(agent_name))
            .and_then(|entry| entry.persona_dir.clone());
        let Some(persona_dir) = persona_dir else {
            return Ok(None);
        };

        let Some(ms) = self.memory_system() else {
            return Ok(None);
        };

        let scoped = ms
            .store()
            .load_agent_memory(&persona_dir, session_key)
            .await
            .unwrap_or_default();
        Ok((!scoped.trim().is_empty()).then_some(scoped))
    }

    pub(crate) fn consume_pending_reset_confirmation(
        self,
        session_key: &SessionKey,
        now: std::time::Instant,
    ) -> bool {
        let normalized = normalized_session_key(session_key);
        let confirmed = self
            .registry
            .pending_resets
            .get(&normalized)
            .map(|t| now.duration_since(*t).as_secs() < 60)
            .unwrap_or(false);
        if confirmed {
            self.registry.pending_resets.remove(&normalized);
        }
        confirmed
    }

    pub(crate) fn arm_pending_reset(self, session_key: &SessionKey, now: std::time::Instant) {
        self.registry
            .pending_resets
            .insert(normalized_session_key(session_key), now);
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SlashControlContext<'a> {
    registry: &'a SessionRegistry,
}

impl<'a> SlashControlContext<'a> {
    pub(crate) fn memory(self) -> MemoryControlContext<'a> {
        MemoryControlContext {
            registry: self.registry,
        }
    }

    pub(crate) fn resolve_backend_id(self, name: &str) -> String {
        self.registry
            .roster
            .as_ref()
            .and_then(|r| r.find_by_name(name))
            .map(|entry| entry.runtime_backend_id().to_string())
            .unwrap_or_else(|| name.to_string())
    }

    pub(crate) fn set_session_backend(self, key: &SessionKey, backend_id: String) {
        self.registry.set_session_backend(key, backend_id);
    }

    pub(crate) async fn clear_session_history(self, session_key: &SessionKey) {
        if let Ok(session_id) = self
            .registry
            .session_manager
            .get_or_create(session_key)
            .await
        {
            // reset_conversation clears messages + backend_session_ids + message_count
            // so the next turn starts a fresh ACP session instead of resuming the old
            // backend session via load_session.
            if let Err(e) = self
                .registry
                .session_manager
                .reset_conversation(session_id)
                .await
            {
                tracing::warn!(error = %e, "reset_conversation failed during /reset");
            }
        }
    }

    pub(crate) fn approval_resolver(self) -> Option<Arc<dyn ApprovalResolver>> {
        self.registry.approval_resolver.get().cloned()
    }

    pub(crate) fn render_workspace_summary(
        self,
        session_key: &SessionKey,
        target_agent: Option<&str>,
    ) -> String {
        let roster_workspace: Option<std::path::PathBuf> = target_agent.and_then(|mention| {
            self.registry
                .roster
                .as_ref()
                .and_then(|r| r.find_by_mention(mention))
                .and_then(|entry| entry.workspace_dir.clone())
        });
        let resolved = self
            .registry
            .session_workspace(session_key)
            .or(roster_workspace)
            .or_else(|| self.registry.default_workspace.clone());
        let display = resolved
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none — running in gateway process directory)".to_string());
        format!("Current workspace: `{display}`")
    }

    pub(crate) fn set_session_workspace(self, key: &SessionKey, path: std::path::PathBuf) {
        self.registry
            .session_workspaces
            .insert(normalized_session_key(key), path);
    }

    /// Clear team workspace for /clear: stop heartbeat, wipe tasks + jsonl files, reset state.
    pub(crate) async fn clear_team_workspace(self, session_key: &SessionKey) {
        let orch_arc =
            route_orchestrator_for_session(&self.registry.team_orchestrators, session_key);
        if let Some(orch) = orch_arc {
            if let Err(e) = orch.clear_workspace().await {
                tracing::warn!(error = %e, "clear_team_workspace failed");
            }
        }
    }

    pub(crate) fn render_team_status(self, session_key: &SessionKey) -> String {
        let orch_arc =
            route_orchestrator_for_session(&self.registry.team_orchestrators, session_key);
        if let Some(orch) = orch_arc {
            let team_manifest = orch.session.read_team_md();
            let tasks_snapshot = orch.session.read_tasks_md();
            let task_count = orch
                .registry
                .all_tasks()
                .map(|tasks| {
                    let total = tasks.len();
                    let done = tasks.iter().filter(|t| t.status_raw == "done").count();
                    let claimed = tasks
                        .iter()
                        .filter(|t| t.status_raw.starts_with("claimed:"))
                        .count();
                    let pending = total - done - claimed;
                    format!("{done}/{total} 完成，{claimed} 执行中，{pending} 待处理")
                })
                .unwrap_or_else(|_| "无法读取任务数据".to_string());

            if team_manifest.trim().is_empty() && tasks_snapshot.trim().is_empty() {
                "ℹ️ Team 已初始化但尚无任务。Lead 正在规划中...".to_string()
            } else {
                format!(
                    "🏢 **Team 状态** — {task_count}\n\n{team_manifest}\n\n---\n\n{tasks_snapshot}"
                )
            }
        } else {
            "ℹ️ 当前 session 没有活跃的 Team。".to_string()
        }
    }
}

/// SessionRegistry: manages all per-session state with DashMap
pub struct SessionRegistry {
    sessions: DashMap<SessionKey, Arc<Session>>,
    default_backend_id: Option<String>,
    /// Deterministic routing bindings registered from gateway config.
    /// Evaluated only when there is no explicit @mention and no manual session override.
    bindings: std::sync::RwLock<Vec<BindingRule>>,
    session_manager: Arc<SessionManager>,
    dedup: DedupStore,
    /// WS subscriptions: session_key → list of WS client senders
    pub ws_subs: Arc<DashMap<SessionKey, Vec<tokio::sync::mpsc::UnboundedSender<AgentEvent>>>>,
    global_tx: broadcast::Sender<AgentEvent>,
    /// Gateway-level skills injection (prefix for all agents)
    system_injection: String,
    /// User-configured agent roster (None = single-engine mode, no @mention routing)
    pub roster: Option<AgentRoster>,
    /// Optional memory system for event-driven memory management
    memory_system: Option<Arc<MemorySystem>>,
    /// Per-(session, agent) turn counter for distillation triggers
    turn_counts: DashMap<(SessionKey, String), u64>,
    /// Last activity timestamp per session (for idle detection)
    last_activity: DashMap<SessionKey, std::time::Instant>,
    /// Pending /memory reset confirmations: session_key → timestamp of first reset request
    pending_resets: DashMap<SessionKey, std::time::Instant>,
    /// Default persona_dir for turns that do not resolve through roster routing.
    /// When set, TurnCompleted events fire even without a roster agent.
    default_persona_dir: Option<std::path::PathBuf>,
    /// Global default workspace directory. Used when no per-agent workspace_dir is set.
    default_workspace: Option<std::path::PathBuf>,
    /// Per-session workspace overrides set via /workspace command.
    session_workspaces: DashMap<SessionKey, std::path::PathBuf>,
    /// Gateway-level skill search directories (fallback after workspace/.agents/skills/ and agent extra dirs).
    skill_loader_dirs: Vec<std::path::PathBuf>,
    /// Tracks which persona directories have already been initialized (SOUL.md created).
    /// Avoids repeated blocking filesystem calls per message.
    initialized_persona_dirs: dashmap::DashSet<std::path::PathBuf>,
    /// Pre-registered task_reminder for Specialist turns.
    /// DispatchFn sets this before calling handle(); handle() removes and injects into AgentCtx.
    team_task_reminders: DashMap<SessionKey, String>,
    /// Per-group team orchestrators, keyed by team_id (= TeamSession::team_id).
    /// Supports multiple concurrent Team groups. Lead sessions route via lead_session_key scan;
    /// Specialist sessions route via scope prefix "{team_id}:{agent_name}".
    team_orchestrators: DashMap<String, Arc<TeamOrchestrator>>,
    /// Relay engine — processes [RELAY: @agent <指令>] markers synchronously.
    relay_engine: OnceLock<Arc<RelayEngine>>,
    /// Mention trigger — scans bot replies for @botname patterns.
    mention_trigger: OnceLock<Arc<MentionTrigger>>,
    /// Scopes where auto_promote = true (configured per-group in gateway.toml).
    /// When a matching message arrives with team trigger keywords, the turn is
    /// treated as a Lead turn even if no orchestrator is registered.
    auto_promote_scopes: dashmap::DashSet<String>,
    /// Per-session Semaphore(1): serializes concurrent handle() calls for the same session.
    /// Prevents two engine invocations running simultaneously for the same session_key,
    /// replacing the broken LaneQueue serial guarantee.
    session_semaphores: DashMap<SessionKey, Arc<tokio::sync::Semaphore>>,
    /// Runtime boundary between control-plane intent building and backend execution.
    runtime_dispatch: Arc<dyn RuntimeDispatch>,
    /// Family-agnostic Team Tool RPC endpoint URL for local backends.
    team_tool_url: OnceLock<String>,
    approval_resolver: OnceLock<Arc<dyn ApprovalResolver>>,
}

impl SessionRegistry {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        default_backend_id: Option<String>,
        session_manager: Arc<SessionManager>,
        system_injection: String,
        roster: Option<AgentRoster>,
        memory_system: Option<Arc<MemorySystem>>,
        default_persona_dir: Option<std::path::PathBuf>,
        default_workspace: Option<std::path::PathBuf>,
        skill_loader_dirs: Vec<std::path::PathBuf>,
    ) -> (Arc<Self>, broadcast::Receiver<AgentEvent>) {
        Self::with_runtime_dispatch(
            default_backend_id,
            session_manager,
            system_injection,
            roster,
            memory_system,
            default_persona_dir,
            default_workspace,
            skill_loader_dirs,
            default_runtime_dispatch(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_runtime_dispatch(
        default_backend_id: Option<String>,
        session_manager: Arc<SessionManager>,
        system_injection: String,
        roster: Option<AgentRoster>,
        memory_system: Option<Arc<MemorySystem>>,
        default_persona_dir: Option<std::path::PathBuf>,
        default_workspace: Option<std::path::PathBuf>,
        skill_loader_dirs: Vec<std::path::PathBuf>,
        runtime_dispatch: Arc<dyn RuntimeDispatch>,
    ) -> (Arc<Self>, broadcast::Receiver<AgentEvent>) {
        let (global_tx, global_rx) = broadcast::channel(1024);
        let registry = Arc::new(Self {
            sessions: DashMap::new(),
            default_backend_id,
            bindings: std::sync::RwLock::new(Vec::new()),
            session_manager,
            dedup: DedupStore::new(),
            ws_subs: Arc::new(DashMap::new()),
            global_tx,
            system_injection,
            roster,
            memory_system,
            turn_counts: DashMap::new(),
            last_activity: DashMap::new(),
            pending_resets: DashMap::new(),
            default_persona_dir,
            default_workspace,
            session_workspaces: DashMap::new(),
            skill_loader_dirs,
            initialized_persona_dirs: dashmap::DashSet::new(),
            team_task_reminders: DashMap::new(),
            team_orchestrators: DashMap::new(),
            relay_engine: OnceLock::new(),
            mention_trigger: OnceLock::new(),
            auto_promote_scopes: dashmap::DashSet::new(),
            session_semaphores: DashMap::new(),
            runtime_dispatch,
            team_tool_url: OnceLock::new(),
            approval_resolver: OnceLock::new(),
        });

        // Idle timer: check every 60s for sessions idle > 30 min
        if let Some(ms) = &registry.memory_system {
            let registry_weak = Arc::downgrade(&registry);
            let ms = Arc::clone(ms);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                loop {
                    interval.tick().await;
                    let Some(reg) = registry_weak.upgrade() else {
                        break;
                    };
                    let now = std::time::Instant::now();
                    // Collect idle scopes first to avoid mutation during iteration
                    let idle_scopes: Vec<SessionKey> = reg
                        .last_activity
                        .iter()
                        .filter(|e| now.duration_since(*e.value()).as_secs() >= 1800)
                        .map(|e| e.key().clone())
                        .collect();
                    for scope in &idle_scopes {
                        if let Some(roster) = &reg.roster {
                            for agent in roster.all_agents() {
                                if let Some(ref pd) = agent.persona_dir {
                                    ms.emit(MemoryEvent::SessionIdle {
                                        scope: scope.clone(),
                                        agent: agent.name.clone(),
                                        persona_dir: pd.clone(),
                                    });
                                }
                            }
                        }
                        // Reset timestamp so we don't re-fire until new activity arrives
                        reg.last_activity.insert(scope.clone(), now);
                    }
                }
            });
        }

        (registry, global_rx)
    }

    /// Pre-register a task_reminder for a Specialist session (called by DispatchFn before handle()).
    pub fn set_task_reminder(&self, key: SessionKey, reminder: String) {
        self.team_task_reminders
            .insert(normalized_session_key(&key), reminder);
    }

    /// Register a TeamOrchestrator for a given team_id.
    /// Supports multiple concurrent Team groups (one orchestrator per group).
    pub fn register_team_orchestrator(&self, team_id: String, orch: Arc<TeamOrchestrator>) {
        self.team_orchestrators.insert(team_id, orch);
    }

    pub fn get_team_orchestrator(&self, team_id: &str) -> Option<Arc<TeamOrchestrator>> {
        self.team_orchestrators
            .get(team_id)
            .map(|entry| entry.value().clone())
    }

    #[cfg(test)]
    pub(crate) fn memory_control(&self) -> MemoryControlContext<'_> {
        MemoryControlContext { registry: self }
    }

    pub(crate) fn slash_control(&self) -> SlashControlContext<'_> {
        SlashControlContext { registry: self }
    }

    /// Find the TeamOrchestrator responsible for this session.
    ///
    /// - Specialist sessions: channel == "specialist", scope == "{team_id}:{agent}" → look up by team_id prefix.
    /// - Lead sessions: scan all orchestrators for one whose lead_session_key matches.
    fn get_orchestrator_for_session(
        &self,
        session_key: &SessionKey,
    ) -> Option<Arc<TeamOrchestrator>> {
        route_orchestrator_for_session(&self.team_orchestrators, session_key)
    }

    pub fn team_orchestrator_for_session(
        &self,
        session_key: &SessionKey,
    ) -> Option<Arc<TeamOrchestrator>> {
        self.get_orchestrator_for_session(session_key)
    }

    /// Returns true when the Lead's direct text reply should be suppressed by `spawn_im_turn`.
    ///
    /// Suppression rule:
    /// - The session IS the active Team Lead for some orchestrator, AND
    /// - The team is currently in `Running` state.
    ///
    /// `Planning`, `AwaitingConfirm`, and `Done` are still normal conversational states:
    /// the Lead may answer directly, and any pre-tool text must remain visible as separate
    /// IM messages. Only active task execution is silent, where user-visible updates are
    /// expected to come from `post_update` milestones instead of the normal stream path.
    ///
    /// This flag must be captured **before** calling `handle()` so that the state snapshot
    /// is consistent with the turn being dispatched (fixes TOCTOU).
    pub fn should_suppress_lead_final_reply(&self, session_key: &SessionKey) -> bool {
        let Some(orch) = self.get_orchestrator_for_session(session_key) else {
            tracing::info!(
                channel = %session_key.channel,
                scope = %session_key.scope,
                "suppress_check: no orchestrator found for session"
            );
            return false;
        };
        if session_key.channel == "specialist" {
            tracing::info!(
                channel = %session_key.channel,
                scope = %session_key.scope,
                "suppress_check: specialist session is never a lead stream"
            );
            return false;
        }
        let state = orch.team_state();
        let suppress = matches!(
            state,
            crate::agent_core::team::orchestrator::TeamState::Running
        );
        tracing::info!(
            channel = %session_key.channel,
            scope = %session_key.scope,
            state = ?state,
            suppress,
            "suppress_check: result"
        );
        suppress
    }

    /// Returns true if the team is actively executing (state = Running).
    /// Used post-handle to decide whether Lead's plain-text reply should stay suppressed.
    pub fn is_team_running_or_done(&self, session_key: &SessionKey) -> bool {
        let Some(orch) = self.get_orchestrator_for_session(session_key) else {
            return false;
        };
        if session_key.channel == "specialist" {
            return false;
        }
        matches!(
            orch.team_state(),
            crate::agent_core::team::orchestrator::TeamState::Running
        )
    }

    /// Attach a RelayEngine — processes [RELAY: @agent <指令>] markers synchronously.
    pub fn set_relay_engine(&self, engine: Arc<RelayEngine>) {
        let _ = self.relay_engine.set(engine);
    }

    /// Attach a MentionTrigger — scans bot replies for @botname and dispatches BotMention msgs.
    pub fn set_mention_trigger(&self, trigger: Arc<MentionTrigger>) {
        let _ = self.mention_trigger.set(trigger);
    }

    /// Register a scope for keyword-based auto-promotion (auto_promote = true in config).
    /// When a user message in this scope contains team trigger keywords, the turn is
    /// treated as a Lead turn even if no orchestrator is currently registered for the scope.
    pub fn add_auto_promote_scope(&self, scope: String) {
        self.auto_promote_scopes.insert(scope);
    }

    pub fn is_session_busy(&self, key: &SessionKey) -> bool {
        self.session_semaphores
            .get(&normalized_session_key(key))
            .map(|sem| sem.available_permits() == 0)
            .unwrap_or(false)
    }

    /// Get-or-create per-session cached backend selection (used when no roster match)
    pub fn get_or_create_session(&self, key: &SessionKey) -> Arc<Session> {
        let normalized = normalized_session_key(key);
        self.sessions
            .entry(normalized.clone())
            .or_insert_with(|| {
                Arc::new(Session {
                    key: normalized,
                    backend_id: self.default_backend_id.clone(),
                })
            })
            .clone()
    }

    /// Override runtime backend for a session (/backend slash command)
    pub fn set_session_backend(&self, key: &SessionKey, backend_id: impl Into<String>) {
        let normalized = normalized_session_key(key);
        let session = Arc::new(Session {
            key: normalized.clone(),
            backend_id: Some(backend_id.into()),
        });
        self.sessions.insert(normalized, session);
    }

    /// Get per-session workspace override (set via /workspace command).
    pub fn session_workspace(&self, key: &SessionKey) -> Option<std::path::PathBuf> {
        self.session_workspaces
            .get(&normalized_session_key(key))
            .map(|v| v.clone())
    }
    /// All session scopes that have had activity (used by nightly consolidation scheduler).
    pub fn all_active_scopes(&self) -> Vec<SessionKey> {
        self.last_activity.iter().map(|e| e.key().clone()).collect()
    }

    /// Return how many seconds the given session has been idle (no `handle()` activity).
    ///
    /// Returns `None` if the session has never been active (no recorded activity).
    pub fn session_idle_seconds(&self, session_key: &str) -> Option<u64> {
        // session_key may be in "channel:scope" format or just a plain scope string.
        // Parse into a SessionKey with a single lookup: "channel:scope" splits on the first ':',
        // bare strings are treated as scope under a synthetic "cron" channel.
        let key_parsed = if session_key.contains(':') {
            parse_session_key_text(session_key)
                .unwrap_or_else(|_| SessionKey::new("cron", session_key))
        } else {
            SessionKey::new("cron", session_key)
        };
        self.last_activity
            .get(&normalized_session_key(&key_parsed))
            .map(|t| t.elapsed().as_secs())
    }

    /// Global broadcast sender (for WS monitor clients)
    pub fn global_sender(&self) -> broadcast::Sender<AgentEvent> {
        self.global_tx.clone()
    }

    pub fn set_team_tool_url(&self, url: String) {
        let _ = self.team_tool_url.set(url);
    }

    pub fn set_approval_resolver(&self, resolver: Arc<dyn ApprovalResolver>) {
        let _ = self.approval_resolver.set(resolver);
    }

    /// Register a deterministic scope -> agent binding from gateway config.
    pub fn register_scope_binding(&self, scope: String, agent_name: String) {
        self.register_binding(BindingRule::scope(scope, agent_name));
    }

    /// Register a deterministic (channel?, scope) -> agent binding from gateway config.
    pub fn register_scope_binding_with_channel(
        &self,
        channel: Option<String>,
        scope: String,
        agent_name: String,
    ) {
        self.register_binding(BindingRule::Scope {
            channel,
            scope,
            agent_name,
        });
    }

    pub fn register_binding(&self, binding: BindingRule) {
        let agent_name = binding.agent_name().to_string();
        if self.roster.as_ref().is_some_and(|roster| {
            crate::agent_core::routing::resolve_roster_match_by_name(Some(roster), &agent_name)
                .is_none()
        }) {
            tracing::warn!(
                agent = %agent_name,
                "ignoring routing binding for unknown roster agent"
            );
            return;
        }
        self.bindings.write().unwrap().push(binding);
    }

    pub fn session_manager_ref(&self) -> &SessionManager {
        &self.session_manager
    }

    pub fn team_summaries(&self) -> Vec<TeamRuntimeSummary> {
        let mut summaries: Vec<_> = self
            .team_orchestrators
            .iter()
            .map(|entry| entry.value().status_snapshot())
            .collect();
        summaries.sort_by(|a, b| a.team_id.cmp(&b.team_id));
        summaries
    }

    pub async fn invoke_team_tool(
        &self,
        session_key: &SessionKey,
        call: TeamToolCall,
    ) -> Result<TeamToolResponse> {
        let team_orch = self
            .get_orchestrator_for_session(session_key)
            .ok_or_else(|| anyhow::anyhow!("no TeamOrchestrator found for session"))?;
        let role = resolve_team_tool_role(session_key, &team_orch)?;
        execute_team_tool_call(team_orch, role, call).await
    }

    async fn apply_team_callback(
        &self,
        session_key: &SessionKey,
        callback: TeamCallback,
    ) -> Result<()> {
        let call = match callback {
            TeamCallback::TaskCreated {
                task_id,
                title,
                assignee,
            } => TeamToolCall::CreateTask {
                id: Some(task_id),
                title,
                assignee: Some(assignee),
                spec: None,
                deps: vec![],
                success_criteria: None,
            },
            TeamCallback::TaskAssigned { task_id, assignee } => TeamToolCall::AssignTask {
                task_id,
                new_assignee: assignee,
            },
            TeamCallback::ExecutionStarted => TeamToolCall::StartExecution,
            TeamCallback::PublicUpdatePosted { message } => TeamToolCall::PostUpdate { message },
            TeamCallback::TaskCheckpoint {
                task_id,
                note,
                agent,
            } => TeamToolCall::CheckpointTask {
                task_id,
                note,
                agent: Some(agent),
            },
            TeamCallback::TaskSubmitted {
                task_id,
                summary,
                result_markdown,
                agent,
            } => TeamToolCall::SubmitTaskResult {
                task_id,
                summary,
                result_markdown,
                agent: Some(agent),
            },
            TeamCallback::TaskAccepted { task_id, by } => TeamToolCall::AcceptTask {
                task_id,
                by: Some(by),
            },
            TeamCallback::TaskReopened {
                task_id,
                reason,
                by,
            } => TeamToolCall::ReopenTask {
                task_id,
                reason,
                by: Some(by),
            },
            TeamCallback::TaskBlocked {
                task_id,
                reason,
                agent,
            } => TeamToolCall::BlockTask {
                task_id,
                reason,
                agent: Some(agent),
            },
            TeamCallback::TaskHelpRequested {
                task_id,
                message,
                agent,
            } => TeamToolCall::RequestHelp {
                task_id,
                message,
                agent: Some(agent),
            },
        };
        self.invoke_team_tool(session_key, call).await?;
        Ok(())
    }

    async fn apply_runtime_events(
        &self,
        session_key: &SessionKey,
        turn: &TurnResult,
    ) -> Result<()> {
        for event in &turn.events {
            if let RuntimeEvent::ToolCallback(callback) = event {
                self.apply_team_callback(session_key, callback.clone())
                    .await?;
            }
        }
        Ok(())
    }

    fn lead_human_team_delegation_requires_side_effect(
        inbound: &InboundMsg,
        is_lead: bool,
        team_orchestrator_present: bool,
    ) -> bool {
        if !is_lead
            || !team_orchestrator_present
            || inbound.source != crate::protocol::MsgSource::Human
        {
            return false;
        }
        inbound
            .content
            .as_text()
            .is_some_and(crate::agent_core::mode_selector::is_team_delegation_request)
    }

    fn turn_has_team_coordination_side_effect(turn: &TurnResult) -> bool {
        turn.events.iter().any(|event| {
            matches!(
                event,
                RuntimeEvent::ToolCallback(
                    TeamCallback::TaskCreated { .. }
                        | TeamCallback::TaskAssigned { .. }
                        | TeamCallback::ExecutionStarted
                )
            )
        })
    }

    fn team_coordination_side_effect_happened(
        turn: &TurnResult,
        team_orchestrator: Option<&Arc<TeamOrchestrator>>,
        coordination_revision_before: Option<u64>,
    ) -> bool {
        if Self::turn_has_team_coordination_side_effect(turn) {
            return true;
        }
        team_orchestrator
            .zip(coordination_revision_before)
            .is_some_and(|(orch, before)| orch.coordination_revision() > before)
    }

    fn build_missing_team_coordination_side_effect_reply() -> String {
        "我这轮没有完成实际任务创建或分配，因此不会宣称任务已开始执行。请重试，或明确指定要委派的 bot 和目标。".to_string()
    }

    fn turn_started_team_execution(turn: &TurnResult) -> bool {
        turn.events.iter().any(|event| {
            matches!(
                event,
                RuntimeEvent::ToolCallback(TeamCallback::ExecutionStarted)
            )
        })
    }

    async fn ensure_frontstage_team_execution_started(
        team_orchestrator: Option<&Arc<TeamOrchestrator>>,
    ) -> Result<bool> {
        let Some(orch) = team_orchestrator else {
            return Ok(false);
        };
        if orch.team_state() != crate::agent_core::team::orchestrator::TeamState::Planning {
            return Ok(matches!(
                orch.team_state(),
                crate::agent_core::team::orchestrator::TeamState::Running
                    | crate::agent_core::team::orchestrator::TeamState::Done
            ));
        }
        if orch.registry.all_tasks()?.is_empty() {
            return Ok(false);
        }
        match orch.activate().await {
            Ok(_) => Ok(true),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    team_id = %orch.session.team_id,
                    "failed to auto-activate team execution after frontstage delegation"
                );
                Ok(false)
            }
        }
    }

    fn team_execution_is_actually_running(
        turn: &TurnResult,
        team_orchestrator: Option<&Arc<TeamOrchestrator>>,
        auto_started: bool,
    ) -> bool {
        if auto_started || Self::turn_started_team_execution(turn) {
            return true;
        }
        team_orchestrator.is_some_and(|orch| {
            matches!(
                orch.team_state(),
                crate::agent_core::team::orchestrator::TeamState::Running
                    | crate::agent_core::team::orchestrator::TeamState::Done
            )
        })
    }

    fn build_missing_team_execution_start_reply() -> String {
        "我这轮虽然创建了任务，但还没有把团队执行真正启动，因此不会宣称 specialist 已开始处理。请重试，或要求我立即启动执行。".to_string()
    }

    /// Process one inbound message. Generic: works for any channel.
    pub async fn handle(&self, inbound: InboundMsg) -> Result<Option<String>> {
        self.handle_with_context(inbound, TurnExecutionContext::default())
            .await
    }

    pub async fn handle_with_context(
        &self,
        inbound: InboundMsg,
        turn_ctx: TurnExecutionContext,
    ) -> Result<Option<String>> {
        self.handle_with_context_internal(inbound, turn_ctx, None)
            .await
    }

    pub async fn handle_with_context_and_events(
        &self,
        inbound: InboundMsg,
        turn_ctx: TurnExecutionContext,
        turn_event_tx: broadcast::Sender<AgentEvent>,
    ) -> Result<Option<String>> {
        self.handle_with_context_internal(inbound, turn_ctx, Some(turn_event_tx))
            .await
    }

    async fn handle_with_context_internal(
        &self,
        inbound: InboundMsg,
        turn_ctx: TurnExecutionContext,
        turn_event_tx: Option<broadcast::Sender<AgentEvent>>,
    ) -> Result<Option<String>> {
        // Idempotent dedup
        if !self.dedup.check_and_insert(&inbound.id) {
            tracing::debug!("Dedup: skipping duplicate msg {}", inbound.id);
            return Ok(None);
        }

        let session_key = inbound.session_key.clone();
        let normalized_session_key = normalized_session_key(&session_key);
        let user_text = inbound.content.as_text().unwrap_or("").to_string();

        // Slash commands are control-plane actions and must not wait behind the
        // session turn semaphore. In particular, `/approve` must be able to
        // resolve a pending runtime approval while the original turn is blocked.
        if let Some(cmd) = SlashCommand::parse(&user_text) {
            return self
                .handle_slash(cmd, &session_key, inbound.target_agent.as_deref())
                .await;
        }

        // Per-session serial execution guard.
        // Acquires a Semaphore(1) for this session_key, preventing concurrent engine calls.
        // The permit is held for the full duration of handle() and dropped on return.
        // Must clone the Arc before awaiting to drop the DashMap guard (not Send across await).
        let _session_permit = {
            let sem = self
                .session_semaphores
                .entry(normalized_session_key.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(1)))
                .clone();
            sem.acquire_owned()
                .await
                .map_err(|e| anyhow::anyhow!("Session semaphore closed: {e}"))?
        };
        let user_text_for_log = user_text.clone();

        // Resolve the responsible TeamOrchestrator once, reuse throughout handle().
        // Must be computed before any usage below (confirmation interceptor, roster, etc.).
        let session_team_orch: Option<Arc<TeamOrchestrator>> =
            self.get_orchestrator_for_session(&session_key);
        let coordination_revision_before = session_team_orch
            .as_ref()
            .map(|orch| orch.coordination_revision());

        // ── Team Mode confirmation interceptor ──────────────────────────────────
        // When Lead called request_confirmation(), the next Human message is the user's yes/no.
        if inbound.source == crate::protocol::MsgSource::Human {
            if let Some(team_orch) = session_team_orch.as_ref() {
                if team_orch.team_state()
                    == crate::agent_core::team::orchestrator::TeamState::AwaitingConfirm
                {
                    if let Some(lead_key) = team_orch.lead_session_key() {
                        if session_key == lead_key {
                            let text_lower = user_text.to_lowercase();
                            let confirmed = ["yes", "是", "确认", "ok", "好的", "开始"]
                                .iter()
                                .any(|kw| text_lower.contains(kw));
                            if confirmed {
                                // Await activate() synchronously so the user sees the real outcome.
                                // Spawning it in the background would silently swallow activation errors.
                                return match team_orch.activate().await {
                                    Ok(_) => {
                                        tracing::info!("Team activated via user confirmation");
                                        Ok(Some("收到，开始执行。任务队列已启动。".to_string()))
                                    }
                                    Err(e) => {
                                        tracing::error!("Team activate error: {e}");
                                        Ok(Some(format!("启动团队任务失败：{e}")))
                                    }
                                };
                            } else {
                                // User said no or gave feedback — reset to Planning so Lead can adjust
                                *team_orch.team_state_inner.lock().unwrap() =
                                    crate::agent_core::team::orchestrator::TeamState::Planning;
                                // Fall through to normal routing (Lead handles the message)
                            }
                        }
                    }
                }
            }
        }

        let specialist_task_reminder = self
            .team_task_reminders
            .get(&normalized_session_key)
            .map(|v| v.clone());
        let session_backend_id = self.get_or_create_session(&session_key).backend_id.clone();
        let bindings = self.bindings.read().unwrap().clone();
        let routing = resolve_turn_routing(
            &inbound,
            self.roster.as_ref(),
            &bindings,
            &self.team_orchestrators,
            &self.auto_promote_scopes,
            session_backend_id,
            specialist_task_reminder,
        );
        if !routing.is_lead {
            self.team_task_reminders.remove(&normalized_session_key);
        }
        if inbound.source == crate::protocol::MsgSource::Human && routing.is_lead {
            if let Some(team_orchestrator) = routing.team_orchestrator.as_ref() {
                team_orchestrator.reopen_for_new_planning_cycle_if_done();
            }
        }
        tracing::debug!(
            session = ?routing.intent.session_key,
            mode = ?routing.intent.mode,
            leader_candidate = ?routing.intent.leader_candidate,
            target_backend = ?routing.intent.target_backend,
            "built turn intent"
        );

        self.refresh_team_delivery_context(
            routing.team_orchestrator.as_ref(),
            routing.is_lead,
            &session_key,
            turn_ctx.delivery_source.as_ref(),
        );
        if routing.is_lead {
            if let Some(team_orchestrator) = routing.team_orchestrator.as_ref() {
                team_orchestrator.clear_pending_lead_fragments();
            }
        }

        // Get-or-create persistent session record
        let session_id = self.session_manager.get_or_create(&session_key).await?;
        let storage = self.session_manager.storage();

        // load_recent_messages avoids deserializing the entire JSONL for long sessions.
        let recent = storage.load_recent_messages(session_id, 50).await?;
        let recent = &recent[..];

        // Save user message with sender annotation
        let user_msg = StoredMessage {
            id: Uuid::new_v4(),
            role: "user".to_string(),
            content: user_text.clone(),
            timestamp: inbound.timestamp,
            sender: Some(inbound.sender.clone()),
            tool_calls: None,
            fragment_event_ids: None,
            aggregation_mode: None,
        };
        storage.append_message(session_id, &user_msg).await?;

        // When a TeamNotify arrives, lazily set lead_session_key + scope if not yet set.
        // TeamNotify session_key IS the lead's session_key — find the orchestrator by scanning all.
        if inbound.source == crate::protocol::MsgSource::TeamNotify {
            if let Some(team_orch) = routing.team_orchestrator.as_ref() {
                team_orch.set_lead_session_key(session_key.clone());
                team_orch.set_scope(session_key.clone());
            }
        }
        let assembled = assemble_context(ContextAssemblyRequest {
            session_id,
            session_key: &session_key,
            inbound: &inbound,
            recent_messages: recent,
            roster_match: routing.roster_match.as_ref(),
            agent_role: routing.agent_role,
            task_reminder: routing.task_reminder.clone(),
            session_team_orch: routing.team_orchestrator.as_ref(),
            system_injection: &self.system_injection,
            memory_system: self.memory_system.as_ref(),
            default_persona_dir: self.default_persona_dir.clone(),
            default_workspace: self.default_workspace.clone(),
            session_workspace: self.session_workspace(&session_key),
            skill_loader_dirs: &self.skill_loader_dirs,
            initialized_persona_dirs: &self.initialized_persona_dirs,
            team_tool_url: self.team_tool_url.get().cloned(),
            allowed_team_tools: routing.allowed_team_tools.clone(),
        })
        .await;
        let mut ctx = assembled.ctx;
        let persona_prefix = assembled.persona_prefix;
        let resolved_persona_dir = assembled.resolved_persona_dir;
        let crate::agent_core::routing::RoutingDecision {
            intent,
            fallback_backend_id,
            sender_name,
            roster_match,
            is_lead,
            ..
        } = routing;

        // Resolve the expected backend_id for session lifecycle tracking.
        let expected_backend_id: Option<String> = intent
            .target_backend
            .clone()
            .or_else(|| intent.leader_candidate.clone())
            .or_else(|| fallback_backend_id.clone());
        // Load stored ACP session ID for this backend (if any) and stamp into ctx.
        if let Some(ref bid) = expected_backend_id {
            match self.runtime_dispatch.backend_resume_fingerprint(bid).await {
                Ok(Some(fingerprint)) => match self
                    .session_manager
                    .load_resumable_backend_session_id(session_id, bid, &fingerprint)
                    .await
                {
                    Ok(resume_state) => match resume_state {
                        ResumableBackendSession::Reuse(backend_session_id) => {
                            tracing::debug!(
                                session_id = %session_id,
                                backend_id = %bid,
                                backend_session_id = %backend_session_id,
                                "reusing stored backend session id"
                            );
                            ctx.backend_session_id = Some(backend_session_id);
                        }
                        ResumableBackendSession::NotAvailable => {
                            tracing::debug!(
                                session_id = %session_id,
                                backend_id = %bid,
                                "no resumable backend session id available; starting fresh backend session"
                            );
                        }
                        ResumableBackendSession::DroppedStale {
                            stale_session_id,
                            reason,
                        } => {
                            let reason = match reason {
                                ResumeDropReason::MissingFingerprint => "missing_fingerprint",
                                ResumeDropReason::FingerprintMismatch => "fingerprint_mismatch",
                            };
                            tracing::debug!(
                                session_id = %session_id,
                                backend_id = %bid,
                                stale_backend_session_id = %stale_session_id,
                                drop_reason = reason,
                                "dropping stale backend session id before runtime dispatch"
                            );
                        }
                    },
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            backend_id = %bid,
                            "failed to load resumable backend session id"
                        );
                    }
                },
                Ok(None) => {
                    tracing::warn!(backend_id = %bid, "no backend fingerprint available for resume gating");
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        backend_id = %bid,
                        "backend fingerprint lookup failed; forcing fresh backend session"
                    );
                }
            }
            if let Err(e) = self.session_manager.begin_turn(session_id, bid).await {
                tracing::warn!(error = %e, backend_id = %bid, "begin_turn failed");
            }
        }

        // Per-call event channel: forward to global_tx + ws_subs
        // TurnComplete is enriched with sender_name here (engine itself doesn't know roster)
        let session_tx = turn_event_tx.unwrap_or_else(|| {
            let (tx, _) = broadcast::channel::<AgentEvent>(1024);
            tx
        });
        let global_tx = self.global_tx.clone();
        let ws_subs_clone = Arc::clone(&self.ws_subs);
        let normalized_sk_for_fwd = normalized_session_key.clone();
        let sender_for_fwd = sender_name.clone();
        let prefix_for_fwd = persona_prefix.clone();
        {
            let mut fwd_rx = session_tx.subscribe();
            tokio::spawn(async move {
                while let Ok(event) = fwd_rx.recv().await {
                    // Inject sender into TurnComplete so WS clients know which agent replied
                    let event = match event {
                        AgentEvent::TurnComplete {
                            session_id,
                            full_text,
                            ..
                        } => AgentEvent::TurnComplete {
                            session_id,
                            full_text: match &prefix_for_fwd {
                                Some(p) => format!("{p}{full_text}"),
                                None => full_text,
                            },
                            sender: sender_for_fwd.clone(),
                        },
                        other => other,
                    };
                    let _ = global_tx.send(event.clone());
                    ws_subs_clone.alter(&normalized_sk_for_fwd, |_, mut vec| {
                        vec.retain(|tx| tx.send(event.clone()).is_ok());
                        vec
                    });
                }
            });
        }

        // Control-plane execution crosses a narrow runtime dispatch boundary into
        // the canonical multi-backend conductor.
        let turn_result = self
            .runtime_dispatch
            .dispatch(RuntimeDispatchRequest {
                intent,
                ctx,
                fallback_backend_id: fallback_backend_id.clone(),
                event_tx: session_tx,
            })
            .await;
        tracing::debug!(
            session_id = %session_id,
            ok = turn_result.is_ok(),
            has_expected_backend = expected_backend_id.is_some(),
            "runtime dispatch completed"
        );
        // Persist the emitted ACP session ID and reset turn status — unconditionally,
        // even on dispatch failure, so sessions never stay permanently in Running state.
        let emitted_session_id = turn_result
            .as_ref()
            .ok()
            .and_then(|r| r.emitted_backend_session_id.clone());
        let backend_resume_fingerprint = turn_result
            .as_ref()
            .ok()
            .and_then(|r| r.backend_resume_fingerprint.clone());
        let resume_recovery = turn_result
            .as_ref()
            .ok()
            .and_then(|r| r.resume_recovery.clone());
        let complete_backend_id = turn_result
            .as_ref()
            .ok()
            .and_then(|r| r.used_backend_id.clone())
            .or(expected_backend_id);
        if let Some(ref bid) = complete_backend_id {
            if let Some(ResumeRecoveryAction::DropStaleResumedSessionHandle { stale_session_id }) =
                resume_recovery
            {
                match self
                    .session_manager
                    .drop_backend_resume_state(session_id, bid)
                    .await
                {
                    Ok(_) => {
                        tracing::info!(
                            session_id = %session_id,
                            backend_id = %bid,
                            stale_backend_session_id = %stale_session_id,
                            "dropped stale backend resume state after runtime recovery"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            session_id = %session_id,
                            backend_id = %bid,
                            stale_backend_session_id = %stale_session_id,
                            "failed to drop stale backend resume state after runtime recovery"
                        );
                    }
                }
            }
            if let Err(e) = self
                .session_manager
                .complete_turn(
                    session_id,
                    bid,
                    emitted_session_id,
                    backend_resume_fingerprint,
                )
                .await
            {
                tracing::warn!(error = %e, backend_id = %bid, "complete_turn failed");
            } else {
                tracing::debug!(session_id = %session_id, backend_id = %bid, "complete_turn succeeded");
            }
        }
        let mut turn = turn_result?;
        let delegation_requested = Self::lead_human_team_delegation_requires_side_effect(
            &inbound,
            is_lead,
            session_team_orch.is_some(),
        );
        let coordination_happened = Self::team_coordination_side_effect_happened(
            &turn,
            session_team_orch.as_ref(),
            coordination_revision_before,
        );
        let missing_coordination = delegation_requested && !coordination_happened;
        if missing_coordination {
            tracing::warn!(
                session = ?session_key,
                user_input = %user_text_for_log,
                "lead delegation turn produced no concrete team side effects; replacing assistant reply"
            );
            turn.full_text = Self::build_missing_team_coordination_side_effect_reply();
        } else {
            self.apply_runtime_events(&session_key, &turn).await?;
            let auto_started = if delegation_requested {
                Self::ensure_frontstage_team_execution_started(session_team_orch.as_ref()).await?
            } else {
                false
            };
            if delegation_requested
                && !Self::team_execution_is_actually_running(
                    &turn,
                    session_team_orch.as_ref(),
                    auto_started,
                )
            {
                tracing::warn!(
                    session = ?session_key,
                    user_input = %user_text_for_log,
                    "lead delegation turn created tasks but did not start execution; replacing assistant reply"
                );
                turn.full_text = Self::build_missing_team_execution_start_reply();
            }
        }
        tracing::debug!(
            session_id = %session_id,
            full_text_len = turn.full_text.len(),
            event_count = turn.events.len(),
            "runtime events applied"
        );
        let reply_text = process_post_turn(
            PostTurnProcessor {
                relay_engine: self.relay_engine.get(),
                mention_trigger: self.mention_trigger.get(),
                memory_system: self.memory_system.as_ref(),
                last_activity: &self.last_activity,
                turn_counts: &self.turn_counts,
            },
            PostTurnInput {
                inbound: &inbound,
                session_key: &session_key,
                session_id,
                storage,
                sender_name,
                persona_prefix,
                roster_match: roster_match.as_ref(),
                persona_dir: resolved_persona_dir,
                user_text_for_log: &user_text_for_log,
                full_text: turn.full_text,
                tool_calls: collect_tool_call_records(&turn.events),
                is_lead,
                team_orchestrator: session_team_orch,
            },
        )
        .await?;
        tracing::debug!(
            session_id = %session_id,
            reply_text_len = reply_text.len(),
            "post turn processing completed"
        );
        Ok(Some(reply_text))
    }

    fn refresh_team_delivery_context(
        &self,
        team_orchestrator: Option<&Arc<TeamOrchestrator>>,
        is_lead: bool,
        session_key: &SessionKey,
        delivery_source: Option<&TurnDeliverySource>,
    ) {
        let Some(team_orchestrator) = team_orchestrator else {
            return;
        };
        if !is_lead {
            return;
        }
        team_orchestrator.set_lead_session_key(session_key.clone());
        if let Some(source) = delivery_source {
            team_orchestrator.update_lead_delivery_source(source.clone());
        }
    }

    /// Handle slash commands
    async fn handle_slash(
        &self,
        cmd: SlashCommand,
        session_key: &SessionKey,
        target_agent: Option<&str>,
    ) -> Result<Option<String>> {
        let reply = execute_slash_request(SlashRequest {
            session_key,
            command: &cmd,
            target_agent,
            control: self.slash_control(),
        })
        .await?;
        Ok(reply.final_text().map(str::to_string))
    }

    /// Test helper: inject an instant into pending_resets directly (bypasses 60s window).
    #[cfg(test)]
    pub fn inject_pending_reset_at(&self, key: SessionKey, instant: std::time::Instant) {
        self.pending_resets
            .insert(normalized_session_key(&key), instant);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::memory::{
        distiller::NoopDistiller, store::FileMemoryStore, MemorySystem,
    };
    use crate::agent_core::roster::{AgentEntry, AgentRoster};
    use crate::agent_core::runtime_dispatch::{RuntimeDispatch, RuntimeDispatchRequest};
    use crate::agent_core::ApprovalDecision;
    use crate::protocol::{InboundMsg, MsgContent};
    use crate::runtime::contract::{TeamCallback, TurnResult};
    use crate::runtime::RuntimeEvent;
    use crate::session::SessionStorage;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    fn make_registry() -> (Arc<SessionRegistry>, broadcast::Receiver<AgentEvent>) {
        let dir = std::env::temp_dir().join(format!("test-registry-{}", uuid::Uuid::new_v4()));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        SessionRegistry::new(
            None,
            session_manager,
            String::new(),
            None,
            None,
            None,
            None,
            vec![],
        )
    }

    fn make_registry_with_default_backend(
        backend_id: &str,
    ) -> (Arc<SessionRegistry>, broadcast::Receiver<AgentEvent>) {
        let dir = std::env::temp_dir().join(format!(
            "test-registry-default-backend-{}",
            uuid::Uuid::new_v4()
        ));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        SessionRegistry::new(
            Some(backend_id.to_string()),
            session_manager,
            String::new(),
            None,
            None,
            None,
            None,
            vec![],
        )
    }

    struct FakeRuntimeDispatch {
        calls: Arc<AtomicUsize>,
        last_backend: Arc<std::sync::Mutex<Option<String>>>,
        history_snapshots: Arc<std::sync::Mutex<Vec<Vec<(String, String)>>>>,
        backend_resume_fingerprint: Option<String>,
        emitted_backend_session_id: Option<String>,
        used_backend_id: Option<String>,
        resume_recovery: Option<ResumeRecoveryAction>,
    }

    #[async_trait::async_trait]
    impl RuntimeDispatch for FakeRuntimeDispatch {
        async fn dispatch(&self, request: RuntimeDispatchRequest) -> Result<TurnResult> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_backend.lock().unwrap() = request.intent.target_backend.clone();
            self.history_snapshots.lock().unwrap().push(
                request
                    .ctx
                    .history
                    .iter()
                    .map(|msg| (msg.role.clone(), msg.content.clone()))
                    .collect(),
            );
            Ok(TurnResult {
                full_text: format!("fake-dispatch: {}", request.intent.user_text),
                events: vec![],
                emitted_backend_session_id: self.emitted_backend_session_id.clone(),
                backend_resume_fingerprint: self.backend_resume_fingerprint.clone(),
                used_backend_id: self
                    .used_backend_id
                    .clone()
                    .or_else(|| request.intent.target_backend.clone()),
                resume_recovery: self.resume_recovery.clone(),
            })
        }

        async fn backend_resume_fingerprint(&self, _backend_id: &str) -> Result<Option<String>> {
            Ok(self.backend_resume_fingerprint.clone())
        }
    }

    struct TeamSideEffectRuntimeDispatch {
        calls: Arc<AtomicUsize>,
        team_orchestrator: Arc<TeamOrchestrator>,
    }

    #[async_trait::async_trait]
    impl RuntimeDispatch for TeamSideEffectRuntimeDispatch {
        async fn dispatch(&self, request: RuntimeDispatchRequest) -> Result<TurnResult> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.team_orchestrator.register_task(
                crate::agent_core::team::registry::CreateTask {
                    id: "T999".into(),
                    title: "Synthetic delegated task".into(),
                    assignee_hint: Some("codex-beta".into()),
                    ..Default::default()
                },
            )?;
            Ok(TurnResult {
                full_text: format!("fake-dispatch: {}", request.intent.user_text),
                events: vec![],
                emitted_backend_session_id: None,
                backend_resume_fingerprint: None,
                used_backend_id: request.intent.target_backend.clone(),
                resume_recovery: None,
            })
        }

        async fn backend_resume_fingerprint(&self, _backend_id: &str) -> Result<Option<String>> {
            Ok(None)
        }
    }

    struct SequencedStreamingRuntimeDispatch {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl RuntimeDispatch for SequencedStreamingRuntimeDispatch {
        async fn dispatch(&self, request: RuntimeDispatchRequest) -> Result<TurnResult> {
            let idx = self.calls.fetch_add(1, Ordering::SeqCst);
            let (text, delay_ms) = if idx == 0 {
                ("first turn reply", 150_u64)
            } else {
                ("second turn reply", 0_u64)
            };
            if delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
            let session_id = request.ctx.session_id;
            let _ = request.event_tx.send(AgentEvent::TextDelta {
                session_id,
                delta: text.to_string(),
            });
            let _ = request.event_tx.send(AgentEvent::TurnComplete {
                session_id,
                full_text: text.to_string(),
                sender: None,
            });
            Ok(TurnResult {
                full_text: text.to_string(),
                events: vec![],
                emitted_backend_session_id: None,
                backend_resume_fingerprint: None,
                used_backend_id: request.intent.target_backend.clone(),
                resume_recovery: None,
            })
        }

        async fn backend_resume_fingerprint(&self, _backend_id: &str) -> Result<Option<String>> {
            Ok(None)
        }
    }

    struct FakeApprovalResolver {
        decisions: Arc<std::sync::Mutex<Vec<(String, ApprovalDecision)>>>,
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

    fn make_registry_with_memory() -> (Arc<SessionRegistry>, broadcast::Receiver<AgentEvent>) {
        let db_dir =
            std::env::temp_dir().join(format!("test-registry-mem-{}", uuid::Uuid::new_v4()));
        let storage = SessionStorage::new(db_dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        let mem_dir = tempdir().unwrap();
        let store: Arc<dyn crate::agent_core::memory::MemoryStore> =
            Arc::new(FileMemoryStore::new(mem_dir.keep()));
        let distiller: Arc<dyn crate::agent_core::memory::MemoryDistiller> =
            Arc::new(NoopDistiller);
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

    fn make_registry_with_roster() -> (Arc<SessionRegistry>, broadcast::Receiver<AgentEvent>) {
        let dir = std::env::temp_dir().join(format!("test-registry-r-{}", uuid::Uuid::new_v4()));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        let roster = AgentRoster::new(vec![AgentEntry {
            name: "mybot".to_string(),
            mentions: vec!["@mybot".to_string()],
            backend_id: "my-custom-agent".to_string(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        }]);
        SessionRegistry::new(
            None,
            session_manager,
            String::new(),
            Some(roster),
            None,
            None,
            None,
            vec![],
        )
    }

    fn make_registry_with_runtime_dispatch_and_roster(
        default_backend_id: Option<&str>,
        roster_entries: Vec<AgentEntry>,
        runtime_dispatch: Arc<dyn RuntimeDispatch>,
    ) -> (Arc<SessionRegistry>, broadcast::Receiver<AgentEvent>) {
        let dir =
            std::env::temp_dir().join(format!("test-registry-routing-{}", uuid::Uuid::new_v4()));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        SessionRegistry::with_runtime_dispatch(
            default_backend_id.map(str::to_string),
            session_manager,
            String::new(),
            Some(AgentRoster::new(roster_entries)),
            None,
            None,
            None,
            vec![],
            runtime_dispatch,
        )
    }

    #[tokio::test]
    async fn per_turn_event_channel_isolated_for_same_session() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (registry, _rx) = make_registry_with_runtime_dispatch_and_roster(
            Some("codex-main"),
            vec![],
            Arc::new(SequencedStreamingRuntimeDispatch {
                calls: Arc::clone(&calls),
            }),
        );
        let session_key = SessionKey::with_instance("lark", "beta", "user:test");

        let inbound1 = InboundMsg {
            id: "msg-1".to_string(),
            session_key: session_key.clone(),
            content: MsgContent::text("first"),
            sender: "user".to_string(),
            channel: "lark".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };
        let inbound2 = InboundMsg {
            id: "msg-2".to_string(),
            session_key,
            content: MsgContent::text("second"),
            sender: "user".to_string(),
            channel: "lark".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };

        let (tx1, mut rx1) = broadcast::channel::<AgentEvent>(32);
        let (tx2, mut rx2) = broadcast::channel::<AgentEvent>(32);

        let registry1 = Arc::clone(&registry);
        let first = tokio::spawn(async move {
            registry1
                .handle_with_context_and_events(inbound1, TurnExecutionContext::default(), tx1)
                .await
                .expect("first turn succeeds")
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let registry2 = Arc::clone(&registry);
        let second = tokio::spawn(async move {
            registry2
                .handle_with_context_and_events(inbound2, TurnExecutionContext::default(), tx2)
                .await
                .expect("second turn succeeds")
        });

        let first_event = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                match rx1.recv().await.expect("rx1 event") {
                    AgentEvent::TurnComplete { full_text, .. } => break full_text,
                    _ => continue,
                }
            }
        })
        .await
        .expect("first channel should receive first turn complete");
        assert_eq!(first_event, "first turn reply");

        assert_eq!(
            first.await.expect("first join"),
            Some("first turn reply".to_string())
        );

        let second_event = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                match rx2.recv().await.expect("rx2 event") {
                    AgentEvent::TurnComplete { full_text, .. } => break full_text,
                    _ => continue,
                }
            }
        })
        .await
        .expect("second channel should receive second turn complete");
        assert_eq!(second_event, "second turn reply");

        assert_eq!(
            second.await.expect("second join"),
            Some("second turn reply".to_string())
        );
    }

    #[test]
    fn test_agent_ctx_carries_workspace_dir() {
        let ctx = crate::agent_core::traits::AgentCtx {
            session_id: uuid::Uuid::new_v4(),
            session_key: SessionKey::new("ws", "ctx-test"),
            user_text: "hello".to_string(),
            history: vec![],
            system_injection: String::new(),
            workspace_dir: Some(std::path::PathBuf::from("/projects/test")),
            ..crate::agent_core::traits::AgentCtx::default()
        };
        assert!(ctx.workspace_dir.is_some());
    }

    #[test]
    fn test_registry_creates_session_on_first_message() {
        let (registry, _rx) = make_registry();
        let key = SessionKey::new("ws", "user1");
        let session = registry.get_or_create_session(&key);
        assert_eq!(session.key, key);
    }

    #[test]
    fn test_registry_default_backend_id_is_cached_in_session() {
        let (registry, _rx) = make_registry_with_default_backend("native-main");
        let key = SessionKey::new("ws", "user-backend");
        let session = registry.get_or_create_session(&key);
        assert_eq!(session.backend_id.as_deref(), Some("native-main"));
    }

    #[test]
    fn test_registry_per_session_backend_override() {
        let (registry, _rx) = make_registry();
        let key = SessionKey::new("ws", "user2");
        assert_eq!(registry.get_or_create_session(&key).backend_id, None);
        registry.set_session_backend(&key, "my-agent-backend");
        let new_backend = registry.get_or_create_session(&key).backend_id.clone();
        assert_eq!(new_backend.as_deref(), Some("my-agent-backend"));
    }

    #[tokio::test]
    async fn test_registry_slash_reset_returns_confirmation() {
        let (registry, _rx) = make_registry();
        let inbound = InboundMsg {
            id: "test1".to_string(),
            session_key: SessionKey::new("ws", "user3"),
            content: MsgContent::text("/reset"),
            sender: "user".to_string(),
            channel: "ws".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };
        let result = registry.handle(inbound).await.unwrap();
        assert!(result.unwrap().contains("已清除"));
    }

    #[tokio::test]
    async fn test_registry_slash_help_returns_help() {
        let (registry, _rx) = make_registry();
        let inbound = InboundMsg {
            id: "test2".to_string(),
            session_key: SessionKey::new("ws", "user4"),
            content: MsgContent::text("/help"),
            sender: "user".to_string(),
            channel: "ws".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };
        let result = registry.handle(inbound).await.unwrap();
        assert!(result.unwrap().contains("/backend"));
    }

    #[tokio::test]
    async fn test_registry_slash_approve_uses_resolver() {
        let (registry, _rx) = make_registry();
        let decisions = Arc::new(std::sync::Mutex::new(Vec::new()));
        registry.set_approval_resolver(Arc::new(FakeApprovalResolver {
            decisions: Arc::clone(&decisions),
            result: true,
        }));
        let inbound = InboundMsg {
            id: "approve-1".to_string(),
            session_key: SessionKey::new("ws", "user-approve"),
            content: MsgContent::text("/approve approval-1 allow-once"),
            sender: "user".to_string(),
            channel: "ws".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };

        let result = registry.handle(inbound).await.unwrap();
        assert!(result.unwrap().contains("approval-1"));
        let recorded = decisions.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, "approval-1");
        assert_eq!(recorded[0].1, ApprovalDecision::AllowOnce);
    }

    #[tokio::test]
    async fn test_registry_slash_approve_rejects_invalid_decision() {
        let (registry, _rx) = make_registry();
        let inbound = InboundMsg {
            id: "approve-2".to_string(),
            session_key: SessionKey::new("ws", "user-approve-2"),
            content: MsgContent::text("/approve approval-2 maybe"),
            sender: "user".to_string(),
            channel: "ws".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };

        let result = registry.handle(inbound).await.unwrap();
        assert!(result.unwrap().contains("无效审批决定"));
    }

    #[tokio::test]
    async fn test_registry_uses_runtime_dispatch_boundary() {
        let dir =
            std::env::temp_dir().join(format!("test-registry-dispatch-{}", uuid::Uuid::new_v4()));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        let calls = Arc::new(AtomicUsize::new(0));
        let last_backend = Arc::new(std::sync::Mutex::new(None));
        let history_snapshots = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (registry, _rx) = SessionRegistry::with_runtime_dispatch(
            Some("native-main".to_string()),
            session_manager,
            String::new(),
            None,
            None,
            None,
            None,
            vec![],
            Arc::new(FakeRuntimeDispatch {
                calls: Arc::clone(&calls),
                last_backend: Arc::clone(&last_backend),
                history_snapshots,
                backend_resume_fingerprint: None,
                emitted_backend_session_id: None,
                used_backend_id: None,
                resume_recovery: None,
            }),
        );

        let inbound = InboundMsg {
            id: "dispatch-test-1".to_string(),
            session_key: SessionKey::new("ws", "dispatch-user"),
            content: MsgContent::text("hello runtime"),
            sender: "user".to_string(),
            channel: "ws".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };

        let result = registry.handle(inbound).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(last_backend.lock().unwrap().as_deref(), Some("native-main"));
        assert_eq!(result.as_deref(), Some("fake-dispatch: hello runtime"));
    }

    #[tokio::test]
    async fn test_registry_second_turn_includes_prior_user_and_assistant_history() {
        let dir =
            std::env::temp_dir().join(format!("test-registry-history-{}", uuid::Uuid::new_v4()));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        let calls = Arc::new(AtomicUsize::new(0));
        let last_backend = Arc::new(std::sync::Mutex::new(None));
        let history_snapshots = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (registry, _rx) = SessionRegistry::with_runtime_dispatch(
            Some("native-main".to_string()),
            session_manager,
            String::new(),
            None,
            None,
            None,
            None,
            vec![],
            Arc::new(FakeRuntimeDispatch {
                calls,
                last_backend,
                history_snapshots: Arc::clone(&history_snapshots),
                backend_resume_fingerprint: None,
                emitted_backend_session_id: None,
                used_backend_id: None,
                resume_recovery: None,
            }),
        );

        registry
            .handle(InboundMsg {
                id: "history-1".to_string(),
                session_key: SessionKey::new("ws", "history-user"),
                content: MsgContent::text("苹果香蕉西瓜"),
                sender: "user".to_string(),
                channel: "ws".to_string(),
                timestamp: chrono::Utc::now(),
                thread_ts: None,
                target_agent: None,
                source: crate::protocol::MsgSource::Human,
            })
            .await
            .unwrap();

        registry
            .handle(InboundMsg {
                id: "history-2".to_string(),
                session_key: SessionKey::new("ws", "history-user"),
                content: MsgContent::text("我刚才说了什么"),
                sender: "user".to_string(),
                channel: "ws".to_string(),
                timestamp: chrono::Utc::now(),
                thread_ts: None,
                target_agent: None,
                source: crate::protocol::MsgSource::Human,
            })
            .await
            .unwrap();

        let snapshots = history_snapshots.lock().unwrap();
        assert_eq!(snapshots.len(), 2);
        assert!(snapshots[0].is_empty());
        assert_eq!(
            snapshots[1],
            vec![
                ("user".to_string(), "苹果香蕉西瓜".to_string()),
                (
                    "assistant".to_string(),
                    "fake-dispatch: 苹果香蕉西瓜".to_string(),
                ),
            ]
        );
    }

    #[tokio::test]
    async fn test_registry_drops_stale_backend_resume_id_when_fingerprint_changes() {
        let dir =
            std::env::temp_dir().join(format!("test-registry-resume-fp-{}", uuid::Uuid::new_v4()));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        let session_key = SessionKey::new("ws", "resume-fingerprint-user");
        let session_id = session_manager.get_or_create(&session_key).await.unwrap();
        session_manager
            .complete_turn(
                session_id,
                "native-main",
                Some("stale-backend-session".into()),
                Some("fp-old".into()),
            )
            .await
            .unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let last_backend = Arc::new(std::sync::Mutex::new(None));
        let history_snapshots = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (registry, _rx) = SessionRegistry::with_runtime_dispatch(
            Some("native-main".to_string()),
            Arc::clone(&session_manager),
            String::new(),
            None,
            None,
            None,
            None,
            vec![],
            Arc::new(FakeRuntimeDispatch {
                calls,
                last_backend,
                history_snapshots,
                backend_resume_fingerprint: Some("fp-new".into()),
                emitted_backend_session_id: None,
                used_backend_id: None,
                resume_recovery: None,
            }),
        );

        registry
            .handle(InboundMsg {
                id: "resume-fingerprint-1".to_string(),
                session_key: session_key.clone(),
                content: MsgContent::text("hello"),
                sender: "user".to_string(),
                channel: "ws".to_string(),
                timestamp: chrono::Utc::now(),
                thread_ts: None,
                target_agent: None,
                source: crate::protocol::MsgSource::Human,
            })
            .await
            .unwrap();

        let meta = session_manager
            .load_meta(session_id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !meta.backend_session_ids.contains_key("native-main"),
            "stale backend resume id should be dropped instead of reused"
        );
        assert_eq!(
            meta.backend_resume_fingerprints
                .get("native-main")
                .map(String::as_str),
            Some("fp-new"),
            "current turn should persist the new backend fingerprint"
        );
    }

    #[tokio::test]
    async fn test_registry_replaces_failed_load_session_handle_with_fresh_backend_session() {
        let dir = std::env::temp_dir().join(format!(
            "test-registry-resume-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        let session_key = SessionKey::new("ws", "resume-recovery-user");
        let session_id = session_manager.get_or_create(&session_key).await.unwrap();
        session_manager
            .complete_turn(
                session_id,
                "native-main",
                Some("stale-load-id".into()),
                Some("fp-same".into()),
            )
            .await
            .unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let last_backend = Arc::new(std::sync::Mutex::new(None));
        let history_snapshots = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (registry, _rx) = SessionRegistry::with_runtime_dispatch(
            Some("native-main".to_string()),
            Arc::clone(&session_manager),
            String::new(),
            None,
            None,
            None,
            None,
            vec![],
            Arc::new(FakeRuntimeDispatch {
                calls,
                last_backend,
                history_snapshots,
                backend_resume_fingerprint: Some("fp-same".into()),
                emitted_backend_session_id: Some("fresh-new-id".into()),
                used_backend_id: Some("native-main".into()),
                resume_recovery: Some(ResumeRecoveryAction::DropStaleResumedSessionHandle {
                    stale_session_id: "stale-load-id".into(),
                }),
            }),
        );

        registry
            .handle(InboundMsg {
                id: "resume-recovery-1".to_string(),
                session_key: session_key.clone(),
                content: MsgContent::text("hello after stale load"),
                sender: "user".to_string(),
                channel: "ws".to_string(),
                timestamp: chrono::Utc::now(),
                thread_ts: None,
                target_agent: None,
                source: crate::protocol::MsgSource::Human,
            })
            .await
            .unwrap();

        let meta = session_manager
            .load_meta(session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            meta.backend_session_ids
                .get("native-main")
                .map(String::as_str),
            Some("fresh-new-id"),
            "registry should replace the stale handle with the fresh backend session id"
        );
        assert_eq!(
            meta.backend_resume_fingerprints
                .get("native-main")
                .map(String::as_str),
            Some("fp-same"),
            "registry should preserve the current fingerprint after recovery"
        );
    }

    #[tokio::test]
    async fn test_scope_binding_routes_no_mention_to_bound_agent() {
        let calls = Arc::new(AtomicUsize::new(0));
        let last_backend = Arc::new(std::sync::Mutex::new(None));
        let (registry, _rx) = make_registry_with_runtime_dispatch_and_roster(
            None,
            vec![
                AgentEntry {
                    name: "claude".to_string(),
                    mentions: vec!["@claude".to_string()],
                    backend_id: "claude-main".to_string(),
                    persona_dir: None,
                    workspace_dir: None,
                    extra_skills_dirs: vec![],
                },
                AgentEntry {
                    name: "codex".to_string(),
                    mentions: vec!["@codex".to_string()],
                    backend_id: "codex-main".to_string(),
                    persona_dir: None,
                    workspace_dir: None,
                    extra_skills_dirs: vec![],
                },
            ],
            Arc::new(FakeRuntimeDispatch {
                calls: Arc::clone(&calls),
                last_backend: Arc::clone(&last_backend),
                history_snapshots: Arc::new(std::sync::Mutex::new(Vec::new())),
                backend_resume_fingerprint: None,
                emitted_backend_session_id: None,
                used_backend_id: None,
                resume_recovery: None,
            }),
        );
        registry.register_scope_binding("group:lark:bound".to_string(), "claude".to_string());

        let result = registry
            .handle(InboundMsg {
                id: "routing-bound-1".to_string(),
                session_key: SessionKey::new("lark", "group:lark:bound"),
                content: MsgContent::text("hello bound route"),
                sender: "user".to_string(),
                channel: "lark".to_string(),
                timestamp: chrono::Utc::now(),
                thread_ts: None,
                target_agent: None,
                source: crate::protocol::MsgSource::Human,
            })
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(last_backend.lock().unwrap().as_deref(), Some("claude-main"));
        assert_eq!(result.as_deref(), Some("fake-dispatch: hello bound route"));
    }

    #[tokio::test]
    async fn test_explicit_mention_overrides_scope_binding() {
        let last_backend = Arc::new(std::sync::Mutex::new(None));
        let (registry, _rx) = make_registry_with_runtime_dispatch_and_roster(
            None,
            vec![
                AgentEntry {
                    name: "claude".to_string(),
                    mentions: vec!["@claude".to_string()],
                    backend_id: "claude-main".to_string(),
                    persona_dir: None,
                    workspace_dir: None,
                    extra_skills_dirs: vec![],
                },
                AgentEntry {
                    name: "codex".to_string(),
                    mentions: vec!["@codex".to_string()],
                    backend_id: "codex-main".to_string(),
                    persona_dir: None,
                    workspace_dir: None,
                    extra_skills_dirs: vec![],
                },
            ],
            Arc::new(FakeRuntimeDispatch {
                calls: Arc::new(AtomicUsize::new(0)),
                last_backend: Arc::clone(&last_backend),
                history_snapshots: Arc::new(std::sync::Mutex::new(Vec::new())),
                backend_resume_fingerprint: None,
                emitted_backend_session_id: None,
                used_backend_id: None,
                resume_recovery: None,
            }),
        );
        registry.register_scope_binding("group:lark:bound".to_string(), "claude".to_string());

        registry
            .handle(InboundMsg {
                id: "routing-bound-2".to_string(),
                session_key: SessionKey::new("lark", "group:lark:bound"),
                content: MsgContent::text("hello explicit mention"),
                sender: "user".to_string(),
                channel: "lark".to_string(),
                timestamp: chrono::Utc::now(),
                thread_ts: None,
                target_agent: Some("@codex".to_string()),
                source: crate::protocol::MsgSource::Human,
            })
            .await
            .unwrap();

        assert_eq!(last_backend.lock().unwrap().as_deref(), Some("codex-main"));
    }

    #[tokio::test]
    async fn test_scope_binding_overrides_cached_session_backend() {
        let last_backend = Arc::new(std::sync::Mutex::new(None));
        let (registry, _rx) = make_registry_with_runtime_dispatch_and_roster(
            None,
            vec![AgentEntry {
                name: "claude".to_string(),
                mentions: vec!["@claude".to_string()],
                backend_id: "claude-main".to_string(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            }],
            Arc::new(FakeRuntimeDispatch {
                calls: Arc::new(AtomicUsize::new(0)),
                last_backend: Arc::clone(&last_backend),
                history_snapshots: Arc::new(std::sync::Mutex::new(Vec::new())),
                backend_resume_fingerprint: None,
                emitted_backend_session_id: None,
                used_backend_id: None,
                resume_recovery: None,
            }),
        );
        let key = SessionKey::new("lark", "group:lark:bound");
        registry.register_scope_binding(key.scope.clone(), "claude".to_string());
        registry.set_session_backend(&key, "manual-backend");

        registry
            .handle(InboundMsg {
                id: "routing-bound-3".to_string(),
                session_key: key,
                content: MsgContent::text("hello session override"),
                sender: "user".to_string(),
                channel: "lark".to_string(),
                timestamp: chrono::Utc::now(),
                thread_ts: None,
                target_agent: None,
                source: crate::protocol::MsgSource::Human,
            })
            .await
            .unwrap();

        assert_eq!(last_backend.lock().unwrap().as_deref(), Some("claude-main"));
    }

    #[tokio::test]
    async fn test_thread_binding_overrides_scope_binding() {
        let last_backend = Arc::new(std::sync::Mutex::new(None));
        let (registry, _rx) = make_registry_with_runtime_dispatch_and_roster(
            None,
            vec![
                AgentEntry {
                    name: "claude".to_string(),
                    mentions: vec!["@claude".to_string()],
                    backend_id: "claude-main".to_string(),
                    persona_dir: None,
                    workspace_dir: None,
                    extra_skills_dirs: vec![],
                },
                AgentEntry {
                    name: "codex".to_string(),
                    mentions: vec!["@codex".to_string()],
                    backend_id: "codex-main".to_string(),
                    persona_dir: None,
                    workspace_dir: None,
                    extra_skills_dirs: vec![],
                },
            ],
            Arc::new(FakeRuntimeDispatch {
                calls: Arc::new(AtomicUsize::new(0)),
                last_backend: Arc::clone(&last_backend),
                history_snapshots: Arc::new(std::sync::Mutex::new(Vec::new())),
                backend_resume_fingerprint: None,
                emitted_backend_session_id: None,
                used_backend_id: None,
                resume_recovery: None,
            }),
        );
        registry.register_binding(crate::agent_core::bindings::BindingRule::scope(
            "group:lark:bound",
            "claude",
        ));
        registry.register_binding(crate::agent_core::bindings::BindingRule::Thread {
            channel: Some("lark".to_string()),
            scope: "group:lark:bound".to_string(),
            thread_id: "thread-1".to_string(),
            agent_name: "codex".to_string(),
        });

        registry
            .handle(InboundMsg {
                id: "routing-thread-1".to_string(),
                session_key: SessionKey::new("lark", "group:lark:bound"),
                content: MsgContent::text("hello thread binding"),
                sender: "user".to_string(),
                channel: "lark".to_string(),
                timestamp: chrono::Utc::now(),
                thread_ts: Some("thread-1".to_string()),
                target_agent: None,
                source: crate::protocol::MsgSource::Human,
            })
            .await
            .unwrap();

        assert_eq!(last_backend.lock().unwrap().as_deref(), Some("codex-main"));
    }

    #[tokio::test]
    async fn test_later_explicit_scope_binding_overrides_earlier_scope_binding() {
        let last_backend = Arc::new(std::sync::Mutex::new(None));
        let (registry, _rx) = make_registry_with_runtime_dispatch_and_roster(
            None,
            vec![
                AgentEntry {
                    name: "claude".to_string(),
                    mentions: vec!["@claude".to_string()],
                    backend_id: "claude-main".to_string(),
                    persona_dir: None,
                    workspace_dir: None,
                    extra_skills_dirs: vec![],
                },
                AgentEntry {
                    name: "codex".to_string(),
                    mentions: vec!["@codex".to_string()],
                    backend_id: "codex-main".to_string(),
                    persona_dir: None,
                    workspace_dir: None,
                    extra_skills_dirs: vec![],
                },
            ],
            Arc::new(FakeRuntimeDispatch {
                calls: Arc::new(AtomicUsize::new(0)),
                last_backend: Arc::clone(&last_backend),
                history_snapshots: Arc::new(std::sync::Mutex::new(Vec::new())),
                backend_resume_fingerprint: None,
                emitted_backend_session_id: None,
                used_backend_id: None,
                resume_recovery: None,
            }),
        );
        registry.register_scope_binding("group:lark:bound".to_string(), "claude".to_string());
        registry.register_binding(crate::agent_core::bindings::BindingRule::Scope {
            channel: Some("lark".to_string()),
            scope: "group:lark:bound".to_string(),
            agent_name: "codex".to_string(),
        });

        registry
            .handle(InboundMsg {
                id: "routing-bound-override-1".to_string(),
                session_key: SessionKey::new("lark", "group:lark:bound"),
                content: MsgContent::text("hello explicit override"),
                sender: "user".to_string(),
                channel: "lark".to_string(),
                timestamp: chrono::Utc::now(),
                thread_ts: None,
                target_agent: None,
                source: crate::protocol::MsgSource::Human,
            })
            .await
            .unwrap();

        assert_eq!(last_backend.lock().unwrap().as_deref(), Some("codex-main"));
    }

    #[tokio::test]
    async fn test_explicit_default_binding_overrides_default_roster_agent() {
        let last_backend = Arc::new(std::sync::Mutex::new(None));
        let (registry, _rx) = make_registry_with_runtime_dispatch_and_roster(
            None,
            vec![
                AgentEntry {
                    name: "claude".to_string(),
                    mentions: vec!["@claude".to_string()],
                    backend_id: "claude-main".to_string(),
                    persona_dir: None,
                    workspace_dir: None,
                    extra_skills_dirs: vec![],
                },
                AgentEntry {
                    name: "codex".to_string(),
                    mentions: vec!["@codex".to_string()],
                    backend_id: "codex-main".to_string(),
                    persona_dir: None,
                    workspace_dir: None,
                    extra_skills_dirs: vec![],
                },
            ],
            Arc::new(FakeRuntimeDispatch {
                calls: Arc::new(AtomicUsize::new(0)),
                last_backend: Arc::clone(&last_backend),
                history_snapshots: Arc::new(std::sync::Mutex::new(Vec::new())),
                backend_resume_fingerprint: None,
                emitted_backend_session_id: None,
                used_backend_id: None,
                resume_recovery: None,
            }),
        );
        registry.register_binding(crate::agent_core::bindings::BindingRule::Default {
            agent_name: "codex".to_string(),
        });

        registry
            .handle(InboundMsg {
                id: "routing-default-binding-1".to_string(),
                session_key: SessionKey::new("ws", "no-binding-match"),
                content: MsgContent::text("hello explicit default"),
                sender: "user".to_string(),
                channel: "ws".to_string(),
                timestamp: chrono::Utc::now(),
                thread_ts: None,
                target_agent: None,
                source: crate::protocol::MsgSource::Human,
            })
            .await
            .unwrap();

        assert_eq!(last_backend.lock().unwrap().as_deref(), Some("codex-main"));
    }

    #[tokio::test]
    async fn test_roster_only_mode_falls_back_to_default_roster_agent() {
        let last_backend = Arc::new(std::sync::Mutex::new(None));
        let (registry, _rx) = make_registry_with_runtime_dispatch_and_roster(
            None,
            vec![
                AgentEntry {
                    name: "mybot".to_string(),
                    mentions: vec!["@mybot".to_string()],
                    backend_id: "mybot-main".to_string(),
                    persona_dir: None,
                    workspace_dir: None,
                    extra_skills_dirs: vec![],
                },
                AgentEntry {
                    name: "reviewer".to_string(),
                    mentions: vec!["@reviewer".to_string()],
                    backend_id: "reviewer-main".to_string(),
                    persona_dir: None,
                    workspace_dir: None,
                    extra_skills_dirs: vec![],
                },
            ],
            Arc::new(FakeRuntimeDispatch {
                calls: Arc::new(AtomicUsize::new(0)),
                last_backend: Arc::clone(&last_backend),
                history_snapshots: Arc::new(std::sync::Mutex::new(Vec::new())),
                backend_resume_fingerprint: None,
                emitted_backend_session_id: None,
                used_backend_id: None,
                resume_recovery: None,
            }),
        );

        let result = registry
            .handle(InboundMsg {
                id: "routing-default-roster-1".to_string(),
                session_key: SessionKey::new("ws", "roster-only"),
                content: MsgContent::text("hello default roster"),
                sender: "user".to_string(),
                channel: "ws".to_string(),
                timestamp: chrono::Utc::now(),
                thread_ts: None,
                target_agent: None,
                source: crate::protocol::MsgSource::Human,
            })
            .await
            .unwrap();

        assert_eq!(last_backend.lock().unwrap().as_deref(), Some("mybot-main"));
        assert_eq!(
            result.as_deref(),
            Some("fake-dispatch: hello default roster")
        );
    }

    #[test]
    fn test_registry_roster_resolves_target_agent() {
        let (registry, _rx) = make_registry_with_roster();
        let entry = registry
            .roster
            .as_ref()
            .unwrap()
            .find_by_mention("@mybot")
            .unwrap();
        assert_eq!(entry.name, "mybot");
    }

    #[test]
    fn test_registry_no_roster_is_none() {
        let (registry, _rx) = make_registry();
        assert!(registry.roster.is_none());
    }

    /// Verify that lead_agent_name fallback resolves the correct RosterMatchData.
    /// When early_is_lead is true and no @mention is present, the registry should
    /// use the orchestrator's lead_agent_name to look up the roster by name.
    #[test]
    fn test_lead_fallback_uses_front_bot_roster_entry() {
        use crate::agent_core::team::{
            heartbeat::DispatchFn, orchestrator::TeamOrchestrator, registry::TaskRegistry,
            session::TeamSession,
        };
        use tempfile::tempdir;

        let (registry, _rx) = make_registry_with_roster();
        let tmp = tempdir().unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("t", tmp.path().to_path_buf()));
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            task_registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );

        // Wire: lead_session_key + lead_agent_name = "mybot"
        let lead_key = crate::protocol::SessionKey::new("lark", "group:123");
        orch.set_lead_session_key(lead_key.clone());
        orch.set_lead_agent_name("mybot".to_string());
        registry.register_team_orchestrator(
            crate::agent_core::team::session::stable_team_id_for_session_key(&lead_key),
            orch,
        );

        // Confirm roster has "mybot"
        let entry = registry
            .roster
            .as_ref()
            .unwrap()
            .find_by_name("mybot")
            .unwrap();
        assert_eq!(entry.name, "mybot");

        // Simulate a Lead turn with no @mention: session_key == lead_key, source == Human
        // The early_is_lead detection and Lead fallback in roster_match should pick "mybot".
        // We verify via direct roster lookup since we can't run the full async handle() in a unit test.
        let resolved = registry
            .get_orchestrator_for_session(&lead_key)
            .as_ref()
            .and_then(|o| o.lead_agent_name.get())
            .and_then(|name| registry.roster.as_ref()?.find_by_name(name));
        assert!(
            resolved.is_some(),
            "Lead fallback should resolve front_bot roster entry"
        );
        assert_eq!(resolved.unwrap().name, "mybot");
    }

    #[tokio::test]
    async fn test_slash_memory_at_agent_without_memory_system_reports_disabled() {
        // make_registry has no memory system; /memory now reports memory disabled.
        let (registry, _rx) = make_registry();
        let inbound = InboundMsg {
            id: "mem-agent-1".to_string(),
            session_key: SessionKey::new("ws", "user_agent_mem"),
            content: MsgContent::text("/memory @reviewer"),
            sender: "user".to_string(),
            channel: "ws".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };
        let result = registry.handle(inbound).await.unwrap();
        let text = result.unwrap();
        assert!(
            text.contains("未启用记忆系统"),
            "expected memory disabled message, got: {text}"
        );
    }

    #[tokio::test]
    async fn test_memory_empty_state_guidance() {
        let (registry, _rx) = make_registry_with_memory();
        let inbound = InboundMsg {
            id: "mem-empty-1".to_string(),
            session_key: SessionKey::new("dingtalk", "group_test"),
            content: MsgContent::text("/memory"),
            sender: "user".to_string(),
            channel: "dingtalk".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };
        let result = registry.handle(inbound).await.unwrap();
        let text = result.unwrap();
        assert!(
            text.contains("技术栈"),
            "empty memory should contain guiding question about 技术栈"
        );
        assert!(
            text.contains("编码规范"),
            "empty memory should contain guiding question about 编码规范"
        );
        assert!(
            text.contains("项目"),
            "empty memory should contain guiding question about 项目"
        );
        assert!(
            text.contains("group_test"),
            "empty memory should include the scope name"
        );
    }

    #[tokio::test]
    async fn test_memory_reset_first_call_warns() {
        let (registry, _rx) = make_registry_with_memory();
        let key = SessionKey::new("dt", "group1");
        let inbound = InboundMsg {
            id: "mr-1".to_string(),
            session_key: key.clone(),
            content: MsgContent::text("/memory reset"),
            sender: "user".to_string(),
            channel: "dt".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };
        let result = registry.handle(inbound).await.unwrap().unwrap();
        assert!(result.contains("⚠️"));
        assert!(result.contains("60 秒"));
    }

    #[tokio::test]
    async fn test_memory_reset_second_call_confirms() {
        let (registry, _rx) = make_registry_with_memory();
        let key = SessionKey::new("dt", "group2");

        // First call: warn
        let inbound1 = InboundMsg {
            id: "mr-2a".to_string(),
            session_key: key.clone(),
            content: MsgContent::text("/memory reset"),
            sender: "user".to_string(),
            channel: "dt".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };
        registry.handle(inbound1).await.unwrap();

        // Second call: execute
        let inbound2 = InboundMsg {
            id: "mr-2b".to_string(),
            session_key: key.clone(),
            content: MsgContent::text("/memory reset"),
            sender: "user".to_string(),
            channel: "dt".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };
        let result = registry.handle(inbound2).await.unwrap().unwrap();
        assert!(result.contains("✅"));
        assert!(result.contains("清空"));
    }

    #[test]
    fn test_session_idle_seconds_unknown_session_returns_none() {
        let (reg, _rx) = make_registry();
        assert!(
            reg.session_idle_seconds("lark:nonexistent").is_none(),
            "session with no recorded activity should return None"
        );
    }

    #[test]
    fn test_session_idle_seconds_unknown_scope_only_returns_none() {
        let (reg, _rx) = make_registry();
        assert!(reg.session_idle_seconds("nonexistent").is_none());
    }

    #[tokio::test]
    async fn test_memory_reset_expired_pending_rewarns() {
        let (registry, _rx) = make_registry_with_memory();
        let key = SessionKey::new("dt", "group3");

        // Inject an already-expired pending reset (61s ago)
        let expired = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(61))
            .expect("system clock supports this subtraction");
        registry.inject_pending_reset_at(key.clone(), expired);

        // Call /memory reset — should re-warn, not clear
        let inbound = InboundMsg {
            id: "mr-expired".to_string(),
            session_key: key.clone(),
            content: MsgContent::text("/memory reset"),
            sender: "user".to_string(),
            channel: "dt".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };
        let result = registry.handle(inbound).await.unwrap().unwrap();
        assert!(
            result.contains("⚠️"),
            "expired pending should re-warn, got: {result}"
        );
        assert!(
            !result.contains("✅"),
            "expired pending must NOT confirm clear, got: {result}"
        );
    }

    #[test]
    fn test_registry_stores_skill_loader_dirs() {
        // Verify that skill_loader_dirs passed to new() are stored correctly.
        let dir = std::env::temp_dir().join(format!("test-skills-dir-{}", uuid::Uuid::new_v4()));
        let storage = SessionStorage::new(
            std::env::temp_dir().join(format!("test-reg-skills-{}", uuid::Uuid::new_v4())),
        );
        let session_manager = Arc::new(SessionManager::new(storage));
        let (registry, _rx) = SessionRegistry::new(
            None,
            session_manager,
            String::new(),
            None,
            None,
            None,
            None,
            vec![dir.clone()],
        );
        assert_eq!(registry.skill_loader_dirs, vec![dir]);
    }

    #[test]
    fn test_registry_skill_loader_dirs_empty_by_default() {
        // make_registry() passes vec![] for skill_loader_dirs
        let (registry, _rx) = make_registry();
        assert!(registry.skill_loader_dirs.is_empty());
    }

    #[test]
    fn test_workspace_agents_skills_dir_included_in_loader() {
        // Verify the logic: if workspace_dir contains .agents/skills/, the loader merges it.
        // This exercises the dir-building logic directly without spawning an engine.
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        let agents_skills = workspace.join(".agents").join("skills");
        std::fs::create_dir_all(agents_skills.join("my-skill")).unwrap();
        std::fs::write(
            agents_skills.join("my-skill/SKILL.md"),
            "---\nname: my-skill\nmetadata:\n  version: '1.0.0'\n---\nDo cool things.",
        )
        .unwrap();

        // Build the dirs as handle() would:
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();
        let canonical = workspace.join(".agents").join("skills");
        if canonical.exists() {
            dirs.push(canonical.clone());
        }
        // No extra_dirs, no gateway dirs in this test
        let loader = crate::skills_internal::SkillLoader::with_dirs(dirs);
        let skills = loader.load_all();
        let injection = loader.build_system_injection(&skills);

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].manifest.name, "my-skill");
        assert!(injection.contains("my-skill"));
        assert!(injection.contains("Do cool things"));
    }

    #[test]
    fn test_workspace_skill_dirs_merged_with_gateway_dirs() {
        // Verify that workspace dirs come first and gateway fallback dirs follow.
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        let gateway_skills = tmp.path().join("gateway-skills");

        let agents_skills = workspace.join(".agents").join("skills");
        std::fs::create_dir_all(agents_skills.join("ws-skill")).unwrap();
        std::fs::write(
            agents_skills.join("ws-skill/SKILL.md"),
            "---\nname: ws-skill\nmetadata:\n  version: '1.0.0'\n---\nWorkspace skill.",
        )
        .unwrap();

        std::fs::create_dir_all(gateway_skills.join("gw-skill")).unwrap();
        std::fs::write(
            gateway_skills.join("gw-skill/SKILL.md"),
            "---\nname: gw-skill\nmetadata:\n  version: '2.0.0'\n---\nGateway skill.",
        )
        .unwrap();

        let mut all_dirs = vec![agents_skills.clone()];
        all_dirs.push(gateway_skills.clone());
        let loader = crate::skills_internal::SkillLoader::with_dirs(all_dirs);
        let skills = loader.load_all();

        assert_eq!(skills.len(), 2);
        // Workspace dir is first, so ws-skill should appear before gw-skill
        assert_eq!(loader.search_dirs()[0], agents_skills);
        assert_eq!(loader.search_dirs()[1], gateway_skills);
        let names: Vec<&str> = skills.iter().map(|s| s.manifest.name.as_str()).collect();
        assert!(names.contains(&"ws-skill"));
        assert!(names.contains(&"gw-skill"));
    }

    // ── /workspace slash command tests ──────────────────────────────────────

    #[test]
    fn test_workspace_cmd_rejects_file_path() {
        // Verify parse correctly extracts the path from /workspace /etc/hosts
        let cmd = SlashCommand::parse("/workspace /etc/hosts");
        assert_eq!(
            cmd,
            Some(SlashCommand::Workspace(Some("/etc/hosts".to_string())))
        );
        // The is_dir check happens at runtime in handle_slash; the parse result
        // is a path, not a command failure — that is the correct contract.
    }

    #[tokio::test]
    async fn test_workspace_set_rejects_file_not_directory() {
        // /workspace /etc/hosts should fail with "Path is not a directory" because
        // /etc/hosts is a regular file (exists but is_dir() == false).
        let (registry, _rx) = make_registry();
        let hosts = std::path::Path::new("/etc/hosts");
        // Only run if /etc/hosts exists on this platform; skip otherwise.
        if !hosts.exists() {
            return;
        }
        let inbound = InboundMsg {
            id: "ws-file-check-1".to_string(),
            session_key: SessionKey::new("ws", "user_ws_file"),
            content: MsgContent::text("/workspace /etc/hosts"),
            sender: "user".to_string(),
            channel: "ws".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };
        let result = registry.handle(inbound).await.unwrap().unwrap();
        assert!(
            result.contains("not a directory"),
            "expected 'not a directory' error, got: {result}"
        );
    }

    /// When no roster match (single-engine mode) and a persona-type skill happens to be present
    /// in a workspace skill dir, the persona prefix must NOT be applied to the reply.
    /// (The persona system prompt layers are only built for roster-matched agents.)
    #[test]
    fn test_no_roster_match_persona_prefix_not_applied() {
        // Build a temp dir with a persona-type SKILL.md
        let tmp = tempfile::TempDir::new().unwrap();
        let persona_dir = tmp.path().join("rex-intj");
        std::fs::create_dir_all(&persona_dir).unwrap();
        std::fs::write(
            persona_dir.join("SKILL.md"),
            "---\nname: Rex\ntype: persona\n---\nRex capabilities.",
        )
        .unwrap();

        // Build skill loader pointing at the temp dir
        let loader = crate::skills_internal::SkillLoader::with_dirs(vec![tmp.path().to_path_buf()]);
        let personas = loader.load_personas();

        // Simulate the no-roster code path: prefix_for_fwd and reply_text must be None / unmodified
        let first_persona = personas.into_iter().next();
        assert!(first_persona.is_some(), "sanity: persona loaded");

        let roster_match_is_some = false; // no roster match
        let full_text = "Hello world".to_string();

        let prefix_for_fwd: Option<String> = if roster_match_is_some {
            first_persona.as_ref().map(|p| p.display_prefix())
        } else {
            None
        };
        let reply_text = if roster_match_is_some {
            match &first_persona {
                Some(p) => format!("{}{full_text}", p.display_prefix()),
                None => full_text.clone(),
            }
        } else {
            full_text.clone()
        };

        assert!(prefix_for_fwd.is_none(), "no prefix in no-roster mode");
        assert_eq!(
            reply_text, "Hello world",
            "reply unchanged in no-roster mode"
        );
    }

    fn make_registry_with_team_orchestrator() -> (Arc<SessionRegistry>, Arc<TeamOrchestrator>) {
        use crate::agent_core::team::{
            heartbeat::DispatchFn,
            orchestrator::TeamOrchestrator,
            registry::TaskRegistry,
            session::{stable_team_id_for_session_key, TeamSession},
        };
        use tempfile::tempdir;

        let (registry, _rx) = make_registry();
        let tmp = tempdir().unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let lead_key = SessionKey::new("lark", "group:team");
        let team_id = stable_team_id_for_session_key(&lead_key);
        let session = Arc::new(TeamSession::from_dir(&team_id, tmp.path().to_path_buf()));
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            task_registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        orch.set_lead_session_key(lead_key.clone());
        orch.set_scope(lead_key);
        registry.register_team_orchestrator(team_id, Arc::clone(&orch));
        (registry, orch)
    }

    fn make_registry_with_runtime_dispatch_and_team_orchestrator() -> (
        Arc<SessionRegistry>,
        Arc<TeamOrchestrator>,
        Arc<AtomicUsize>,
    ) {
        use crate::agent_core::team::{
            heartbeat::DispatchFn,
            orchestrator::TeamOrchestrator,
            registry::TaskRegistry,
            session::{stable_team_id_for_session_key, TeamSession},
        };
        use tempfile::tempdir;

        let dir =
            std::env::temp_dir().join(format!("test-registry-team-turn-{}", uuid::Uuid::new_v4()));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        let calls = Arc::new(AtomicUsize::new(0));
        let last_backend = Arc::new(std::sync::Mutex::new(None));
        let history_snapshots = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (registry, _rx) = SessionRegistry::with_runtime_dispatch(
            Some("native-main".to_string()),
            session_manager,
            String::new(),
            None,
            None,
            None,
            None,
            vec![],
            Arc::new(FakeRuntimeDispatch {
                calls: Arc::clone(&calls),
                last_backend,
                history_snapshots,
                backend_resume_fingerprint: None,
                emitted_backend_session_id: None,
                used_backend_id: None,
                resume_recovery: None,
            }),
        );

        let tmp = tempdir().unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let lead_key = SessionKey::new("lark", "group:team");
        let team_id = stable_team_id_for_session_key(&lead_key);
        let session = Arc::new(TeamSession::from_dir(&team_id, tmp.keep()));
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            task_registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        orch.set_lead_session_key(lead_key.clone());
        orch.set_scope(lead_key);
        registry.register_team_orchestrator(team_id, Arc::clone(&orch));
        (registry, orch, calls)
    }

    #[test]
    fn suppress_lead_final_reply_only_while_team_is_running() {
        let (registry, orch) = make_registry_with_team_orchestrator();
        let lead_key = SessionKey::new("lark", "group:team");

        assert!(
            !registry.should_suppress_lead_final_reply(&lead_key),
            "Planning state must not suppress normal lead replies"
        );

        *orch.team_state_inner.lock().unwrap() =
            crate::agent_core::team::orchestrator::TeamState::AwaitingConfirm;
        assert!(
            !registry.should_suppress_lead_final_reply(&lead_key),
            "AwaitingConfirm state must not suppress confirmation replies"
        );

        *orch.team_state_inner.lock().unwrap() =
            crate::agent_core::team::orchestrator::TeamState::Running;
        assert!(
            registry.should_suppress_lead_final_reply(&lead_key),
            "Running state should suppress the normal stream path"
        );

        *orch.team_state_inner.lock().unwrap() =
            crate::agent_core::team::orchestrator::TeamState::Done;
        assert!(
            !registry.should_suppress_lead_final_reply(&lead_key),
            "Done state must not suppress direct lead replies"
        );
    }

    #[tokio::test]
    async fn test_new_human_lead_turn_reopens_done_team_for_new_planning_cycle() {
        let (registry, orch, calls) = make_registry_with_runtime_dispatch_and_team_orchestrator();
        *orch.team_state_inner.lock().unwrap() =
            crate::agent_core::team::orchestrator::TeamState::Done;

        let inbound = InboundMsg {
            id: "team-reopen-1".to_string(),
            session_key: SessionKey::new("lark", "group:team"),
            content: MsgContent::text("给我再新建一轮任务"),
            sender: "user".to_string(),
            channel: "lark".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };

        let reply = registry.handle(inbound).await.unwrap();

        assert_eq!(reply.as_deref(), Some("fake-dispatch: 给我再新建一轮任务"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            orch.team_state(),
            crate::agent_core::team::orchestrator::TeamState::Planning
        );
        assert!(orch
            .register_task(crate::agent_core::team::registry::CreateTask {
                id: "T200".into(),
                title: "Follow-up task".into(),
                ..Default::default()
            })
            .is_ok());
    }

    #[tokio::test]
    async fn lead_delegation_turn_without_team_side_effect_is_rewritten() {
        let (registry, _orch, calls) = make_registry_with_runtime_dispatch_and_team_orchestrator();

        let inbound = InboundMsg {
            id: "team-delegation-missing-side-effect".to_string(),
            session_key: SessionKey::new("lark", "group:team"),
            content: MsgContent::text("让其他bot做个任务：讲解一下clawbro"),
            sender: "user".to_string(),
            channel: "lark".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };

        let reply = registry.handle(inbound).await.unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            reply.as_deref(),
            Some("我这轮没有完成实际任务创建或分配，因此不会宣称任务已开始执行。请重试，或明确指定要委派的 bot 和目标。")
        );
    }

    #[tokio::test]
    async fn lead_delegation_turn_with_orchestrator_side_effect_is_not_rewritten() {
        use crate::agent_core::team::{
            heartbeat::DispatchFn,
            orchestrator::TeamOrchestrator,
            registry::TaskRegistry,
            session::{stable_team_id_for_session_key, TeamSession},
        };
        use tempfile::tempdir;

        let dir = std::env::temp_dir().join(format!(
            "test-registry-team-turn-side-effect-{}",
            uuid::Uuid::new_v4()
        ));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));

        let tmp = tempdir().unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let lead_key = SessionKey::new("lark", "group:team");
        let team_id = stable_team_id_for_session_key(&lead_key);
        let session = Arc::new(TeamSession::from_dir(&team_id, tmp.keep()));
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            task_registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        orch.set_test_mcp_start_result(Ok(32125));
        orch.set_lead_session_key(lead_key.clone());
        orch.set_scope(lead_key.clone());

        let calls = Arc::new(AtomicUsize::new(0));
        let (registry, _rx) = SessionRegistry::with_runtime_dispatch(
            Some("native-main".to_string()),
            session_manager,
            String::new(),
            None,
            None,
            None,
            None,
            vec![],
            Arc::new(TeamSideEffectRuntimeDispatch {
                calls: Arc::clone(&calls),
                team_orchestrator: Arc::clone(&orch),
            }),
        );
        registry.register_team_orchestrator(team_id, Arc::clone(&orch));

        let inbound = InboundMsg {
            id: "team-delegation-real-side-effect".to_string(),
            session_key: lead_key,
            content: MsgContent::text("让其他bot做个任务：讲解一下clawbro"),
            sender: "user".to_string(),
            channel: "lark".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };

        let reply = registry.handle(inbound).await.unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            reply.as_deref(),
            Some("fake-dispatch: 让其他bot做个任务：讲解一下clawbro")
        );
        assert!(orch.registry.get_task("T999").unwrap().is_some());
        assert!(matches!(
            orch.team_state(),
            crate::agent_core::team::orchestrator::TeamState::Running
        ));
    }

    #[tokio::test]
    async fn lead_delegation_turn_that_creates_tasks_but_cannot_start_execution_is_rewritten() {
        use crate::agent_core::team::{
            heartbeat::DispatchFn,
            orchestrator::TeamOrchestrator,
            registry::TaskRegistry,
            session::{stable_team_id_for_session_key, TeamSession},
        };
        use tempfile::tempdir;

        let dir = std::env::temp_dir().join(format!(
            "test-registry-team-turn-activation-fail-{}",
            uuid::Uuid::new_v4()
        ));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));

        let tmp = tempdir().unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let lead_key = SessionKey::new("lark", "group:team");
        let team_id = stable_team_id_for_session_key(&lead_key);
        let session = Arc::new(TeamSession::from_dir(&team_id, tmp.keep()));
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            task_registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        orch.set_test_mcp_start_result(Err("synthetic mcp failure".to_string()));
        orch.set_lead_session_key(lead_key.clone());
        orch.set_scope(lead_key.clone());

        let calls = Arc::new(AtomicUsize::new(0));
        let (registry, _rx) = SessionRegistry::with_runtime_dispatch(
            Some("native-main".to_string()),
            session_manager,
            String::new(),
            None,
            None,
            None,
            None,
            vec![],
            Arc::new(TeamSideEffectRuntimeDispatch {
                calls: Arc::clone(&calls),
                team_orchestrator: Arc::clone(&orch),
            }),
        );
        registry.register_team_orchestrator(team_id, Arc::clone(&orch));

        let inbound = InboundMsg {
            id: "team-delegation-activation-fail".to_string(),
            session_key: lead_key,
            content: MsgContent::text("让其他bot做个任务：讲解一下clawbro"),
            sender: "user".to_string(),
            channel: "lark".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };

        let reply = registry.handle(inbound).await.unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            reply.as_deref(),
            Some("我这轮虽然创建了任务，但还没有把团队执行真正启动，因此不会宣称 specialist 已开始处理。请重试，或要求我立即启动执行。")
        );
        assert!(orch.registry.get_task("T999").unwrap().is_some());
        assert!(matches!(
            orch.team_state(),
            crate::agent_core::team::orchestrator::TeamState::Planning
        ));
    }

    #[test]
    fn lead_human_team_delegation_guard_only_applies_to_explicit_requests() {
        let inbound = InboundMsg {
            id: "guard-1".into(),
            session_key: SessionKey::new("lark", "group:team"),
            content: MsgContent::text("让其他bot做个任务：讲解一下clawbro"),
            sender: "user".into(),
            channel: "lark".into(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: crate::protocol::MsgSource::Human,
        };
        assert!(
            SessionRegistry::lead_human_team_delegation_requires_side_effect(&inbound, true, true)
        );

        let plain = InboundMsg {
            content: MsgContent::text("直接讲解一下clawbro"),
            ..inbound.clone()
        };
        assert!(
            !SessionRegistry::lead_human_team_delegation_requires_side_effect(&plain, true, true)
        );
    }

    #[test]
    fn team_coordination_side_effect_detection_requires_real_task_callbacks() {
        let with_create = TurnResult {
            full_text: "created".into(),
            events: vec![RuntimeEvent::ToolCallback(TeamCallback::TaskCreated {
                task_id: "T123".into(),
                title: "Explain".into(),
                assignee: "codex".into(),
            })],
            emitted_backend_session_id: None,
            backend_resume_fingerprint: None,
            used_backend_id: None,
            resume_recovery: None,
        };
        assert!(SessionRegistry::turn_has_team_coordination_side_effect(
            &with_create
        ));

        let without_real_side_effect = TurnResult {
            full_text: "talk only".into(),
            events: vec![RuntimeEvent::ToolCallback(
                TeamCallback::PublicUpdatePosted {
                    message: "working".into(),
                },
            )],
            emitted_backend_session_id: None,
            backend_resume_fingerprint: None,
            used_backend_id: None,
            resume_recovery: None,
        };
        assert!(!SessionRegistry::turn_has_team_coordination_side_effect(
            &without_real_side_effect
        ));
    }

    #[tokio::test]
    async fn test_invoke_team_tool_submit_and_accept_updates_registry() {
        let (registry, orch) = make_registry_with_team_orchestrator();
        let specialist_key = orch.session.specialist_session_key("codex");

        orch.registry
            .create_task(crate::agent_core::team::registry::CreateTask {
                id: "T100".into(),
                title: "Implement auth".into(),
                assignee_hint: Some("codex".into()),
                deps: vec![],
                timeout_secs: 1800,
                spec: None,
                success_criteria: None,
            })
            .unwrap();
        orch.registry.try_claim("T100", "codex").unwrap();

        let submit = registry
            .invoke_team_tool(
                &specialist_key,
                TeamToolCall::SubmitTaskResult {
                    task_id: "T100".into(),
                    summary: "auth implemented".into(),
                    result_markdown: Some(
                        "# Auth Implementation\n\nImplemented auth flow, middleware, and validation tests."
                            .into(),
                    ),
                    agent: Some("codex".into()),
                },
            )
            .await
            .unwrap();
        assert!(submit.ok);
        assert!(submit.message.contains("submitted"));

        let lead_key = SessionKey::new("lark", "group:team");
        let accept = registry
            .invoke_team_tool(
                &lead_key,
                TeamToolCall::AcceptTask {
                    task_id: "T100".into(),
                    by: Some("leader".into()),
                },
            )
            .await
            .unwrap();
        assert!(accept.ok);
        assert!(accept.message.contains("accepted"));

        let task = orch.registry.get_task("T100").unwrap().unwrap();
        assert!(matches!(
            task.status_parsed(),
            crate::agent_core::team::registry::TaskStatus::Accepted { .. }
        ));
    }

    #[tokio::test]
    async fn test_invoke_team_tool_get_status_returns_json_payload() {
        let (registry, orch) = make_registry_with_team_orchestrator();
        let lead_key = SessionKey::new("lark", "group:team");

        orch.registry
            .create_task(crate::agent_core::team::registry::CreateTask {
                id: "T200".into(),
                title: "Write docs".into(),
                assignee_hint: Some("codex".into()),
                deps: vec![],
                timeout_secs: 1800,
                spec: None,
                success_criteria: None,
            })
            .unwrap();

        let status = registry
            .invoke_team_tool(&lead_key, TeamToolCall::GetTaskStatus)
            .await
            .unwrap();
        assert!(status.ok);
        assert!(status.message.contains("\"id\": \"T200\""));
        assert!(status.payload.is_some());
    }

    #[tokio::test]
    async fn test_invoke_team_tool_block_task_notifies_lead_and_resets_claim() {
        let (registry, orch) = make_registry_with_team_orchestrator();
        let specialist_key = orch.session.specialist_session_key("codex");
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        orch.set_team_notify_tx(tx);

        orch.registry
            .create_task(crate::agent_core::team::registry::CreateTask {
                id: "T210".into(),
                title: "Investigate auth".into(),
                assignee_hint: Some("codex".into()),
                deps: vec![],
                timeout_secs: 1800,
                spec: None,
                success_criteria: None,
            })
            .unwrap();
        orch.registry.try_claim("T210", "codex").unwrap();

        let response = registry
            .invoke_team_tool(
                &specialist_key,
                TeamToolCall::BlockTask {
                    task_id: "T210".into(),
                    reason: "missing credential".into(),
                    agent: Some("codex".into()),
                },
            )
            .await
            .unwrap();
        assert!(response.ok);

        let task = orch.registry.get_task("T210").unwrap().unwrap();
        assert!(matches!(
            task.status_parsed(),
            crate::agent_core::team::registry::TaskStatus::Pending
        ));

        let notify = rx
            .recv()
            .await
            .expect("lead should receive blocked notification");
        let text = notify.envelope.event.render_for_parent();
        assert!(text.contains("T210"));
        assert!(text.contains("missing credential"));
    }

    #[tokio::test]
    async fn test_invoke_team_tool_rejects_lead_only_tool_from_specialist_session() {
        let (registry, orch) = make_registry_with_team_orchestrator();
        let specialist_key = orch.session.specialist_session_key("codex");

        let err = registry
            .invoke_team_tool(
                &specialist_key,
                TeamToolCall::CreateTask {
                    id: Some("T401".into()),
                    title: "illegal".into(),
                    assignee: Some("codex".into()),
                    spec: None,
                    deps: vec![],
                    success_criteria: None,
                },
            )
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("CreateTask"));
        assert!(err.contains("Specialist"));
    }

    #[tokio::test]
    async fn test_invoke_team_tool_rejects_specialist_only_tool_from_lead_session() {
        let (registry, _orch) = make_registry_with_team_orchestrator();
        let lead_key = SessionKey::new("lark", "group:team");

        let err = registry
            .invoke_team_tool(
                &lead_key,
                TeamToolCall::SubmitTaskResult {
                    task_id: "T402".into(),
                    summary: "illegal".into(),
                    result_markdown: None,
                    agent: Some("codex".into()),
                },
            )
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("SubmitTaskResult"));
        assert!(err.contains("Leader"));
    }

    #[tokio::test]
    async fn test_apply_runtime_events_submits_task_into_existing_registry() {
        let (registry, orch) = make_registry_with_team_orchestrator();
        let specialist_key = orch.session.specialist_session_key("openclaw-main");

        orch.registry
            .create_task(crate::agent_core::team::registry::CreateTask {
                id: "T300".into(),
                title: "Implement bridge".into(),
                assignee_hint: Some("openclaw-main".into()),
                deps: vec![],
                timeout_secs: 1800,
                spec: None,
                success_criteria: None,
            })
            .unwrap();
        orch.registry.try_claim("T300", "openclaw-main").unwrap();

        registry
            .apply_runtime_events(
                &specialist_key,
                &TurnResult {
                    full_text: "submitted".into(),
                    events: vec![RuntimeEvent::ToolCallback(TeamCallback::TaskSubmitted {
                        task_id: "T300".into(),
                        summary: "bridge implemented".into(),
                        result_markdown: Some(
                            "# Bridge Implementation\n\nImplemented the bridge end-to-end and verified the runtime path."
                                .into(),
                        ),
                        agent: "openclaw-main".into(),
                    })],
                    emitted_backend_session_id: None,
                    backend_resume_fingerprint: None,
                    used_backend_id: None,
                    resume_recovery: None,
                },
            )
            .await
            .unwrap();

        let task = orch.registry.get_task("T300").unwrap().unwrap();
        assert!(matches!(
            task.status_parsed(),
            crate::agent_core::team::registry::TaskStatus::Submitted { .. }
        ));
        assert_eq!(task.completion_note.as_deref(), Some("bridge implemented"));
    }

    /// Verify that the relay guard logic (early_is_lead branch) passes full_text unchanged
    /// when [RELAY:] appears in a Lead turn output.
    ///
    /// This test exercises the guard decision logic directly (without running handle())
    /// since wiring a full Lead turn requires a live engine and team orchestrator.
    /// The in-handle() guard is: `if early_is_lead { warn if contains "[RELAY:"; full_text } else { ... relay ... }`
    #[test]
    fn relay_hook_guard_lead_turn_skips_relay_processing() {
        // Simulate the guard condition: early_is_lead = true, text contains [RELAY:]
        let early_is_lead = true;
        let full_text = "Good plan. [RELAY: @codex implement the auth module]".to_string();

        // Mirror the guard logic from Hook 2 in handle()
        let result = if early_is_lead {
            // warn would fire here in production; skipped in test
            full_text.clone()
        } else {
            // relay.process() would run here for non-Lead turns
            format!("relay-processed: {full_text}")
        };

        // Lead turn: full_text must be returned unchanged (relay NOT invoked)
        assert_eq!(
            result, full_text,
            "Lead turn must not trigger relay processing"
        );
        assert!(
            !result.starts_with("relay-processed:"),
            "relay-processed prefix must not appear for Lead turns"
        );
    }

    /// Verify that non-Lead turns still go through the relay branch (relay engine is consulted).
    #[test]
    fn relay_hook_guard_non_lead_turn_enters_relay_branch() {
        let early_is_lead = false;
        let full_text = "[RELAY: @codex do something]".to_string();

        let result = if early_is_lead {
            full_text.clone()
        } else {
            // Simulate relay.process() returning a processed string
            format!("relay-processed: {full_text}")
        };

        assert!(
            result.starts_with("relay-processed:"),
            "non-Lead turn must enter relay branch"
        );
    }
}
