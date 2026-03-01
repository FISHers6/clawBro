// quickai-gateway/crates/qai-agent/src/registry.rs
//! SessionRegistry: per-session engine management + generic @mention routing.
//! Architectural role: Gateway orchestration layer (not platform-specific).
//! - Channels extract @mentions → InboundMsg.target_agent
//! - Registry resolves target_agent via AgentRoster (generic name lookup)
//! - No platform-specific text parsing here

use crate::dedup::DedupStore;
use crate::memory::{MemoryEvent, MemorySystem, MemoryTarget};
use crate::memory::cap_to_words;
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
}

impl SessionRegistry {
    pub fn new(
        default_engine_cfg: EngineConfig,
        session_manager: Arc<SessionManager>,
        system_injection: String,
        roster: Option<AgentRoster>,
        memory_system: Option<Arc<MemorySystem>>,
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
        });

        // Idle timer: check every 60s for sessions idle > 30 min
        if let Some(ms) = &registry.memory_system {
            let registry_weak = Arc::downgrade(&registry);
            let ms = Arc::clone(ms);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                loop {
                    interval.tick().await;
                    let Some(reg) = registry_weak.upgrade() else { break };
                    let now = std::time::Instant::now();
                    for entry in reg.last_activity.iter() {
                        if now.duration_since(*entry.value()).as_secs() >= 1800 {
                            if let Some(roster) = &reg.roster {
                                for agent in roster.all_agents() {
                                    if let Some(ref pd) = agent.persona_dir {
                                        ms.emit(MemoryEvent::SessionIdle {
                                            scope: entry.key().clone(),
                                            agent: agent.name.clone(),
                                            persona_dir: pd.clone(),
                                        });
                                    }
                                }
                            }
                        }
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
                Arc::new(Session { key: key.clone(), engine })
            })
            .clone()
    }

    /// Override engine for a session (/engine slash command)
    pub fn set_session_engine(&self, key: &SessionKey, config: EngineConfig) {
        let engine = EngineSelector::build(&config);
        let session = Arc::new(Session { key: key.clone(), engine });
        self.sessions.insert(key.clone(), session);
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
            return self.handle_slash(cmd, &session_key, inbound.target_agent.as_deref()).await;
        }

        // ── Generic routing via target_agent (set by Channel) ──
        // Clone needed data from roster match to avoid holding borrow across await
        let roster_match: Option<(EngineConfig, String, Option<std::path::PathBuf>)> =
            inbound.target_agent.as_deref().and_then(|mention| {
                self.roster
                    .as_ref()
                    .and_then(|r| r.find_by_mention(mention))
                    .map(|entry| {
                        (entry.engine.clone(), entry.name.clone(), entry.persona_dir.clone())
                    })
            });

        // Select engine: roster match → fresh engine per turn; no match → session-cached engine
        let (engine, sender_name): (BoxEngine, Option<String>) =
            if let Some((engine_cfg, name, _)) = &roster_match {
                // AcpEngine is stateless per-turn; no need to cache in session for roster entries
                (EngineSelector::build(engine_cfg), Some(format!("@{}", name)))
            } else {
                // No @mention or no roster: use the session's persistent engine (supports /engine)
                let session = self.get_or_create_session(&session_key);
                (Arc::clone(&session.engine), None)
            };

        // Load persona: compose per-agent system_injection (SOUL + IDENTITY + MEMORY + skills)
        let system_injection = if let Some((_, _, persona_dir)) = &roster_match {
            if let Some(dir) = persona_dir.as_deref() {
                let persona = AgentPersona::load_from_dir_scoped(dir, &session_key);
                let shared_mem = if let Some(ms) = &self.memory_system {
                    ms.store().load_shared_memory(&session_key).await.unwrap_or_default()
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
                HistoryMsg { role: m.role.clone(), content }
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

        // Build AgentCtx for the engine
        let ctx = AgentCtx {
            session_id,
            user_text,
            history,
            system_injection,
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
                        AgentEvent::TurnComplete { session_id, full_text, .. } => {
                            AgentEvent::TurnComplete {
                                session_id,
                                full_text,
                                sender: sender_for_fwd.clone(),
                            }
                        }
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

        // ── Memory events (non-blocking) ──
        if let Some(ms) = &self.memory_system {
            if let Some((_, agent_name_raw, Some(persona_dir))) = &roster_match {
                let agent_name = agent_name_raw.trim_start_matches('@').to_string();
                let pd = persona_dir.clone();
                let pd_for_event = persona_dir.clone();
                let sk = session_key.clone();
                let log_entry = format!(
                    "**[{}]**: {}\n\n**[@{}]**: {}",
                    inbound.sender, user_text_for_log,
                    agent_name, full_text
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
                    persona_dir: pd_for_event,
                    turn_count: new_count,
                });
            }
            self.last_activity.insert(session_key.clone(), std::time::Instant::now());
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
                        self.roster.as_ref()?.find_by_mention(mention)?.persona_dir.clone()
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
            SlashCommand::Memory => {
                if let Some(ms) = &self.memory_system {
                    let store = ms.store();
                    let shared = store.load_shared_memory(session_key).await.unwrap_or_default();
                    let response = if shared.is_empty() {
                        "📭 当前还没有关于这个范围的共享记忆。\n可用 /remember <内容> 添加。".to_string()
                    } else {
                        format!("📚 共享记忆：\n\n{}", cap_to_words(&shared, 500))
                    };
                    return Ok(Some(response));
                }
            }
            SlashCommand::Forget(keyword) => {
                if let Some(ms) = &self.memory_system {
                    let store = ms.store();
                    let shared = store.load_shared_memory(session_key).await.unwrap_or_default();
                    let filtered: String = shared
                        .lines()
                        .filter(|line| !line.to_lowercase().contains(&keyword.to_lowercase()))
                        .map(|l| format!("{l}\n"))
                        .collect();
                    store.overwrite_shared(session_key, &filtered).await.ok();
                }
            }
            SlashCommand::MemoryReset => {
                if let Some(ms) = &self.memory_system {
                    ms.store().overwrite_shared(session_key, "").await.ok();
                }
            }
        }
        Ok(Some(cmd.confirmation_text()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::{AgentEntry, AgentRoster};
    use qai_protocol::{InboundMsg, MsgContent};
    use qai_session::SessionStorage;

    fn make_registry() -> (Arc<SessionRegistry>, broadcast::Receiver<AgentEvent>) {
        let dir = std::env::temp_dir().join(format!("test-registry-{}", uuid::Uuid::new_v4()));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        SessionRegistry::new(EngineConfig::default(), session_manager, String::new(), None, None)
    }

    fn make_registry_with_roster() -> (Arc<SessionRegistry>, broadcast::Receiver<AgentEvent>) {
        let dir = std::env::temp_dir().join(format!("test-registry-r-{}", uuid::Uuid::new_v4()));
        let storage = SessionStorage::new(dir);
        let session_manager = Arc::new(SessionManager::new(storage));
        let roster = AgentRoster::new(vec![AgentEntry {
            name: "mybot".to_string(),
            mentions: vec!["@mybot".to_string()],
            engine: EngineConfig::RustAgent { binary: Some("my-custom-agent".to_string()) },
            persona_dir: None,
        }]);
        SessionRegistry::new(EngineConfig::default(), session_manager, String::new(), Some(roster), None)
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
        let default_name = registry.get_or_create_session(&key).engine.name().to_string();
        registry.set_session_engine(
            &key,
            EngineConfig::RustAgent { binary: Some("my-agent".to_string()) },
        );
        let new_name = registry.get_or_create_session(&key).engine.name().to_string();
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
}
