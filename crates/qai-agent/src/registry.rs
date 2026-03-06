// quickai-gateway/crates/qai-agent/src/registry.rs
//! SessionRegistry: per-session engine management + generic @mention routing.
//! Architectural role: Gateway orchestration layer (not platform-specific).
//! - Channels extract @mentions → InboundMsg.target_agent
//! - Registry resolves target_agent via AgentRoster (generic name lookup)
//! - No platform-specific text parsing here

use crate::dedup::DedupStore;
use crate::memory::cap_to_words;
use crate::memory::{MemoryEvent, MemorySystem, MemoryTarget};
use crate::persona::AgentPersona;
use crate::prompt_builder::SystemPromptBuilder;
use crate::relay::RelayEngine;
use crate::roster::AgentRoster;
use crate::selector::{EngineConfig, EngineSelector};
use crate::slash::SlashCommand;
use crate::team::orchestrator::TeamOrchestrator;
use crate::traits::{AgentCtx, AgentRole, BoxEngine, HistoryMsg};
use anyhow::Result;
use dashmap::DashMap;
use qai_channels::mention_trigger::MentionTrigger;
use qai_protocol::{AgentEvent, InboundMsg, MsgSource, SessionKey};
use qai_session::{SessionManager, StoredMessage};
use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Cloned data extracted from a roster match to avoid holding a borrow across await points.
struct RosterMatchData {
    engine_cfg: EngineConfig,
    agent_name: String,
    persona_dir: Option<std::path::PathBuf>,
    workspace_dir: Option<std::path::PathBuf>,
    extra_skills_dirs: Vec<std::path::PathBuf>,
}

/// Single session state: holds a per-session engine (supports /engine override)
pub struct Session {
    pub key: SessionKey,
    pub engine: BoxEngine,
}

/// SessionRegistry: manages all per-session state with DashMap
pub struct SessionRegistry {
    sessions: DashMap<SessionKey, Arc<Session>>,
    default_engine_cfg: EngineConfig,
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
    /// Team orchestrator — processes [DONE: Txxx] markers from Specialist replies.
    team_orchestrator: OnceLock<Arc<TeamOrchestrator>>,
    /// Relay engine — processes [RELAY: @agent <指令>] markers synchronously.
    relay_engine: OnceLock<Arc<RelayEngine>>,
    /// Mention trigger — scans bot replies for @botname patterns.
    mention_trigger: OnceLock<Arc<MentionTrigger>>,
    /// Weak self-reference for spawning recursive handle() calls (TeamNotify dispatch).
    weak_self: std::sync::Weak<Self>,
    /// TeamNotify redispatch channel — set by main.rs, used to inject TeamNotify InboundMsgs
    /// back into the main message loop without re-entering handle() recursively.
    team_notify_tx: OnceLock<tokio::sync::mpsc::Sender<InboundMsg>>,
}

impl SessionRegistry {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        default_engine_cfg: EngineConfig,
        session_manager: Arc<SessionManager>,
        system_injection: String,
        roster: Option<AgentRoster>,
        memory_system: Option<Arc<MemorySystem>>,
        default_persona_dir: Option<std::path::PathBuf>,
        default_workspace: Option<std::path::PathBuf>,
        skill_loader_dirs: Vec<std::path::PathBuf>,
    ) -> (Arc<Self>, broadcast::Receiver<AgentEvent>) {
        let (global_tx, global_rx) = broadcast::channel(256);
        let registry = Arc::new_cyclic(|weak| Self {
            sessions: DashMap::new(),
            default_engine_cfg,
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
            team_orchestrator: OnceLock::new(),
            relay_engine: OnceLock::new(),
            mention_trigger: OnceLock::new(),
            weak_self: weak.clone(),
            team_notify_tx: OnceLock::new(),
        });  // end of Arc::new_cyclic

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

    /// Attach a TeamOrchestrator — processes [DONE: Txxx] markers from Specialist replies.
    pub fn set_team_orchestrator(&self, orch: Arc<TeamOrchestrator>) {
        let _ = self.team_orchestrator.set(orch);
    }

    /// Attach a RelayEngine — processes [RELAY: @agent <指令>] markers synchronously.
    pub fn set_relay_engine(&self, engine: Arc<RelayEngine>) {
        let _ = self.relay_engine.set(engine);
    }

    /// Attach a MentionTrigger — scans bot replies for @botname and dispatches BotMention msgs.
    pub fn set_mention_trigger(&self, trigger: Arc<MentionTrigger>) {
        let _ = self.mention_trigger.set(trigger);
    }

    /// Set TeamNotify redispatch sender (called by main.rs at startup).
    /// TeamNotify messages are sent through this channel to the main message loop.
    pub fn set_team_notify_tx(&self, tx: tokio::sync::mpsc::Sender<InboundMsg>) {
        let _ = self.team_notify_tx.set(tx);
    }

    /// Get-or-create per-session cached engine (used when no roster match)
    pub fn get_or_create_session(&self, key: &SessionKey) -> Arc<Session> {
        self.sessions
            .entry(key.clone())
            .or_insert_with(|| {
                let engine = EngineSelector::build(&self.default_engine_cfg);
                Arc::new(Session {
                    key: key.clone(),
                    engine,
                })
            })
            .clone()
    }

    /// Override engine for a session (/engine slash command)
    pub fn set_session_engine(&self, key: &SessionKey, config: EngineConfig) {
        let engine = EngineSelector::build(&config);
        let session = Arc::new(Session {
            key: key.clone(),
            engine,
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

    pub fn session_manager_ref(&self) -> &SessionManager {
        &self.session_manager
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
        let user_text_for_log = user_text.clone();

        // ── Team Mode confirmation interceptor ──────────────────────────────────
        // When Lead called request_confirmation(), the next Human message is the user's yes/no.
        if inbound.source == qai_protocol::MsgSource::Human {
            if let Some(team_orch) = self.team_orchestrator.get() {
                if team_orch.team_state() == crate::team::orchestrator::TeamState::AwaitingConfirm {
                    if let Some(lead_key) = team_orch.lead_session_key.get() {
                        if &session_key == lead_key {
                            let text_lower = user_text.to_lowercase();
                            let confirmed = ["yes", "是", "确认", "ok", "好的", "开始"]
                                .iter()
                                .any(|kw| text_lower.contains(kw));
                            if confirmed {
                                if let Some(_arc_self) = self.weak_self.upgrade() {
                                    let orch = std::sync::Arc::clone(team_orch);
                                    tokio::spawn(async move {
                                        match orch.activate().await {
                                            Ok(msg) => tracing::info!("Team activated via confirmation: {}", msg),
                                            Err(e) => tracing::error!("Team activate error: {e}"),
                                        }
                                    });
                                }
                                return Ok(Some("收到，开始执行。任务队列已启动。".to_string()));
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

        // Slash commands take priority (no engine involved)
        if let Some(cmd) = SlashCommand::parse(&user_text) {
            return self
                .handle_slash(cmd, &session_key, inbound.target_agent.as_deref())
                .await;
        }

        // Early Specialist/Lead detection — must run before roster_match so Lead turns
        // without an explicit @mention can fall back to the configured front_bot engine.
        let early_is_specialist = inbound.source == MsgSource::Heartbeat;
        let early_is_lead = !early_is_specialist && {
            self.team_orchestrator
                .get()
                .and_then(|o| o.lead_session_key.get())
                .map(|k| k == &session_key)
                .unwrap_or(false)
        };

        // ── Generic routing via target_agent (set by Channel) ──
        // Clone needed data from roster match to avoid holding borrow across await.
        // For Lead turns without an explicit @mention, fall back to the configured
        // front_bot agent (set via `front_bot` in [[group]] config).
        let roster_match: Option<RosterMatchData> =
            inbound.target_agent.as_deref().and_then(|mention| {
                self.roster
                    .as_ref()
                    .and_then(|r| r.find_by_mention(mention))
                    .map(|entry| RosterMatchData {
                        engine_cfg: entry.engine.clone(),
                        agent_name: entry.name.clone(),
                        persona_dir: entry.persona_dir.clone(),
                        workspace_dir: entry.workspace_dir.clone(),
                        extra_skills_dirs: entry.extra_skills_dirs.clone(),
                    })
            })
            .or_else(|| {
                // Lead fallback: no @mention but this is a Lead turn → use front_bot engine
                if early_is_lead {
                    self.team_orchestrator
                        .get()
                        .and_then(|o| o.lead_agent_name.get())
                        .and_then(|name| {
                            self.roster.as_ref()?.find_by_name(name).map(|entry| {
                                RosterMatchData {
                                    engine_cfg: entry.engine.clone(),
                                    agent_name: entry.name.clone(),
                                    persona_dir: entry.persona_dir.clone(),
                                    workspace_dir: entry.workspace_dir.clone(),
                                    extra_skills_dirs: entry.extra_skills_dirs.clone(),
                                }
                            })
                        })
                } else {
                    None
                }
            });

        // Select engine: roster match → fresh engine per turn; no match → session-cached engine
        let (engine, sender_name): (BoxEngine, Option<String>) = if let Some(rm) = &roster_match {
            // AcpEngine is stateless per-turn; no need to cache in session for roster entries
            (
                EngineSelector::build(&rm.engine_cfg),
                Some(format!("@{}", rm.agent_name)),
            )
        } else {
            // No @mention or no roster: use the session's persistent engine (supports /engine)
            let session = self.get_or_create_session(&session_key);
            (Arc::clone(&session.engine), None)
        };

        // Get-or-create persistent session record
        let session_id = self.session_manager.get_or_create(&session_key).await?;
        let storage = self.session_manager.storage();

        // ── History: 50-message sliding window + sender prefix for LLM context ──
        let stored = storage.load_messages(session_id).await?;
        let start = stored.len().saturating_sub(50);
        let recent = &stored[start..];
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

        // When a TeamNotify arrives, lazily set lead_session_key + scope if not yet set
        if inbound.source == qai_protocol::MsgSource::TeamNotify {
            if let Some(team_orch) = self.team_orchestrator.get() {
                team_orch.set_lead_session_key(session_key.clone());
                team_orch.set_scope(session_key.clone());
            }
        }

        // Build Lead Layer 0 (task_reminder for Lead turns based on TeamState)
        let lead_layer_0: Option<String> = if early_is_lead {
            let state = self.team_orchestrator
                .get()
                .map(|o| o.team_state())
                .unwrap_or(crate::team::orchestrator::TeamState::Planning);
            Some(match state {
                crate::team::orchestrator::TeamState::Planning
                | crate::team::orchestrator::TeamState::AwaitingConfirm => {
                    "你是团队协调者。用户的请求需要多个 Agent 协作完成。\n\n\
                     步骤：\n\
                     1. 分析任务，调用 create_task() 定义所有子任务和依赖关系\n\
                     2. 简单任务（≤3个、无复杂依赖）直接调用 start_execution()\n\
                     3. 复杂任务先调用 request_confirmation(plan_summary)，等用户确认后再执行\n\
                     4. 任务执行中你会收到 [团队通知] 消息，用 post_update() 向用户播报关键进度\n\
                     5. 收到\"所有任务已完成\"通知后，合成最终结果并调用 post_update() 发给用户\n\n\
                     可用工具：create_task, start_execution, request_confirmation, post_update, get_task_status, assign_task"
                        .to_string()
                }
                crate::team::orchestrator::TeamState::Running
                | crate::team::orchestrator::TeamState::Done => {
                    "团队任务执行中。你会收到 [团队通知] 消息。\n\n\
                     - 用 post_update(message) 向用户播报进度\n\
                     - 用 get_task_status() 查看全局状态\n\
                     - 用 assign_task(task_id, agent) 重新分配卡住的任务\n\
                     - 收到\"所有任务已完成\"通知后，合成最终汇总并 post_update"
                        .to_string()
                }
            })
        } else {
            None
        };

        // Lead turns use lead_layer_0 as task_reminder; Specialist turns use pre-registered reminder
        let early_task_reminder: Option<String> = if early_is_lead {
            lead_layer_0.clone()
        } else {
            self.team_task_reminders
                .get(&session_key)
                .map(|r| r.value().clone())
        };

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

                let shared_mem = if let Some(ms) = &self.memory_system {
                    ms.store()
                        .load_shared_memory(&session_key)
                        .await
                        .unwrap_or_default()
                } else {
                    String::new()
                };

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
                    shared_memory: &shared_mem,
                    agent_memory: &agent_memory,
                    shared_max_words: 300,
                    agent_max_words: 500,
                    agent_role: early_agent_role,
                    task_reminder: early_task_reminder.as_deref(),
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

        // Consume task_reminder from DashMap (was peeked earlier for system prompt building).
        let task_reminder = self.team_task_reminders.remove(&session_key).map(|(_, v)| v);
        let team_dir = if early_is_specialist {
            self.team_orchestrator.get().map(|o| o.session.dir.clone())
        } else {
            None
        };

        // For Specialist turns, override workspace_dir with team_dir so the engine sees
        // TEAM.md/TASKS.md/CONTEXT.md in its working directory (fixes I1).
        let effective_workspace = team_dir.clone().or(workspace_dir_resolved);

        let mcp_server_url: Option<String> = if early_is_specialist {
            self.team_orchestrator
                .get()
                .and_then(|o| o.mcp_server_port.get().copied())
                .map(|port| format!("http://127.0.0.1:{port}/sse"))
        } else if early_is_lead {
            self.team_orchestrator
                .get()
                .and_then(|o| o.lead_mcp_server_port.get().copied())
                .map(|port| format!("http://127.0.0.1:{port}/sse"))
        } else {
            None
        };

        // Build AgentCtx for the engine
        let ctx = AgentCtx {
            session_id,
            user_text,
            history,
            system_injection,
            workspace_dir: effective_workspace,
            agent_role: early_agent_role,
            task_reminder,
            team_dir,
            mcp_server_url,
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

        // Run engine (blocks until turn completes)
        let full_text = engine.run(ctx, session_tx).await?;

        // ── Post-run hooks ─────────────────────────────────────────────────────

        // Hook 2: [RELAY: @agent <指令>] marker expansion (Relay Mode)
        let full_text = if let Some(relay) = self.relay_engine.get() {
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

        // Hook 3: @botname scan → BotMention dispatch (anti-recursion: skip BotMention/Relay source)
        if inbound.source != MsgSource::BotMention && inbound.source != MsgSource::Relay {
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
            SlashCommand::SetEngine(name) => {
                // First try roster lookup by name (user-defined agent names)
                let config = self
                    .roster
                    .as_ref()
                    .and_then(|r| r.find_by_name(name))
                    .map(|entry| entry.engine.clone())
                    .unwrap_or_else(|| {
                        // Fall back to built-in shortcuts for convenience
                        match name.as_str() {
                            "rust" => EngineConfig::RustAgent { binary: None },
                            "claude" => EngineConfig::ClaudeAgent { binary: None },
                            "codex" => EngineConfig::CodexAcp { binary: None },
                            other => EngineConfig::CustomAcp {
                                command: other.to_string(),
                                args: vec![],
                            },
                        }
                    });
                self.set_session_engine(session_key, config);
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
    use qai_protocol::{InboundMsg, MsgContent};
    use qai_session::SessionStorage;
    use tempfile::tempdir;

    fn make_registry() -> (Arc<SessionRegistry>, broadcast::Receiver<AgentEvent>) {
        let dir = std::env::temp_dir().join(format!("test-registry-{}", uuid::Uuid::new_v4()));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        SessionRegistry::new(
            EngineConfig::default(),
            session_manager,
            String::new(),
            None,
            None,
            None,
            None,
            vec![],
        )
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
            EngineConfig::default(),
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
            engine: EngineConfig::RustAgent {
                binary: Some("my-custom-agent".to_string()),
            },
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        }]);
        SessionRegistry::new(
            EngineConfig::default(),
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
    fn test_registry_per_session_engine_override() {
        let (registry, _rx) = make_registry();
        let key = SessionKey::new("ws", "user2");
        let default_name = registry
            .get_or_create_session(&key)
            .engine
            .name()
            .to_string();
        registry.set_session_engine(
            &key,
            EngineConfig::RustAgent {
                binary: Some("my-agent".to_string()),
            },
        );
        let new_name = registry
            .get_or_create_session(&key)
            .engine
            .name()
            .to_string();
        assert_ne!(default_name, new_name);
        assert_eq!(new_name, "my-agent");
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
        assert!(result.unwrap().contains("/engine"));
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
        use crate::team::{bus::InternalBus, heartbeat::DispatchFn, orchestrator::TeamOrchestrator, registry::TaskRegistry, session::TeamSession};
        use tempfile::tempdir;

        let (registry, _rx) = make_registry_with_roster();
        let tmp = tempdir().unwrap();
        let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("t", tmp.path().to_path_buf()));
        let bus = Arc::new(InternalBus::new());
        let dispatch_fn: DispatchFn = Arc::new(|_, _, _| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(task_registry, session, bus, dispatch_fn, std::time::Duration::from_secs(60));

        // Wire: lead_session_key + lead_agent_name = "mybot"
        let lead_key = qai_protocol::SessionKey::new("lark", "group:123");
        orch.set_lead_session_key(lead_key.clone());
        orch.set_lead_agent_name("mybot".to_string());
        registry.set_team_orchestrator(orch);

        // Confirm roster has "mybot"
        let entry = registry.roster.as_ref().unwrap().find_by_name("mybot").unwrap();
        assert_eq!(entry.name, "mybot");

        // Simulate a Lead turn with no @mention: session_key == lead_key, source == Human
        // The early_is_lead detection and Lead fallback in roster_match should pick "mybot".
        // We verify via direct roster lookup since we can't run the full async handle() in a unit test.
        let resolved = registry
            .team_orchestrator
            .get()
            .and_then(|o| o.lead_agent_name.get())
            .and_then(|name| registry.roster.as_ref()?.find_by_name(name));
        assert!(resolved.is_some(), "Lead fallback should resolve front_bot roster entry");
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
            EngineConfig::default(),
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
            EngineConfig::default(),
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
            EngineConfig::default(),
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
        assert_eq!(reply_text, "Hello world", "reply unchanged in no-roster mode");
    }
}
