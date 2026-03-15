use crate::channel_registry::ChannelRegistry;
use crate::config::{DeliveryPurposeConfig, GatewayConfig, ProgressPresentationMode};
use crate::delivery_resolver::{resolve_delivery, ResolvedDelivery};
use crate::progress_presentation;
use async_trait::async_trait;
use qai_agent::team::orchestrator::TeamOrchestrator;
use qai_agent::team::session::{ChannelSendSourceKind, ChannelSendStatus};
use qai_agent::{
    throttled_stream, OutputSink, SessionRegistry, StreamControl, TurnDeliverySource,
    TurnExecutionContext,
};
use qai_channels::Channel;
use qai_protocol::{InboundMsg, OutboundMsg, SessionKey};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, Mutex};

/// No-op sink used for Team Lead turns — all user-visible output goes through
/// `post_update` tool calls (via milestone_fn → ch.send()), not the stream path.
struct NullSink;

#[async_trait]
impl OutputSink for NullSink {
    async fn send_thinking(&self) -> Option<String> {
        None
    }
    async fn send_delta(&self, _: &str, _: Option<&str>) {}
    async fn send_final(&self, _: &str, _: Option<&str>) {}
}

pub struct ImProgressSink {
    channel: Arc<dyn Channel>,
    sender_channel_instance: Option<String>,
    reply_to: Option<String>,
    thread_ts: Option<String>,
    session_key: SessionKey,
    presentation: ProgressPresentationMode,
    recent_progress: Mutex<HashMap<String, Instant>>,
    team_orchestrator: Option<Arc<TeamOrchestrator>>,
}

impl ImProgressSink {
    const PROGRESS_DEDUPE_WINDOW: Duration = Duration::from_secs(2);

    pub fn new(
        channel: Arc<dyn Channel>,
        sender_channel_instance: Option<String>,
        session_key: SessionKey,
        reply_to: Option<String>,
        thread_ts: Option<String>,
        presentation: ProgressPresentationMode,
        team_orchestrator: Option<Arc<TeamOrchestrator>>,
    ) -> Self {
        Self {
            channel,
            sender_channel_instance,
            reply_to,
            thread_ts,
            session_key,
            presentation,
            recent_progress: Mutex::new(HashMap::new()),
            team_orchestrator,
        }
    }
}

#[async_trait]
impl OutputSink for ImProgressSink {
    async fn send_thinking(&self) -> Option<String> {
        None
    }

    async fn send_delta(&self, _accumulated: &str, _placeholder_id: Option<&str>) {}

    async fn send_progress(&self, progress: &str, _placeholder_id: Option<&str>) {
        if self.presentation != ProgressPresentationMode::ProgressCompact {
            return;
        }
        let now = Instant::now();
        let mut recent = self.recent_progress.lock().await;
        recent.retain(|_, seen_at| now.duration_since(*seen_at) <= Self::PROGRESS_DEDUPE_WINDOW);
        if recent
            .get(progress)
            .is_some_and(|seen_at| now.duration_since(*seen_at) <= Self::PROGRESS_DEDUPE_WINDOW)
        {
            return;
        }
        recent.insert(progress.to_string(), now);
        let msg = OutboundMsg {
            session_key: self.session_key.clone(),
            content: qai_protocol::MsgContent::text(progress),
            reply_to: self.reply_to.clone(),
            thread_ts: self.thread_ts.clone(),
        };
        let send_result = self.channel.send(&msg).await;
        if let Err(e) = &send_result {
            tracing::warn!(channel = %self.channel.name(), "IM send_progress failed: {e}");
        }
        record_team_channel_send(
            self.team_orchestrator.as_ref(),
            &self.session_key,
            self.sender_channel_instance.as_deref(),
            self.reply_to.as_deref(),
            self.thread_ts.as_deref(),
            ChannelSendSourceKind::ToolPlaceholder,
            progress,
            false,
            send_result.as_ref().map(|_| ()).map_err(|e| e.to_string()),
        );
    }

    async fn send_final(&self, text: &str, _placeholder_id: Option<&str>) {
        let msg = OutboundMsg {
            session_key: self.session_key.clone(),
            content: qai_protocol::MsgContent::text(text),
            reply_to: self.reply_to.clone(),
            thread_ts: self.thread_ts.clone(),
        };
        let send_result = self.channel.send(&msg).await;
        if let Err(e) = &send_result {
            tracing::error!(channel = %self.channel.name(), "IM send_final failed: {e}");
        } else {
            tracing::debug!(
                channel = %self.channel.name(),
                text_len = text.len(),
                "IM send_final succeeded"
            );
        }
        record_team_channel_send(
            self.team_orchestrator.as_ref(),
            &self.session_key,
            self.sender_channel_instance.as_deref(),
            self.reply_to.as_deref(),
            self.thread_ts.as_deref(),
            ChannelSendSourceKind::LeadText,
            text,
            false,
            send_result.as_ref().map(|_| ()).map_err(|e| e.to_string()),
        );
    }

    fn progress_for_tool_start(&self, tool_name: &str) -> Option<String> {
        progress_presentation::format_tool_start(self.presentation, tool_name)
    }

    fn progress_for_tool_result(&self, tool_name: Option<&str>) -> Option<String> {
        progress_presentation::format_tool_result(self.presentation, tool_name)
    }

    fn progress_for_tool_failure(&self, tool_name: &str) -> Option<String> {
        progress_presentation::format_tool_failure(self.presentation, tool_name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinalDeliveryDecision {
    Suppress,
    StreamFinal,
    DirectSend,
}

fn decide_final_delivery(
    snapshot_suppressed: bool,
    team_running_after_handle: bool,
) -> FinalDeliveryDecision {
    match (snapshot_suppressed, team_running_after_handle) {
        (_, true) => FinalDeliveryDecision::Suppress,
        (true, false) => FinalDeliveryDecision::DirectSend,
        (false, false) => FinalDeliveryDecision::StreamFinal,
    }
}

fn record_team_channel_send(
    team_orchestrator: Option<&Arc<TeamOrchestrator>>,
    session_key: &SessionKey,
    sender_channel_instance: Option<&str>,
    reply_to: Option<&str>,
    thread_ts: Option<&str>,
    source_kind: ChannelSendSourceKind,
    text: &str,
    record_leader_fragment: bool,
    send_result: std::result::Result<(), String>,
) {
    let Some(team_orchestrator) = team_orchestrator else {
        return;
    };
    if record_leader_fragment && source_kind == ChannelSendSourceKind::LeadText {
        team_orchestrator.record_leader_fragment(
            qai_agent::team::session::LeaderUpdateKind::FinalAnswerFragment,
            text,
        );
    }
    let source_agent = team_orchestrator
        .lead_agent_name
        .get()
        .cloned()
        .unwrap_or_else(|| "leader".to_string());
    let (status, error) = match send_result {
        Ok(()) => (ChannelSendStatus::Sent, None),
        Err(error) => (ChannelSendStatus::SendFailed, Some(error)),
    };
    if let Err(err) = team_orchestrator.session.record_channel_send(
        &session_key.channel,
        sender_channel_instance,
        session_key.channel_instance.as_deref(),
        &session_key.scope,
        team_orchestrator.lead_session_key().as_ref(),
        team_orchestrator.lead_delivery_source().as_ref(),
        reply_to,
        thread_ts,
        source_kind,
        &source_agent,
        None,
        None,
        text,
        status,
        error.as_deref(),
    ) {
        tracing::warn!(
            team_id = %team_orchestrator.session.team_id,
            error = %err,
            "Failed to append channel send ledger entry"
        );
    }
}

fn delivery_parts_or_fallback(
    resolved: Option<ResolvedDelivery>,
    fallback_channel: Arc<dyn Channel>,
    fallback_session_key: SessionKey,
    fallback_reply_to: Option<String>,
    fallback_thread_ts: Option<String>,
) -> (
    Arc<dyn Channel>,
    SessionKey,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    if let Some(resolved) = resolved {
        (
            resolved.sender,
            resolved.session_key,
            resolved.reply_to,
            resolved.thread_ts,
            resolved.sender_channel_instance,
        )
    } else {
        (
            fallback_channel,
            fallback_session_key,
            fallback_reply_to,
            fallback_thread_ts,
            None,
        )
    }
}

pub fn spawn_im_turn(
    registry: Arc<SessionRegistry>,
    channel: Arc<dyn Channel>,
    channel_registry: Arc<ChannelRegistry>,
    cfg: Arc<GatewayConfig>,
    inbound: InboundMsg,
    presentation: ProgressPresentationMode,
) {
    let channel_name = channel.name().to_string();
    let session_key = inbound.session_key.clone();
    let thread_ts = inbound.thread_ts.clone();
    let reply_to = Some(inbound.id.clone());
    let event_rx = registry.global_sender().subscribe();
    let (control_tx, control_rx) = oneshot::channel::<StreamControl>();

    // Capture before spawning tasks to snapshot state consistently (fixes TOCTOU).
    // When true: Lead communicates only via post_update — stream path must be silent.
    let suppress_lead_final = registry.should_suppress_lead_final_reply(&session_key);
    let team_orchestrator_for_session = registry.team_orchestrator_for_session(&session_key);
    let lead_agent_name = team_orchestrator_for_session
        .as_ref()
        .and_then(|orch| orch.lead_agent_name.get().cloned());
    let stored_delivery_source = team_orchestrator_for_session
        .as_ref()
        .and_then(|orch| orch.lead_delivery_source());
    let active_delivery_source = TurnDeliverySource::from_session_key(&session_key)
        .with_reply_context(Some(inbound.id.clone()), inbound.thread_ts.clone());
    let resolved_delivery = resolve_delivery(
        cfg.as_ref(),
        channel_registry.as_ref(),
        DeliveryPurposeConfig::LeadFinal,
        &session_key,
        Some(&active_delivery_source),
        stored_delivery_source.as_ref(),
        lead_agent_name.as_deref(),
        Some(inbound.id.as_str()),
        inbound.thread_ts.as_deref(),
    );
    let turn_ctx = TurnExecutionContext {
        delivery_source: Some(active_delivery_source),
    };

    let registry_for_stream = registry.clone();
    let session_key_for_stream = session_key.clone();
    let channel_name_for_stream = channel_name.clone();
    let team_orchestrator_for_stream = team_orchestrator_for_session.clone();
    let resolved_delivery_for_stream = resolved_delivery.clone();
    let reply_to_for_stream = reply_to.clone();
    let thread_ts_for_stream = thread_ts.clone();
    let fallback_channel_for_stream = channel.clone();
    tokio::spawn(async move {
        let session_id = match registry_for_stream
            .session_manager_ref()
            .get_or_create(&session_key_for_stream)
            .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(channel = %channel_name_for_stream, "get session_id failed: {e}");
                return;
            }
        };

        if suppress_lead_final {
            // Lead in Team mode: all stream output suppressed; post_update is the only channel.
            throttled_stream(event_rx, session_id, &NullSink, None, control_rx).await;
        } else {
            let (
                stream_channel,
                stream_session_key,
                stream_reply_to,
                stream_thread_ts,
                stream_sender_channel_instance,
            ) = delivery_parts_or_fallback(
                resolved_delivery_for_stream,
                fallback_channel_for_stream,
                session_key_for_stream.clone(),
                reply_to_for_stream,
                thread_ts_for_stream,
            );
            let sink = ImProgressSink::new(
                stream_channel,
                stream_sender_channel_instance,
                stream_session_key,
                stream_reply_to,
                stream_thread_ts,
                presentation,
                team_orchestrator_for_stream,
            );
            throttled_stream(event_rx, session_id, &sink, None, control_rx).await;
        }
    });

    let channel_name_for_handle = channel_name.clone();
    let session_key_for_handle = session_key.clone();
    let reply_to_for_handle = Some(inbound.id.clone());
    let thread_ts_for_handle = inbound.thread_ts.clone();
    let team_orchestrator_for_handle = team_orchestrator_for_session;
    let resolved_delivery_for_handle = resolved_delivery;
    let fallback_channel_for_handle = channel.clone();
    tokio::spawn(async move {
        match registry.handle_with_context(inbound, turn_ctx).await {
            Ok(Some(reply)) => {
                match decide_final_delivery(
                    suppress_lead_final,
                    registry.is_team_running_or_done(&session_key_for_handle),
                ) {
                    FinalDeliveryDecision::Suppress => {
                        let _ = control_tx.send(StreamControl::Stop);
                    }
                    FinalDeliveryDecision::DirectSend => {
                        // The stream used NullSink because the turn started in Running.
                        // If Team is no longer running post-handle, send directly.
                        let _ = control_tx.send(StreamControl::Stop);
                        let (
                            send_channel,
                            send_session_key,
                            send_reply_to,
                            send_thread_ts,
                            send_sender_channel_instance,
                        ) = delivery_parts_or_fallback(
                            resolved_delivery_for_handle,
                            fallback_channel_for_handle,
                            session_key_for_handle.clone(),
                            reply_to_for_handle,
                            thread_ts_for_handle,
                        );
                        let msg = OutboundMsg {
                            session_key: send_session_key.clone(),
                            content: qai_protocol::MsgContent::text(&reply),
                            reply_to: send_reply_to,
                            thread_ts: send_thread_ts,
                        };
                        let send_result = send_channel.send(&msg).await;
                        if let Err(e) = &send_result {
                            tracing::error!(channel = %channel_name_for_handle, "lead direct send failed: {e}");
                        }
                        record_team_channel_send(
                            team_orchestrator_for_handle.as_ref(),
                            &send_session_key,
                            send_sender_channel_instance.as_deref(),
                            msg.reply_to.as_deref(),
                            msg.thread_ts.as_deref(),
                            ChannelSendSourceKind::LeadText,
                            &reply,
                            false,
                            send_result.as_ref().map(|_| ()).map_err(|e| e.to_string()),
                        );
                    }
                    FinalDeliveryDecision::StreamFinal => {
                        let _ = control_tx.send(StreamControl::Final(reply));
                    }
                }
            }
            Ok(None) => {
                let _ = control_tx.send(StreamControl::Stop);
            }
            Err(e) => {
                tracing::error!(channel = %channel_name_for_handle, "registry handle error: {e}");
                let _ = control_tx.send(StreamControl::Final(format!("❌ 错误: {e}")));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use qai_agent::team::heartbeat::DispatchFn;
    use qai_agent::team::orchestrator::TeamOrchestrator;
    use qai_agent::team::registry::TaskRegistry;
    use qai_agent::team::session::TeamSession;
    use qai_protocol::MsgContent;
    use std::sync::{Arc, Mutex as StdMutex};
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    struct MockChannel {
        sent: StdMutex<Vec<String>>,
    }

    #[async_trait]
    impl Channel for MockChannel {
        fn name(&self) -> &str {
            "mock"
        }

        async fn send(&self, msg: &OutboundMsg) -> Result<()> {
            let MsgContent::Text { text } = &msg.content else {
                unreachable!()
            };
            self.sent.lock().unwrap().push(text.clone());
            Ok(())
        }

        async fn listen(&self, _tx: mpsc::Sender<InboundMsg>) -> Result<()> {
            Ok(())
        }
    }

    fn sink(presentation: ProgressPresentationMode) -> (ImProgressSink, Arc<MockChannel>) {
        let channel = Arc::new(MockChannel {
            sent: StdMutex::new(Vec::new()),
        });
        let sink = ImProgressSink::new(
            channel.clone(),
            None,
            SessionKey {
                channel: "mock".to_string(),
                scope: "user:test".to_string(),
                channel_instance: None,
            },
            Some("reply-id".to_string()),
            None,
            presentation,
            None,
        );
        (sink, channel)
    }

    fn make_team_orchestrator() -> (Arc<TeamOrchestrator>, tempfile::TempDir) {
        let tmp = tempdir().unwrap();
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let session = Arc::new(TeamSession::from_dir("test-team", tmp.path().to_path_buf()));
        let dispatch_fn: DispatchFn = Arc::new(|_agent, _task| Box::pin(async { Ok(()) }));
        let orchestrator =
            TeamOrchestrator::new(registry, session, dispatch_fn, Duration::from_secs(3600));
        (orchestrator, tmp)
    }

    #[tokio::test]
    async fn final_only_suppresses_progress_messages() {
        let (sink, channel) = sink(ProgressPresentationMode::FinalOnly);
        sink.send_progress("⏳ 正在搜索代码", None).await;
        assert!(channel.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn compact_progress_dedupes_repeated_labels() {
        let (sink, channel) = sink(ProgressPresentationMode::ProgressCompact);
        sink.send_progress("⏳ 正在搜索代码", None).await;
        sink.send_progress("⏳ 正在搜索代码", None).await;
        sink.send_progress("⏳ 正在整理结果", None).await;
        let sent = channel.sent.lock().unwrap().clone();
        assert_eq!(
            sent,
            vec!["⏳ 正在搜索代码".to_string(), "⏳ 正在整理结果".to_string()]
        );
    }

    #[tokio::test]
    async fn compact_progress_dedupes_repeated_labels_within_window() {
        let (sink, channel) = sink(ProgressPresentationMode::ProgressCompact);
        sink.send_progress("⏳ 正在搜索代码", None).await;
        sink.send_progress("⏳ 正在整理结果", None).await;
        sink.send_progress("⏳ 正在搜索代码", None).await;
        let sent = channel.sent.lock().unwrap().clone();
        assert_eq!(
            sent,
            vec!["⏳ 正在搜索代码".to_string(), "⏳ 正在整理结果".to_string()]
        );
    }

    #[tokio::test]
    async fn compact_progress_allows_same_label_after_window() {
        let (sink, channel) = sink(ProgressPresentationMode::ProgressCompact);
        sink.send_progress("⏳ 正在搜索代码", None).await;
        tokio::time::sleep(ImProgressSink::PROGRESS_DEDUPE_WINDOW + Duration::from_millis(20))
            .await;
        sink.send_progress("⏳ 正在搜索代码", None).await;
        let sent = channel.sent.lock().unwrap().clone();
        assert_eq!(
            sent,
            vec!["⏳ 正在搜索代码".to_string(), "⏳ 正在搜索代码".to_string()]
        );
    }

    #[tokio::test]
    async fn send_final_does_not_leave_pending_lead_fragments() {
        let (team_orchestrator, _tmp) = make_team_orchestrator();
        let channel = Arc::new(MockChannel {
            sent: StdMutex::new(Vec::new()),
        });
        let sink = ImProgressSink::new(
            channel,
            None,
            SessionKey {
                channel: "mock".to_string(),
                scope: "user:test".to_string(),
                channel_instance: None,
            },
            Some("reply-id".to_string()),
            None,
            ProgressPresentationMode::FinalOnly,
            Some(team_orchestrator.clone()),
        );

        sink.send_final("hello", None).await;

        assert!(team_orchestrator.take_pending_lead_fragments().is_empty());
    }

    #[test]
    fn final_delivery_rechecks_team_state_after_handle() {
        assert_eq!(
            decide_final_delivery(false, true),
            FinalDeliveryDecision::Suppress
        );
        assert_eq!(
            decide_final_delivery(false, false),
            FinalDeliveryDecision::StreamFinal
        );
        assert_eq!(
            decide_final_delivery(true, false),
            FinalDeliveryDecision::DirectSend
        );
    }
}
