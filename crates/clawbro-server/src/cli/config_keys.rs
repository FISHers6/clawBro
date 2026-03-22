use crate::config::{BindingConfig, DeliverySenderBindingConfig, DeliveryTargetOverrideConfig};
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderKey(String);

impl ProviderKey {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Display for ProviderKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BackendKey(String);

impl BackendKey {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Display for BackendKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentKey(String);

impl AgentKey {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Display for AgentKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TeamScopeKey {
    channel: String,
    scope: String,
}

impl TeamScopeKey {
    pub fn new(channel: impl Into<String>, scope: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            scope: scope.into(),
        }
    }

    pub fn channel(&self) -> &str {
        self.channel.as_str()
    }

    pub fn scope(&self) -> &str {
        self.scope.as_str()
    }
}

impl Display for TeamScopeKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.channel, self.scope)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChannelInstanceKey {
    channel: String,
    instance_id: String,
}

impl ChannelInstanceKey {
    pub fn new(channel: impl Into<String>, instance_id: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            instance_id: instance_id.into(),
        }
    }

    pub fn channel(&self) -> &str {
        self.channel.as_str()
    }

    pub fn instance_id(&self) -> &str {
        self.instance_id.as_str()
    }
}

impl Display for ChannelInstanceKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.channel, self.instance_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BindingKey(String);

impl BindingKey {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn from_binding(binding: &BindingConfig) -> Self {
        let key = match binding {
            BindingConfig::Thread {
                agent,
                scope,
                thread_id,
                channel,
            } => format!(
                "thread:{}:{}:{}:{}",
                agent,
                scope,
                thread_id,
                channel.as_deref().unwrap_or("*")
            ),
            BindingConfig::Scope {
                agent,
                scope,
                channel,
            } => format!(
                "scope:{}:{}:{}",
                agent,
                scope,
                channel.as_deref().unwrap_or("*")
            ),
            BindingConfig::Peer {
                agent,
                peer_kind,
                peer_id,
                channel,
            } => format!(
                "peer:{}:{peer_kind:?}:{}:{}",
                agent,
                peer_id,
                channel.as_deref().unwrap_or("*")
            ),
            BindingConfig::Team { agent, team_id } => {
                format!("team:{}:{}", agent, team_id)
            }
            BindingConfig::ChannelInstance {
                agent,
                channel,
                channel_instance,
            } => format!(
                "channel_instance:{}:{}:{}",
                agent, channel, channel_instance
            ),
            BindingConfig::Channel { agent, channel } => {
                format!("channel:{}:{}", agent, channel)
            }
            BindingConfig::Default { agent } => format!("default:{}", agent),
        };
        Self(key)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Display for BindingKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeliverySenderBindingKey(String);

impl DeliverySenderBindingKey {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn from_binding(binding: &DeliverySenderBindingConfig) -> Self {
        Self(format!(
            "{:?}:{}:{}:{}",
            binding.purpose,
            binding.agent.as_deref().unwrap_or("*"),
            binding.channel.as_deref().unwrap_or("*"),
            binding.channel_instance
        ))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Display for DeliverySenderBindingKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeliveryTargetOverrideKey(String);

impl DeliveryTargetOverrideKey {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn from_binding(binding: &DeliveryTargetOverrideConfig) -> Self {
        Self(format!(
            "{:?}:{}:{}:{}:{}",
            binding.purpose,
            binding.agent.as_deref().unwrap_or("*"),
            binding.channel.as_deref().unwrap_or("*"),
            binding.channel_instance.as_deref().unwrap_or("*"),
            binding.scope
        ))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Display for DeliveryTargetOverrideKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        BindingPeerKindConfig, DeliveryPurposeConfig, DeliveryTargetOverrideConfig,
    };

    #[test]
    fn team_scope_key_formats_channel_and_scope() {
        let key = TeamScopeKey::new("wechat", "user:abc");
        assert_eq!(key.to_string(), "wechat:user:abc");
    }

    #[test]
    fn binding_key_is_stable_for_scope_binding() {
        let binding = BindingConfig::Scope {
            agent: "claw".to_string(),
            scope: "user:abc".to_string(),
            channel: Some("wechat".to_string()),
        };
        let key = BindingKey::from_binding(&binding);
        assert_eq!(key.as_str(), "scope:claw:user:abc:wechat");
    }

    #[test]
    fn binding_key_captures_peer_bindings() {
        let binding = BindingConfig::Peer {
            agent: "claw".to_string(),
            peer_kind: BindingPeerKindConfig::User,
            peer_id: "u-1".to_string(),
            channel: None,
        };
        let key = BindingKey::from_binding(&binding);
        assert!(key.as_str().starts_with("peer:claw:User:u-1:"));
    }

    #[test]
    fn delivery_target_override_key_includes_scope() {
        let binding = DeliveryTargetOverrideConfig {
            purpose: DeliveryPurposeConfig::LeadFinal,
            agent: Some("claw".to_string()),
            channel: Some("wechat".to_string()),
            channel_instance: None,
            scope: "user:abc".to_string(),
            reply_to: None,
            thread_ts: None,
        };
        let key = DeliveryTargetOverrideKey::from_binding(&binding);
        assert!(key.0.contains("user:abc"));
    }
}
