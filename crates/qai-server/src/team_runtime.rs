use crate::config::{GatewayConfig, InteractionMode};
use anyhow::Result;
use qai_agent::SessionRegistry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

pub async fn wire_team_runtime(
    registry: Arc<SessionRegistry>,
    cfg: &GatewayConfig,
    channel_map: Arc<HashMap<String, Arc<dyn qai_channels::Channel>>>,
    heartbeat_interval: Duration,
) -> Result<()> {
    use qai_agent::team::{
        heartbeat::DispatchFn, orchestrator::TeamOrchestrator, registry::TaskRegistry,
        session::TeamSession,
    };

    let (team_notify_tx, mut team_notify_rx) = mpsc::channel::<qai_protocol::InboundMsg>(256);
    let team_notify_tx_for_orch = team_notify_tx.clone();

    let team_groups: Vec<_> = cfg
        .groups
        .iter()
        .filter(|g| matches!(g.mode.interaction, InteractionMode::Team))
        .collect();

    for group in team_groups {
        let team_id: String = group
            .scope
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();

        let session = match TeamSession::new(&group.scope, &team_id) {
            Ok(s) => Arc::new(s),
            Err(e) => {
                tracing::error!(scope = %group.scope, "Failed to create TeamSession: {e:#}");
                continue;
            }
        };
        let db_path = session.dir.join("tasks.db");
        let task_registry = match TaskRegistry::new(db_path.to_str().unwrap_or(":memory:")) {
            Ok(r) => Arc::new(r),
            Err(e) => {
                tracing::error!(scope = %group.scope, "Failed to open TaskRegistry: {e:#}");
                continue;
            }
        };

        let registry_for_dispatch = Arc::clone(&registry);
        let task_reg_for_dispatch = Arc::clone(&task_registry);
        let team_session_for_dispatch = Arc::clone(&session);
        let dispatch_fn: DispatchFn = Arc::new(move |agent: String, task| {
            let registry = Arc::clone(&registry_for_dispatch);
            let task_reg = Arc::clone(&task_reg_for_dispatch);
            let team_session = Arc::clone(&team_session_for_dispatch);
            Box::pin(async move {
                let specialist_key = team_session.specialist_session_key(&agent);
                let specialist_channel = specialist_key.channel.clone();
                let reminder = team_session.build_task_reminder(&task, &task_reg);
                registry.set_task_reminder(specialist_key.clone(), reminder);
                let msg = qai_protocol::InboundMsg {
                    id: uuid::Uuid::new_v4().to_string(),
                    session_key: specialist_key,
                    content: qai_protocol::MsgContent::text(
                        task.spec.as_deref().unwrap_or(&task.title),
                    ),
                    sender: "orchestrator".to_string(),
                    channel: specialist_channel,
                    timestamp: chrono::Utc::now(),
                    thread_ts: None,
                    target_agent: Some(format!("@{}", agent)),
                    source: qai_protocol::MsgSource::Heartbeat,
                };
                let result = registry.handle(msg).await;
                if let Ok(Some(ref reply_text)) = result {
                    let _ = team_session.append_specialist_reply(&agent, &task.id, reply_text);
                }
                result.map(|_| ())
            })
        });

        let team_orch = TeamOrchestrator::new(
            task_registry,
            Arc::clone(&session),
            dispatch_fn,
            heartbeat_interval,
        );

        let channels_for_notify = Arc::clone(&channel_map);
        team_orch.set_notify_fn(Arc::new(
            move |scope: qai_protocol::SessionKey, msg: String| {
                let channels = Arc::clone(&channels_for_notify);
                tokio::spawn(async move {
                    if let Some(ch) = channels.get(&scope.channel) {
                        let outbound = qai_protocol::OutboundMsg {
                            session_key: scope,
                            content: qai_protocol::MsgContent::text(msg),
                            reply_to: None,
                            thread_ts: None,
                        };
                        if let Err(e) = ch.send(&outbound).await {
                            tracing::error!("Milestone notify send error: {e}");
                        }
                    }
                });
            },
        ));

        let channel_name: &str = if let Some(ref ch) = group.mode.channel {
            ch.as_str()
        } else if cfg
            .channels
            .dingtalk
            .as_ref()
            .map(|c| c.enabled)
            .unwrap_or(false)
        {
            "dingtalk"
        } else if cfg
            .channels
            .lark
            .as_ref()
            .map(|c| c.enabled)
            .unwrap_or(false)
        {
            "lark"
        } else {
            "ws"
        };
        let lead_key = qai_protocol::SessionKey::new(channel_name, &group.scope);
        team_orch.set_lead_session_key(lead_key.clone());
        team_orch.set_scope(lead_key);
        if let Some(front_bot) = &group.mode.front_bot {
            team_orch.set_lead_agent_name(front_bot.clone());
            tracing::info!(front_bot = %front_bot, scope = %group.scope, "Lead agent wired from front_bot");
        }
        if !group.team.roster.is_empty() {
            team_orch.set_available_specialists(group.team.roster.clone());
            tracing::info!(specialists = ?group.team.roster, scope = %group.scope, "Available specialists wired");
        }

        team_orch.set_team_notify_tx(team_notify_tx_for_orch.clone());

        match team_orch.start_mcp_server().await {
            Ok(()) => {
                tracing::info!(scope = %group.scope, team_id = %team_id, "SharedTeamMcpServer started")
            }
            Err(e) => {
                tracing::error!(scope = %group.scope, "Failed to start SharedTeamMcpServer: {e:#}")
            }
        }

        registry.register_team_orchestrator(team_id.clone(), team_orch);
        tracing::info!(scope = %group.scope, team_id = %team_id, "TeamOrchestrator registered");
    }

    for group in cfg.groups.iter().filter(|g| g.mode.auto_promote) {
        registry.add_auto_promote_scope(group.scope.clone());
        tracing::info!(scope = %group.scope, "auto_promote keyword detection enabled");
    }

    {
        let registry_for_notify = Arc::clone(&registry);
        tokio::spawn(async move {
            while let Some(inbound) = team_notify_rx.recv().await {
                match registry_for_notify.handle(inbound).await {
                    Ok(Some(_reply)) => {}
                    Ok(None) => {}
                    Err(e) => tracing::error!("TeamNotify handle error: {e}"),
                }
            }
        });
        tracing::info!("TeamNotify redispatch task started");
    }

    Ok(())
}
