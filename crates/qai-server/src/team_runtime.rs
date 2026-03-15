use crate::channel_registry::ChannelRegistry;
use crate::config::{GatewayConfig, InteractionMode};
use crate::delivery_resolver::resolve_delivery;
use anyhow::Result;
use qai_agent::team::completion_routing::{RoutingDeliveryStatus, TeamRoutingEnvelope};
use qai_agent::team::milestone::TeamMilestoneEvent;
use qai_agent::team::milestone_delivery::{milestone_dedupe_key, milestone_is_public};
use qai_agent::team::session::{ChannelSendSourceKind, ChannelSendStatus};
use qai_agent::{SessionRegistry, TurnExecutionContext};
use qai_protocol::{InboundMsg, MsgContent, MsgSource, SessionKey};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc;

fn default_channel_instance_for_scope(
    cfg: &GatewayConfig,
    channel_name: &str,
    scope: &str,
) -> Option<String> {
    if !scope.starts_with("user:") {
        return None;
    }
    match channel_name {
        "lark" => cfg
            .channels
            .lark
            .as_ref()
            .map(|lark| lark.default_instance_id().to_string()),
        _ => None,
    }
}

pub async fn wire_team_runtime(
    registry: Arc<SessionRegistry>,
    cfg: &GatewayConfig,
    channel_map: Arc<ChannelRegistry>,
    heartbeat_interval: Duration,
) -> Result<()> {
    use qai_agent::team::{
        completion_routing::{RoutingDeliveryStatus, TeamNotifyRequest},
        heartbeat::DispatchFn,
        orchestrator::TeamOrchestrator,
        registry::TaskRegistry,
        session::{stable_team_id_for_session_key, TeamSession},
    };

    let (team_notify_tx, mut team_notify_rx) = mpsc::channel::<TeamNotifyRequest>(256);
    let team_notify_tx_for_orch = team_notify_tx.clone();
    let team_scopes = cfg.normalized_team_scopes();
    let cfg_for_delivery = Arc::new(cfg.clone());
    tracing::info!(
        count = team_scopes.len(),
        "wire_team_runtime: team scopes found"
    );

    for team_scope in &team_scopes {
        tracing::info!(scope = %team_scope.scope, name = ?team_scope.name, "wire_team_runtime: wiring team scope");
        let channel_name: String = if let Some(ref ch) = team_scope.mode.channel {
            ch.clone()
        } else if cfg
            .channels
            .dingtalk
            .as_ref()
            .map(|c| c.enabled)
            .unwrap_or(false)
        {
            "dingtalk".to_string()
        } else if cfg
            .channels
            .lark
            .as_ref()
            .map(|c| c.enabled)
            .unwrap_or(false)
        {
            "lark".to_string()
        } else {
            "ws".to_string()
        };
        let lead_channel_instance =
            default_channel_instance_for_scope(cfg, &channel_name, &team_scope.scope);
        let lead_key = qai_protocol::SessionKey {
            channel: channel_name.clone(),
            channel_instance: lead_channel_instance.clone(),
            scope: team_scope.scope.clone(),
        };
        let team_id = stable_team_id_for_session_key(&lead_key);
        let session = match TeamSession::new(&team_scope.scope, &team_id) {
            Ok(s) => Arc::new(s),
            Err(e) => {
                tracing::error!(scope = %team_scope.scope, "Failed to create TeamSession: {e:#}");
                continue;
            }
        };
        let db_path = session.dir.join("tasks.db");
        let task_registry = match TaskRegistry::new(db_path.to_str().unwrap_or(":memory:")) {
            Ok(r) => Arc::new(r),
            Err(e) => {
                tracing::error!(scope = %team_scope.scope, "Failed to open TaskRegistry: {e:#}");
                continue;
            }
        };
        let registry_for_dispatch = Arc::clone(&registry);
        let task_reg_for_dispatch = Arc::clone(&task_registry);
        let team_session_for_dispatch = Arc::clone(&session);
        let dispatch_requester_key = lead_key.clone();
        let team_orch_for_dispatch: Arc<OnceLock<Arc<TeamOrchestrator>>> =
            Arc::new(OnceLock::new());
        let team_orch_for_dispatch_in_closure = Arc::clone(&team_orch_for_dispatch);
        let team_orch_for_milestone: Arc<OnceLock<Arc<TeamOrchestrator>>> =
            Arc::new(OnceLock::new());
        let dispatch_fn: DispatchFn = Arc::new(move |agent: String, task| {
            let registry = Arc::clone(&registry_for_dispatch);
            let task_reg = Arc::clone(&task_reg_for_dispatch);
            let team_session = Arc::clone(&team_session_for_dispatch);
            let requester_key = dispatch_requester_key.clone();
            let team_orch_cell = Arc::clone(&team_orch_for_dispatch_in_closure);
            Box::pin(async move {
                let specialist_key = team_session.specialist_session_key(&agent);
                let specialist_channel = specialist_key.channel.clone();
                let reminder = team_session.build_task_reminder(&task, &task_reg);
                registry.set_task_reminder(specialist_key.clone(), reminder);
                let dispatch_started_at = chrono::Utc::now();
                if let Some(team_orch) = team_orch_cell.get() {
                    team_orch.record_dispatch_start(
                        &task.id,
                        &agent,
                        requester_key.clone(),
                        None,
                        team_orch.lead_delivery_source(),
                    );
                }
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
                let result = registry
                    .handle_with_context(msg, TurnExecutionContext::default())
                    .await;
                let reply_excerpt = result.as_ref().ok().and_then(|reply| {
                    reply
                        .as_ref()
                        .map(|text| truncate_for_missing_completion(text, 240))
                });
                if let Ok(Some(ref reply_text)) = result {
                    let _ = team_session.append_specialist_reply(&agent, &task.id, reply_text);
                }
                if let Some(team_orch) = team_orch_cell.get() {
                    let outcome =
                        team_orch.classify_specialist_turn(&task.id, &agent, dispatch_started_at);
                    if matches!(
                        outcome,
                        qai_agent::team::specialist_turn::SpecialistTurnOutcome::MissingCompletion
                    ) {
                        team_orch.handle_specialist_missing_completion(
                            &task.id,
                            &agent,
                            reply_excerpt.as_deref(),
                        )?;
                        let session_id = registry
                            .session_manager_ref()
                            .get_or_create(&team_session.specialist_session_key(&agent))
                            .await?;
                        registry
                            .session_manager_ref()
                            .reset_conversation(session_id)
                            .await?;
                    }
                }
                if result.is_ok() {
                    if let Some(team_orch) = team_orch_cell.get() {
                        team_orch.notify_task_dispatched(&task.id, &task.title, &agent);
                    }
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
        let _ = team_orch_for_dispatch.set(Arc::clone(&team_orch));
        let _ = team_orch_for_milestone.set(Arc::clone(&team_orch));

        let channels_for_notify = Arc::clone(&channel_map);
        let session_for_notify = Arc::clone(&session);
        let public_updates_mode = team_scope.team.public_updates;
        let lead_agent_name_for_notify = team_scope.mode.front_bot.clone();
        let cfg_for_milestone = Arc::clone(&cfg_for_delivery);
        let team_orch_for_milestone_in_closure = Arc::clone(&team_orch_for_milestone);
        team_orch.set_milestone_fn(Arc::new(move |scope: qai_protocol::SessionKey, event| {
            use qai_agent::team::milestone::render_for_im;
            if !milestone_is_public(&event, public_updates_mode) {
                tracing::debug!(
                    scope = %scope.scope,
                    kind = %event.kind_str(),
                    "Suppressing internal-only team milestone from direct channel delivery"
                );
                return;
            }
            if let Some(dedupe_key) = milestone_dedupe_key(&event) {
                match session_for_notify.mark_delivery_dedupe(&scope.scope, &dedupe_key) {
                    Ok(true) => {}
                    Ok(false) => {
                        let _ = session_for_notify
                            .record_delivery_dedupe_hit(&scope.scope, &dedupe_key);
                        tracing::debug!(
                            scope = %scope.scope,
                            kind = %event.kind_str(),
                            "Suppressing duplicate team milestone channel delivery"
                        );
                        return;
                    }
                    Err(err) => {
                        tracing::warn!(
                            scope = %scope.scope,
                            kind = %event.kind_str(),
                            error = %err,
                            "Failed to persist milestone delivery dedupe key"
                        );
                    }
                }
            }
            let msg = render_for_im(&event);
            let channels = Arc::clone(&channels_for_notify);
            let session_for_record = Arc::clone(&session_for_notify);
            let lead_agent_name = lead_agent_name_for_notify.clone();
            let dedupe_key_for_record = milestone_dedupe_key(&event);
            let cfg = Arc::clone(&cfg_for_milestone);
            let team_orch_cell = Arc::clone(&team_orch_for_milestone_in_closure);
            tokio::spawn(async move {
                let stored_source = team_orch_cell
                    .get()
                    .and_then(|team_orch| team_orch.lead_delivery_source());
                let (source_kind, source_agent) =
                    milestone_channel_origin(&event, lead_agent_name.as_deref());
                let resolved = resolve_delivery(
                    cfg.as_ref(),
                    channels.as_ref(),
                    milestone_delivery_purpose(&event),
                    &scope,
                    None,
                    stored_source.as_ref(),
                    Some(&source_agent),
                    None,
                    None,
                );
                let (outbound, send_result, sender_channel_instance) =
                    if let Some(resolved) = resolved {
                        let sender_channel_instance = resolved.sender_channel_instance.clone();
                        let outbound = resolved.outbound_text(&msg);
                        let send_result = resolved.sender.send(&outbound).await;
                        (outbound, send_result, sender_channel_instance)
                    } else if let Some(ch) = channels.resolve_for_session(&scope) {
                        let outbound = qai_protocol::OutboundMsg {
                            session_key: scope.clone(),
                            content: qai_protocol::MsgContent::text(msg),
                            reply_to: stored_source
                                .as_ref()
                                .and_then(|source| source.reply_to.clone()),
                            thread_ts: stored_source
                                .as_ref()
                                .and_then(|source| source.thread_ts.clone()),
                        };
                        let send_result = ch.send(&outbound).await;
                        (outbound, send_result, None)
                    } else {
                        return;
                    };
                if let Err(e) = &send_result {
                    tracing::error!("Milestone notify send error: {e}");
                }
                let (status, error) = match send_result {
                    Ok(()) => (ChannelSendStatus::Sent, None),
                    Err(err) => (ChannelSendStatus::SendFailed, Some(err.to_string())),
                };
                if let Err(err) = session_for_record.record_channel_send(
                    &outbound.session_key.channel,
                    sender_channel_instance.as_deref(),
                    outbound.session_key.channel_instance.as_deref(),
                    &outbound.session_key.scope,
                    None,
                    stored_source.as_ref(),
                    outbound.reply_to.as_deref(),
                    outbound.thread_ts.as_deref(),
                    source_kind,
                    &source_agent,
                    milestone_task_id(&event),
                    dedupe_key_for_record.as_deref(),
                    outbound.content.as_text().unwrap_or_default(),
                    status,
                    error.as_deref(),
                ) {
                    tracing::warn!(
                        team_id = %session_for_record.team_id,
                        error = %err,
                        "Failed to append milestone channel send ledger entry"
                    );
                }
            });
        }));

        team_orch.set_lead_session_key(lead_key.clone());
        team_orch.set_scope(lead_key);
        if let Some(front_bot) = &team_scope.mode.front_bot {
            team_orch.set_lead_agent_name(front_bot.clone());
            tracing::info!(front_bot = %front_bot, scope = %team_scope.scope, "Lead agent wired from front_bot");
        }
        if !team_scope.team.roster.is_empty() {
            team_orch.set_available_specialists(team_scope.team.roster.clone());
            tracing::info!(specialists = ?team_scope.team.roster, scope = %team_scope.scope, "Available specialists wired");
        }
        team_orch.set_max_parallel(team_scope.team.max_parallel);
        tracing::info!(
            scope = %team_scope.scope,
            max_parallel = team_scope.team.max_parallel,
            "team dispatch limit wired"
        );

        team_orch.set_team_notify_tx(team_notify_tx_for_orch.clone());

        team_orch.start_mcp_server().await.map_err(|e| {
            anyhow::anyhow!(
                "failed to start SharedTeamMcpServer for scope '{}' (team '{}'): {e:#}",
                team_scope.scope,
                team_id
            )
        })?;
        tracing::info!(scope = %team_scope.scope, team_id = %team_id, "SharedTeamMcpServer started");

        registry.register_team_orchestrator(team_id.clone(), team_orch);
        tracing::info!(scope = %team_scope.scope, team_id = %team_id, "TeamOrchestrator registered");
    }

    for group in cfg.groups.iter().filter(|g| g.mode.auto_promote) {
        registry.add_auto_promote_scope(group.scope.clone());
        tracing::info!(scope = %group.scope, "auto_promote keyword detection enabled");
    }

    for group in cfg
        .groups
        .iter()
        .filter(|group| !matches!(group.mode.interaction, InteractionMode::Team))
    {
        if let Some(front_bot) = &group.mode.front_bot {
            registry.register_scope_binding_with_channel(
                group.mode.channel.clone(),
                group.scope.clone(),
                front_bot.clone(),
            );
            tracing::info!(scope = %group.scope, front_bot = %front_bot, "scope binding registered");
        }
    }

    for team_scope in &team_scopes {
        if let Some(front_bot) = &team_scope.mode.front_bot {
            registry.register_scope_binding_with_channel(
                team_scope.mode.channel.clone(),
                team_scope.scope.clone(),
                front_bot.clone(),
            );
            tracing::info!(scope = %team_scope.scope, front_bot = %front_bot, "team scope binding registered");
        }
    }

    for binding in &cfg.bindings {
        registry.register_binding(binding.to_binding_rule());
        tracing::info!(agent = %binding.agent_name(), kind = ?binding, "routing binding registered");
    }

    {
        let registry_for_notify = Arc::clone(&registry);
        tokio::spawn(async move {
            while let Some(request) = team_notify_rx.recv().await {
                let text = request.envelope.event.render_for_parent();
                let mut delivered = None;

                for (attempt_index, target) in routing_attempt_targets(&request.envelope)
                    .into_iter()
                    .enumerate()
                {
                    let busy = registry_for_notify.is_session_busy(&target);
                    let turn_ctx = team_notify_turn_context(&request.envelope, &target);
                    let inbound = team_notify_inbound(
                        &target,
                        &text,
                        turn_ctx
                            .delivery_source
                            .as_ref()
                            .and_then(|source| source.thread_ts.clone()),
                    );
                    match registry_for_notify
                        .handle_with_context(inbound, turn_ctx)
                        .await
                    {
                        Ok(Some(_)) | Ok(None) => {
                            delivered = Some(request.envelope.clone().with_delivery_status(
                                delivery_status_for_attempt(attempt_index, busy),
                            ));
                            break;
                        }
                        Err(e) => {
                            let requester_scope = request
                                .envelope
                                .requester_session_key
                                .as_ref()
                                .map(|key| key.scope.as_str())
                                .unwrap_or("<none>");
                            tracing::warn!(
                                team_id = %request.envelope.team_id,
                                requester = %requester_scope,
                                target = %target.scope,
                                attempt_index,
                                "TeamNotify delivery attempt failed: {e}"
                            );
                        }
                    }
                }

                if let Some(team_orch) =
                    registry_for_notify.get_team_orchestrator(&request.envelope.team_id)
                {
                    if let Some(delivered) = delivered {
                        team_orch.mark_routing_event_delivered(&delivered);
                    } else {
                        let pending = request
                            .envelope
                            .clone()
                            .with_delivery_status(RoutingDeliveryStatus::PersistedPending);
                        team_orch.persist_pending_routing_event(pending);
                    }
                }
            }
        });
        tracing::info!("TeamNotify redispatch task started");
    }

    Ok(())
}

fn milestone_channel_origin(
    event: &TeamMilestoneEvent,
    lead_agent_name: Option<&str>,
) -> (ChannelSendSourceKind, String) {
    match event {
        TeamMilestoneEvent::LeadMessage { .. } => (
            ChannelSendSourceKind::LeadText,
            lead_agent_name.unwrap_or("leader").to_string(),
        ),
        TeamMilestoneEvent::TaskDispatched { agent, .. }
        | TeamMilestoneEvent::TaskCheckpoint { agent, .. }
        | TeamMilestoneEvent::TaskSubmitted { agent, .. }
        | TeamMilestoneEvent::TaskBlocked { agent, .. }
        | TeamMilestoneEvent::TaskDone { agent, .. } => {
            (ChannelSendSourceKind::Milestone, agent.clone())
        }
        TeamMilestoneEvent::TaskFailed { agent, .. } => {
            (ChannelSendSourceKind::Milestone, agent.clone())
        }
        TeamMilestoneEvent::TasksUnlocked { .. } | TeamMilestoneEvent::AllTasksDone => {
            (ChannelSendSourceKind::Milestone, "team-runtime".to_string())
        }
    }
}

fn milestone_delivery_purpose(event: &TeamMilestoneEvent) -> crate::config::DeliveryPurposeConfig {
    match event {
        TeamMilestoneEvent::LeadMessage { .. } => crate::config::DeliveryPurposeConfig::LeadMessage,
        _ => crate::config::DeliveryPurposeConfig::Milestone,
    }
}

fn milestone_task_id(event: &TeamMilestoneEvent) -> Option<&str> {
    match event {
        TeamMilestoneEvent::TaskDispatched { task_id, .. }
        | TeamMilestoneEvent::TaskCheckpoint { task_id, .. }
        | TeamMilestoneEvent::TaskSubmitted { task_id, .. }
        | TeamMilestoneEvent::TaskBlocked { task_id, .. }
        | TeamMilestoneEvent::TaskFailed { task_id, .. }
        | TeamMilestoneEvent::TaskDone { task_id, .. } => Some(task_id.as_str()),
        TeamMilestoneEvent::TasksUnlocked { .. }
        | TeamMilestoneEvent::AllTasksDone
        | TeamMilestoneEvent::LeadMessage { .. } => None,
    }
}

fn truncate_for_missing_completion(text: &str, max_chars: usize) -> String {
    let mut truncated = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        truncated.push_str("...");
    }
    truncated
}

fn routing_attempt_targets(envelope: &TeamRoutingEnvelope) -> Vec<SessionKey> {
    let mut targets = Vec::with_capacity(1 + envelope.fallback_session_keys.len());
    if let Some(requester) = &envelope.requester_session_key {
        targets.push(requester.clone());
    }
    for key in &envelope.fallback_session_keys {
        if !targets.contains(key) {
            targets.push(key.clone());
        }
    }
    targets
}

fn delivery_status_for_attempt(attempt_index: usize, busy: bool) -> RoutingDeliveryStatus {
    if attempt_index > 0 {
        RoutingDeliveryStatus::FallbackRedirected
    } else if busy {
        RoutingDeliveryStatus::QueuedDelivered
    } else {
        RoutingDeliveryStatus::DirectDelivered
    }
}

fn team_notify_inbound(target: &SessionKey, text: &str, thread_ts: Option<String>) -> InboundMsg {
    InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: target.clone(),
        content: MsgContent::text(text),
        sender: "gateway".to_string(),
        channel: target.channel.clone(),
        timestamp: chrono::Utc::now(),
        thread_ts,
        target_agent: None,
        source: MsgSource::TeamNotify,
    }
}

fn team_notify_turn_context(
    envelope: &TeamRoutingEnvelope,
    target: &SessionKey,
) -> TurnExecutionContext {
    let delivery_source = envelope
        .delivery_source
        .as_ref()
        .filter(|source| source.session_key() == *target)
        .cloned();
    TurnExecutionContext { delivery_source }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ChannelsSection, GatewayConfig, LarkSection, ProgressPresentationMode};
    use qai_agent::team::completion_routing::{RoutingDeliveryStatus, TeamRoutingEvent};
    use qai_agent::team::milestone::TeamMilestoneEvent;
    use qai_agent::team::milestone_delivery::{milestone_is_public, TeamPublicUpdatesMode};
    use qai_agent::team::session::stable_team_id_for_session_key;

    #[test]
    fn routing_attempt_targets_dedupes_requester_and_fallbacks() {
        let requester = SessionKey::new("ws", "group:req");
        let lead = SessionKey::new("ws", "group:lead");
        let envelope = TeamRoutingEnvelope {
            run_id: "run-1".into(),
            parent_run_id: None,
            requester_session_key: Some(requester.clone()),
            fallback_session_keys: vec![requester.clone(), lead.clone(), lead.clone()],
            team_id: "team-1".into(),
            delivery_status: RoutingDeliveryStatus::NotRouted,
            event: TeamRoutingEvent::failed("T001", "boom"),
            delivery_source: None,
        };

        let targets = routing_attempt_targets(&envelope);
        assert_eq!(targets, vec![requester, lead]);
    }

    #[test]
    fn routing_attempt_targets_allow_fallback_only_delivery() {
        let lead = SessionKey::new("ws", "group:lead");
        let envelope = TeamRoutingEnvelope {
            run_id: "run-1".into(),
            parent_run_id: None,
            requester_session_key: None,
            fallback_session_keys: vec![lead.clone()],
            team_id: "team-1".into(),
            delivery_status: RoutingDeliveryStatus::NotRouted,
            event: TeamRoutingEvent::failed("T001", "boom"),
            delivery_source: None,
        };

        let targets = routing_attempt_targets(&envelope);
        assert_eq!(targets, vec![lead]);
    }

    #[test]
    fn delivery_status_uses_fallback_redirected_for_secondary_target() {
        assert_eq!(
            delivery_status_for_attempt(0, false),
            RoutingDeliveryStatus::DirectDelivered
        );
        assert_eq!(
            delivery_status_for_attempt(0, true),
            RoutingDeliveryStatus::QueuedDelivered
        );
        assert_eq!(
            delivery_status_for_attempt(1, false),
            RoutingDeliveryStatus::FallbackRedirected
        );
    }

    #[test]
    fn public_milestone_visibility_respects_mode() {
        assert!(milestone_is_public(
            &TeamMilestoneEvent::LeadMessage {
                text: "hello".into()
            },
            TeamPublicUpdatesMode::Minimal
        ));
        assert!(!milestone_is_public(
            &TeamMilestoneEvent::AllTasksDone,
            TeamPublicUpdatesMode::Minimal
        ));
        assert!(milestone_is_public(
            &TeamMilestoneEvent::AllTasksDone,
            TeamPublicUpdatesMode::Normal
        ));
        assert!(!milestone_is_public(
            &TeamMilestoneEvent::TaskDone {
                task_id: "T1".into(),
                task_title: "task".into(),
                agent: "worker".into(),
                done_count: 1,
                total: 1,
            },
            TeamPublicUpdatesMode::Normal
        ));
        assert!(milestone_is_public(
            &TeamMilestoneEvent::TaskFailed {
                task_id: "T1".into(),
                agent: "worker".into(),
                reason: "boom".into(),
            },
            TeamPublicUpdatesMode::Normal
        ));
    }

    #[test]
    fn dm_team_identity_uses_default_lark_instance() {
        let cfg = GatewayConfig {
            channels: ChannelsSection {
                lark: Some(LarkSection {
                    enabled: true,
                    presentation: ProgressPresentationMode::FinalOnly,
                    trigger_policy: None,
                    default_instance: Some("default".into()),
                    instances: vec![],
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let lead_instance =
            default_channel_instance_for_scope(&cfg, "lark", "user:ou_test").expect("instance");
        let registered = SessionKey::with_instance("lark", &lead_instance, "user:ou_test");
        let inbound = SessionKey::with_instance("lark", "default", "user:ou_test");

        assert_eq!(
            stable_team_id_for_session_key(&registered),
            stable_team_id_for_session_key(&inbound)
        );
    }

    #[test]
    fn group_team_identity_does_not_force_default_lark_instance() {
        let cfg = GatewayConfig {
            channels: ChannelsSection {
                lark: Some(LarkSection {
                    enabled: true,
                    presentation: ProgressPresentationMode::FinalOnly,
                    trigger_policy: None,
                    default_instance: Some("default".into()),
                    instances: vec![],
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            default_channel_instance_for_scope(&cfg, "lark", "group:oc_test"),
            None
        );
    }
}
