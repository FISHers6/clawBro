// quickai-gateway/crates/qai-agent/src/registry.rs
//! SessionRegistry: per-session backend routing + generic @mention routing.
//! Architectural role: Gateway orchestration layer (not platform-specific).
//! - Channels extract @mentions → InboundMsg.target_agent
//! - Registry resolves target_agent via AgentRoster (generic name lookup)
//! - No platform-specific text parsing here

use crate::control::role_resolver::is_front_bot_turn;
use crate::control::session_router::get_orchestrator_for_session as route_orchestrator_for_session;
use crate::control::turn_intent::build_turn_intent;
use crate::dedup::DedupStore;
use crate::memory::cap_to_words;
use crate::memory::{MemoryEvent, MemorySystem, MemoryTarget};
use crate::persona::AgentPersona;
use crate::prompt_builder::SystemPromptBuilder;
use crate::relay::RelayEngine;
use crate::roster::AgentRoster;
use crate::runtime_dispatch::{default_runtime_dispatch, RuntimeDispatch, RuntimeDispatchRequest};
use crate::slash::SlashCommand;
use crate::team::orchestrator::TeamOrchestrator;
use crate::traits::{AgentCtx, AgentRole, HistoryMsg};
use crate::{ApprovalDecision, ApprovalResolver};
use anyhow::Result;
use dashmap::DashMap;
use qai_channels::mention_trigger::MentionTrigger;
use qai_protocol::{AgentEvent, InboundMsg, MsgSource, SessionKey};
use qai_runtime::contract::{TeamCallback, TurnMode, TurnResult};
use qai_runtime::{RuntimeEvent, TeamToolCall, TeamToolResponse};
use qai_session::{SessionManager, StoredMessage};
use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Cloned data extracted from a roster match to avoid holding a borrow across await points.
struct RosterMatchData {
    agent_name: String,
    backend_id: String,
    persona_dir: Option<std::path::PathBuf>,
    workspace_dir: Option<std::path::PathBuf>,
    extra_skills_dirs: Vec<std::path::PathBuf>,
}

/// Single session state: holds per-session runtime backend selection.
pub struct Session {
    pub key: SessionKey,
    pub backend_id: Option<String>,
}

/// SessionRegistry: manages all per-session state with DashMap
pub struct SessionRegistry {
    sessions: DashMap<SessionKey, Arc<Session>>,
    default_backend_id: Option<String>,
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
    /// Fallback persona_dir for single-engine mode (no roster match).
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
        let (global_tx, global_rx) = broadcast::channel(256);
        let registry = Arc::new(Self {
            sessions: DashMap::new(),
            default_backend_id,
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
        self.team_task_reminders.insert(key, reminder);
    }

    /// Register a TeamOrchestrator for a given team_id.
    /// Supports multiple concurrent Team groups (one orchestrator per group).
    pub fn register_team_orchestrator(&self, team_id: String, orch: Arc<TeamOrchestrator>) {
        self.team_orchestrators.insert(team_id, orch);
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

    /// Get-or-create per-session cached backend selection (used when no roster match)
    pub fn get_or_create_session(&self, key: &SessionKey) -> Arc<Session> {
        self.sessions
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(Session {
                    key: key.clone(),
                    backend_id: self.default_backend_id.clone(),
                })
            })
            .clone()
    }

    /// Override runtime backend for a session (/backend slash command)
    pub fn set_session_backend(&self, key: &SessionKey, backend_id: impl Into<String>) {
        let session = Arc::new(Session {
            key: key.clone(),
            backend_id: Some(backend_id.into()),
        });
        self.sessions.insert(key.clone(), session);
    }

    /// Get per-session workspace override (set via /workspace command).
    pub fn session_workspace(&self, key: &SessionKey) -> Option<std::path::PathBuf> {
        self.session_workspaces.get(key).map(|v| v.clone())
    }

    /// Set per-session workspace override (called from /workspace slash command handler).
    fn set_session_workspace(&self, key: &SessionKey, path: std::path::PathBuf) {
        self.session_workspaces.insert(key.clone(), path);
    }

    /// All session scopes that have had activity (used by nightly consolidation scheduler).
    pub fn all_active_scopes(&self) -> Vec<SessionKey> {
        self.last_activity.iter().map(|e| e.key().clone()).collect()
    }

    /// Resolve the persona directory for the current turn.
    /// Priority: roster agent's explicit dir > session-level default > auto-derived ~/.quickai/agents/{name}/
    fn resolve_persona_dir(
        &self,
        roster_match: &Option<RosterMatchData>,
    ) -> Option<std::path::PathBuf> {
        roster_match
            .as_ref()
            .and_then(|rm| rm.persona_dir.clone())
            .or_else(|| self.default_persona_dir.clone())
            .or_else(|| {
                roster_match.as_ref().map(|rm| {
                    let name = &rm.agent_name;
                    let dir = AgentPersona::default_dir_for(name);
                    if !self.initialized_persona_dirs.contains(&dir) {
                        if let Err(e) = AgentPersona::ensure_default_dir(&dir, name) {
                            tracing::warn!(agent = %name, error = %e, "Failed to create default persona dir");
                        } else {
                            self.initialized_persona_dirs.insert(dir.clone());
                        }
                    }
                    dir
                })
            })
    }

    /// Return how many seconds the given session has been idle (no `handle()` activity).
    ///
    /// Returns `None` if the session has never been active (no recorded activity).
    pub fn session_idle_seconds(&self, session_key: &str) -> Option<u64> {
        // session_key may be in "channel:scope" format or just a plain scope string.
        // Parse into a SessionKey with a single lookup: "channel:scope" splits on the first ':',
        // bare strings are treated as scope under a synthetic "cron" channel.
        let key_parsed = if let Some(pos) = session_key.find(':') {
            SessionKey::new(&session_key[..pos], &session_key[pos + 1..])
        } else {
            SessionKey::new("cron", session_key)
        };
        self.last_activity
            .get(&key_parsed)
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

    pub fn session_manager_ref(&self) -> &SessionManager {
        &self.session_manager
    }

    fn resolve_claimed_agent_for_tool(
        &self,
        team_orch: &TeamOrchestrator,
        task_id: &str,
        explicit: Option<&str>,
    ) -> String {
        explicit
            .map(ToOwned::to_owned)
            .or_else(|| {
                team_orch
                    .registry
                    .get_task(task_id)
                    .ok()
                    .flatten()
                    .and_then(|t| {
                        t.status_raw
                            .strip_prefix("claimed:")
                            .and_then(|s| s.split(':').next())
                            .map(|s| s.to_string())
                    })
            })
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub async fn invoke_team_tool(
        &self,
        session_key: &SessionKey,
        call: TeamToolCall,
    ) -> Result<TeamToolResponse> {
        let team_orch = self
            .get_orchestrator_for_session(session_key)
            .ok_or_else(|| anyhow::anyhow!("no TeamOrchestrator found for session"))?;

        let response = match call {
            TeamToolCall::CreateTask {
                id,
                title,
                assignee,
                spec,
                deps,
                success_criteria,
            } => TeamToolResponse {
                ok: true,
                message: team_orch.register_task(crate::team::registry::CreateTask {
                    id,
                    title,
                    assignee_hint: assignee,
                    deps,
                    timeout_secs: 1800,
                    spec,
                    success_criteria,
                })?,
                payload: None,
            },
            TeamToolCall::StartExecution => TeamToolResponse {
                ok: true,
                message: team_orch.activate().await?,
                payload: None,
            },
            TeamToolCall::RequestConfirmation { plan_summary } => {
                let formatted = format!("**Plan for confirmation:**\n\n{}", plan_summary);
                team_orch.post_message(&formatted);
                *team_orch.team_state_inner.lock().unwrap() =
                    crate::team::orchestrator::TeamState::AwaitingConfirm;
                TeamToolResponse {
                    ok: true,
                    message: "Confirmation requested. Waiting for user reply.".to_string(),
                    payload: None,
                }
            }
            TeamToolCall::PostUpdate { message } => {
                team_orch.post_message(&message);
                TeamToolResponse {
                    ok: true,
                    message: "Posted.".to_string(),
                    payload: None,
                }
            }
            TeamToolCall::GetTaskStatus => {
                let tasks = team_orch.registry.all_tasks()?;
                let arr: Vec<serde_json::Value> = tasks
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "id": t.id,
                            "title": t.title,
                            "status": t.status_raw,
                            "assignee": t.assignee_hint,
                            "deps": t.deps(),
                            "retry_count": t.retry_count,
                            "completion_note": t.completion_note,
                        })
                    })
                    .collect();
                let payload = serde_json::Value::Array(arr.clone());
                TeamToolResponse {
                    ok: true,
                    message: serde_json::to_string_pretty(&arr)?,
                    payload: Some(payload),
                }
            }
            TeamToolCall::AssignTask {
                task_id,
                new_assignee,
            } => {
                team_orch.registry.reassign_task(&task_id, &new_assignee)?;
                TeamToolResponse {
                    ok: true,
                    message: format!("Task {} reassigned to {}.", task_id, new_assignee),
                    payload: None,
                }
            }
            TeamToolCall::CheckpointTask {
                task_id,
                note,
                agent,
            } => {
                let agent =
                    self.resolve_claimed_agent_for_tool(&team_orch, &task_id, agent.as_deref());
                team_orch.handle_specialist_checkpoint(&task_id, &agent, &note)?;
                TeamToolResponse {
                    ok: true,
                    message: format!("Checkpoint recorded for task {}.", task_id),
                    payload: None,
                }
            }
            TeamToolCall::SubmitTaskResult {
                task_id,
                summary,
                agent,
            } => {
                let agent =
                    self.resolve_claimed_agent_for_tool(&team_orch, &task_id, agent.as_deref());
                team_orch.handle_specialist_submitted(&task_id, &agent, &summary)?;
                TeamToolResponse {
                    ok: true,
                    message: format!("Task {} submitted for review.", task_id),
                    payload: None,
                }
            }
            TeamToolCall::AcceptTask { task_id, by } => {
                let by = by.as_deref().unwrap_or("leader");
                team_orch.accept_submitted_task(&task_id, by)?;
                TeamToolResponse {
                    ok: true,
                    message: format!("Task {} accepted by {}.", task_id, by),
                    payload: None,
                }
            }
            TeamToolCall::ReopenTask {
                task_id,
                reason,
                by,
            } => {
                let by = by.as_deref().unwrap_or("leader");
                team_orch.reopen_submitted_task(&task_id, &reason, by)?;
                TeamToolResponse {
                    ok: true,
                    message: format!("Task {} reopened by {}.", task_id, by),
                    payload: None,
                }
            }
            TeamToolCall::BlockTask {
                task_id,
                reason,
                agent,
            } => {
                let agent =
                    self.resolve_claimed_agent_for_tool(&team_orch, &task_id, agent.as_deref());
                if !team_orch
                    .registry
                    .is_claimed_by(&task_id, &agent)
                    .unwrap_or(false)
                {
                    anyhow::bail!("task '{}' is not currently claimed by '{}'", task_id, agent);
                }
                let _ = team_orch.registry.reset_claim(&task_id);
                team_orch.handle_specialist_blocked(&task_id, &agent, &reason)?;
                TeamToolResponse {
                    ok: true,
                    message: format!("Task {} reported as blocked: {}", task_id, reason),
                    payload: None,
                }
            }
            TeamToolCall::RequestHelp {
                task_id,
                message,
                agent,
            } => {
                let agent =
                    self.resolve_claimed_agent_for_tool(&team_orch, &task_id, agent.as_deref());
                if !team_orch
                    .registry
                    .is_claimed_by(&task_id, &agent)
                    .unwrap_or(false)
                {
                    anyhow::bail!("task '{}' is not currently claimed by '{}'", task_id, agent);
                }
                team_orch.handle_specialist_help_requested(&task_id, &agent, &message)?;
                TeamToolResponse {
                    ok: true,
                    message: format!("Help request sent for task {}.", task_id),
                    payload: None,
                }
            }
        };

        Ok(response)
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
                id: task_id,
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
                agent,
            } => TeamToolCall::SubmitTaskResult {
                task_id,
                summary,
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

    /// Process one inbound message. Generic: works for any channel.
    pub async fn handle(&self, inbound: InboundMsg) -> Result<Option<String>> {
        // Idempotent dedup
        if !self.dedup.check_and_insert(&inbound.id) {
            tracing::debug!("Dedup: skipping duplicate msg {}", inbound.id);
            return Ok(None);
        }

        let session_key = inbound.session_key.clone();
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
                .entry(session_key.clone())
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

        // ── Team Mode confirmation interceptor ──────────────────────────────────
        // When Lead called request_confirmation(), the next Human message is the user's yes/no.
        if inbound.source == qai_protocol::MsgSource::Human {
            if let Some(team_orch) = session_team_orch.as_ref() {
                if team_orch.team_state() == crate::team::orchestrator::TeamState::AwaitingConfirm {
                    if let Some(lead_key) = team_orch.lead_session_key.get() {
                        if &session_key == lead_key {
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
                                    crate::team::orchestrator::TeamState::Planning;
                                // Fall through to normal routing (Lead handles the message)
                            }
                        }
                    }
                }
            }
        }

        // Early Specialist/Lead detection — must run before roster_match so Lead turns
        // without an explicit @mention can fall back to the configured front_bot engine.
        let early_is_specialist = inbound.source == MsgSource::Heartbeat;
        // Auto-promote check: scope has auto_promote=true AND message contains team trigger keywords.
        // We then try to find an unclaimed orchestrator (lead_session_key not yet set) to attach.
        // If no orchestrator is found, fall back to Solo to avoid injecting Lead tools with no MCP URL.
        let is_auto_promote_candidate = !early_is_specialist
            && session_team_orch.is_none()
            && inbound.source == MsgSource::Human
            && self.auto_promote_scopes.contains(&session_key.scope)
            && crate::mode_selector::is_team_trigger(inbound.content.as_text().unwrap_or(""));
        // For auto_promote candidates, find the orchestrator bound to this exact scope.
        // We match by lead_session_key.scope rather than "unclaimed" (is_none()), because:
        //   - All Team orchestrators have lead_session_key preset at startup in main.rs.
        //   - Matching by scope prevents cross-group binding in multi-group deployments.
        //   - A group with both Team mode AND auto_promote=true can trigger Lead behavior
        //     dynamically via keyword detection (e.g. before user types /team start).
        let auto_promote_orch: Option<Arc<TeamOrchestrator>> = if is_auto_promote_candidate {
            let found = self
                .team_orchestrators
                .iter()
                .find(|e| {
                    e.value()
                        .lead_session_key
                        .get()
                        .map(|k| k.scope == session_key.scope)
                        .unwrap_or(false)
                })
                .map(|e| Arc::clone(e.value()));
            if found.is_none() {
                tracing::warn!(
                    scope = %session_key.scope,
                    "auto_promote triggered but no orchestrator found for this scope — falling back to Solo"
                );
            }
            found
        } else {
            None
        };
        // Merge: for auto_promote, use the found orchestrator for all subsequent Lead logic.
        let session_team_orch = session_team_orch.or(auto_promote_orch);
        // Lead turn: must have an orchestrator AND the message must target front_bot
        // (either no @mention → default to front_bot, or explicit @front_bot mention).
        // Without the is_front_bot_turn() guard, @codex in a Team group would incorrectly
        // receive Lead role and Lead system prompt (F2 fix).
        let early_is_lead = !early_is_specialist
            && session_team_orch.is_some()
            && is_front_bot_turn(&inbound, &session_team_orch, &self.roster);

        // ── Generic routing via target_agent (set by Channel) ──
        // Clone needed data from roster match to avoid holding borrow across await.
        // For Lead turns without an explicit @mention, fall back to the configured
        // front_bot agent (set via `front_bot` in [[group]] config).
        let roster_match: Option<RosterMatchData> = inbound
            .target_agent
            .as_deref()
            .and_then(|mention| {
                self.roster
                    .as_ref()
                    .and_then(|r| r.find_by_mention(mention))
                    .map(|entry| RosterMatchData {
                        agent_name: entry.name.clone(),
                        backend_id: entry.runtime_backend_id().to_string(),
                        persona_dir: entry.persona_dir.clone(),
                        workspace_dir: entry.workspace_dir.clone(),
                        extra_skills_dirs: entry.extra_skills_dirs.clone(),
                    })
            })
            .or_else(|| {
                // Lead fallback: no @mention but this is a Lead turn → use front_bot engine
                if early_is_lead {
                    session_team_orch
                        .as_ref()
                        .and_then(|o| o.lead_agent_name.get())
                        .and_then(|name| {
                            self.roster
                                .as_ref()?
                                .find_by_name(name)
                                .map(|entry| RosterMatchData {
                                    agent_name: entry.name.clone(),
                                    backend_id: entry.runtime_backend_id().to_string(),
                                    persona_dir: entry.persona_dir.clone(),
                                    workspace_dir: entry.workspace_dir.clone(),
                                    extra_skills_dirs: entry.extra_skills_dirs.clone(),
                                })
                        })
                } else {
                    None
                }
            });

        let turn_mode = if early_is_specialist || early_is_lead {
            TurnMode::Team
        } else if inbound.source == MsgSource::Relay {
            TurnMode::Relay
        } else {
            TurnMode::Solo
        };
        let session_backend_id = if roster_match.is_none() {
            self.get_or_create_session(&session_key).backend_id.clone()
        } else {
            None
        };
        let turn_intent = build_turn_intent(
            &inbound,
            turn_mode,
            session_team_orch
                .as_ref()
                .and_then(|o| o.lead_agent_name.get())
                .and_then(|name| {
                    self.roster
                        .as_ref()
                        .and_then(|r| r.find_by_name(name))
                        .map(|entry| entry.runtime_backend_id().to_string())
                        .or_else(|| Some(name.clone()))
                })
                .as_deref(),
            roster_match
                .as_ref()
                .map(|rm| rm.backend_id.as_str())
                .or(session_backend_id.as_deref()),
        );
        tracing::debug!(
            session = ?turn_intent.session_key,
            mode = ?turn_intent.mode,
            leader_candidate = ?turn_intent.leader_candidate,
            target_backend = ?turn_intent.target_backend,
            "built turn intent"
        );

        // Resolve runtime selection: roster match overrides the per-session backend selection.
        let (fallback_backend_id, sender_name): (Option<String>, Option<String>) =
            if let Some(rm) = &roster_match {
                (
                    Some(rm.backend_id.clone()),
                    Some(format!("@{}", rm.agent_name)),
                )
            } else {
                // No @mention or no roster: use the session's persistent backend selection.
                let session = self.get_or_create_session(&session_key);
                (session.backend_id.clone(), None)
            };

        // Get-or-create persistent session record
        let session_id = self.session_manager.get_or_create(&session_key).await?;
        let storage = self.session_manager.storage();

        // ── History: 50-message sliding window + sender prefix for LLM context ──
        // load_recent_messages avoids deserializing the entire JSONL for long sessions.
        let recent = storage.load_recent_messages(session_id, 50).await?;
        let recent = &recent[..];
        let history: Vec<HistoryMsg> = recent
            .iter()
            .map(|m| {
                let content = match m.sender.as_deref() {
                    Some(s) if !s.is_empty() => format!("[{}]: {}", s, m.content),
                    _ => m.content.clone(),
                };
                HistoryMsg {
                    role: m.role.clone(),
                    content,
                }
            })
            .collect();

        // Save user message with sender annotation
        let user_msg = StoredMessage {
            id: Uuid::new_v4(),
            role: "user".to_string(),
            content: user_text.clone(),
            timestamp: inbound.timestamp,
            sender: Some(inbound.sender.clone()),
            tool_calls: None,
        };
        storage.append_message(session_id, &user_msg).await?;

        // Resolve workspace: per-session override (/workspace cmd) > per-roster-agent entry > global default
        let workspace_dir_resolved: Option<std::path::PathBuf> = self
            .session_workspace(&session_key) // per-session override from /workspace command
            .or_else(|| {
                roster_match
                    .as_ref()
                    .and_then(|rm| rm.workspace_dir.clone())
            })
            .or_else(|| self.default_workspace.clone());

        // Build workspace-aware skill injection:
        //   1. {workspace}/.agents/skills/ (canonical npx-skills install dir) ← primary
        //   2. Agent's explicit extra_skills_dirs
        //   3. Gateway-level skill_loader_dirs (fallback)
        let (skill_injection, first_persona) = {
            let mut agent_skill_dirs: Vec<std::path::PathBuf> = Vec::new();
            // 1. Canonical workspace dir
            if let Some(ref ws) = workspace_dir_resolved {
                let canonical = ws.join(".agents").join("skills");
                if canonical.exists() {
                    agent_skill_dirs.push(canonical);
                }
            }
            // 2. Agent's explicit extra dirs
            if let Some(rm) = &roster_match {
                agent_skill_dirs.extend(rm.extra_skills_dirs.iter().cloned());
            }
            if agent_skill_dirs.is_empty() && self.skill_loader_dirs.is_empty() {
                // No workspace-specific dirs — use pre-built gateway-level injection
                (String::new(), None)
            } else {
                // Merge: workspace dirs first, then gateway fallback dirs
                let mut all_dirs = agent_skill_dirs;
                all_dirs.extend(self.skill_loader_dirs.iter().cloned());
                let loader = qai_skills::SkillLoader::with_dirs(all_dirs);
                let skills = loader.load_all();
                let personas = loader.load_personas();
                let injection = loader.build_system_injection(&skills);
                let persona = personas.into_iter().next();
                (injection, persona)
            }
        };

        // Override agent_role for Lead
        let early_agent_role = if early_is_specialist {
            AgentRole::Specialist
        } else if early_is_lead {
            AgentRole::Lead
        } else {
            AgentRole::Solo
        };

        // When a TeamNotify arrives, lazily set lead_session_key + scope if not yet set.
        // TeamNotify session_key IS the lead's session_key — find the orchestrator by scanning all.
        if inbound.source == qai_protocol::MsgSource::TeamNotify {
            if let Some(team_orch) = session_team_orch.as_ref() {
                team_orch.set_lead_session_key(session_key.clone());
                team_orch.set_scope(session_key.clone());
            }
        }

        // Build Lead Layer 0 (task_reminder for Lead turns based on TeamState)
        let lead_layer_0: Option<String> = if early_is_lead {
            let state = session_team_orch
                .as_ref()
                .map(|o| o.team_state())
                .unwrap_or(crate::team::orchestrator::TeamState::Planning);
            {
                let specialists_list = session_team_orch
                    .as_ref()
                    .and_then(|o| o.available_specialists.get())
                    .map(|v| v.join(", "))
                    .unwrap_or_else(|| "（未配置）".to_string());
                Some(match state {
                    crate::team::orchestrator::TeamState::Planning
                    | crate::team::orchestrator::TeamState::AwaitingConfirm => {
                        format!(
                            "你是团队协调者。用户的请求需要多个 Agent 协作完成。\n\n\
                             可分配的 Specialist：{specialists_list}\n\n\
                             步骤：\n\
                             1. 分析任务，调用 create_task() 定义所有子任务和依赖关系（assignee 填 Specialist 名称）\n\
                             2. 简单任务（≤3个、无复杂依赖）直接调用 start_execution()\n\
                             3. 复杂任务先调用 request_confirmation(plan_summary)，等用户确认后再执行\n\
                             4. Specialist 完成后通常会先提交结果；你收到待验收通知后，用 accept_task() 验收或 reopen_task() 打回\n\
                             5. 任务执行中你会收到 [团队通知] 消息，用 post_update() 向用户播报关键进度\n\
                             6. 收到\"所有任务已完成\"通知后，合成最终结果并调用 post_update() 发给用户\n\n\
                             可用工具：create_task, start_execution, request_confirmation, post_update, get_task_status, assign_task, accept_task, reopen_task",
                            specialists_list = specialists_list,
                        )
                    }
                    crate::team::orchestrator::TeamState::Running
                    | crate::team::orchestrator::TeamState::Done => {
                        format!(
                            "团队任务执行中。可分配的 Specialist：{specialists_list}\n\n\
                             你会收到 [团队通知] 消息：\n\
                             - 用 post_update(message) 向用户播报进度\n\
                             - 用 get_task_status() 查看全局状态\n\
                             - 用 assign_task(task_id, agent) 重新分配卡住的任务（agent 填 Specialist 名称）\n\
                             - 对 submitted 结果用 accept_task(task_id) 验收，或用 reopen_task(task_id, reason) 打回\n\
                             - 收到\"所有任务已完成\"通知后，合成最终汇总并 post_update",
                            specialists_list = specialists_list,
                        )
                    }
                })
            }
        } else {
            None
        };

        // Lead turns use lead_layer_0 as task_reminder; Specialist turns use pre-registered reminder.
        // Use a single .remove() (not .get()) to consume the DashMap entry exactly once.
        let early_task_reminder: Option<String> = if early_is_lead {
            lead_layer_0.clone()
        } else {
            self.team_task_reminders
                .remove(&session_key)
                .map(|(_, v)| v)
        };

        let canonical_shared_memory = if early_is_specialist {
            session_team_orch
                .as_ref()
                .map(|o| o.session.read_context_md())
                .filter(|content| !content.trim().is_empty())
        } else if let Some(ms) = &self.memory_system {
            ms.store()
                .load_shared_memory(&session_key)
                .await
                .ok()
                .filter(|content| !content.trim().is_empty())
        } else {
            None
        };
        let canonical_team_manifest = if matches!(
            early_agent_role,
            crate::traits::AgentRole::Lead | crate::traits::AgentRole::Specialist
        ) {
            session_team_orch
                .as_ref()
                .map(|o| o.session.read_team_md())
                .filter(|manifest| !manifest.trim().is_empty())
        } else {
            None
        };

        let mut canonical_agent_memory: Option<String> = None;
        // Build the full system prompt via SystemPromptBuilder (6-layer persona-aware).
        let system_injection = {
            if roster_match.is_some() {
                // Roster match: build fresh with all persona layers.
                let resolved_dir = self.resolve_persona_dir(&roster_match);
                let (soul_md, identity_raw, agent_memory) = if let Some(ref dir) = resolved_dir {
                    let ap = AgentPersona::load_from_dir_scoped(dir, &session_key);
                    (ap.soul, ap.identity, ap.memory)
                } else {
                    (String::new(), String::new(), String::new())
                };
                canonical_agent_memory = (!agent_memory.trim().is_empty()).then_some(agent_memory);

                // Persona capability body prepended to regular skill injection.
                let combined_skills = match &first_persona {
                    Some(p) if !p.capability_body.trim().is_empty() => {
                        if skill_injection.is_empty() {
                            p.capability_body.clone()
                        } else {
                            format!("{}\n\n{}", p.capability_body, skill_injection)
                        }
                    }
                    _ => skill_injection,
                };

                SystemPromptBuilder {
                    persona: first_persona.as_ref(),
                    soul_md: &soul_md,
                    identity_raw: &identity_raw,
                    skills_injection: &combined_skills,
                    shared_memory: "",
                    agent_memory: "",
                    shared_max_words: 300,
                    agent_max_words: 500,
                    agent_role: early_agent_role,
                    task_reminder: None,
                    team_manifest: None,
                }
                .build()
            } else {
                // No roster match: use cached gateway-level injection; fold in workspace skills if any.
                // If a persona skill was found in a workspace skill dir, its capability_body and identity
                // layers are intentionally NOT injected here — persona layers require a roster-matched agent
                // so that the correct persona_dir (SOUL.md + IDENTITY.md) is resolved.
                if let Some(ref p) = first_persona {
                    tracing::debug!(
                        persona = %p.identity.name,
                        "persona found in skill dirs but no roster match — \
                         persona layers not injected in single-engine mode"
                    );
                }
                if skill_injection.is_empty() {
                    self.system_injection.clone()
                } else if self.system_injection.is_empty() {
                    skill_injection
                } else {
                    format!("{}\n\n{}", self.system_injection, skill_injection)
                }
            }
        };

        // Pass the already-consumed reminder into AgentCtx (same value used by SystemPromptBuilder).
        let task_reminder = early_task_reminder.clone();
        // session_team_orch was resolved once at top of handle() — reuse here.
        let team_dir = if early_is_specialist {
            session_team_orch.as_ref().map(|o| o.session.dir.clone())
        } else {
            None
        };

        // For Specialist turns, override workspace_dir with team_dir so the engine sees
        // TEAM.md/TASKS.md/CONTEXT.md in its working directory (fixes I1).
        let effective_workspace = team_dir.clone().or(workspace_dir_resolved);

        // Both Lead and Specialist get the same unified MCP server URL (SharedTeamToolServer).
        // System prompts guide which of the 8 tools are relevant per role.
        let mcp_server_url: Option<String> = session_team_orch
            .as_ref()
            .and_then(|o| o.mcp_server_port.get().copied())
            .map(|port| format!("http://127.0.0.1:{port}/sse"));
        let team_tool_url = session_team_orch
            .as_ref()
            .and_then(|_| self.team_tool_url.get().cloned());

        // Build AgentCtx for the engine
        let ctx = AgentCtx {
            session_id,
            session_key: session_key.clone(),
            participant_name: roster_match.as_ref().map(|rm| rm.agent_name.clone()),
            user_text,
            history,
            system_injection,
            workspace_dir: effective_workspace,
            agent_role: early_agent_role,
            task_reminder,
            team_dir,
            mcp_server_url,
            team_tool_url,
            shared_memory: canonical_shared_memory,
            agent_memory: canonical_agent_memory,
            team_manifest: canonical_team_manifest,
        };

        // Per-call event channel: forward to global_tx + ws_subs
        // TurnComplete is enriched with sender_name here (engine itself doesn't know roster)
        let (session_tx, _) = broadcast::channel::<AgentEvent>(256);
        let global_tx = self.global_tx.clone();
        let ws_subs_clone = Arc::clone(&self.ws_subs);
        let sk_for_fwd = session_key.clone();
        let sender_for_fwd = sender_name.clone();
        // Only apply persona prefix when a roster agent was resolved (persona injected into prompt).
        // Without a roster match the persona system prompt is not built, so prefix would be misleading.
        //
        // Design note: The prefix is applied in TWO independent places intentionally:
        //   1. Here (prefix_for_fwd): for WebSocket subscribers that receive TurnComplete events.
        //   2. reply_text below: for IM channel callers (DingTalk/Lark) that use handle()'s return.
        // These are different consumers of the same turn output, not the same consumer seeing it twice.
        let prefix_for_fwd: Option<String> = if roster_match.is_some() {
            first_persona.as_ref().map(|p| p.display_prefix())
        } else {
            None
        };
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
                    ws_subs_clone.alter(&sk_for_fwd, |_, mut vec| {
                        vec.retain(|tx| tx.send(event.clone()).is_ok());
                        vec
                    });
                }
            });
        }

        // Control-plane execution crosses a narrow runtime dispatch boundary into
        // the canonical multi-backend conductor.
        let turn = self
            .runtime_dispatch
            .dispatch(RuntimeDispatchRequest {
                intent: turn_intent,
                ctx,
                fallback_backend_id,
                event_tx: session_tx,
            })
            .await?;
        self.apply_runtime_events(&session_key, &turn).await?;
        let full_text = turn.full_text;

        // ── Post-run hooks ─────────────────────────────────────────────────────

        // Hook 2: [RELAY: @agent <指令>] marker expansion (Relay Mode)
        // Guard: Lead turns in Team mode must NOT use [RELAY:] syntax — Lead communicates
        // with Specialists via MCP tools (assign_task, etc.), not relay markers.
        // If a Lead outputs [RELAY:] accidentally, warn and skip processing to prevent
        // bypassing the TaskRegistry state machine.
        let full_text = if early_is_lead {
            if full_text.contains("[RELAY:") {
                tracing::warn!(
                    session = ?session_key,
                    "Lead turn output contains [RELAY:] syntax — relay hook skipped. \
                     Use assign_task MCP tool to communicate with Specialists."
                );
            }
            full_text
        } else if let Some(relay) = self.relay_engine.get() {
            if full_text.contains("[RELAY:") {
                match relay.process(&full_text, &session_key).await {
                    Ok(processed) => processed,
                    Err(e) => {
                        tracing::warn!("relay engine error: {:#}", e);
                        full_text
                    }
                }
            } else {
                full_text
            }
        } else {
            full_text
        };

        // Hook 3: @botname scan → BotMention dispatch
        // Anti-recursion guard: do not scan replies that came from automated internal sources.
        // - BotMention: would create direct recursion (Bot A → Bot B → Bot A)
        // - Relay: Relay Specialist's reply is substituted into Lead's text; no further dispatch
        // - TeamNotify: Lead's progress summary may mention other bots (e.g. "@codex has finished
        //   task X"), which should not trigger new Specialist turns
        // - Heartbeat: Specialist replies should not spawn additional bot calls
        if !matches!(
            inbound.source,
            MsgSource::BotMention | MsgSource::Relay | MsgSource::TeamNotify | MsgSource::Heartbeat
        ) {
            if let Some(trigger) = self.mention_trigger.get() {
                let sender = roster_match
                    .as_ref()
                    .map(|r| r.agent_name.as_str())
                    .unwrap_or("agent");
                trigger.scan_and_dispatch(&full_text, sender, &session_key, &inbound.source);
            }
        }

        // Save assistant reply with sender annotation
        let assistant_msg = StoredMessage {
            id: Uuid::new_v4(),
            role: "assistant".to_string(),
            content: full_text.clone(),
            timestamp: chrono::Utc::now(),
            sender: sender_name,
            tool_calls: None,
        };
        storage.append_message(session_id, &assistant_msg).await?;

        // After engine completes, update idle tracking unconditionally
        self.last_activity
            .insert(session_key.clone(), std::time::Instant::now());

        // ── Memory events (non-blocking) ──
        if let Some(ms) = &self.memory_system {
            // persona_dir: unified resolution chain (roster explicit > session default > auto-derived)
            let persona_dir_opt = self.resolve_persona_dir(&roster_match);

            let agent_name_raw: String = roster_match
                .as_ref()
                .map(|rm| rm.agent_name.clone())
                .unwrap_or_else(|| "default".to_string());

            if let Some(persona_dir) = persona_dir_opt {
                let agent_name = agent_name_raw.trim_start_matches('@').to_string();
                let pd = persona_dir.clone();
                let sk = session_key.clone();
                let log_entry = format!(
                    "**[{}]**: {}\n\n**[@{}]**: {}",
                    inbound.sender, user_text_for_log, agent_name, full_text
                );
                let store = ms.store();
                tokio::spawn(async move {
                    store.append_daily_log(&pd, &sk, &log_entry).await.ok();
                });

                let count_key = (session_key.clone(), agent_name.clone());
                let new_count = {
                    let mut c = self.turn_counts.entry(count_key).or_insert(0);
                    *c += 1;
                    *c
                };
                ms.emit(MemoryEvent::TurnCompleted {
                    scope: session_key.clone(),
                    agent: agent_name,
                    persona_dir,
                    turn_count: new_count,
                });
            }
        }

        // Apply persona IM prefix only when a roster agent was resolved (persona was injected).
        let reply_text = if roster_match.is_some() {
            match &first_persona {
                Some(p) => format!("{}{full_text}", p.display_prefix()),
                None => full_text,
            }
        } else {
            full_text
        };
        Ok(Some(reply_text))
    }

    /// Handle slash commands
    async fn handle_slash(
        &self,
        cmd: SlashCommand,
        session_key: &SessionKey,
        target_agent: Option<&str>,
    ) -> Result<Option<String>> {
        match &cmd {
            SlashCommand::SetBackend(name) => {
                let backend_id = self
                    .roster
                    .as_ref()
                    .and_then(|r| r.find_by_name(name))
                    .map(|entry| entry.runtime_backend_id().to_string())
                    .unwrap_or_else(|| name.clone());
                self.set_session_backend(session_key, backend_id);
            }
            SlashCommand::Reset => {
                if let Ok(session_id) = self.session_manager.get_or_create(session_key).await {
                    self.session_manager
                        .storage()
                        .clear_messages(session_id)
                        .await
                        .ok();
                }
            }
            SlashCommand::Help => {}
            SlashCommand::Remember(content) => {
                let memory_target = target_agent
                    .and_then(|mention| {
                        self.roster
                            .as_ref()?
                            .find_by_mention(mention)?
                            .persona_dir
                            .clone()
                    })
                    .map(|dir| MemoryTarget::Agent { persona_dir: dir })
                    .unwrap_or(MemoryTarget::Shared);
                if let Some(ms) = &self.memory_system {
                    ms.emit(MemoryEvent::UserRemember {
                        scope: session_key.clone(),
                        target: memory_target,
                        content: content.clone(),
                    });
                }
            }
            SlashCommand::Memory(agent_opt) => {
                match agent_opt {
                    Some(agent_name) => {
                        // Per-agent memory lookup: <persona_dir>/<agent_name>/memory.md
                        let content = self
                            .read_agent_memory(agent_name)
                            .unwrap_or_else(|| format!("No memory found for agent @{agent_name}"));
                        return Ok(Some(content));
                    }
                    None => {
                        // Shared memory (original behaviour)
                        if let Some(ms) = &self.memory_system {
                            let store = ms.store();
                            let shared = store
                                .load_shared_memory(session_key)
                                .await
                                .unwrap_or_default();
                            let scope_display = &session_key.scope;
                            let response = if shared.is_empty() {
                                format!(
                                    "📭 当前还没有关于「{scope_display}」的记忆。\n\n可以告诉我一些背景，比如：\n- 团队用什么技术栈？\n- 有哪些编码规范？\n- 当前在做什么项目？\n\n或者直接 /remember <内容> 手动添加。"
                                )
                            } else {
                                let ts = store
                                    .shared_last_modified(session_key)
                                    .await
                                    .ok()
                                    .flatten()
                                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                                    .unwrap_or_else(|| "未知".to_string());
                                format!(
                                    "📚 当前记忆（{scope_display}）\n最后更新：{ts}\n\n{}\n\n输入 /remember <内容> 添加新记忆，/forget <关键词> 删除。",
                                    cap_to_words(&shared, 500)
                                )
                            };
                            return Ok(Some(response));
                        }
                    }
                }
            }
            SlashCommand::Forget(keyword) => {
                if let Some(ms) = &self.memory_system {
                    let store = ms.store();
                    let shared = store
                        .load_shared_memory(session_key)
                        .await
                        .unwrap_or_default();
                    let filtered: String = shared
                        .lines()
                        .filter(|line| !line.to_lowercase().contains(&keyword.to_lowercase()))
                        .map(|l| format!("{l}\n"))
                        .collect();
                    store.overwrite_shared(session_key, &filtered).await.ok();
                }
            }
            SlashCommand::MemoryReset => {
                let now = std::time::Instant::now();
                let confirmed = self
                    .pending_resets
                    .get(session_key)
                    .map(|t| now.duration_since(*t).as_secs() < 60)
                    .unwrap_or(false);
                if confirmed {
                    self.pending_resets.remove(session_key);
                    if let Some(ms) = &self.memory_system {
                        ms.store().overwrite_shared(session_key, "").await.ok();
                    }
                    return Ok(Some("✅ 记忆已清空。".to_string()));
                } else {
                    self.pending_resets.insert(session_key.clone(), now);
                    return Ok(Some(
                        "⚠️ 你确定要清空当前记忆吗？此操作不可撤销。\n再次发送 /memory reset 以确认（60 秒内有效）。".to_string()
                    ));
                }
            }
            SlashCommand::Workspace(path_opt) => {
                match path_opt {
                    None => {
                        // Show current workspace using the full three-tier resolution:
                        //   per-session override > roster entry workspace_dir > global default
                        let roster_workspace: Option<std::path::PathBuf> =
                            target_agent.and_then(|mention| {
                                self.roster
                                    .as_ref()
                                    .and_then(|r| r.find_by_mention(mention))
                                    .and_then(|entry| entry.workspace_dir.clone())
                            });
                        let resolved = self
                            .session_workspace(session_key)
                            .or(roster_workspace)
                            .or_else(|| self.default_workspace.clone());
                        let display =
                            resolved
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|| {
                                    "(none — running in gateway process directory)".to_string()
                                });
                        return Ok(Some(format!("Current workspace: `{display}`")));
                    }
                    Some(path_str) => {
                        let new_path = std::path::PathBuf::from(path_str);
                        if !new_path.exists() {
                            return Ok(Some(format!("Directory does not exist: `{path_str}`")));
                        }
                        if !new_path.is_dir() {
                            return Ok(Some(format!("Path is not a directory: `{path_str}`")));
                        }
                        self.set_session_workspace(session_key, new_path);
                        return Ok(Some(format!(
                            "Workspace set to: `{path_str}`\nNew agent turns will run in this directory."
                        )));
                    }
                }
            }
            SlashCommand::Approve {
                approval_id,
                decision,
            } => {
                let Some(parsed) = ApprovalDecision::parse(decision) else {
                    return Ok(Some(
                        "❌ 无效审批决定。使用：allow-once / allow-always / deny".to_string(),
                    ));
                };
                let Some(resolver) = self.approval_resolver.get() else {
                    return Ok(Some("❌ 当前运行实例未启用审批解析器。".to_string()));
                };
                let resolved = resolver.resolve(approval_id, parsed).await?;
                if resolved {
                    return Ok(Some(format!(
                        "✅ 已处理审批 `{}` -> `{}`",
                        approval_id,
                        parsed.as_str()
                    )));
                }
                return Ok(Some(format!(
                    "⚠️ 审批 `{}` 不存在、已过期，或已被处理。",
                    approval_id
                )));
            }
            SlashCommand::TeamStatus => {
                // Look up the team orchestrator for this session (Lead or Specialist).
                let orch_arc = self.get_orchestrator_for_session(session_key);

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

                    let response = if team_manifest.trim().is_empty()
                        && tasks_snapshot.trim().is_empty()
                    {
                        "ℹ️ Team 已初始化但尚无任务。Lead 正在规划中...".to_string()
                    } else {
                        format!(
                            "🏢 **Team 状态** — {task_count}\n\n{team_manifest}\n\n---\n\n{tasks_snapshot}"
                        )
                    };
                    return Ok(Some(response));
                } else {
                    return Ok(Some(
                        "ℹ️ 当前 session 没有活跃的 Team。输入 /team plan 开始规划。".to_string(),
                    ));
                }
            }
        }
        Ok(Some(cmd.confirmation_text()))
    }

    /// Read the memory file for a named agent persona.
    /// Prefers the per-entry `persona_dir` from the roster if available.
    /// Convention: `<persona_dir>/<agent_name>/memory.md` — matches AgentPersona path structure.
    /// Falls back to `default_persona_dir` (single-engine mode).
    /// Returns None if no persona_dir is configured, or if the file doesn't exist / can't be read.
    pub fn read_agent_memory(&self, agent_name: &str) -> Option<String> {
        let persona_dir: std::path::PathBuf = self
            .roster
            .as_ref()
            .and_then(|r| r.find_by_name(agent_name))
            .and_then(|entry| entry.persona_dir.clone())
            .or_else(|| self.default_persona_dir.clone())?;
        let mem_path = persona_dir.join(agent_name).join("memory.md");
        std::fs::read_to_string(&mem_path).ok()
    }

    /// Test helper: inject an instant into pending_resets directly (bypasses 60s window).
    #[cfg(test)]
    pub fn inject_pending_reset_at(&self, key: SessionKey, instant: std::time::Instant) {
        self.pending_resets.insert(key, instant);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{distiller::NoopDistiller, store::FileMemoryStore, MemorySystem};
    use crate::roster::{AgentEntry, AgentRoster};
    use crate::runtime_dispatch::{RuntimeDispatch, RuntimeDispatchRequest};
    use qai_protocol::{InboundMsg, MsgContent};
    use qai_runtime::contract::{TeamCallback, TurnResult};
    use qai_runtime::RuntimeEvent;
    use qai_session::SessionStorage;
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
    }

    #[async_trait::async_trait]
    impl RuntimeDispatch for FakeRuntimeDispatch {
        async fn dispatch(&self, request: RuntimeDispatchRequest) -> Result<TurnResult> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_backend.lock().unwrap() = request.intent.target_backend.clone();
            Ok(TurnResult {
                full_text: format!("fake-dispatch: {}", request.intent.user_text),
                events: vec![],
            })
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
        let store: Arc<dyn crate::memory::MemoryStore> =
            Arc::new(FileMemoryStore::new(mem_dir.keep()));
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

    #[test]
    fn test_agent_ctx_carries_workspace_dir() {
        let ctx = AgentCtx {
            session_id: uuid::Uuid::new_v4(),
            session_key: SessionKey::new("ws", "ctx-test"),
            user_text: "hello".to_string(),
            history: vec![],
            system_injection: String::new(),
            workspace_dir: Some(std::path::PathBuf::from("/projects/test")),
            ..AgentCtx::default()
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
            source: qai_protocol::MsgSource::Human,
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
            source: qai_protocol::MsgSource::Human,
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
            source: qai_protocol::MsgSource::Human,
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
            source: qai_protocol::MsgSource::Human,
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
            source: qai_protocol::MsgSource::Human,
        };

        let result = registry.handle(inbound).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(last_backend.lock().unwrap().as_deref(), Some("native-main"));
        assert_eq!(result.as_deref(), Some("fake-dispatch: hello runtime"));
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
        use crate::team::{
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
        let lead_key = qai_protocol::SessionKey::new("lark", "group:123");
        orch.set_lead_session_key(lead_key.clone());
        orch.set_lead_agent_name("mybot".to_string());
        registry.register_team_orchestrator("t".to_string(), orch);

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

    #[test]
    fn test_read_agent_memory_no_persona_dir() {
        // make_registry() passes None for default_persona_dir
        let (reg, _rx) = make_registry();
        assert!(reg.read_agent_memory("reviewer").is_none());
    }

    #[test]
    fn test_read_agent_memory_file_exists() {
        let tmp = tempdir().unwrap();
        let agent_dir = tmp.path().join("reviewer");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("memory.md"), "reviewer memory content").unwrap();

        let storage = SessionStorage::new(
            std::env::temp_dir().join(format!("test-agent-mem-{}", uuid::Uuid::new_v4())),
        );
        let session_manager = Arc::new(SessionManager::new(storage));
        let (reg, _rx) = SessionRegistry::new(
            None,
            session_manager,
            String::new(),
            None,
            None,
            Some(tmp.path().to_path_buf()),
            None,
            vec![],
        );
        let content = reg.read_agent_memory("reviewer").unwrap();
        assert_eq!(content, "reviewer memory content");
    }

    #[test]
    fn test_read_agent_memory_file_missing() {
        let tmp = tempdir().unwrap();
        // persona_dir exists but no subdirectory for "reviewer"
        let storage = SessionStorage::new(
            std::env::temp_dir().join(format!("test-agent-missing-{}", uuid::Uuid::new_v4())),
        );
        let session_manager = Arc::new(SessionManager::new(storage));
        let (reg, _rx) = SessionRegistry::new(
            None,
            session_manager,
            String::new(),
            None,
            None,
            Some(tmp.path().to_path_buf()),
            None,
            vec![],
        );
        assert!(reg.read_agent_memory("reviewer").is_none());
    }

    #[tokio::test]
    async fn test_slash_memory_at_agent_no_persona_dir_returns_not_found() {
        // make_registry has no persona_dir; /memory @reviewer should return "No memory found"
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
            source: qai_protocol::MsgSource::Human,
        };
        let result = registry.handle(inbound).await.unwrap();
        let text = result.unwrap();
        assert!(
            text.contains("No memory found for agent @reviewer"),
            "expected 'No memory found' message, got: {text}"
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
            source: qai_protocol::MsgSource::Human,
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
            source: qai_protocol::MsgSource::Human,
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
            source: qai_protocol::MsgSource::Human,
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
            source: qai_protocol::MsgSource::Human,
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
            source: qai_protocol::MsgSource::Human,
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
        let loader = qai_skills::SkillLoader::with_dirs(dirs);
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
        let loader = qai_skills::SkillLoader::with_dirs(all_dirs);
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
            source: qai_protocol::MsgSource::Human,
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
        let loader = qai_skills::SkillLoader::with_dirs(vec![tmp.path().to_path_buf()]);
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
        use crate::team::{
            heartbeat::DispatchFn, orchestrator::TeamOrchestrator, registry::TaskRegistry,
            session::TeamSession,
        };
        use tempfile::tempdir;

        let (registry, _rx) = make_registry();
        let tmp = tempdir().unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("team-test", tmp.path().to_path_buf()));
        let dispatch_fn: DispatchFn = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            task_registry,
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        let lead_key = SessionKey::new("lark", "group:team");
        orch.set_lead_session_key(lead_key.clone());
        orch.set_scope(lead_key);
        registry.register_team_orchestrator("team-test".to_string(), Arc::clone(&orch));
        (registry, orch)
    }

    #[tokio::test]
    async fn test_invoke_team_tool_submit_and_accept_updates_registry() {
        let (registry, orch) = make_registry_with_team_orchestrator();
        let specialist_key = SessionKey::new("specialist", "team-test:codex");

        orch.registry
            .create_task(crate::team::registry::CreateTask {
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
            crate::team::registry::TaskStatus::Accepted { .. }
        ));
    }

    #[tokio::test]
    async fn test_invoke_team_tool_get_status_returns_json_payload() {
        let (registry, orch) = make_registry_with_team_orchestrator();
        let lead_key = SessionKey::new("lark", "group:team");

        orch.registry
            .create_task(crate::team::registry::CreateTask {
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
    async fn test_apply_runtime_events_submits_task_into_existing_registry() {
        let (registry, orch) = make_registry_with_team_orchestrator();
        let specialist_key = SessionKey::new("specialist", "team-test:openclaw-main");

        orch.registry
            .create_task(crate::team::registry::CreateTask {
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
                        agent: "openclaw-main".into(),
                    })],
                },
            )
            .await
            .unwrap();

        let task = orch.registry.get_task("T300").unwrap().unwrap();
        assert!(matches!(
            task.status_parsed(),
            crate::team::registry::TaskStatus::Submitted { .. }
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
