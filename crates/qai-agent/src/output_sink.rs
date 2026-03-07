//! OutputSink trait: unified IM / WS output strategy
//! throttled_stream: 500ms debounce for streaming delta updates

use async_trait::async_trait;
use qai_protocol::AgentEvent;
use std::time::Duration;
use tokio::sync::broadcast;

#[async_trait]
pub trait OutputSink: Send + Sync {
    /// Send "thinking..." placeholder message; returns message ID for later edits
    async fn send_thinking(&self) -> Option<String>;
    /// Update with accumulated text (called at 500ms intervals during streaming)
    async fn send_delta(&self, accumulated: &str, placeholder_id: Option<&str>);
    /// Send final complete reply (replaces placeholder or sends new message)
    async fn send_final(&self, text: &str, placeholder_id: Option<&str>);
}

/// Consume events from `event_rx`, calling `sink` at 500ms intervals for TextDelta,
/// and `send_final` on TurnComplete or Error.
///
/// The `cancel` receiver allows the caller to signal early termination — e.g. when
/// `handle()` returns `Ok(None)` (dedup hit) and no `TurnComplete` event will ever arrive.
/// Dropping the paired `Sender` also cancels the stream.
///
/// Usage:
/// ```ignore
/// let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
/// let placeholder_id = sink.send_thinking().await;
/// throttled_stream(event_rx, session_id, sink, placeholder_id, cancel_rx).await;
/// ```
pub async fn throttled_stream(
    mut event_rx: broadcast::Receiver<AgentEvent>,
    target_session_id: uuid::Uuid,
    sink: &dyn OutputSink,
    placeholder_id: Option<String>,
    mut cancel: tokio::sync::oneshot::Receiver<()>,
) {
    let throttle = Duration::from_millis(500);
    let mut accumulated = String::new();
    let placeholder = placeholder_id.as_deref();

    loop {
        tokio::select! {
            biased; // check cancel first to avoid processing stale events after cancellation
            _ = &mut cancel => break,
            event = event_rx.recv() => {
                match event {
                    Ok(e) => match e {
                        AgentEvent::TextDelta { session_id, delta }
                            if session_id == target_session_id =>
                        {
                            accumulated.push_str(&delta);
                        }
                        AgentEvent::TurnComplete {
                            session_id,
                            full_text,
                            ..
                        } if session_id == target_session_id => {
                            sink.send_final(&full_text, placeholder).await;
                            break;
                        }
                        AgentEvent::Error {
                            session_id,
                            message,
                        } if session_id == target_session_id => {
                            sink.send_final(&format!("❌ 错误: {message}"), placeholder)
                                .await;
                            break;
                        }
                        _ => {} // filter other sessions or irrelevant events
                    },
                    Err(_) => break, // channel closed or lagged (broadcast overflow)
                }
            }
            _ = tokio::time::sleep(throttle) => {
                // Flush accumulated text at 500ms intervals during streaming
                if !accumulated.is_empty() {
                    sink.send_delta(&accumulated, placeholder).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    struct MockSink {
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl OutputSink for MockSink {
        async fn send_thinking(&self) -> Option<String> {
            self.calls.lock().unwrap().push("thinking".to_string());
            Some("placeholder_id".to_string())
        }
        async fn send_delta(&self, accumulated: &str, _placeholder: Option<&str>) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("delta:{accumulated}"));
        }
        async fn send_final(&self, text: &str, _placeholder: Option<&str>) {
            self.calls.lock().unwrap().push(format!("final:{text}"));
        }
    }

    /// Returns a live cancel channel. The returned Sender must be kept alive for the
    /// duration of the stream (dropping it resolves the receiver, triggering cancel).
    fn make_cancel() -> (
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        tokio::sync::oneshot::channel::<()>()
    }

    #[tokio::test]
    async fn test_throttled_stream_turn_complete() {
        let (tx, rx) = broadcast::channel(16);
        let session_id = Uuid::new_v4();
        let calls = Arc::new(Mutex::new(vec![]));
        let sink = MockSink {
            calls: calls.clone(),
        };

        let _ = tx.send(AgentEvent::TextDelta {
            session_id,
            delta: "hello".to_string(),
        });
        let _ = tx.send(AgentEvent::TurnComplete {
            session_id,
            full_text: "hello world".to_string(),
            sender: None,
        });

        let (_keep, cancel_rx) = make_cancel();
        throttled_stream(rx, session_id, &sink, None, cancel_rx).await;

        let recorded = calls.lock().unwrap().clone();
        assert!(recorded.iter().any(|s| s.starts_with("final:")));
        let final_msg = recorded.iter().find(|s| s.starts_with("final:")).unwrap();
        assert!(final_msg.contains("hello world"));
    }

    #[tokio::test]
    async fn test_throttled_stream_ignores_other_sessions() {
        let (tx, rx) = broadcast::channel(16);
        let my_session = Uuid::new_v4();
        let other_session = Uuid::new_v4();
        let calls = Arc::new(Mutex::new(vec![]));
        let sink = MockSink {
            calls: calls.clone(),
        };

        let _ = tx.send(AgentEvent::TextDelta {
            session_id: other_session,
            delta: "noise".to_string(),
        });
        let _ = tx.send(AgentEvent::TurnComplete {
            session_id: my_session,
            full_text: "my reply".to_string(),
            sender: None,
        });

        let (_keep, cancel_rx) = make_cancel();
        throttled_stream(rx, my_session, &sink, None, cancel_rx).await;

        let recorded = calls.lock().unwrap().clone();
        let final_msg = recorded.iter().find(|s| s.starts_with("final:")).unwrap();
        assert!(final_msg.contains("my reply"));
        assert!(!recorded.iter().any(|s| s.contains("noise")));
    }

    #[tokio::test]
    async fn test_throttled_stream_cancel_exits_without_final() {
        let (_tx, rx) = broadcast::channel::<AgentEvent>(16);
        let session_id = Uuid::new_v4();
        let calls = Arc::new(Mutex::new(vec![]));
        let sink = MockSink {
            calls: calls.clone(),
        };

        // Send cancel immediately — stream should exit without calling send_final
        let (cancel_tx, cancel_rx) = make_cancel();
        cancel_tx.send(()).unwrap();
        throttled_stream(rx, session_id, &sink, None, cancel_rx).await;

        let recorded = calls.lock().unwrap().clone();
        assert!(
            !recorded.iter().any(|s| s.starts_with("final:")),
            "cancelled stream must not call send_final"
        );
    }
}
