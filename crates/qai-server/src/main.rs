use anyhow::Result;
use async_trait::async_trait;
use qai_agent::{OutputSink, SessionRegistry};
use qai_channels::Channel as _;
use qai_server::config;
use qai_server::gateway;
use qai_server::state::AppState;
use qai_session::{SessionManager, SessionStorage};
use qai_skills::SkillLoader;
use std::sync::Arc;
use std::time::Duration;

fn next_delay(current: Duration) -> Duration {
    (current * 2).min(Duration::from_secs(300))
}

/// OutputSink implementation for Feishu/Lark IM streaming.
/// Edits the placeholder message at 500ms intervals with accumulated text,
/// and sends (or edits) the final message when the turn completes.
struct LarkImSink {
    channel: Arc<qai_channels::LarkChannel>,
    reply_to: Option<String>,
    thread_ts: Option<String>,
    session_key: qai_protocol::SessionKey,
}

#[async_trait]
impl OutputSink for LarkImSink {
    async fn send_thinking(&self) -> Option<String> {
        // The placeholder is sent externally before throttled_stream is called;
        // this method is a no-op for LarkImSink.
        None
    }

    async fn send_delta(&self, accumulated: &str, placeholder_id: Option<&str>) {
        if let Some(msg_id) = placeholder_id {
            if let Err(e) = self.channel.edit_message(msg_id, accumulated).await {
                tracing::warn!("Lark edit_message (delta) failed: {e}");
            }
        }
    }

    async fn send_final(&self, text: &str, placeholder_id: Option<&str>) {
        if let Some(msg_id) = placeholder_id {
            if let Err(e) = self.channel.edit_message(msg_id, text).await {
                tracing::error!("Lark edit_message (final) failed: {e}");
            }
        } else {
            let msg = qai_protocol::OutboundMsg {
                session_key: self.session_key.clone(),
                content: qai_protocol::MsgContent::text(text),
                reply_to: self.reply_to.clone(),
                thread_ts: self.thread_ts.clone(),
            };
            if let Err(e) = self.channel.send(&msg).await {
                tracing::error!("Lark send_final failed: {e}");
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("quickai_gateway=info".parse()?),
        )
        .init();

    let cfg = config::GatewayConfig::load()?;
    tracing::info!("Loaded config: engine={:?}", cfg.agent.engine);

    // 初始化 Session 存储
    let storage = SessionStorage::new(cfg.session.dir.clone());
    let session_manager = Arc::new(SessionManager::new(storage));

    // 加载 Skills（主目录 + global_dirs 合并）
    let mut all_skill_dirs = vec![cfg.skills.dir.clone()];
    all_skill_dirs.extend(cfg.skills.global_dirs.iter().cloned());
    let skill_loader = SkillLoader::new(all_skill_dirs.clone());
    let skills = skill_loader.load_all();
    let system_injection = skill_loader.build_system_injection(&skills);
    tracing::info!("Loaded {} skills", skills.len());

    // 初始化 Engine
    let engine_cfg = cfg.agent.engine.clone();
    tracing::info!("Engine: {:?}", engine_cfg);

    // Build AgentRoster from config (None if no agents configured)
    let roster = if cfg.agent_roster.is_empty() {
        None
    } else {
        Some(qai_agent::AgentRoster::new(cfg.agent_roster.clone()))
    };

    // Initialize MemorySystem
    use qai_agent::memory::triggers::default_triggers;
    use qai_agent::memory::{AcpDistiller, FileMemoryStore, MemorySystem};

    let memory_system = {
        let store = std::sync::Arc::new(FileMemoryStore::new(cfg.memory.shared_dir.clone()));
        let distiller = std::sync::Arc::new(AcpDistiller::new(&cfg.memory.distiller_binary));
        let triggers = default_triggers(cfg.memory.distill_every_n);
        Some(MemorySystem::new(triggers, store, distiller))
    };
    // Keep a reference for cron and nightly scheduler (registry takes ownership of its copy)
    let memory_system_ref = memory_system.clone();

    // 初始化 SessionRegistry（替换 AgentRunner）
    let (registry, _event_rx) = SessionRegistry::new(
        engine_cfg,
        session_manager,
        system_injection,
        roster,
        memory_system,
        Some(cfg.memory.shared_dir.clone()),
        cfg.gateway.default_workspace.clone(),
        all_skill_dirs,
    );
    // 使用 registry 内部的 global_tx，确保事件正确广播
    let event_tx = registry.global_sender();

    let state = AppState {
        registry: registry.clone(),
        event_tx: event_tx.clone(),
        cfg: Arc::new(cfg.clone()),
    };

    // Channel registry for cron output: maps channel name → channel Arc
    let mut cron_channel_map: std::collections::HashMap<String, Arc<dyn qai_channels::Channel>> =
        std::collections::HashMap::new();

    // 启动 Channel 监听（DingTalk）
    if let Some(dt_cfg) = &cfg.channels.dingtalk {
        if dt_cfg.enabled {
            if let Ok(dt_config) = qai_channels::dingtalk::DingTalkConfig::from_env() {
                let channel = Arc::new(qai_channels::DingTalkChannel::new(
                    dt_config,
                    cfg.gateway.require_mention_in_groups,
                ));
                cron_channel_map.insert(
                    "dingtalk".to_string(),
                    channel.clone() as Arc<dyn qai_channels::Channel>,
                );
                let registry_clone = registry.clone();
                let channel_clone = channel.clone();
                let (tx, mut rx) = tokio::sync::mpsc::channel(64);

                // 监听线程：接收 DingTalk 消息 → tx（带断线重连）
                tokio::spawn(async move {
                    let mut delay = Duration::from_secs(5);
                    loop {
                        match qai_channels::Channel::listen(channel.as_ref(), tx.clone()).await {
                            Err(e) => {
                                delay = next_delay(delay);
                                tracing::error!(
                                    "DingTalk listen error (retry in {:?}): {e}",
                                    delay
                                );
                            }
                            Ok(()) => {
                                delay = Duration::from_secs(5);
                                tracing::info!(
                                    "DingTalk WS closed normally, reconnecting in {:?}",
                                    delay
                                );
                            }
                        }
                        tokio::time::sleep(delay).await;
                    }
                });

                // 派发线程：处理消息 → 回复到 DingTalk
                tokio::spawn(async move {
                    while let Some(inbound) = rx.recv().await {
                        let session_key = inbound.session_key.clone();
                        let thread_ts = inbound.thread_ts.clone();
                        let reply_to = Some(inbound.id.clone());

                        match registry_clone.handle(inbound).await {
                            Ok(Some(full_text)) => {
                                let reply = qai_protocol::OutboundMsg {
                                    session_key,
                                    content: qai_protocol::MsgContent::text(full_text),
                                    reply_to,
                                    thread_ts,
                                };
                                if let Err(e) = channel_clone.send(&reply).await {
                                    tracing::error!("DingTalk send error: {e}");
                                }
                            }
                            Ok(None) => {
                                tracing::debug!("Dedup: skipped duplicate message");
                            }
                            Err(e) => {
                                tracing::error!("Registry handle error: {e}");
                            }
                        }
                    }
                });

                tracing::info!("DingTalk channel started");
            } else {
                tracing::warn!("DingTalk enabled but DINGTALK_APP_KEY/SECRET not set");
            }
        }
    }

    // 启动 Channel 监听（Lark/飞书）
    if let Some(lark_cfg) = &cfg.channels.lark {
        if lark_cfg.enabled {
            let lark_channel_result = {
                let app_id = std::env::var("LARK_APP_ID");
                let app_secret = std::env::var("LARK_APP_SECRET");
                match (app_id, app_secret) {
                    (Ok(id), Ok(secret)) => Ok(qai_channels::LarkChannel::new(
                        id,
                        secret,
                        cfg.gateway.require_mention_in_groups,
                    )),
                    _ => Err(anyhow::anyhow!("LARK_APP_ID or LARK_APP_SECRET not set")),
                }
            };
            match lark_channel_result {
                Ok(channel) => {
                    let channel = Arc::new(channel);
                    cron_channel_map.insert(
                        "lark".to_string(),
                        channel.clone() as Arc<dyn qai_channels::Channel>,
                    );
                    let registry_clone = registry.clone();
                    let channel_clone = channel.clone();
                    let (tx, mut rx) = tokio::sync::mpsc::channel(64);

                    tokio::spawn(async move {
                        let mut delay = Duration::from_secs(5);
                        loop {
                            match qai_channels::Channel::listen(channel.as_ref(), tx.clone()).await
                            {
                                Err(e) => {
                                    delay = next_delay(delay);
                                    tracing::error!(
                                        "Lark listen error (retry in {:?}): {e}",
                                        delay
                                    );
                                }
                                Ok(()) => {
                                    delay = Duration::from_secs(5);
                                    tracing::info!(
                                        "Lark WS closed normally, reconnecting in {:?}",
                                        delay
                                    );
                                }
                            }
                            tokio::time::sleep(delay).await;
                        }
                    });

                    tokio::spawn(async move {
                        while let Some(inbound) = rx.recv().await {
                            let session_key = inbound.session_key.clone();
                            let thread_ts = inbound.thread_ts.clone();
                            let reply_to = Some(inbound.id.clone()); // Feishu message_id for reply threading

                            // Subscribe to global event stream BEFORE handle() fires events,
                            // so no events are missed.
                            let event_rx = registry_clone.global_sender().subscribe();

                            // 1. Send placeholder "thinking..." message and capture its ID.
                            let placeholder_msg = qai_protocol::OutboundMsg {
                                session_key: session_key.clone(),
                                content: qai_protocol::MsgContent::text("⏳ 思考中..."),
                                reply_to: reply_to.clone(),
                                thread_ts: thread_ts.clone(),
                            };
                            let placeholder_id =
                                channel_clone.send_and_get_id(&placeholder_msg).await.ok();

                            // 2. Start throttled streaming consumer in a separate task.
                            //    It will get session_id itself and then consume events at 500ms
                            //    intervals, editing the placeholder message in-place.
                            let channel2 = channel_clone.clone();
                            let registry2 = registry_clone.clone();
                            let sk2 = session_key.clone();
                            let reply_to2 = reply_to.clone();
                            let thread_ts2 = thread_ts.clone();
                            let ph_id2 = placeholder_id.clone();
                            tokio::spawn(async move {
                                let session_id =
                                    match registry2.session_manager_ref().get_or_create(&sk2).await
                                    {
                                        Ok(id) => id,
                                        Err(e) => {
                                            tracing::error!("Lark: get session_id failed: {e}");
                                            return;
                                        }
                                    };

                                let sink = LarkImSink {
                                    channel: channel2,
                                    reply_to: reply_to2,
                                    thread_ts: thread_ts2,
                                    session_key: sk2,
                                };

                                qai_agent::throttled_stream(event_rx, session_id, &sink, ph_id2)
                                    .await;
                            });

                            // 3. Run handle() in its own task — emits events to broadcast channel.
                            let registry3 = registry_clone.clone();
                            tokio::spawn(async move {
                                if let Err(e) = registry3.handle(inbound).await {
                                    tracing::error!("Lark registry handle error: {e}");
                                }
                            });
                        }
                    });

                    tracing::info!("Lark channel started");
                }
                Err(e) => {
                    tracing::warn!("Lark enabled but credentials not set: {e}");
                }
            }
        }
    }

    // 初始化 CronStore（持久化到 ~/.quickai/cron.db）
    let cron_db = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".quickai")
        .join("cron.db");
    if let Some(parent) = cron_db.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let cron_store = Arc::new(qai_cron::CronStore::open(&cron_db)?);

    // Sync cron jobs declared in config.toml into the SQLite store
    for job in &cfg.cron_jobs {
        if let Err(e) = cron_store.upsert_by_name(
            &job.name,
            &job.expr,
            &job.prompt,
            &job.session_key,
            job.enabled,
            job.agent.as_deref(),
            job.condition.as_deref(),
        ) {
            tracing::warn!("Failed to sync cron job {:?} from config: {e}", job.name);
        }
    }
    if !cfg.cron_jobs.is_empty() {
        tracing::info!(
            "Synced {} cron job(s) from config.toml",
            cfg.cron_jobs.len()
        );
    }

    let cron_channel_map = Arc::new(cron_channel_map);

    // 启动 CronScheduler
    {
        let cron_registry = registry.clone();
        let cron_memory = memory_system_ref.clone();
        let cron_channels = cron_channel_map.clone();
        let cron_trigger: qai_cron::TriggerFn = Arc::new(
            move |session_key_str: String,
                  prompt: String,
                  agent_opt: Option<String>,
                  condition: Option<String>| {
                let registry = cron_registry.clone();
                let memory = cron_memory.clone();
                let channels = cron_channels.clone();
                tokio::spawn(async move {
                    // Check condition before firing
                    if let Some(ref cond_str) = condition {
                        if let Some(cond) = qai_cron::CronCondition::parse(cond_str) {
                            match cond {
                                qai_cron::CronCondition::IdleGtSeconds(threshold) => {
                                    let idle = registry
                                        .session_idle_seconds(&session_key_str)
                                        .unwrap_or(0);
                                    if idle < threshold {
                                        tracing::debug!(
                                            session = %session_key_str,
                                            idle_secs = idle,
                                            threshold = threshold,
                                            "Cron job skipped: session not idle long enough"
                                        );
                                        return;
                                    }
                                }
                            }
                        }
                    }

                    // Parse "channel:scope" into SessionKey.
                    // Fall back to channel="cron", scope=full string if no colon.
                    let session_key = if let Some(pos) = session_key_str.find(':') {
                        qai_protocol::SessionKey::new(
                            &session_key_str[..pos],
                            &session_key_str[pos + 1..],
                        )
                    } else {
                        qai_protocol::SessionKey::new("cron", session_key_str.as_str())
                    };
                    let msg = qai_protocol::InboundMsg {
                        id: uuid::Uuid::new_v4().to_string(),
                        session_key: session_key.clone(),
                        content: qai_protocol::MsgContent::text(prompt),
                        sender: "cron".to_string(),
                        channel: "cron".to_string(),
                        timestamp: chrono::Utc::now(),
                        thread_ts: None,
                        target_agent: agent_opt,
                        source: qai_protocol::MsgSource::Cron,
                    };
                    match registry.handle(msg).await {
                        Ok(Some(result)) => {
                            // Send result to IM channel if one is registered for this session's channel
                            if let Some(ch) = channels.get(&session_key.channel) {
                                let outbound = qai_protocol::OutboundMsg {
                                    session_key: session_key.clone(),
                                    content: qai_protocol::MsgContent::text(&result),
                                    reply_to: None,
                                    thread_ts: None,
                                };
                                if let Err(e) = ch.send(&outbound).await {
                                    tracing::error!(
                                        "Cron output send to channel '{}' failed: {e}",
                                        session_key.channel
                                    );
                                }
                            }
                            // Emit CronJobCompleted so CronResultTrigger can write to shared memory
                            if let Some(ms) = &memory {
                                let summary: String = result.chars().take(300).collect();
                                ms.emit(qai_agent::MemoryEvent::CronJobCompleted {
                                    scope: session_key,
                                    agent: "cron".to_string(),
                                    persona_dir: std::path::PathBuf::new(),
                                    result_summary: summary,
                                });
                            }
                        }
                        Ok(None) => {}
                        Err(e) => tracing::error!("Cron trigger failed: {e}"),
                    }
                })
            },
        );
        let scheduler = qai_cron::CronScheduler::new(cron_store, cron_trigger);
        tokio::spawn(async move { scheduler.run().await });
        tracing::info!("CronScheduler started (polling every 1s, db={:?})", cron_db);
    }

    // 启动 NightlyConsolidation 调度器（每天本地零点合并 agent 私有记忆 → 共享记忆）
    if let Some(ms) = &memory_system_ref {
        let ms_clone = ms.clone();
        let registry_clone = registry.clone();
        let agent_roster = cfg.agent_roster.clone();
        tokio::spawn(async move {
            use chrono::Timelike;
            loop {
                // Sleep until next local midnight
                let now = chrono::Local::now();
                let secs_since_midnight = now.num_seconds_from_midnight() as u64;
                let secs_until_midnight = 86400u64.saturating_sub(secs_since_midnight).max(1);
                tokio::time::sleep(Duration::from_secs(secs_until_midnight)).await;

                // Collect agent dirs from roster entries that have a persona_dir
                let agent_dirs: Vec<(String, std::path::PathBuf)> = agent_roster
                    .iter()
                    .filter_map(|entry| {
                        entry
                            .persona_dir
                            .as_ref()
                            .map(|pd| (entry.name.clone(), pd.clone()))
                    })
                    .collect();

                if !agent_dirs.is_empty() {
                    for scope in registry_clone.all_active_scopes() {
                        ms_clone.emit(qai_agent::MemoryEvent::NightlyConsolidation {
                            scope,
                            agent_dirs: agent_dirs.clone(),
                        });
                    }
                    tracing::info!(
                        "NightlyConsolidation emitted for {} agent(s)",
                        agent_dirs.len()
                    );
                }
            }
        });
        tracing::info!("NightlyConsolidation scheduler started");
    }

    // 启动 Gateway HTTP/WS 服务器
    let addr = gateway::server::start(state, &cfg.gateway.host, cfg.gateway.port).await?;

    // 写端口到 ~/.quickai/gateway.port（供 Tauri 壳读取）
    let port_file = dirs::home_dir()
        .unwrap_or_default()
        .join(".quickai")
        .join("gateway.port");
    let port_dir = port_file
        .parent()
        .ok_or_else(|| anyhow::anyhow!("port file path has no parent directory"))?;
    tokio::fs::create_dir_all(port_dir).await?;
    tokio::fs::write(&port_file, addr.port().to_string()).await?;
    tracing::info!("Gateway port {} written to {:?}", addr.port(), port_file);

    // 阻塞等待
    tokio::signal::ctrl_c().await?;
    tracing::info!("Gateway shutting down");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_next_delay_doubles() {
        assert_eq!(next_delay(Duration::from_secs(5)), Duration::from_secs(10));
        assert_eq!(next_delay(Duration::from_secs(30)), Duration::from_secs(60));
    }

    #[test]
    fn test_next_delay_caps_at_5min() {
        assert_eq!(
            next_delay(Duration::from_secs(200)),
            Duration::from_secs(300)
        );
        assert_eq!(
            next_delay(Duration::from_secs(300)),
            Duration::from_secs(300)
        );
        assert_eq!(
            next_delay(Duration::from_secs(400)),
            Duration::from_secs(300)
        );
    }
}
