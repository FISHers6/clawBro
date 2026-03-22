use crate::protocol::{InboundMsg, OutboundMsg};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

use super::traits::Channel;

/// No-op Channel implementation for web chat via WebSocket.
///
/// Web chat responses are already broadcast via the global `event_tx` WS broadcast,
/// so no IM delivery is needed. This stub satisfies the `Arc<dyn Channel>` requirement
/// of `spawn_im_turn` without performing any actual IO.
pub struct WsVirtualChannel;

#[async_trait]
impl Channel for WsVirtualChannel {
    fn name(&self) -> &str {
        "ws"
    }

    async fn send(&self, _msg: &OutboundMsg) -> Result<()> {
        Ok(())
    }

    async fn listen(&self, _tx: mpsc::Sender<InboundMsg>) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{MsgContent, SessionKey};

    #[test]
    fn ws_virtual_channel_name_is_ws() {
        let ch = WsVirtualChannel;
        assert_eq!(ch.name(), "ws");
    }

    #[tokio::test]
    async fn ws_virtual_channel_send_is_noop() {
        let ch = WsVirtualChannel;
        let msg = OutboundMsg {
            session_key: SessionKey::new("ws", "user:test"),
            content: MsgContent::text("hello"),
            reply_to: None,
            thread_ts: None,
        };
        ch.send(&msg).await.unwrap();
    }
}
