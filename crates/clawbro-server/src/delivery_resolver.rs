use crate::channel_registry::ChannelRegistry;
use crate::config::{
    DeliveryPurposeConfig, DeliverySenderBindingConfig, DeliveryTargetOverrideConfig, GatewayConfig,
};
use crate::agent_core::TurnDeliverySource;
use crate::protocol::{OutboundMsg, SessionKey};
use std::sync::Arc;

#[derive(Clone)]
pub struct ResolvedDelivery {
    pub sender: Arc<dyn crate::channels_internal::Channel>,
    pub sender_channel: String,
    pub sender_channel_instance: Option<String>,
    pub session_key: SessionKey,
    pub reply_to: Option<String>,
    pub thread_ts: Option<String>,
}

impl ResolvedDelivery {
    pub fn outbound_text(&self, text: impl AsRef<str>) -> OutboundMsg {
        OutboundMsg {
            session_key: self.session_key.clone(),
            content: crate::protocol::MsgContent::text(text.as_ref()),
            reply_to: self.reply_to.clone(),
            thread_ts: self.thread_ts.clone(),
        }
    }
}

pub fn resolve_delivery(
    cfg: &GatewayConfig,
    channels: &ChannelRegistry,
    purpose: DeliveryPurposeConfig,
    natural_session_key: &SessionKey,
    active_source: Option<&TurnDeliverySource>,
    stored_source: Option<&TurnDeliverySource>,
    agent: Option<&str>,
    default_reply_to: Option<&str>,
    default_thread_ts: Option<&str>,
) -> Option<ResolvedDelivery> {
    let mut target_session_key = active_source
        .or(stored_source)
        .map(TurnDeliverySource::session_key)
        .unwrap_or_else(|| natural_session_key.clone());
    let mut reply_to = active_source
        .and_then(|source| source.reply_to.clone())
        .or_else(|| stored_source.and_then(|source| source.reply_to.clone()))
        .or_else(|| default_reply_to.map(ToOwned::to_owned));
    let mut thread_ts = active_source
        .and_then(|source| source.thread_ts.clone())
        .or_else(|| stored_source.and_then(|source| source.thread_ts.clone()))
        .or_else(|| default_thread_ts.map(ToOwned::to_owned));

    if let Some(target_override) =
        matching_target_override(&cfg.delivery_target_overrides, purpose, agent)
    {
        if let Some(channel) = &target_override.channel {
            target_session_key.channel = channel.clone();
        }
        target_session_key.channel_instance = target_override
            .channel_instance
            .clone()
            .or(target_session_key.channel_instance.clone());
        target_session_key.scope = target_override.scope.clone();
        if let Some(override_reply_to) = &target_override.reply_to {
            reply_to = Some(override_reply_to.clone());
        }
        if let Some(override_thread_ts) = &target_override.thread_ts {
            thread_ts = Some(override_thread_ts.clone());
        }
    }

    let sender_channel = target_session_key.channel.clone();
    let sender_channel_instance = matching_sender_binding(
        &cfg.delivery_sender_bindings,
        purpose,
        agent,
        &sender_channel,
    )
    .map(|binding| binding.channel_instance.clone())
    .or_else(|| target_session_key.channel_instance.clone());
    let sender = channels.resolve(&sender_channel, sender_channel_instance.as_deref())?;

    Some(ResolvedDelivery {
        sender,
        sender_channel,
        sender_channel_instance,
        session_key: target_session_key,
        reply_to,
        thread_ts,
    })
}

fn matching_sender_binding<'a>(
    bindings: &'a [DeliverySenderBindingConfig],
    purpose: DeliveryPurposeConfig,
    agent: Option<&str>,
    channel: &str,
) -> Option<&'a DeliverySenderBindingConfig> {
    bindings.iter().rev().find(|binding| {
        binding.purpose == purpose
            && binding
                .agent
                .as_deref()
                .map(|expected| Some(expected) == agent)
                .unwrap_or(true)
            && binding
                .channel
                .as_deref()
                .map(|expected| expected == channel)
                .unwrap_or(true)
    })
}

fn matching_target_override<'a>(
    overrides: &'a [DeliveryTargetOverrideConfig],
    purpose: DeliveryPurposeConfig,
    agent: Option<&str>,
) -> Option<&'a DeliveryTargetOverrideConfig> {
    overrides.iter().rev().find(|override_cfg| {
        override_cfg.purpose == purpose
            && override_cfg
                .agent
                .as_deref()
                .map(|expected| Some(expected) == agent)
                .unwrap_or(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_registry::ChannelRegistry;
    use anyhow::Result;
    use async_trait::async_trait;
    use crate::protocol::{InboundMsg, SessionKey};
    use tokio::sync::mpsc;

    struct TestChannel {
        name: &'static str,
    }

    #[async_trait]
    impl crate::channels_internal::Channel for TestChannel {
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

    fn test_cfg() -> GatewayConfig {
        GatewayConfig::default()
    }

    fn test_channels() -> ChannelRegistry {
        let mut channels = ChannelRegistry::new();
        channels.register(
            "lark",
            Some("alpha"),
            Arc::new(TestChannel { name: "alpha" }),
            true,
        );
        channels.register(
            "lark",
            Some("beta"),
            Arc::new(TestChannel { name: "beta" }),
            false,
        );
        channels
    }

    #[test]
    fn lead_final_prefers_active_turn_source() {
        let cfg = test_cfg();
        let channels = test_channels();
        let natural = SessionKey::with_instance("lark", "alpha", "group:oc_1");
        let active = TurnDeliverySource::from_session_key(&SessionKey::with_instance(
            "lark",
            "beta",
            "group:oc_1",
        ))
        .with_reply_context(Some("om_1".into()), Some("th_1".into()));
        let stored = TurnDeliverySource::from_session_key(&natural);

        let resolved = resolve_delivery(
            &cfg,
            &channels,
            DeliveryPurposeConfig::LeadFinal,
            &natural,
            Some(&active),
            Some(&stored),
            None,
            None,
            None,
        )
        .expect("resolved");

        assert_eq!(resolved.sender.name(), "beta");
        assert_eq!(
            resolved.session_key.channel_instance.as_deref(),
            Some("beta")
        );
        assert_eq!(resolved.reply_to.as_deref(), Some("om_1"));
        assert_eq!(resolved.thread_ts.as_deref(), Some("th_1"));
    }

    #[test]
    fn milestone_sender_binding_changes_sender_but_inherits_target() {
        let mut cfg = test_cfg();
        cfg.delivery_sender_bindings
            .push(DeliverySenderBindingConfig {
                purpose: DeliveryPurposeConfig::Milestone,
                agent: Some("beta-agent".into()),
                channel: Some("lark".into()),
                channel_instance: "beta".into(),
            });
        let channels = test_channels();
        let natural = SessionKey::new("lark", "group:oc_1");
        let stored = TurnDeliverySource::from_session_key(&SessionKey::with_instance(
            "lark",
            "alpha",
            "group:oc_1",
        ))
        .with_reply_context(None, Some("th_1".into()));

        let resolved = resolve_delivery(
            &cfg,
            &channels,
            DeliveryPurposeConfig::Milestone,
            &natural,
            None,
            Some(&stored),
            Some("beta-agent"),
            None,
            None,
        )
        .expect("resolved");

        assert_eq!(resolved.sender.name(), "beta");
        assert_eq!(resolved.session_key.scope, "group:oc_1");
        assert_eq!(resolved.thread_ts.as_deref(), Some("th_1"));
    }

    #[test]
    fn target_override_replaces_recipient_only_when_configured() {
        let mut cfg = test_cfg();
        cfg.delivery_target_overrides
            .push(DeliveryTargetOverrideConfig {
                purpose: DeliveryPurposeConfig::Approval,
                agent: None,
                channel: Some("lark".into()),
                channel_instance: Some("alpha".into()),
                scope: "user:ou_owner".into(),
                reply_to: None,
                thread_ts: None,
            });
        let channels = test_channels();
        let natural = SessionKey::new("lark", "group:oc_1");

        let resolved = resolve_delivery(
            &cfg,
            &channels,
            DeliveryPurposeConfig::Approval,
            &natural,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("resolved");

        assert_eq!(resolved.session_key.scope, "user:ou_owner");
        assert_eq!(resolved.sender.name(), "alpha");
    }

    #[test]
    fn stored_lead_source_is_used_when_turn_source_is_absent() {
        let cfg = test_cfg();
        let channels = test_channels();
        let natural = SessionKey::new("lark", "group:oc_1");
        let stored = TurnDeliverySource::from_session_key(&SessionKey::with_instance(
            "lark",
            "beta",
            "group:oc_1",
        ));

        let resolved = resolve_delivery(
            &cfg,
            &channels,
            DeliveryPurposeConfig::LeadMessage,
            &natural,
            None,
            Some(&stored),
            Some("leader"),
            None,
            None,
        )
        .expect("resolved");

        assert_eq!(resolved.sender.name(), "beta");
        assert_eq!(
            resolved.session_key.channel_instance.as_deref(),
            Some("beta")
        );
    }
}
