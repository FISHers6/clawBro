use qai_protocol::SessionKey;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct ChannelRegistry {
    channels: HashMap<(String, Option<String>), Arc<dyn qai_channels::Channel>>,
    defaults: HashMap<String, Arc<dyn qai_channels::Channel>>,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        channel: impl Into<String>,
        channel_instance: Option<impl Into<String>>,
        sender: Arc<dyn qai_channels::Channel>,
        is_default: bool,
    ) {
        let channel = channel.into();
        let channel_instance = channel_instance.map(|value| value.into());
        self.channels.insert(
            (channel.clone(), channel_instance.clone()),
            Arc::clone(&sender),
        );
        if is_default || channel_instance.is_none() {
            self.defaults.insert(channel, sender);
        }
    }

    pub fn resolve(
        &self,
        channel: &str,
        channel_instance: Option<&str>,
    ) -> Option<Arc<dyn qai_channels::Channel>> {
        let requested_instance = channel_instance.map(str::to_string);
        self.channels
            .get(&(channel.to_string(), requested_instance))
            .cloned()
            .or_else(|| self.defaults.get(channel).cloned())
    }

    pub fn resolve_for_session(
        &self,
        session_key: &SessionKey,
    ) -> Option<Arc<dyn qai_channels::Channel>> {
        self.resolve(
            &session_key.channel,
            session_key.channel_instance.as_deref(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use async_trait::async_trait;
    use qai_protocol::{InboundMsg, OutboundMsg};
    use tokio::sync::mpsc;

    struct TestChannel {
        name: &'static str,
    }

    #[async_trait]
    impl qai_channels::Channel for TestChannel {
        fn name(&self) -> &str {
            self.name
        }

        async fn send(&self, _msg: &OutboundMsg) -> Result<()> {
            Ok(())
        }

        async fn listen(&self, _tx: mpsc::Sender<InboundMsg>) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn default_instance_is_used_when_session_key_has_no_instance() {
        let mut registry = ChannelRegistry::new();
        registry.register(
            "lark",
            Some("default"),
            Arc::new(TestChannel { name: "default" }),
            true,
        );

        let resolved = registry
            .resolve_for_session(&SessionKey::new("lark", "group:oc_1"))
            .expect("default sender");
        assert_eq!(resolved.name(), "default");
    }

    #[test]
    fn explicit_instance_resolves_requested_sender() {
        let mut registry = ChannelRegistry::new();
        registry.register(
            "lark",
            Some("alpha"),
            Arc::new(TestChannel { name: "alpha" }),
            true,
        );
        registry.register(
            "lark",
            Some("beta"),
            Arc::new(TestChannel { name: "beta" }),
            false,
        );

        let resolved = registry
            .resolve_for_session(&SessionKey::with_instance("lark", "beta", "group:oc_1"))
            .expect("beta sender");
        assert_eq!(resolved.name(), "beta");
    }

    #[test]
    fn legacy_single_instance_registration_can_become_channel_default() {
        let mut registry = ChannelRegistry::new();
        registry.register(
            "lark",
            Option::<String>::None,
            Arc::new(TestChannel { name: "legacy" }),
            true,
        );

        let resolved = registry
            .resolve("lark", Some("default"))
            .expect("legacy sender");
        assert_eq!(resolved.name(), "legacy");
    }
}
