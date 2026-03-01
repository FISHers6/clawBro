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

    // 加载 Skills
    let skill_loader = SkillLoader::new(vec![cfg.skills.dir.clone()]);
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

    // 初始化 SessionRegistry（替换 AgentRunner）
    let (registry, _event_rx) = SessionRegistry::new(
        engine_cfg,
        session_manager,
        system_injection,
        roster,
        memory_system,
    );
    // 使用 registry 内部的 global_tx，确保事件正确广播
    let event_tx = registry.global_sender();

    let state = AppState {
        registry: registry.clone(),
        event_tx: event_tx.clone(),
        cfg: Arc::new(cfg.clone()),
    };

    // 启动 Channel 监听（DingTalk）
    if let Some(dt_cfg) = &cfg.channels.dingtalk {
        if dt_cfg.enabled {
            if let Ok(dt_config) = qai_channels::dingtalk::DingTalkConfig::from_env() {
                let channel = Arc::new(qai_channels::DingTalkChannel::new(dt_config));
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
            match qai_channels::LarkChannel::from_env() {
                Ok(channel) => {
                    let channel = Arc::new(channel);
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

    // 启动 CronScheduler
    {
        let cron_registry = registry.clone();
        let cron_trigger: qai_cron::TriggerFn =
            Arc::new(move |session_key_str: String, prompt: String| {
                let registry = cron_registry.clone();
                tokio::spawn(async move {
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
                        session_key,
                        content: qai_protocol::MsgContent::text(prompt),
                        sender: "cron".to_string(),
                        channel: "cron".to_string(),
                        timestamp: chrono::Utc::now(),
                        thread_ts: None,
                        target_agent: None,
                    };
                    if let Err(e) = registry.handle(msg).await {
                        tracing::error!("Cron trigger failed: {e}");
                    }
                })
            });
        let scheduler = qai_cron::CronScheduler::new(cron_store, cron_trigger);
        tokio::spawn(async move { scheduler.run().await });
        tracing::info!("CronScheduler started (polling every 1s, db={:?})", cron_db);
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
