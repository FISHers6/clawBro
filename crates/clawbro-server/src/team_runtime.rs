use crate::agent_core::team::completion_routing::{
    PendingRoutingRecord, ReviewAttemptDiagnostic, ReviewFailureClassification,
    RoutingDeliveryStatus, TeamRoutingEnvelope,
};
use crate::agent_core::team::milestone::TeamMilestoneEvent;
use crate::agent_core::team::milestone_delivery::{milestone_dedupe_key, milestone_is_public};
use crate::agent_core::team::registry::TaskStatus;
use crate::agent_core::team::session::{ChannelSendSourceKind, ChannelSendStatus, TeamSession};
use crate::agent_core::{SessionRegistry, TurnExecutionContext};
use crate::channel_registry::ChannelRegistry;
use crate::config::{GatewayConfig, InteractionMode};
use crate::delivery_resolver::resolve_delivery;
use crate::protocol::{DashboardEvent, InboundMsg, MsgContent, MsgSource, OutboundMsg, SessionKey};
use anyhow::Result;
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

async fn send_with_reply_fallback(
    channel: &Arc<dyn crate::channels_internal::Channel>,
    outbound: OutboundMsg,
) -> (OutboundMsg, anyhow::Result<()>) {
    match channel.send(&outbound).await {
        Ok(()) => (outbound, Ok(())),
        Err(error) if outbound.reply_to.is_some() => {
            tracing::warn!(
                channel = %channel.name(),
                reply_to = outbound.reply_to.as_deref().unwrap_or(""),
                error = %error,
                "milestone reply_to send failed; retrying as direct scope send"
            );
            let mut fallback = outbound.clone();
            fallback.reply_to = None;
            match channel.send(&fallback).await {
                Ok(()) => (fallback, Ok(())),
                Err(fallback_error) => (fallback, Err(fallback_error)),
            }
        }
        Err(error) => (outbound, Err(error)),
    }
}

pub async fn wire_team_runtime(
    registry: Arc<SessionRegistry>,
    cfg: &GatewayConfig,
    channel_map: Arc<ChannelRegistry>,
    heartbeat_interval: Duration,
) -> Result<()> {
    use crate::agent_core::team::{
        completion_routing::{RoutingDeliveryStatus, TeamNotifyRequest},
        heartbeat::DispatchFn,
        orchestrator::TeamOrchestrator,
        registry::TaskRegistry,
        session::{stable_team_id_for_session_key, TeamSession},
    };

    let (team_notify_tx, mut team_notify_rx) = mpsc::channel::<TeamNotifyRequest>(256);
    let team_notify_tx_for_orch = team_notify_tx.clone();
    let team_scopes = cfg.normalized_team_scopes();
    let mut review_retry_orchestrators = Vec::new();
    let cfg_for_delivery = Arc::new(cfg.clone());
    tracing::info!(
        count = team_scopes.len(),
        "wire_team_runtime: team scopes found"
    );

    for team_scope in &team_scopes {
        tracing::info!(scope = %team_scope.scope, name = ?team_scope.name, "wire_team_runtime: wiring team scope");
        let Some(channel_name) = team_scope.mode.channel.clone() else {
            tracing::error!(
                scope = %team_scope.scope,
                "team scope is missing mode.channel; skipping team runtime wiring for this scope"
            );
            continue;
        };
        let lead_channel_instance =
            default_channel_instance_for_scope(cfg, &channel_name, &team_scope.scope);
        let lead_key = crate::protocol::SessionKey {
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
        let registry_for_milestone = Arc::clone(&registry);
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
                let msg = crate::protocol::InboundMsg {
                    id: uuid::Uuid::new_v4().to_string(),
                    session_key: specialist_key,
                    content: crate::protocol::MsgContent::text(
                        team_session.build_task_dispatch_message(&task),
                    ),
                    sender: "orchestrator".to_string(),
                    channel: specialist_channel,
                    timestamp: chrono::Utc::now(),
                    thread_ts: None,
                    target_agent: Some(format!("@{}", agent)),
                    source: crate::protocol::MsgSource::Heartbeat,
                };
                let result = registry
                    .handle_with_context(msg, TurnExecutionContext::default())
                    .await;
                let specialist_session_id = registry
                    .session_manager_ref()
                    .get_or_create(&team_session.specialist_session_key(&agent))
                    .await?;
                let captured_reply_text = capture_specialist_reply_text(
                    registry.session_manager_ref().storage().as_ref(),
                    specialist_session_id,
                    &result,
                )
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!(
                        error = %error,
                        task_id = %task.id,
                        agent = %agent,
                        "failed to capture specialist reply text for team diagnostics"
                    );
                    None
                });
                let reply_excerpt =
                    missing_completion_excerpt(&result, captured_reply_text.as_deref());
                if let Some(ref reply_text) = captured_reply_text {
                    let _ = team_session.append_specialist_reply(&agent, &task.id, reply_text);
                }
                if let Some(team_orch) = team_orch_cell.get() {
                    let outcome =
                        team_orch.classify_specialist_turn(&task.id, &agent, dispatch_started_at);
                    if matches!(
                        outcome,
                        crate::agent_core::team::specialist_turn::SpecialistTurnOutcome::MissingCompletion
                    ) {
                        if let Some(ref reply_text) = captured_reply_text {
                            let _ = persist_missing_completion_reply_artifacts(
                                team_session.as_ref(),
                                &task.id,
                                &agent,
                                reply_text,
                            );
                        }
                        team_orch.handle_specialist_missing_completion(
                            &task.id,
                            &agent,
                            reply_excerpt.as_deref(),
                        )?;
                        registry
                            .session_manager_ref()
                            .reset_conversation(specialist_session_id)
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
        team_orch.set_milestone_fn(Arc::new(
            move |scope: crate::protocol::SessionKey, event| {
                use crate::agent_core::team::milestone::render_for_im;
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
                let registry_for_send = Arc::clone(&registry_for_milestone);
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
                    let (outbound, send_result, sender_channel_instance) = if let Some(resolved) =
                        resolved
                    {
                        let sender_channel_instance = resolved.sender_channel_instance.clone();
                        let outbound = resolved.outbound_text(&msg);
                        let (outbound, send_result) =
                            send_with_reply_fallback(&resolved.sender, outbound).await;
                        (outbound, send_result, sender_channel_instance)
                    } else if let Some(ch) = channels.resolve_for_session(&scope) {
                        let outbound = crate::protocol::OutboundMsg {
                            session_key: scope.clone(),
                            content: crate::protocol::MsgContent::text(msg),
                            reply_to: stored_source
                                .as_ref()
                                .and_then(|source| source.reply_to.clone()),
                            thread_ts: stored_source
                                .as_ref()
                                .and_then(|source| source.thread_ts.clone()),
                        };
                        let (outbound, send_result) = send_with_reply_fallback(&ch, outbound).await;
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
                    match session_for_record.record_channel_send(
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
                        Ok(record) => registry_for_send.emit_dashboard_event(
                            DashboardEvent::TeamChannelSend {
                                team_id: session_for_record.team_id.clone(),
                                record,
                            },
                        ),
                        Err(err) => {
                            tracing::warn!(
                                team_id = %session_for_record.team_id,
                                error = %err,
                                "Failed to append milestone channel send ledger entry"
                            );
                        }
                    }
                });
            },
        ));

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

        team_orch.bootstrap_workspace_artifacts().map_err(|e| {
            anyhow::anyhow!(
                "failed to bootstrap team workspace for scope '{}' (team '{}'): {e:#}",
                team_scope.scope,
                team_id
            )
        })?;
        registry.register_team_orchestrator(team_id.clone(), team_orch);
        review_retry_orchestrators.push(
            registry
                .get_team_orchestrator(&team_id)
                .expect("team orchestrator should be immediately retrievable after registration"),
        );
        tracing::info!(scope = %team_scope.scope, team_id = %team_id, "TeamOrchestrator registered");
    }

    if !review_retry_orchestrators.is_empty() {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                for team_orch in &review_retry_orchestrators {
                    team_orch.retry_due_pending_routing_events();
                }
            }
        });
        tracing::info!("Team review retry task started");
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
                let base_record = request.clone().into_pending_record();
                if let Some(team_orch) =
                    registry_for_notify.get_team_orchestrator(&request.envelope.team_id)
                {
                    if routing_event_is_stale_for_delivery(team_orch.as_ref(), &request.envelope) {
                        tracing::info!(
                            team_id = %request.envelope.team_id,
                            task_id = %request.envelope.event.task_id,
                            kind = ?request.envelope.event.kind,
                            "Suppressing stale team routing event before lead delivery"
                        );
                        team_orch.mark_routing_event_delivered(
                            &request
                                .envelope
                                .clone()
                                .with_delivery_status(RoutingDeliveryStatus::DirectDelivered),
                        );
                        continue;
                    }
                }
                let text = render_routing_event_for_delivery(&base_record);
                let mut delivered = None;
                let mut pending_record: Option<PendingRoutingRecord> = None;

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
                    if let Some(team_orch) =
                        registry_for_notify.get_team_orchestrator(&request.envelope.team_id)
                    {
                        if base_record.review.is_some() {
                            team_orch.begin_lead_review_attempt(
                                &request.envelope.run_id,
                                &request.envelope.event.task_id,
                                request.envelope.event.kind.clone(),
                            );
                        }
                    }
                    let delivery_result = registry_for_notify
                        .handle_with_context(inbound, turn_ctx)
                        .await;
                    if let Some(team_orch) =
                        registry_for_notify.get_team_orchestrator(&request.envelope.team_id)
                    {
                        if base_record.review.is_some() {
                            team_orch.end_lead_review_attempt(&request.envelope.run_id);
                        }
                    }
                    match delivery_result {
                        Ok(result_text) => {
                            if let Some(team_orch) =
                                registry_for_notify.get_team_orchestrator(&request.envelope.team_id)
                            {
                                if routing_event_still_requires_resolution_after_delivery(
                                    team_orch.as_ref(),
                                    &request.envelope,
                                    base_record.review.as_ref(),
                                ) {
                                    let (classification, reason) =
                                        unresolved_review_failure_from_turn_result(
                                            result_text.as_deref(),
                                            &request.envelope,
                                        );
                                    let record = base_record
                                        .clone()
                                        .with_delivery_status(
                                            RoutingDeliveryStatus::PersistedPending,
                                        )
                                        .note_failed_attempt(
                                            classification,
                                            reason,
                                            Some(next_review_retry_at(
                                                base_record
                                                    .review
                                                    .as_ref()
                                                    .map(|review| review.attempt_count + 1)
                                                    .unwrap_or(1),
                                            )),
                                        );
                                    if let Some(diagnostic) = review_attempt_diagnostic(&record) {
                                        let _ = team_orch
                                            .session
                                            .append_review_attempt_diagnostic(&diagnostic);
                                    }
                                    pending_record = Some(record);
                                    tracing::warn!(
                                        team_id = %request.envelope.team_id,
                                        task_id = %request.envelope.event.task_id,
                                        kind = ?request.envelope.event.kind,
                                        target = %target.scope,
                                        "TeamNotify turn completed without resolving required team action; treating delivery as incomplete"
                                    );
                                    continue;
                                }
                            }
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
                            if let Some(team_orch) =
                                registry_for_notify.get_team_orchestrator(&request.envelope.team_id)
                            {
                                let record = base_record
                                    .clone()
                                    .with_delivery_status(RoutingDeliveryStatus::PersistedPending)
                                    .note_failed_attempt(
                                        ReviewFailureClassification::RuntimeError,
                                        e.to_string(),
                                        Some(next_review_retry_at(
                                            base_record
                                                .review
                                                .as_ref()
                                                .map(|review| review.attempt_count + 1)
                                                .unwrap_or(1),
                                        )),
                                    );
                                if let Some(diagnostic) = review_attempt_diagnostic(&record) {
                                    let _ = team_orch
                                        .session
                                        .append_review_attempt_diagnostic(&diagnostic);
                                }
                                pending_record = Some(record);
                            }
                        }
                    }
                }

                if let Some(team_orch) =
                    registry_for_notify.get_team_orchestrator(&request.envelope.team_id)
                {
                    if let Some(delivered) = delivered {
                        team_orch.mark_routing_event_delivered(&delivered);
                    } else {
                        let pending = pending_record.unwrap_or_else(|| {
                            base_record
                                .clone()
                                .with_delivery_status(RoutingDeliveryStatus::PersistedPending)
                        });
                        team_orch.persist_pending_routing_record(pending);
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

async fn capture_specialist_reply_text(
    storage: &crate::session::SessionStorage,
    specialist_session_id: uuid::Uuid,
    result: &anyhow::Result<Option<String>>,
) -> Result<Option<String>> {
    if let Ok(Some(reply_text)) = result {
        if !reply_text.trim().is_empty() {
            return Ok(Some(reply_text.clone()));
        }
    }

    let recent = storage
        .load_recent_messages(specialist_session_id, 20)
        .await
        .unwrap_or_default();
    Ok(recent
        .iter()
        .rev()
        .find(|msg| msg.role == "assistant" && !msg.content.trim().is_empty())
        .map(|msg| msg.content.clone()))
}

fn missing_completion_excerpt(
    result: &anyhow::Result<Option<String>>,
    captured_reply_text: Option<&str>,
) -> Option<String> {
    if let Some(reply_text) = captured_reply_text {
        return Some(truncate_for_missing_completion(reply_text, 240));
    }
    match result {
        Err(error) => Some(truncate_for_missing_completion(
            &format!("runtime error: {error}"),
            240,
        )),
        // Backend returned zero or empty text — no tool calls, no content.
        // Most likely cause: ACP subprocess cold-start failure or MCP bridge unavailable.
        Ok(None) => Some(
            "zero-output turn: backend returned no text (possible cold-start, subprocess \
             initialization failure, or MCP bridge unavailable)"
                .to_string(),
        ),
        Ok(Some(text)) if text.trim().is_empty() => Some(
            "zero-output turn: backend returned empty text (possible cold-start, subprocess \
             initialization failure, or MCP bridge unavailable)"
                .to_string(),
        ),
        Ok(Some(_)) => None,
    }
}

fn persist_missing_completion_reply_artifacts(
    team_session: &TeamSession,
    task_id: &str,
    agent: &str,
    reply_text: &str,
) -> Result<()> {
    let result_path = team_session.task_dir(task_id).join("result.md");
    let existing = std::fs::read_to_string(&result_path).unwrap_or_default();
    if existing.trim().is_empty() {
        team_session.write_task_result(
            task_id,
            &format!(
                "# Draft Specialist Output\n\nThis task ended without a canonical team completion tool call.\nThe raw assistant reply from `{agent}` was preserved below for review/retry.\n\n---\n\n{reply_text}\n"
            ),
        )?;
    }
    team_session.append_task_progress(
        task_id,
        &format!(
            "[{}] preserved raw reply text from {} before specialist session reset.",
            chrono::Utc::now().to_rfc3339(),
            agent
        ),
    )?;
    Ok(())
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

fn routing_event_is_stale_for_delivery(
    team_orch: &crate::agent_core::team::orchestrator::TeamOrchestrator,
    envelope: &TeamRoutingEnvelope,
) -> bool {
    let task = match team_orch.registry.get_task(&envelope.event.task_id) {
        Ok(Some(task)) => task,
        Ok(None) | Err(_) => return false,
    };

    match envelope.event.kind {
        crate::agent_core::team::completion_routing::TeamRoutingEventKind::TaskCheckpoint => {
            matches!(
                task.status_parsed(),
                TaskStatus::Submitted { .. }
                    | TaskStatus::Accepted { .. }
                    | TaskStatus::Done
                    | TaskStatus::Failed(_)
            )
        }
        crate::agent_core::team::completion_routing::TeamRoutingEventKind::TaskSubmitted => {
            matches!(
                task.status_parsed(),
                TaskStatus::Accepted { .. } | TaskStatus::Done | TaskStatus::Failed(_)
            )
        }
        _ => false,
    }
}

fn render_routing_event_for_delivery(record: &PendingRoutingRecord) -> String {
    let mut rendered = record.envelope.event.render_for_parent();
    let Some(review) = record.review.as_ref() else {
        return rendered;
    };
    if review.attempt_count == 0 {
        return rendered;
    }

    let failure_label = review
        .last_failure_classification
        .map(review_failure_label)
        .unwrap_or("未分类失败");
    let failure_reason = review
        .last_failure_reason
        .as_deref()
        .unwrap_or("上一轮未留下明确失败原因");
    let corrective_contract =
        review_retry_corrective_contract(review.review_kind, &record.envelope.event.task_id);

    rendered = format!(
        "[系统纠偏提醒]\n此前同一控制面事件已失败 {attempts} 次。\n最近一次失败分类：{failure_label}\n最近一次失败原因：{failure_reason}\n\n{corrective_contract}\n\n{rendered}",
        attempts = review.attempt_count,
    );
    rendered
}

fn review_failure_label(classification: ReviewFailureClassification) -> &'static str {
    match classification {
        ReviewFailureClassification::NoOp => "NoOp",
        ReviewFailureClassification::RuntimeError => "RuntimeError",
        ReviewFailureClassification::DeliveryFailure => "DeliveryFailure",
        ReviewFailureClassification::StillRequiresResolution => "StillRequiresResolution",
    }
}

fn review_retry_corrective_contract(
    review_kind: crate::agent_core::team::completion_routing::ReviewRequiredKind,
    task_id: &str,
) -> String {
    match review_kind {
        crate::agent_core::team::completion_routing::ReviewRequiredKind::Submitted => format!(
            "这是 submitted 验收纠偏回合，不是新的用户请求。不要再输出解释性文字作为结束。先检查结果工件，然后在本轮结束前恰好调用一个工具：accept_task(task_id=\"{task_id}\") 或 reopen_task(task_id=\"{task_id}\", reason=\"...\")。"
        ),
        crate::agent_core::team::completion_routing::ReviewRequiredKind::Blocked => format!(
            "这是 blocked 处理纠偏回合，不是新的用户请求。你必须明确处理 {task_id}：优先用内部动作继续推进；只有在确实需要用户决策时，才调用 post_update(...) 向用户说明阻塞并请求决策。"
        ),
        crate::agent_core::team::completion_routing::ReviewRequiredKind::Failed => format!(
            "这是 failed 处理纠偏回合，不是新的用户请求。你必须明确处理 {task_id}：若要交还用户决策，调用 post_update(...) 说明失败、原因以及“重试 / 终止 / 改派”的选择；不要静默结束。"
        ),
        crate::agent_core::team::completion_routing::ReviewRequiredKind::MissingCompletion => format!(
            "这是 missing-completion 纠偏回合，不是新的用户请求。你必须对 {task_id} 采取内部动作继续推进，例如 reopen_task(...) 或 assign_task(...)；仅 post_update(...) 不能算完成。"
        ),
    }
}

fn routing_event_still_requires_resolution_after_delivery(
    team_orch: &crate::agent_core::team::orchestrator::TeamOrchestrator,
    envelope: &TeamRoutingEnvelope,
    review: Option<&crate::agent_core::team::completion_routing::ReviewAttemptMetadata>,
) -> bool {
    let task = match team_orch.registry.get_task(&envelope.event.task_id) {
        Ok(Some(task)) => task,
        Ok(None) | Err(_) => return false,
    };

    match envelope.event.kind {
        crate::agent_core::team::completion_routing::TeamRoutingEventKind::TaskSubmitted => {
            matches!(task.status_parsed(), TaskStatus::Submitted { .. })
        }
        crate::agent_core::team::completion_routing::TeamRoutingEventKind::TaskBlocked
        | crate::agent_core::team::completion_routing::TeamRoutingEventKind::TaskMissingCompletion => {
            let still_held = matches!(task.status_parsed(), TaskStatus::Held { .. });
            if !still_held {
                return false;
            }
            if matches!(
                envelope.event.kind,
                crate::agent_core::team::completion_routing::TeamRoutingEventKind::TaskBlocked
            ) && review_terminal_post_update_recorded(team_orch, &envelope.event.task_id, review)
            {
                return false;
            }
            true
        }
        crate::agent_core::team::completion_routing::TeamRoutingEventKind::TaskFailed => {
            !review_terminal_post_update_recorded(team_orch, &envelope.event.task_id, review)
        }
        _ => false,
    }
}

fn review_terminal_post_update_recorded(
    team_orch: &crate::agent_core::team::orchestrator::TeamOrchestrator,
    task_id: &str,
    review: Option<&crate::agent_core::team::completion_routing::ReviewAttemptMetadata>,
) -> bool {
    let Some(review) = review else {
        return false;
    };
    team_orch
        .session
        .has_post_update_for_task_since(task_id, &review.first_pending_at)
        .unwrap_or(false)
}

fn unresolved_review_failure_from_turn_result(
    result_text: Option<&str>,
    envelope: &TeamRoutingEnvelope,
) -> (ReviewFailureClassification, String) {
    match result_text {
        None => (
            ReviewFailureClassification::NoOp,
            format!(
                "lead review turn for {:?} produced no output and did not resolve the task state",
                envelope.event.kind
            ),
        ),
        Some(text) if text.trim().is_empty() => (
            ReviewFailureClassification::NoOp,
            format!(
                "lead review turn for {:?} produced empty output and did not resolve the task state",
                envelope.event.kind
            ),
        ),
        Some(text) => (
            ReviewFailureClassification::StillRequiresResolution,
            format!(
                "lead review turn for {:?} produced output without resolving the task state: {}",
                envelope.event.kind,
                truncate_for_missing_completion(text, 160)
            ),
        ),
    }
}

fn next_review_retry_at(attempt_count: u32) -> String {
    let backoff_seconds = match attempt_count {
        0 | 1 => 5,
        2 => 15,
        3 => 30,
        _ => 60,
    };
    (chrono::Utc::now() + chrono::Duration::seconds(backoff_seconds)).to_rfc3339()
}

fn review_attempt_diagnostic(record: &PendingRoutingRecord) -> Option<ReviewAttemptDiagnostic> {
    let review = record.review.as_ref()?;
    let classification = review.last_failure_classification?;
    let reason = review.last_failure_reason.clone()?;
    Some(ReviewAttemptDiagnostic {
        ts: chrono::Utc::now().to_rfc3339(),
        run_id: record.envelope.run_id.clone(),
        team_id: record.envelope.team_id.clone(),
        task_id: record.envelope.event.task_id.clone(),
        event_kind: record.envelope.event.kind.clone(),
        attempt_count: review.attempt_count,
        classification,
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::team::completion_routing::{RoutingDeliveryStatus, TeamRoutingEvent};
    use crate::agent_core::team::milestone::TeamMilestoneEvent;
    use crate::agent_core::team::milestone_delivery::{milestone_is_public, TeamPublicUpdatesMode};
    use crate::agent_core::team::orchestrator::TeamOrchestrator;
    use crate::agent_core::team::registry::{CreateTask, TaskRegistry};
    use crate::agent_core::team::session::{stable_team_id_for_session_key, TeamSession};
    use crate::config::{ChannelsSection, GatewayConfig, LarkSection, ProgressPresentationMode};
    use anyhow::Result;
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tempfile::tempdir;
    use tokio::sync::mpsc;
    use uuid::Uuid;

    struct MockChannel {
        sent: Mutex<Vec<OutboundMsg>>,
        fail_when_reply_to: bool,
    }

    #[async_trait]
    impl crate::channels_internal::Channel for MockChannel {
        fn name(&self) -> &str {
            "mock"
        }

        async fn send(&self, msg: &OutboundMsg) -> Result<()> {
            if self.fail_when_reply_to && msg.reply_to.is_some() {
                anyhow::bail!("reply target not found");
            }
            self.sent.lock().unwrap().push(msg.clone());
            Ok(())
        }

        async fn listen(&self, _tx: mpsc::Sender<InboundMsg>) -> Result<()> {
            Ok(())
        }
    }

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
    fn stale_checkpoint_is_suppressed_after_submission() {
        let tmp = tempdir().unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("team-test", tmp.path().to_path_buf()));
        let dispatch_fn: crate::agent_core::team::heartbeat::DispatchFn =
            Arc::new(|_agent, _task| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            registry.clone(),
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        registry
            .create_task(CreateTask {
                id: "T004".into(),
                title: "task".into(),
                assignee_hint: Some("codex-beta".into()),
                deps: vec![],
                timeout_secs: 60,
                spec: None,
                success_criteria: None,
            })
            .unwrap();
        registry.try_claim("T004", "codex-beta").unwrap();
        registry
            .submit_task_result("T004", "codex-beta", "done")
            .unwrap();

        let envelope = TeamRoutingEnvelope {
            run_id: "run-1".into(),
            parent_run_id: None,
            requester_session_key: Some(SessionKey::new("lark", "group:test")),
            fallback_session_keys: vec![],
            team_id: "team-test".into(),
            delivery_status: RoutingDeliveryStatus::NotRouted,
            event: TeamRoutingEvent::checkpoint("T004", "codex-beta", "still working"),
            delivery_source: None,
        };

        assert!(routing_event_is_stale_for_delivery(
            orch.as_ref(),
            &envelope
        ));
    }

    #[test]
    fn submitted_delivery_requires_explicit_resolution_until_status_changes() {
        let tmp = tempdir().unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("team-test", tmp.path().to_path_buf()));
        let dispatch_fn: crate::agent_core::team::heartbeat::DispatchFn =
            Arc::new(|_agent, _task| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            registry.clone(),
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        registry
            .create_task(CreateTask {
                id: "T006".into(),
                title: "task".into(),
                assignee_hint: Some("codex-beta".into()),
                deps: vec![],
                timeout_secs: 60,
                spec: None,
                success_criteria: None,
            })
            .unwrap();
        registry.try_claim("T006", "codex-beta").unwrap();
        registry
            .submit_task_result("T006", "codex-beta", "done")
            .unwrap();

        let envelope = TeamRoutingEnvelope {
            run_id: "run-6".into(),
            parent_run_id: None,
            requester_session_key: Some(SessionKey::new("lark", "group:test")),
            fallback_session_keys: vec![],
            team_id: "team-test".into(),
            delivery_status: RoutingDeliveryStatus::NotRouted,
            event: TeamRoutingEvent::submitted("T006", "codex-beta", "done"),
            delivery_source: None,
        };

        assert!(routing_event_still_requires_resolution_after_delivery(
            orch.as_ref(),
            &envelope,
            None
        ));

        registry.accept_task("T006", "lead").unwrap();

        assert!(!routing_event_still_requires_resolution_after_delivery(
            orch.as_ref(),
            &envelope,
            None
        ));
    }

    #[test]
    fn blocked_and_missing_completion_delivery_require_explicit_resolution_while_task_is_held() {
        let tmp = tempdir().unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("team-test", tmp.path().to_path_buf()));
        let dispatch_fn: crate::agent_core::team::heartbeat::DispatchFn =
            Arc::new(|_agent, _task| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            registry.clone(),
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        registry
            .create_task(CreateTask {
                id: "T007".into(),
                title: "task".into(),
                assignee_hint: Some("codex-beta".into()),
                deps: vec![],
                timeout_secs: 60,
                spec: None,
                success_criteria: None,
            })
            .unwrap();
        registry.try_claim("T007", "codex-beta").unwrap();
        registry
            .hold_claim("T007", "codex-beta", "missing_completion")
            .unwrap();

        let blocked = TeamRoutingEnvelope {
            run_id: "run-7b".into(),
            parent_run_id: None,
            requester_session_key: Some(SessionKey::new("lark", "group:test")),
            fallback_session_keys: vec![],
            team_id: "team-test".into(),
            delivery_status: RoutingDeliveryStatus::NotRouted,
            event: TeamRoutingEvent::blocked("T007", "codex-beta", "blocked"),
            delivery_source: None,
        };
        let missing = TeamRoutingEnvelope {
            run_id: "run-7m".into(),
            parent_run_id: None,
            requester_session_key: Some(SessionKey::new("lark", "group:test")),
            fallback_session_keys: vec![],
            team_id: "team-test".into(),
            delivery_status: RoutingDeliveryStatus::NotRouted,
            event: TeamRoutingEvent::missing_completion("T007", "codex-beta"),
            delivery_source: None,
        };

        assert!(routing_event_still_requires_resolution_after_delivery(
            orch.as_ref(),
            &blocked,
            None
        ));
        assert!(routing_event_still_requires_resolution_after_delivery(
            orch.as_ref(),
            &missing,
            None
        ));

        registry.reassign_task("T007", "worker").unwrap();

        assert!(!routing_event_still_requires_resolution_after_delivery(
            orch.as_ref(),
            &blocked,
            None
        ));
        assert!(!routing_event_still_requires_resolution_after_delivery(
            orch.as_ref(),
            &missing,
            None
        ));
    }

    #[test]
    fn unresolved_review_failure_classification_distinguishes_noop_from_textful_turn() {
        let envelope = TeamRoutingEnvelope {
            run_id: "run-review".into(),
            parent_run_id: None,
            requester_session_key: Some(SessionKey::new("lark", "group:test")),
            fallback_session_keys: vec![],
            team_id: "team-test".into(),
            delivery_status: RoutingDeliveryStatus::NotRouted,
            event: TeamRoutingEvent::submitted("T008", "codex-beta", "done"),
            delivery_source: None,
        };

        let (classification, reason) = unresolved_review_failure_from_turn_result(None, &envelope);
        assert_eq!(classification, ReviewFailureClassification::NoOp);
        assert!(reason.contains("produced no output"));

        let (classification, reason) = unresolved_review_failure_from_turn_result(
            Some("I reviewed it but won't accept yet"),
            &envelope,
        );
        assert_eq!(
            classification,
            ReviewFailureClassification::StillRequiresResolution
        );
        assert!(reason.contains("produced output without resolving"));
    }

    #[test]
    fn render_review_retry_delivery_text_includes_previous_failure_context() {
        let envelope = TeamRoutingEnvelope {
            run_id: "run-review".into(),
            parent_run_id: None,
            requester_session_key: Some(SessionKey::new("lark", "group:test")),
            fallback_session_keys: vec![],
            team_id: "team-test".into(),
            delivery_status: RoutingDeliveryStatus::PersistedPending,
            event: TeamRoutingEvent::submitted("T010", "codex-beta", "done"),
            delivery_source: None,
        };
        let record = PendingRoutingRecord::from_envelope(envelope).note_failed_attempt(
            ReviewFailureClassification::StillRequiresResolution,
            "lead review turn completed without accept_task/reopen_task",
            None,
        );

        let rendered = render_routing_event_for_delivery(&record);
        assert!(rendered.contains("[系统纠偏提醒]"));
        assert!(rendered.contains("此前同一控制面事件已失败 1 次"));
        assert!(rendered.contains("StillRequiresResolution"));
        assert!(rendered.contains("accept_task(task_id=\"T010\")"));
        assert!(rendered.contains("reopen_task(task_id=\"T010\", reason=\"...\")"));
    }

    #[test]
    fn failed_and_blocked_reviews_can_resolve_via_post_update_after_user_notification() {
        let tmp = tempdir().unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("team-test", tmp.path().to_path_buf()));
        let dispatch_fn: crate::agent_core::team::heartbeat::DispatchFn =
            Arc::new(|_agent, _task| Box::pin(async { Ok(()) }));
        let orch = TeamOrchestrator::new(
            registry.clone(),
            session,
            dispatch_fn,
            std::time::Duration::from_secs(60),
        );
        orch.set_lead_agent_name("claude-alpha".into());

        registry
            .create_task(CreateTask {
                id: "T009".into(),
                title: "failed-task".into(),
                assignee_hint: Some("codex-beta".into()),
                deps: vec![],
                timeout_secs: 60,
                spec: None,
                success_criteria: None,
            })
            .unwrap();
        registry.mark_failed("T009", "quota").unwrap();

        let failed = TeamRoutingEnvelope {
            run_id: "run-9f".into(),
            parent_run_id: None,
            requester_session_key: Some(SessionKey::new("lark", "group:test")),
            fallback_session_keys: vec![],
            team_id: "team-test".into(),
            delivery_status: RoutingDeliveryStatus::NotRouted,
            event: TeamRoutingEvent::failed("T009", "quota"),
            delivery_source: None,
        };
        let failed_review =
            crate::agent_core::team::completion_routing::PendingRoutingRecord::from_envelope(
                failed.clone(),
            );
        assert!(routing_event_still_requires_resolution_after_delivery(
            orch.as_ref(),
            &failed,
            failed_review.review.as_ref(),
        ));
        orch.begin_lead_review_attempt("run-9f", "T009", failed.event.kind.clone());
        assert!(orch.post_message("任务失败，是否重试请用户决定"));
        orch.end_lead_review_attempt("run-9f");
        assert!(!routing_event_still_requires_resolution_after_delivery(
            orch.as_ref(),
            &failed,
            failed_review.review.as_ref(),
        ));

        registry
            .create_task(CreateTask {
                id: "T010".into(),
                title: "blocked-task".into(),
                assignee_hint: Some("codex-beta".into()),
                deps: vec![],
                timeout_secs: 60,
                spec: None,
                success_criteria: None,
            })
            .unwrap();
        registry.try_claim("T010", "codex-beta").unwrap();
        registry
            .hold_claim("T010", "codex-beta", "waiting_user")
            .unwrap();

        let blocked = TeamRoutingEnvelope {
            run_id: "run-10b".into(),
            parent_run_id: None,
            requester_session_key: Some(SessionKey::new("lark", "group:test")),
            fallback_session_keys: vec![],
            team_id: "team-test".into(),
            delivery_status: RoutingDeliveryStatus::NotRouted,
            event: TeamRoutingEvent::blocked("T010", "codex-beta", "waiting_user"),
            delivery_source: None,
        };
        let blocked_review =
            crate::agent_core::team::completion_routing::PendingRoutingRecord::from_envelope(
                blocked.clone(),
            );
        assert!(routing_event_still_requires_resolution_after_delivery(
            orch.as_ref(),
            &blocked,
            blocked_review.review.as_ref(),
        ));
        orch.begin_lead_review_attempt("run-10b", "T010", blocked.event.kind.clone());
        assert!(orch.post_message("任务阻塞，需要用户决定下一步"));
        orch.end_lead_review_attempt("run-10b");
        assert!(!routing_event_still_requires_resolution_after_delivery(
            orch.as_ref(),
            &blocked,
            blocked_review.review.as_ref(),
        ));
    }

    #[tokio::test]
    async fn capture_specialist_reply_text_prefers_direct_result() {
        let dir = tempdir().unwrap();
        let storage = crate::session::SessionStorage::new(dir.path().to_path_buf());
        let session_id = Uuid::new_v4();

        let captured = capture_specialist_reply_text(
            &storage,
            session_id,
            &Ok(Some("direct result".to_string())),
        )
        .await
        .unwrap();

        assert_eq!(captured.as_deref(), Some("direct result"));
    }

    #[tokio::test]
    async fn capture_specialist_reply_text_falls_back_to_persisted_assistant_message() {
        let dir = tempdir().unwrap();
        let storage = crate::session::SessionStorage::new(dir.path().to_path_buf());
        let session_id = Uuid::new_v4();
        let session_dir = dir.path().join(session_id.to_string());
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("messages.jsonl"),
            format!(
                "{}\n{}\n",
                serde_json::json!({
                    "id": Uuid::new_v4(),
                    "role": "user",
                    "content": "spec",
                    "timestamp": chrono::Utc::now(),
                    "sender": "orchestrator",
                    "tool_calls": null,
                    "fragment_event_ids": null,
                    "aggregation_mode": null,
                }),
                serde_json::json!({
                    "id": Uuid::new_v4(),
                    "role": "assistant",
                    "content": "persisted assistant output",
                    "timestamp": chrono::Utc::now(),
                    "sender": "@codex-beta",
                    "tool_calls": null,
                    "fragment_event_ids": null,
                    "aggregation_mode": null,
                })
            ),
        )
        .unwrap();

        let captured = capture_specialist_reply_text(&storage, session_id, &Ok(None))
            .await
            .unwrap();

        assert_eq!(captured.as_deref(), Some("persisted assistant output"));
    }

    #[test]
    fn missing_completion_excerpt_falls_back_to_runtime_error() {
        let excerpt =
            missing_completion_excerpt(&Err(anyhow::anyhow!("tool bridge unavailable")), None)
                .unwrap();
        assert!(excerpt.contains("runtime error: tool bridge unavailable"));
    }

    #[test]
    fn missing_completion_excerpt_zero_output_ok_none() {
        let excerpt = missing_completion_excerpt(&Ok(None), None).unwrap();
        assert!(
            excerpt.contains("zero-output turn"),
            "Ok(None) should produce zero-output diagnostic, got: {excerpt}"
        );
        assert!(excerpt.contains("cold-start"));
    }

    #[test]
    fn missing_completion_excerpt_zero_output_ok_empty_string() {
        let excerpt = missing_completion_excerpt(&Ok(Some(String::new())), None).unwrap();
        assert!(
            excerpt.contains("zero-output turn"),
            "Ok(Some(\"\")) should produce zero-output diagnostic, got: {excerpt}"
        );
    }

    #[test]
    fn missing_completion_excerpt_nonempty_result_without_captured_returns_none() {
        // If capture_specialist_reply_text errored and captured_reply_text is None,
        // but result has actual text, we should NOT emit a misleading zero-output diagnostic.
        let excerpt = missing_completion_excerpt(&Ok(Some("actual output".to_string())), None);
        assert!(
            excerpt.is_none(),
            "Non-empty Ok(Some) without captured text should yield None, got: {excerpt:?}"
        );
    }

    #[test]
    fn persist_missing_completion_reply_artifacts_writes_draft_result() {
        let tmp = tempdir().unwrap();
        let session = TeamSession::from_dir("team-test", tmp.path().to_path_buf());

        persist_missing_completion_reply_artifacts(
            &session,
            "T005",
            "codex-beta",
            "raw specialist answer",
        )
        .unwrap();

        let result = std::fs::read_to_string(session.task_dir("T005").join("result.md")).unwrap();
        let progress =
            std::fs::read_to_string(session.task_dir("T005").join("progress.md")).unwrap();
        assert!(result.contains("Draft Specialist Output"));
        assert!(result.contains("raw specialist answer"));
        assert!(progress.contains("preserved raw reply text"));
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

    #[tokio::test]
    async fn milestone_send_retries_without_reply_to() {
        let channel = Arc::new(MockChannel {
            sent: Mutex::new(Vec::new()),
            fail_when_reply_to: true,
        });
        let outbound = OutboundMsg {
            session_key: SessionKey::new("lark", "group:test"),
            content: MsgContent::text("hello"),
            reply_to: Some("reply-id".into()),
            thread_ts: None,
        };
        let (sent, result) = send_with_reply_fallback(
            &(channel.clone() as Arc<dyn crate::channels_internal::Channel>),
            outbound,
        )
        .await;
        result.unwrap();
        assert!(sent.reply_to.is_none());
        assert_eq!(channel.sent.lock().unwrap().len(), 1);
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
