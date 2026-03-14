use crate::config::{GatewayConfig, InteractionMode};
use anyhow::Result;
use qai_agent::team::completion_routing::{RoutingDeliveryStatus, TeamRoutingEnvelope};
use qai_agent::SessionRegistry;
use qai_protocol::{InboundMsg, MsgContent, MsgSource, SessionKey};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc;

pub async fn wire_team_runtime(
    registry: Arc<SessionRegistry>,
    cfg: &GatewayConfig,
    channel_map: Arc<HashMap<String, Arc<dyn qai_channels::Channel>>>,
    heartbeat_interval: Duration,
) -> Result<()> {
    use qai_agent::team::{
        completion_routing::{milestone_reply_policy, RoutingDeliveryStatus, TeamNotifyRequest},
        heartbeat::DispatchFn,
        orchestrator::TeamOrchestrator,
        registry::TaskRegistry,
        session::{stable_team_id, TeamSession},
    };

    let (team_notify_tx, mut team_notify_rx) = mpsc::channel::<TeamNotifyRequest>(256);
    let team_notify_tx_for_orch = team_notify_tx.clone();
    let team_scopes = cfg.normalized_team_scopes();
    tracing::info!(count = team_scopes.len(), "wire_team_runtime: team scopes found");

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
        let team_id = stable_team_id(&channel_name, &team_scope.scope);
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

        // ── Compute lead_key before dispatch_fn so it can be captured ────────────
        let lead_key = qai_protocol::SessionKey::new(&channel_name, &team_scope.scope);

        let registry_for_dispatch = Arc::clone(&registry);
        let task_reg_for_dispatch = Arc::clone(&task_registry);
        let team_session_for_dispatch = Arc::clone(&session);
        let dispatch_requester_key = lead_key.clone();
        let team_orch_for_dispatch: Arc<OnceLock<Arc<TeamOrchestrator>>> =
            Arc::new(OnceLock::new());
        let team_orch_for_dispatch_in_closure = Arc::clone(&team_orch_for_dispatch);
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
                    team_orch.record_dispatch_start(&task.id, &agent, requester_key.clone(), None);
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
                let result = registry.handle(msg).await;
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

        let channels_for_notify = Arc::clone(&channel_map);
        let session_for_notify = Arc::clone(&session);
        team_orch.set_milestone_fn(Arc::new(move |scope: qai_protocol::SessionKey, event| {
            use qai_agent::team::milestone::render_for_im;
            let policy = milestone_reply_policy(&event);
            if !milestone_is_user_deliverable(&policy) {
                tracing::debug!(
                    scope = %scope.scope,
                    kind = %event.kind_str(),
                    "Suppressing internal-only team milestone from direct channel delivery"
                );
                return;
            }
            if let Some(dedupe_key) = policy.dedupe_key.as_ref() {
                match session_for_notify.mark_delivery_dedupe(&scope.scope, dedupe_key) {
                    Ok(true) => {}
                    Ok(false) => {
                        let _ =
                            session_for_notify.record_delivery_dedupe_hit(&scope.scope, dedupe_key);
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
                    let inbound = team_notify_inbound(&target, &text);
                    match registry_for_notify.handle(inbound).await {
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
                        let _ = team_orch.session.append_routing_outcome(&delivered);
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

fn team_notify_inbound(target: &SessionKey, text: &str) -> InboundMsg {
    InboundMsg {
        id: uuid::Uuid::new_v4().to_string(),
        session_key: target.clone(),
        content: MsgContent::text(text),
        sender: "gateway".to_string(),
        channel: target.channel.clone(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: None,
        source: MsgSource::TeamNotify,
    }
}

fn milestone_is_user_deliverable(
    policy: &qai_agent::team::completion_routing::CompletionReplyPolicy,
) -> bool {
    !matches!(
        policy.audience,
        qai_agent::team::completion_routing::CompletionAudience::ParentOnly
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use qai_agent::team::completion_routing::{
        CompletionReplyPolicy, RoutingDeliveryStatus, TeamRoutingEvent,
    };

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
    fn parent_then_user_milestones_are_user_deliverable() {
        assert!(!milestone_is_user_deliverable(
            &CompletionReplyPolicy::internal_only()
        ));
        assert!(milestone_is_user_deliverable(
            &CompletionReplyPolicy::user_visible(None)
        ));
        assert!(milestone_is_user_deliverable(
            &qai_agent::team::completion_routing::milestone_reply_policy(
                &qai_agent::team::milestone::TeamMilestoneEvent::AllTasksDone
            )
        ));
    }
}
