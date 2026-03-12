use crate::config::ProgressPresentationMode;
use crate::progress_presentation;
use async_trait::async_trait;
use qai_agent::{throttled_stream, OutputSink, SessionRegistry, StreamControl};
use qai_channels::Channel;
use qai_protocol::{InboundMsg, OutboundMsg, SessionKey};
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};

pub struct ImProgressSink {
    channel: Arc<dyn Channel>,
    reply_to: Option<String>,
    thread_ts: Option<String>,
    session_key: SessionKey,
    presentation: ProgressPresentationMode,
    last_progress: Mutex<Option<String>>,
}

impl ImProgressSink {
    pub fn new(
        channel: Arc<dyn Channel>,
        session_key: SessionKey,
        reply_to: Option<String>,
        thread_ts: Option<String>,
        presentation: ProgressPresentationMode,
    ) -> Self {
        Self {
            channel,
            reply_to,
            thread_ts,
            session_key,
            presentation,
            last_progress: Mutex::new(None),
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
        let mut last = self.last_progress.lock().await;
        if last.as_deref() == Some(progress) {
            return;
        }
        *last = Some(progress.to_string());
        let msg = OutboundMsg {
            session_key: self.session_key.clone(),
            content: qai_protocol::MsgContent::text(progress),
            reply_to: self.reply_to.clone(),
            thread_ts: self.thread_ts.clone(),
        };
        if let Err(e) = self.channel.send(&msg).await {
            tracing::warn!(channel = %self.channel.name(), "IM send_progress failed: {e}");
        }
    }

    async fn send_final(&self, text: &str, _placeholder_id: Option<&str>) {
        let msg = OutboundMsg {
            session_key: self.session_key.clone(),
            content: qai_protocol::MsgContent::text(text),
            reply_to: self.reply_to.clone(),
            thread_ts: self.thread_ts.clone(),
        };
        if let Err(e) = self.channel.send(&msg).await {
            tracing::error!(channel = %self.channel.name(), "IM send_final failed: {e}");
        } else {
            tracing::debug!(
                channel = %self.channel.name(),
                text_len = text.len(),
                "IM send_final succeeded"
            );
        }
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

pub fn spawn_im_turn(
    registry: Arc<SessionRegistry>,
    channel: Arc<dyn Channel>,
    inbound: InboundMsg,
    presentation: ProgressPresentationMode,
) {
    let channel_name = channel.name().to_string();
    let session_key = inbound.session_key.clone();
    let thread_ts = inbound.thread_ts.clone();
    let reply_to = Some(inbound.id.clone());
    let event_rx = registry.global_sender().subscribe();
    let (control_tx, control_rx) = oneshot::channel::<StreamControl>();

    let registry_for_stream = registry.clone();
    let channel_for_stream = channel.clone();
    let session_key_for_stream = session_key.clone();
    let channel_name_for_stream = channel_name.clone();
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

        let sink = ImProgressSink::new(
            channel_for_stream,
            session_key_for_stream,
            reply_to,
            thread_ts,
            presentation,
        );
        throttled_stream(event_rx, session_id, &sink, None, control_rx).await;
    });

    let channel_name_for_handle = channel_name.clone();
    tokio::spawn(async move {
        match registry.handle(inbound).await {
            Ok(Some(reply)) => {
                let _ = control_tx.send(StreamControl::Final(reply));
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
    use qai_protocol::MsgContent;
    use std::sync::{Arc, Mutex as StdMutex};
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
            SessionKey {
                channel: "mock".to_string(),
                scope: "user:test".to_string(),
            },
            Some("reply-id".to_string()),
            None,
            presentation,
        );
        (sink, channel)
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
}
