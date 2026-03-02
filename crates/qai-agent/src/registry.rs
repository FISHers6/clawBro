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
use crate::roster::AgentRoster;
use crate::selector::{EngineConfig, EngineSelector};
use crate::slash::SlashCommand;
use crate::traits::{AgentCtx, BoxEngine, HistoryMsg};
use anyhow::Result;
use dashmap::DashMap;
use qai_protocol::{AgentEvent, InboundMsg, SessionKey};
use qai_session::{SessionManager, StoredMessage};
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

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
}

impl SessionRegistry {
    pub fn new(
        default_engine_cfg: EngineConfig,
        session_manager: Arc<SessionManager>,
        system_injection: String,
        roster: Option<AgentRoster>,
        memory_system: Option<Arc<MemorySystem>>,
        default_persona_dir: Option<std::path::PathBuf>,
        default_workspace: Option<std::path::PathBuf>,
    ) -> (Arc<Self>, broadcast::Receiver<AgentEvent>) {
        let (global_tx, global_rx) = broadcast::channel(256);
        let registry = Arc::new(Self {
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

        // Slash commands take priority (no engine involved)
        if let Some(cmd) = SlashCommand::parse(&user_text) {
            return self
                .handle_slash(cmd, &session_key, inbound.target_agent.as_deref())
                .await;
        }

        // ── Generic routing via target_agent (set by Channel) ──
        // Clone needed data from roster match to avoid holding borrow across await
        // Tuple: (EngineConfig, name, persona_dir, workspace_dir)
        let roster_match: Option<(EngineConfig, String, Option<std::path::PathBuf>, Option<std::path::PathBuf>)> =
            inbound.target_agent.as_deref().and_then(|mention| {
                self.roster
                    .as_ref()
                    .and_then(|r| r.find_by_mention(mention))
                    .map(|entry| {
                        (
                            entry.engine.clone(),
                            entry.name.clone(),
                            entry.persona_dir.clone(),
                            entry.workspace_dir.clone(),
                        )
                    })
            });

        // Select engine: roster match → fresh engine per turn; no match → session-cached engine
        let (engine, sender_name): (BoxEngine, Option<String>) =
            if let Some((engine_cfg, name, _, _)) = &roster_match {
                // AcpEngine is stateless per-turn; no need to cache in session for roster entries
                (
                    EngineSelector::build(engine_cfg),
                    Some(format!("@{}", name)),
                )
            } else {
                // No @mention or no roster: use the session's persistent engine (supports /engine)
                let session = self.get_or_create_session(&session_key);
                (Arc::clone(&session.engine), None)
            };

        // Load persona: compose per-agent system_injection (SOUL + IDENTITY + MEMORY + skills)
        let system_injection = if let Some((_, _, persona_dir, _)) = &roster_match {
            if let Some(dir) = persona_dir.as_deref() {
                let persona = AgentPersona::load_from_dir_scoped(dir, &session_key);
                let shared_mem = if let Some(ms) = &self.memory_system {
                    ms.store()
                        .load_shared_memory(&session_key)
                        .await
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                persona.build_system_injection_v2(&self.system_injection, &shared_mem, 300, 500)
            } else {
                self.system_injection.clone()
            }
        } else {
            self.system_injection.clone()
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

        // Resolve workspace: per-roster-agent entry > global default
        let workspace_dir_resolved: Option<std::path::PathBuf> = roster_match
            .as_ref()
            .and_then(|(_, _, _, workspace_dir)| workspace_dir.clone())
            .or_else(|| self.default_workspace.clone());

        // Build AgentCtx for the engine
        let ctx = AgentCtx {
            session_id,
            user_text,
            history,
            system_injection,
            workspace_dir: workspace_dir_resolved,
        };

        // Per-call event channel: forward to global_tx + ws_subs
        // TurnComplete is enriched with sender_name here (engine itself doesn't know roster)
        let (session_tx, _) = broadcast::channel::<AgentEvent>(256);
        let global_tx = self.global_tx.clone();
        let ws_subs_clone = Arc::clone(&self.ws_subs);
        let sk_for_fwd = session_key.clone();
        let sender_for_fwd = sender_name.clone();
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
                            full_text,
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
            // persona_dir: roster agent dir takes priority; fall back to default (single-engine mode)
            let persona_dir_opt: Option<std::path::PathBuf> = roster_match
                .as_ref()
                .and_then(|(_, _, pd, _)| pd.clone())
                .or_else(|| self.default_persona_dir.clone());

            let agent_name_raw: String = roster_match
                .as_ref()
                .map(|(_, name, _, _)| name.clone())
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

        Ok(Some(full_text))
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
        )
    }

    fn make_registry_with_memory() -> (Arc<SessionRegistry>, broadcast::Receiver<AgentEvent>) {
        let db_dir = std::env::temp_dir().join(format!("test-registry-mem-{}", uuid::Uuid::new_v4()));
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
        }]);
        SessionRegistry::new(
            EngineConfig::default(),
            session_manager,
            String::new(),
            Some(roster),
            None,
            None,
            None,
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
            std::env::temp_dir()
                .join(format!("test-agent-mem-{}", uuid::Uuid::new_v4())),
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
        );
        let content = reg.read_agent_memory("reviewer").unwrap();
        assert_eq!(content, "reviewer memory content");
    }

    #[test]
    fn test_read_agent_memory_file_missing() {
        let tmp = tempdir().unwrap();
        // persona_dir exists but no subdirectory for "reviewer"
        let storage = SessionStorage::new(
            std::env::temp_dir()
                .join(format!("test-agent-missing-{}", uuid::Uuid::new_v4())),
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
        };
        let result = registry.handle(inbound).await.unwrap();
        let text = result.unwrap();
        assert!(text.contains("技术栈"), "empty memory should contain guiding question about 技术栈");
        assert!(text.contains("编码规范"), "empty memory should contain guiding question about 编码规范");
        assert!(text.contains("项目"), "empty memory should contain guiding question about 项目");
        assert!(text.contains("group_test"), "empty memory should include the scope name");
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
        };
        let result = registry.handle(inbound).await.unwrap().unwrap();
        assert!(result.contains("⚠️"), "expired pending should re-warn, got: {result}");
        assert!(!result.contains("✅"), "expired pending must NOT confirm clear, got: {result}");
    }
}
