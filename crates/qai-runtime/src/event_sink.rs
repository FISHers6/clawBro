use crate::contract::RuntimeEvent;
use anyhow::Context;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct RuntimeEventSink {
    tx: mpsc::UnboundedSender<RuntimeEvent>,
}

impl RuntimeEventSink {
    pub fn new(tx: mpsc::UnboundedSender<RuntimeEvent>) -> Self {
        Self { tx }
    }

    pub fn emit(&self, event: RuntimeEvent) -> anyhow::Result<()> {
        self.tx
            .send(event)
            .context("runtime event receiver dropped")
    }
}
