//! OutputSink trait: unified IM / WS output strategy
//! throttled_stream: 500ms debounce for streaming delta updates

use async_trait::async_trait;
use qai_protocol::AgentEvent;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::broadcast;

#[async_trait]
pub trait OutputSink: Send + Sync {
    /// Send "thinking..." placeholder message; returns message ID for later edits
    async fn send_thinking(&self) -> Option<String>;
    /// Update with accumulated text (called at 500ms intervals during streaming)
    async fn send_delta(&self, accumulated: &str, placeholder_id: Option<&str>);
    /// Send compact progress updates for channels that choose to expose them.
    async fn send_progress(&self, _progress: &str, _placeholder_id: Option<&str>) {}
    /// Send a mid-turn text segment (agent spoke before calling a tool).
    /// This is a complete message unit, but the turn is not yet finished.
    /// Default implementation delegates to send_final (correct for IM channels).
    async fn send_segment(&self, text: &str) {
        self.send_final(text, None).await;
    }
    /// Send final complete reply (replaces placeholder or sends new message)
    async fn send_final(&self, text: &str, placeholder_id: Option<&str>);
    /// Map a tool start event into a channel-specific compact progress label.
    fn progress_for_tool_start(&self, _tool_name: &str) -> Option<String> {
        None
    }
    /// Map a tool completion event into a channel-specific compact progress label.
    fn progress_for_tool_result(&self, _tool_name: Option<&str>) -> Option<String> {
        None
    }
    /// Map a tool failure event into a channel-specific compact progress label.
    fn progress_for_tool_failure(&self, _tool_name: &str) -> Option<String> {
        None
    }
}

/// Explicit stream completion signal from the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamControl {
    Stop,
    Final(String),
}

/// Consume events from `event_rx`, calling `sink` at 500ms intervals for TextDelta,
/// and `send_final` on TurnComplete or Error.
///
/// The `control` receiver allows the caller to signal explicit completion semantics:
/// - `Stop` for no-op / dedup turns
/// - `Final(text)` for synchronous control-plane replies
/// - dropped sender for generic early cancellation
///
/// Usage:
/// ```ignore
/// let (control_tx, control_rx) =
///     tokio::sync::oneshot::channel::<StreamControl>();
/// let placeholder_id = sink.send_thinking().await;
/// let _ = control_tx.send(StreamControl::Final("done".to_string()));
/// throttled_stream(event_rx, session_id, sink, placeholder_id, control_rx).await;
/// ```
pub async fn throttled_stream(
    mut event_rx: broadcast::Receiver<AgentEvent>,
    target_session_id: uuid::Uuid,
    sink: &dyn OutputSink,
    placeholder_id: Option<String>,
    mut control: tokio::sync::oneshot::Receiver<StreamControl>,
) {
    let throttle = Duration::from_millis(500);
    let mut accumulated = String::new();
    // placeholder is mutable: cleared after a mid-turn segment flush so the
    // next segment is sent as a fresh message rather than editing the old one.
    let mut current_placeholder: Option<String> = placeholder_id;
    let mut active_tools = HashMap::<String, String>::new();
    let mut last_progress: Option<String> = None;
    // Set to true once we have flushed at least one mid-turn segment.
    // When true, TurnComplete must NOT use full_text (which is the entire
    // turn's text and would re-send already-delivered segments). Instead it
    // sends only the remaining accumulated text since the last flush.
    let mut has_flushed_segment = false;

    loop {
        let placeholder = current_placeholder.as_deref();
        tokio::select! {
            biased; // check explicit control first to avoid stale placeholder updates
            signal = &mut control => {
                match signal {
                    Ok(StreamControl::Stop) | Err(_) => break,
                    Ok(StreamControl::Final(text)) => {
                        sink.send_final(&text, placeholder).await;
                        break;
                    }
                }
            },
            event = event_rx.recv() => {
                match event {
                    Ok(e) => match e {
                        AgentEvent::TextDelta { session_id, delta }
                            if session_id == target_session_id =>
                        {
                            accumulated.push_str(&delta);
                        }
                        AgentEvent::ToolCallStart {
                            session_id,
                            tool_name,
                            call_id,
                        } if session_id == target_session_id => {
                            // Flush any text the agent produced before this tool call as a
                            // separate message. This mirrors AionUi's resetMessageTracking()
                            // on tool_call: the agent's pre-tool narration is a distinct
                            // message from the post-tool answer.
                            if !accumulated.is_empty() {
                                sink.send_segment(&accumulated).await;
                                accumulated.clear();
                                current_placeholder = None;
                                last_progress = None;
                                has_flushed_segment = true;
                            }
                            active_tools.insert(call_id, tool_name.clone());
                            if let Some(progress) = sink.progress_for_tool_start(&tool_name) {
                                if last_progress.as_deref() != Some(progress.as_str()) {
                                    sink.send_progress(&progress, current_placeholder.as_deref()).await;
                                    last_progress = Some(progress);
                                }
                            }
                        }
                        AgentEvent::ToolCallResult {
                            session_id,
                            call_id,
                            ..
                        } if session_id == target_session_id => {
                            let tool_name = active_tools.remove(&call_id);
                            if let Some(progress) =
                                sink.progress_for_tool_result(tool_name.as_deref())
                            {
                                if last_progress.as_deref() != Some(progress.as_str()) {
                                    sink.send_progress(&progress, current_placeholder.as_deref()).await;
                                    last_progress = Some(progress);
                                }
                            }
                        }
                        AgentEvent::ToolCallFailed {
                            session_id,
                            tool_name,
                            call_id,
                            ..
                        } if session_id == target_session_id => {
                            active_tools.remove(&call_id);
                            if let Some(progress) = sink.progress_for_tool_failure(&tool_name) {
                                if last_progress.as_deref() != Some(progress.as_str()) {
                                    sink.send_progress(&progress, current_placeholder.as_deref()).await;
                                    last_progress = Some(progress);
                                }
                            }
                        }
                        AgentEvent::TurnComplete {
                            session_id,
                            full_text,
                            ..
                        } if session_id == target_session_id => {
                            if has_flushed_segment {
                                // Mid-turn segments were already delivered. Only send
                                // the text accumulated since the last flush (post-tool
                                // answer). full_text contains the entire turn and must
                                // not be re-sent lest pre-tool narration is duplicated.
                                if !accumulated.is_empty() {
                                    sink.send_final(&accumulated, placeholder).await;
                                }
                            } else {
                                // No segmentation happened: full_text is authoritative
                                // (backend may emit a richer final text than the sum of
                                // TextDelta events, e.g. reasoning models).
                                sink.send_final(&full_text, placeholder).await;
                            }
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
        async fn send_progress(&self, progress: &str, _placeholder: Option<&str>) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("progress:{progress}"));
        }
        async fn send_segment(&self, text: &str) {
            self.calls.lock().unwrap().push(format!("segment:{text}"));
        }
        async fn send_final(&self, text: &str, _placeholder: Option<&str>) {
            self.calls.lock().unwrap().push(format!("final:{text}"));
        }
        fn progress_for_tool_start(&self, tool_name: &str) -> Option<String> {
            Some(format!("start:{tool_name}"))
        }
        fn progress_for_tool_result(&self, tool_name: Option<&str>) -> Option<String> {
            Some(format!("result:{}", tool_name.unwrap_or("unknown")))
        }
        fn progress_for_tool_failure(&self, tool_name: &str) -> Option<String> {
            Some(format!("failed:{tool_name}"))
        }
    }

    /// Returns a live cancel channel. The returned Sender must be kept alive for the
    /// duration of the stream (dropping it resolves the receiver, triggering cancel).
    fn make_cancel() -> (
        tokio::sync::oneshot::Sender<StreamControl>,
        tokio::sync::oneshot::Receiver<StreamControl>,
    ) {
        tokio::sync::oneshot::channel::<StreamControl>()
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
        cancel_tx.send(StreamControl::Stop).unwrap();
        throttled_stream(rx, session_id, &sink, None, cancel_rx).await;

        let recorded = calls.lock().unwrap().clone();
        assert!(
            !recorded.iter().any(|s| s.starts_with("final:")),
            "cancelled stream must not call send_final"
        );
    }

    #[tokio::test]
    async fn test_throttled_stream_control_final_sends_sync_reply() {
        let (_tx, rx) = broadcast::channel::<AgentEvent>(16);
        let session_id = Uuid::new_v4();
        let calls = Arc::new(Mutex::new(vec![]));
        let sink = MockSink {
            calls: calls.clone(),
        };

        let (control_tx, control_rx) = make_cancel();
        control_tx
            .send(StreamControl::Final("sync control reply".to_string()))
            .unwrap();
        throttled_stream(rx, session_id, &sink, None, control_rx).await;

        let recorded = calls.lock().unwrap().clone();
        assert!(
            recorded.iter().any(|s| s == "final:sync control reply"),
            "explicit final control signal must send final reply"
        );
    }

    #[tokio::test]
    async fn test_throttled_stream_emits_compact_progress_events() {
        let (tx, rx) = broadcast::channel(16);
        let session_id = Uuid::new_v4();
        let calls = Arc::new(Mutex::new(vec![]));
        let sink = MockSink {
            calls: calls.clone(),
        };

        let _ = tx.send(AgentEvent::ToolCallStart {
            session_id,
            tool_name: "View".to_string(),
            call_id: "call-1".to_string(),
        });
        let _ = tx.send(AgentEvent::ToolCallResult {
            session_id,
            call_id: "call-1".to_string(),
            result: "ok".to_string(),
        });
        let _ = tx.send(AgentEvent::TurnComplete {
            session_id,
            full_text: "done".to_string(),
            sender: None,
        });

        let (_keep, cancel_rx) = make_cancel();
        throttled_stream(rx, session_id, &sink, None, cancel_rx).await;

        let recorded = calls.lock().unwrap().clone();
        assert!(recorded.iter().any(|s| s == "progress:start:View"));
        assert!(recorded.iter().any(|s| s == "progress:result:View"));
        assert!(recorded.iter().any(|s| s == "final:done"));
    }

    #[tokio::test]
    async fn test_throttled_stream_dedupes_repeated_progress_labels() {
        let (tx, rx) = broadcast::channel(16);
        let session_id = Uuid::new_v4();
        let calls = Arc::new(Mutex::new(vec![]));
        let sink = MockSink {
            calls: calls.clone(),
        };

        let _ = tx.send(AgentEvent::ToolCallStart {
            session_id,
            tool_name: "View".to_string(),
            call_id: "call-1".to_string(),
        });
        let _ = tx.send(AgentEvent::ToolCallStart {
            session_id,
            tool_name: "View".to_string(),
            call_id: "call-2".to_string(),
        });
        let _ = tx.send(AgentEvent::TurnComplete {
            session_id,
            full_text: "done".to_string(),
            sender: None,
        });

        let (_keep, cancel_rx) = make_cancel();
        throttled_stream(rx, session_id, &sink, None, cancel_rx).await;

        let recorded = calls.lock().unwrap().clone();
        let progress_count = recorded
            .iter()
            .filter(|s| *s == "progress:start:View")
            .count();
        assert_eq!(
            progress_count, 1,
            "duplicate compact progress should be coalesced"
        );
    }

    /// Agent says something, calls a tool, says something else.
    /// The pre-tool text must be delivered as a segment BEFORE the tool starts,
    /// and the post-tool text as a separate final message.
    #[tokio::test]
    async fn test_pre_tool_text_flushed_as_segment() {
        let (tx, rx) = broadcast::channel(32);
        let session_id = Uuid::new_v4();
        let calls = Arc::new(Mutex::new(vec![]));
        let sink = MockSink {
            calls: calls.clone(),
        };

        // Agent: "I'll look at the file..."
        let _ = tx.send(AgentEvent::TextDelta {
            session_id,
            delta: "I'll look at the file...".to_string(),
        });
        // Tool call arrives — should flush the pre-tool text first
        let _ = tx.send(AgentEvent::ToolCallStart {
            session_id,
            tool_name: "View".to_string(),
            call_id: "call-1".to_string(),
        });
        let _ = tx.send(AgentEvent::ToolCallResult {
            session_id,
            call_id: "call-1".to_string(),
            result: "file content".to_string(),
        });
        // TextDelta events after the tool call carry the post-tool text.
        // TurnComplete.full_text is the ENTIRE turn (pre+post), which must NOT
        // be re-sent when segments have already been flushed.
        let _ = tx.send(AgentEvent::TextDelta {
            session_id,
            delta: "Here's what I found.".to_string(),
        });
        let _ = tx.send(AgentEvent::TurnComplete {
            session_id,
            full_text: "I'll look at the file...Here's what I found.".to_string(),
            sender: None,
        });

        let (_keep, cancel_rx) = make_cancel();
        throttled_stream(rx, session_id, &sink, None, cancel_rx).await;

        let recorded = calls.lock().unwrap().clone();
        // Pre-tool text flushed as segment
        assert!(
            recorded.iter().any(|s| s == "segment:I'll look at the file..."),
            "pre-tool text must be sent as a segment before tool call; got: {recorded:?}"
        );
        // Post-tool answer sent as final (accumulated since last flush, not full_text)
        assert!(
            recorded.iter().any(|s| s == "final:Here's what I found."),
            "post-tool text must be sent as final at TurnComplete; got: {recorded:?}"
        );
        // full_text must NOT be re-sent (would duplicate pre-tool narration)
        assert!(
            !recorded.iter().any(|s| s.contains("I'll look at the file...Here's")),
            "full_text must not be re-sent when segments already flushed; got: {recorded:?}"
        );
        // Segment must appear before the final
        let seg_pos = recorded
            .iter()
            .position(|s| s == "segment:I'll look at the file...")
            .unwrap();
        let fin_pos = recorded
            .iter()
            .position(|s| s == "final:Here's what I found.")
            .unwrap();
        assert!(seg_pos < fin_pos, "segment must precede final");
    }

    /// When there is no pre-tool text, no spurious segment message is sent.
    #[tokio::test]
    async fn test_no_segment_when_tool_call_starts_without_prior_text() {
        let (tx, rx) = broadcast::channel(16);
        let session_id = Uuid::new_v4();
        let calls = Arc::new(Mutex::new(vec![]));
        let sink = MockSink {
            calls: calls.clone(),
        };

        // Tool call without any preceding text
        let _ = tx.send(AgentEvent::ToolCallStart {
            session_id,
            tool_name: "grep".to_string(),
            call_id: "call-1".to_string(),
        });
        let _ = tx.send(AgentEvent::TurnComplete {
            session_id,
            full_text: "Result.".to_string(),
            sender: None,
        });

        let (_keep, cancel_rx) = make_cancel();
        throttled_stream(rx, session_id, &sink, None, cancel_rx).await;

        let recorded = calls.lock().unwrap().clone();
        assert!(
            !recorded.iter().any(|s| s.starts_with("segment:")),
            "no segment should be emitted when there is no pre-tool text; got: {recorded:?}"
        );
        assert!(recorded.iter().any(|s| s == "final:Result."));
    }
}
