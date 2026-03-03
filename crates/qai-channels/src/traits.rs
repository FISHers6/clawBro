use anyhow::Result;
use async_trait::async_trait;
use qai_protocol::{InboundMsg, OutboundMsg};
use tokio::sync::mpsc;

/// Channel = 消息平台适配器（参考 zeroclaw/src/channels/traits.rs）
#[async_trait]
pub trait Channel: Send + Sync {
    fn name(&self) -> &str;

    /// 发送消息到 channel
    async fn send(&self, msg: &OutboundMsg) -> Result<()>;

    /// 启动监听循环，将收到的消息推送到 tx
    async fn listen(&self, tx: mpsc::Sender<InboundMsg>) -> Result<()>;

    /// openclaw 模式的可选扩展（typing 状态）
    async fn update_typing(&self, _scope: &str) -> Result<()> {
        Ok(())
    }
    async fn finalize_draft(&self, _scope: &str, _text: &str) -> Result<()> {
        Ok(())
    }
}

/// Channel 工厂：按名称创建 channel 实例
pub type BoxChannel = Box<dyn Channel>;
