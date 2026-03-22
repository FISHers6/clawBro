use crate::agent_core::{
    ConductorRuntimeDispatch, SessionRegistry, TurnDeliverySource, TurnExecutionContext,
};
use crate::channel_registry::ChannelRegistry;
use crate::config;
use crate::delivery_resolver::resolve_delivery;
use crate::diagnostics::spawn_dashboard_diagnostics_poller;
use crate::gateway;
use crate::im_sink::spawn_im_turn;
use crate::runtime::{
    acp::AcpBackendAdapter, ApprovalBroker, BackendRegistry, ClawBroNativeBackendAdapter,
    OpenClawBackendAdapter,
};
use crate::scheduler_runtime;
use crate::session::{SessionManager, SessionStorage};
use crate::skills_internal::{reconcile_default_skills, SkillLoader};
use crate::state::{AppState, BrokerApprovalResolver};
use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;

fn next_delay(current: Duration) -> Duration {
    (current * 2).min(Duration::from_secs(300))
}

pub async fn run() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("clawbro_gateway=info".parse()?),
        )
        .init();

    let cfg = config::GatewayConfig::load()?;
    cfg.validate_runtime_topology()?;
    let default_skills_report = reconcile_default_skills(&cfg)?;
    for warning in default_skills_report.warnings() {
        tracing::warn!(warning = %warning, "default skills mirror skipped");
    }
    tracing::info!("Loaded config with {} backends", cfg.backends.len());

    // 初始化 Session 存储
    let storage = SessionStorage::new(cfg.session.dir.clone());
    let session_manager = Arc::new(SessionManager::new(storage));

    // Reset any sessions that were stuck Running at last shutdown (crash recovery).
    match session_manager.recover_stuck_sessions().await {
        Ok(recovered) if !recovered.is_empty() => {
            tracing::warn!(
                count = recovered.len(),
                "reset stuck sessions from previous run"
            );
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "recover_stuck_sessions failed; continuing"),
    }

    // 加载 Skills（主目录 + global_dirs 合并）
    let mut all_skill_dirs = vec![cfg.skills.dir.clone()];
    all_skill_dirs.extend(cfg.skills.global_dirs.iter().cloned());
    let skill_loader = SkillLoader::new(all_skill_dirs.clone());
    let skills = skill_loader.load_all();
    let system_injection = skill_loader.build_builtin_system_injection();
    let skill_loader_dirs = skill_loader.search_dirs().to_vec();
    tracing::info!(
        discovered_skills = skills.len(),
        static_builtin_skills = usize::from(!system_injection.trim().is_empty()),
        "Discovered skills and prepared static builtin injection"
    );

    let default_backend_id = cfg.resolved_default_backend_id();
    tracing::info!(
        default_backend = default_backend_id.as_deref().unwrap_or("<none>"),
        "Resolved default backend"
    );

    // Build AgentRoster from config (None if no agents configured)
    let roster = if cfg.agent_roster.is_empty() {
        None
    } else {
        Some(crate::agent_core::AgentRoster::new(
            cfg.agent_roster.clone(),
        ))
    };

    // Initialize MemorySystem
    use crate::agent_core::memory::triggers::default_triggers;
    use crate::agent_core::memory::{AcpDistiller, FileMemoryStore, MemorySystem};

    let memory_system = {
        let store = std::sync::Arc::new(FileMemoryStore::new(cfg.memory.shared_dir.clone()));
        let distiller = std::sync::Arc::new(AcpDistiller::new(&cfg.memory.distiller_binary));
        let triggers = default_triggers(cfg.memory.distill_every_n);
        Some(MemorySystem::new(triggers, store, distiller))
    };
    // Keep a reference for scheduler and nightly scheduler (registry takes ownership of its copy)
    let memory_system_ref = memory_system.clone();

    // 初始化 SessionRegistry（替换 AgentRunner）
    let approvals = ApprovalBroker::default();
    let runtime_registry = Arc::new(BackendRegistry::new());
    runtime_registry
        .register_adapter("acp", Arc::new(AcpBackendAdapter::new(approvals.clone())))
        .await;
    runtime_registry
        .register_adapter(
            "openclaw",
            Arc::new(OpenClawBackendAdapter::new(approvals.clone())),
        )
        .await;
    runtime_registry
        .register_adapter("native", Arc::new(ClawBroNativeBackendAdapter))
        .await;
    for backend in &cfg.backends {
        runtime_registry
            .register_backend(backend.to_backend_spec(
                cfg.resolve_provider_profile(backend.provider_profile.as_deref())?,
            ))
            .await;
    }
    let runtime_dispatch = Arc::new(ConductorRuntimeDispatch::new(Arc::clone(&runtime_registry)));
    let (registry, _event_rx) = SessionRegistry::with_runtime_dispatch(
        default_backend_id,
        session_manager,
        system_injection,
        roster,
        memory_system,
        Some(cfg.memory.shared_dir.clone()),
        cfg.gateway.default_workspace.clone(),
        skill_loader_dirs,
        runtime_dispatch,
    );
    // 使用 registry 内部的 global_tx，确保事件正确广播
    let event_tx = registry.global_sender();
    let dashboard_tx = registry.dashboard_sender();
    registry.set_approval_resolver(Arc::new(BrokerApprovalResolver::new(approvals.clone())));
    approvals.set_dashboard_sender(dashboard_tx.clone());
    registry
        .session_manager_ref()
        .set_dashboard_sender(dashboard_tx.clone());

    let cfg_arc = Arc::new(cfg.clone());

    // Channel registry for server-owned outbound sends.
    let mut cron_channel_map = ChannelRegistry::new();
    let mut dingtalk_webhook_channel: Option<
        Arc<crate::channels_internal::DingTalkWebhookChannel>,
    > = None;

    // 启动 Channel 监听（DingTalk）
    if let Some(dt_cfg) = &cfg.channels.dingtalk {
        if dt_cfg.enabled {
            let dt_presentation = dt_cfg.presentation;
            if let Ok(dt_config) = crate::channels_internal::dingtalk::DingTalkConfig::from_env() {
                let channel = Arc::new(crate::channels_internal::DingTalkChannel::new(
                    dt_config,
                    cfg.gateway.require_mention_in_groups,
                ));
                cron_channel_map.register(
                    "dingtalk",
                    Option::<String>::None,
                    channel.clone() as Arc<dyn crate::channels_internal::Channel>,
                    true,
                );
                let registry_clone = registry.clone();
                let channel_clone = channel.clone();
                let delivery_channels = Arc::new(cron_channel_map.clone());
                let delivery_cfg = cfg_arc.clone();
                let (tx, mut rx) = tokio::sync::mpsc::channel(64);

                // 监听线程：接收 DingTalk 消息 → tx（带断线重连）
                tokio::spawn(async move {
                    let mut delay = Duration::from_secs(5);
                    loop {
                        match crate::channels_internal::Channel::listen(
                            channel.as_ref(),
                            tx.clone(),
                        )
                        .await
                        {
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
                        spawn_im_turn(
                            registry_clone.clone(),
                            channel_clone.clone() as Arc<dyn crate::channels_internal::Channel>,
                            delivery_channels.clone(),
                            delivery_cfg.clone(),
                            inbound,
                            dt_presentation,
                        );
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
            let lark_presentation = lark_cfg.presentation;
            let lark_trigger_policy = lark_cfg.resolved_trigger_policy(&cfg.gateway);
            let lark_instances: anyhow::Result<Vec<config::LarkInstanceConfig>> =
                if !lark_cfg.instances.is_empty() {
                    Ok(lark_cfg.instances.clone())
                } else {
                    let app_id = std::env::var("LARK_APP_ID");
                    let app_secret = std::env::var("LARK_APP_SECRET");
                    match (app_id, app_secret) {
                        (Ok(id), Ok(secret)) => Ok(vec![config::LarkInstanceConfig {
                            id: lark_cfg.default_instance_id().to_string(),
                            app_id: id,
                            app_secret: secret,
                            bot_name: None,
                        }]),
                        _ => Err(anyhow::anyhow!("LARK_APP_ID or LARK_APP_SECRET not set")),
                    }
                };
            match lark_instances {
                Ok(instances) => {
                    let known_lark_bot_mentions = instances
                        .iter()
                        .filter_map(|instance| {
                            instance.bot_name.as_ref().map(|bot_name| {
                                (
                                    bot_name.trim().trim_start_matches('@').to_lowercase(),
                                    instance.id.clone(),
                                )
                            })
                        })
                        .collect::<std::collections::HashMap<_, _>>();
                    let requested_default = lark_cfg.default_instance_id().to_string();
                    let has_requested_default = instances
                        .iter()
                        .any(|instance| instance.id == requested_default);
                    if !has_requested_default && instances.len() > 1 {
                        tracing::warn!(
                            default_instance = %requested_default,
                            "configured Lark default_instance not found; falling back to first instance"
                        );
                    }

                    let mut listeners = Vec::new();
                    for (index, instance) in instances.into_iter().enumerate() {
                        let is_default = (has_requested_default
                            && instance.id == requested_default)
                            || (!has_requested_default && index == 0);
                        let channel =
                            Arc::new(crate::channels_internal::LarkChannel::new_with_instance(
                                instance.id.clone(),
                                instance.bot_name.clone(),
                                instance.app_id,
                                instance.app_secret,
                                lark_trigger_policy,
                                is_default,
                                known_lark_bot_mentions.clone(),
                                is_default,
                            ));
                        cron_channel_map.register(
                            "lark",
                            Some(instance.id.clone()),
                            channel.clone() as Arc<dyn crate::channels_internal::Channel>,
                            is_default,
                        );
                        listeners.push((instance.id.clone(), channel, is_default));
                    }

                    let delivery_channels = Arc::new(cron_channel_map.clone());
                    for (instance_id, channel, is_default) in listeners {
                        let registry_clone = registry.clone();
                        let channel_clone = channel.clone();
                        let delivery_channels = delivery_channels.clone();
                        let delivery_cfg = cfg_arc.clone();
                        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
                        let listen_instance_id = instance_id.clone();

                        tokio::spawn(async move {
                            let mut delay = Duration::from_secs(5);
                            loop {
                                match crate::channels_internal::Channel::listen(
                                    channel.as_ref(),
                                    tx.clone(),
                                )
                                .await
                                {
                                    Err(e) => {
                                        delay = next_delay(delay);
                                        tracing::error!(
                                            instance = %listen_instance_id,
                                            "Lark listen error (retry in {:?}): {e}",
                                            delay
                                        );
                                    }
                                    Ok(()) => {
                                        delay = Duration::from_secs(5);
                                        tracing::info!(
                                            instance = %listen_instance_id,
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
                                spawn_im_turn(
                                    registry_clone.clone(),
                                    channel_clone.clone()
                                        as Arc<dyn crate::channels_internal::Channel>,
                                    delivery_channels.clone(),
                                    delivery_cfg.clone(),
                                    inbound,
                                    lark_presentation,
                                );
                            }
                        });

                        tracing::info!(
                            instance = %instance_id,
                            default = is_default,
                            "Lark channel instance started"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("Lark enabled but credentials not set: {e}");
                }
            }
        }
    }

    // 启动 Channel 监听（WeChat）
    let mut wechat_cancel_token: Option<tokio_util::sync::CancellationToken> = None;
    if let Some(wx_cfg) = &cfg.channels.wechat {
        if wx_cfg.enabled {
            let wx_presentation = wx_cfg.presentation;
            match crate::channels_internal::WeChatConfig::load() {
                Ok(wx_config) => {
                    let wx_cancel = tokio_util::sync::CancellationToken::new();
                    wechat_cancel_token = Some(wx_cancel.clone());
                    let wx_cancel_clone = wx_cancel.clone();
                    let channel = Arc::new(crate::channels_internal::WeChatChannel::new(
                        wx_config,
                        cfg.gateway.require_mention_in_groups,
                        wx_cancel,
                    ));
                    cron_channel_map.register(
                        "wechat",
                        Option::<String>::None,
                        channel.clone() as Arc<dyn crate::channels_internal::Channel>,
                        true,
                    );
                    let registry_clone = registry.clone();
                    let channel_clone = channel.clone();
                    let delivery_channels = Arc::new(cron_channel_map.clone());
                    let delivery_cfg = cfg_arc.clone();
                    let (tx, mut rx) = tokio::sync::mpsc::channel(64);

                    tokio::spawn(async move {
                        let mut delay = Duration::from_secs(5);
                        loop {
                            match crate::channels_internal::Channel::listen(
                                channel.as_ref(),
                                tx.clone(),
                            )
                            .await
                            {
                                Err(e) => {
                                    delay = next_delay(delay);
                                    tracing::error!(
                                        "WeChat listen error (retry in {:?}): {e}",
                                        delay
                                    );
                                }
                                Ok(()) => {
                                    delay = Duration::from_secs(5);
                                    tracing::info!(
                                        "WeChat long-poll ended normally, reconnecting in {:?}",
                                        delay
                                    );
                                }
                            }
                            tokio::select! {
                                _ = tokio::time::sleep(delay) => {},
                                _ = wx_cancel_clone.cancelled() => break,
                            }
                        }
                    });

                    tokio::spawn(async move {
                        while let Some(inbound) = rx.recv().await {
                            spawn_im_turn(
                                registry_clone.clone(),
                                channel_clone.clone() as Arc<dyn crate::channels_internal::Channel>,
                                delivery_channels.clone(),
                                delivery_cfg.clone(),
                                inbound,
                                wx_presentation,
                            );
                        }
                    });

                    tracing::info!("WeChat channel started");
                }
                Err(e) => {
                    tracing::warn!("WeChat enabled but credentials not available: {e}");
                }
            }
        }
    }

    if let Some(webhook_cfg) = &cfg.channels.dingtalk_webhook {
        if webhook_cfg.enabled {
            let channel = Arc::new(crate::channels_internal::DingTalkWebhookChannel::new(
                webhook_cfg.clone(),
            ));
            cron_channel_map.register(
                "dingtalk_webhook",
                Option::<String>::None,
                channel.clone() as Arc<dyn crate::channels_internal::Channel>,
                true,
            );
            tracing::info!(
                path = %channel.webhook_path(),
                "DingTalk custom robot webhook channel enabled"
            );
            dingtalk_webhook_channel = Some(channel);
        }
    }

    let (scheduler_service, scheduler_db) =
        scheduler_runtime::build_scheduler_service(cfg_arc.as_ref()).await?;
    scheduler_service.set_dashboard_sender(dashboard_tx);

    let cron_channel_map = Arc::new(cron_channel_map);
    let config_path = Arc::new(crate::config::config_file_path());
    let state = AppState {
        registry: registry.clone(),
        runtime_registry: Arc::clone(&runtime_registry),
        event_tx: event_tx.clone(),
        cfg: cfg_arc.clone(),
        channel_registry: cron_channel_map.clone(),
        dingtalk_webhook_channel,
        runtime_token: Arc::new(uuid::Uuid::new_v4().to_string()),
        approvals,
        scheduler_service: scheduler_service.clone(),
        config_path,
    };
    spawn_dashboard_diagnostics_poller(state.clone());

    // Approval notify loop: surface runtime approval requests back into the originating IM
    // session so operators can resolve them with `/approve <id> <decision>`.
    {
        let mut approval_rx = event_tx.subscribe();
        let channels_for_approval = cron_channel_map.clone();
        let cfg_for_approval = state.cfg.clone();
        tokio::spawn(async move {
            while let Ok(event) = approval_rx.recv().await {
                let crate::protocol::AgentEvent::ApprovalRequest {
                    session_key,
                    approval_id,
                    prompt,
                    command,
                    ..
                } = event
                else {
                    continue;
                };
                if session_key.channel == "ws" {
                    continue;
                }
                let resolved = resolve_delivery(
                    cfg_for_approval.as_ref(),
                    channels_for_approval.as_ref(),
                    config::DeliveryPurposeConfig::Approval,
                    &session_key,
                    None,
                    None,
                    None,
                    None,
                    None,
                );
                let Some(resolved) = resolved else {
                    continue;
                };
                let header = command
                    .as_deref()
                    .map(|cmd| format!("审批请求：`{cmd}`"))
                    .unwrap_or_else(|| "审批请求".to_string());
                let body = format!(
                    "{header}\n{prompt}\n\n审批 ID: `{approval_id}`\n回复命令：`/approve {approval_id} allow-once`\n或：`/approve {approval_id} allow-always`\n或：`/approve {approval_id} deny`"
                );
                let outbound = resolved.outbound_text(body);
                if let Err(e) = resolved.sender.send(&outbound).await {
                    tracing::error!(
                        channel = %session_key.channel,
                        scope = %session_key.scope,
                        approval_id = %approval_id,
                        "Approval notify send error: {e}"
                    );
                }
            }
            tracing::error!("Approval notify task exited unexpectedly; shutting down");
            std::process::exit(1);
        });
        tracing::info!("Approval notify task started");
    }

    // ── Swarm Wiring ─────────────────────────────────────────────────────────
    // Wire RelayEngine: synchronous [RELAY: @agent <cmd>] delegation (C1 + I2).
    // MsgSource::Relay is set on the inner InboundMsg so Hook 3 (MentionTrigger)
    // does not recurse into the relay result.
    {
        let registry_for_relay = registry.clone();
        let relay_dispatch: crate::agent_core::relay::RelayDispatchFn =
            Box::new(move |target_agent, content, lead_session_key| {
                let registry = registry_for_relay.clone();
                Box::pin(async move {
                    // DEADLOCK FIX (C-2): Relay Specialist MUST NOT reuse the Lead's session_key.
                    //
                    // Lead's handle() holds Semaphore(1) for lead_session_key throughout its entire
                    // execution. Hook 2 (Relay) fires after engine.run() but before the permit drops.
                    // If we called registry.handle() with the same session_key, the inner handle()
                    // would block forever waiting for the same Semaphore(1) → deadlock.
                    //
                    // Fix: give the Relay Specialist a dedicated session_key:
                    //   channel = "relay"
                    //   scope   = "{lead_scope}:{agent_name}"
                    // This ensures an independent Semaphore and independent session history.
                    let agent_bare = target_agent.trim_start_matches('@');
                    let relay_scope = format!("{}:{}", lead_session_key.scope, agent_bare);
                    let relay_session_key = crate::protocol::SessionKey::new("relay", &relay_scope);

                    let msg = crate::protocol::InboundMsg {
                        id: uuid::Uuid::new_v4().to_string(),
                        session_key: relay_session_key,
                        content,
                        sender: "relay".to_string(),
                        channel: "relay".to_string(),
                        timestamp: chrono::Utc::now(),
                        thread_ts: None,
                        target_agent: Some(target_agent),
                        source: crate::protocol::MsgSource::Relay,
                    };
                    registry
                        .handle_with_context(msg, TurnExecutionContext::default())
                        .await
                })
            });
        registry.set_relay_engine(std::sync::Arc::new(
            crate::agent_core::relay::RelayEngine::new(relay_dispatch),
        ));
        tracing::info!("RelayEngine wired");
    }

    // Wire MentionTrigger: scan bot output for @botname, re-inject as BotMention (C1).
    let (redispatch_tx, mut redispatch_rx) =
        tokio::sync::mpsc::channel::<crate::protocol::InboundMsg>(256);
    {
        let bot_names: Vec<String> = cfg.agent_roster.iter().map(|e| e.name.clone()).collect();
        if !bot_names.is_empty() {
            let trigger = std::sync::Arc::new(
                crate::channels_internal::mention_trigger::MentionTrigger::new(
                    bot_names,
                    redispatch_tx.clone(),
                ),
            );
            registry.set_mention_trigger(trigger);
            tracing::info!("MentionTrigger wired ({} bots)", cfg.agent_roster.len());
        }
    }

    crate::team_runtime::wire_team_runtime(
        registry.clone(),
        &cfg,
        cron_channel_map.clone(),
        Duration::from_secs(60),
    )
    .await?;

    // Spawn BotMention redispatch task: MentionTrigger → handle() → IM reply (C1).
    {
        let registry_for_redispatch = registry.clone();
        let channels_for_redispatch = cron_channel_map.clone();
        let cfg_for_redispatch = state.cfg.clone();
        tokio::spawn(async move {
            while let Some(inbound) = redispatch_rx.recv().await {
                let session_key = inbound.session_key.clone();
                let thread_ts = inbound.thread_ts.clone();
                let reply_to = Some(inbound.id.clone());
                let turn_ctx = TurnExecutionContext {
                    delivery_source: Some(
                        TurnDeliverySource::from_session_key(&session_key)
                            .with_reply_context(reply_to.clone(), thread_ts.clone()),
                    ),
                };
                match registry_for_redispatch
                    .handle_with_context(inbound, turn_ctx)
                    .await
                {
                    Ok(Some(reply)) => {
                        if let Some(resolved) = resolve_delivery(
                            cfg_for_redispatch.as_ref(),
                            channels_for_redispatch.as_ref(),
                            config::DeliveryPurposeConfig::BotMention,
                            &session_key,
                            Some(
                                &TurnDeliverySource::from_session_key(&session_key)
                                    .with_reply_context(reply_to.clone(), thread_ts.clone()),
                            ),
                            None,
                            None,
                            reply_to.as_deref(),
                            thread_ts.as_deref(),
                        ) {
                            let outbound = resolved.outbound_text(reply);
                            if let Err(e) = resolved.sender.send(&outbound).await {
                                tracing::error!("BotMention redispatch send error: {e}");
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => tracing::error!("BotMention redispatch handle error: {e}"),
                }
            }
            tracing::error!("BotMention redispatch task exited unexpectedly; shutting down");
            std::process::exit(1);
        });
        tracing::info!("BotMention redispatch task started");
    }

    // ── End Swarm Wiring ─────────────────────────────────────────────────────

    scheduler_runtime::spawn_scheduler_runtime(
        scheduler_service,
        registry.clone(),
        memory_system_ref.clone(),
        cron_channel_map.clone(),
        state.cfg.clone(),
    );
    tracing::info!("Scheduler runtime started (db={:?})", scheduler_db);

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
                        ms_clone.emit(crate::agent_core::MemoryEvent::NightlyConsolidation {
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
    let runtime_token = state.runtime_token.clone();
    let addr = gateway::server::start(state.clone(), &cfg.gateway.host, cfg.gateway.port).await?;
    registry.set_team_tool_url(format!(
        "http://127.0.0.1:{}/runtime/team-tools?token={}",
        addr.port(),
        runtime_token
    ));

    // 写端口到 ~/.clawbro/gateway.port（供 Tauri 壳读取）
    let port_file = dirs::home_dir()
        .unwrap_or_default()
        .join(".clawbro")
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

    // Cancel WeChat listen loop for graceful shutdown
    if let Some(token) = &wechat_cancel_token {
        token.cancel();
        tracing::info!("WeChat cancellation token cancelled");
    }

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
