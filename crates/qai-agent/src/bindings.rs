use qai_protocol::{InboundMsg, SessionKey};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingRule {
    Thread {
        channel: Option<String>,
        scope: String,
        thread_id: String,
        agent_name: String,
    },
    Scope {
        channel: Option<String>,
        scope: String,
        agent_name: String,
    },
    Peer {
        channel: Option<String>,
        kind: BindingPeerKind,
        id: String,
        agent_name: String,
    },
    Team {
        team_id: String,
        agent_name: String,
    },
    Channel {
        channel: String,
        agent_name: String,
    },
    Default {
        agent_name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingPeerKind {
    User,
    Group,
}

impl BindingRule {
    pub fn agent_name(&self) -> &str {
        match self {
            Self::Thread { agent_name, .. }
            | Self::Scope { agent_name, .. }
            | Self::Peer { agent_name, .. }
            | Self::Team { agent_name, .. }
            | Self::Channel { agent_name, .. }
            | Self::Default { agent_name } => agent_name,
        }
    }

    pub fn scope(scope: impl Into<String>, agent_name: impl Into<String>) -> Self {
        Self::Scope {
            channel: None,
            scope: scope.into(),
            agent_name: agent_name.into(),
        }
    }
}

pub fn resolve_binding<'a>(
    inbound: &InboundMsg,
    session_key: &SessionKey,
    bindings: &'a [BindingRule],
) -> Option<&'a BindingRule> {
    bindings
        .iter()
        .rev()
        .find(|binding| matches_thread(binding, inbound, session_key))
        .or_else(|| {
            bindings
                .iter()
                .rev()
                .find(|binding| matches_scope(binding, session_key))
        })
        .or_else(|| {
            bindings
                .iter()
                .rev()
                .find(|binding| matches_peer(binding, session_key))
        })
        .or_else(|| {
            bindings
                .iter()
                .rev()
                .find(|binding| matches_team(binding, session_key))
        })
        .or_else(|| {
            bindings
                .iter()
                .rev()
                .find(|binding| matches_channel(binding, session_key))
        })
        .or_else(|| {
            bindings
                .iter()
                .rev()
                .find(|binding| matches!(binding, BindingRule::Default { .. }))
        })
}

fn channel_matches(binding_channel: &Option<String>, session_key: &SessionKey) -> bool {
    binding_channel
        .as_deref()
        .map(|channel| channel == session_key.channel)
        .unwrap_or(true)
}

fn matches_thread(binding: &BindingRule, inbound: &InboundMsg, session_key: &SessionKey) -> bool {
    let BindingRule::Thread {
        channel,
        scope,
        thread_id,
        ..
    } = binding
    else {
        return false;
    };
    channel_matches(channel, session_key)
        && scope == &session_key.scope
        && inbound.thread_ts.as_deref() == Some(thread_id.as_str())
}

fn matches_scope(binding: &BindingRule, session_key: &SessionKey) -> bool {
    let BindingRule::Scope { channel, scope, .. } = binding else {
        return false;
    };
    channel_matches(channel, session_key) && scope == &session_key.scope
}

fn matches_peer(binding: &BindingRule, session_key: &SessionKey) -> bool {
    let BindingRule::Peer {
        channel, kind, id, ..
    } = binding
    else {
        return false;
    };
    channel_matches(channel, session_key)
        && extract_peer_kind_and_id(session_key)
            .map(|(peer_kind, peer_id)| peer_kind == *kind && peer_id == id.as_str())
            .unwrap_or(false)
}

fn matches_team(binding: &BindingRule, session_key: &SessionKey) -> bool {
    let BindingRule::Team { team_id, .. } = binding else {
        return false;
    };
    extract_team_id(session_key)
        .map(|resolved| resolved == team_id.as_str())
        .unwrap_or(false)
}

fn matches_channel(binding: &BindingRule, session_key: &SessionKey) -> bool {
    let BindingRule::Channel { channel, .. } = binding else {
        return false;
    };
    channel == &session_key.channel
}

pub fn extract_peer_kind_and_id(session_key: &SessionKey) -> Option<(BindingPeerKind, &str)> {
    let (kind, id) = session_key.scope.split_once(':')?;
    match kind {
        "user" => Some((BindingPeerKind::User, id)),
        "group" => Some((BindingPeerKind::Group, id)),
        _ => None,
    }
}

pub fn extract_team_id(session_key: &SessionKey) -> Option<&str> {
    if session_key.channel != "specialist" {
        return None;
    }
    session_key.scope.split(':').next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use qai_protocol::{MsgContent, MsgSource};

    fn inbound(
        channel: &str,
        scope: &str,
        thread_ts: Option<&str>,
        target_agent: Option<&str>,
    ) -> InboundMsg {
        InboundMsg {
            id: "binding-1".into(),
            session_key: SessionKey::new(channel, scope),
            content: MsgContent::text("hello"),
            sender: "user".into(),
            channel: channel.into(),
            timestamp: chrono::Utc::now(),
            thread_ts: thread_ts.map(str::to_string),
            target_agent: target_agent.map(str::to_string),
            source: MsgSource::Human,
        }
    }

    #[test]
    fn binding_resolution_prefers_thread_over_scope_and_channel() {
        let inbound = inbound("lark", "group:oc_123", Some("thread-42"), None);
        let bindings = vec![
            BindingRule::Channel {
                channel: "lark".into(),
                agent_name: "channel-agent".into(),
            },
            BindingRule::Scope {
                channel: Some("lark".into()),
                scope: "group:oc_123".into(),
                agent_name: "scope-agent".into(),
            },
            BindingRule::Thread {
                channel: Some("lark".into()),
                scope: "group:oc_123".into(),
                thread_id: "thread-42".into(),
                agent_name: "thread-agent".into(),
            },
        ];

        let matched = resolve_binding(&inbound, &inbound.session_key, &bindings).unwrap();
        assert_eq!(matched.agent_name(), "thread-agent");
    }

    #[test]
    fn binding_resolution_prefers_scope_over_peer() {
        let inbound = inbound("lark", "group:oc_123", None, None);
        let bindings = vec![
            BindingRule::Peer {
                channel: Some("lark".into()),
                kind: BindingPeerKind::Group,
                id: "oc_123".into(),
                agent_name: "peer-agent".into(),
            },
            BindingRule::Scope {
                channel: Some("lark".into()),
                scope: "group:oc_123".into(),
                agent_name: "scope-agent".into(),
            },
        ];

        let matched = resolve_binding(&inbound, &inbound.session_key, &bindings).unwrap();
        assert_eq!(matched.agent_name(), "scope-agent");
    }

    #[test]
    fn later_binding_overrides_earlier_binding_with_same_precedence() {
        let inbound = inbound("lark", "group:oc_123", None, None);
        let bindings = vec![
            BindingRule::Scope {
                channel: Some("lark".into()),
                scope: "group:oc_123".into(),
                agent_name: "first-agent".into(),
            },
            BindingRule::Scope {
                channel: Some("lark".into()),
                scope: "group:oc_123".into(),
                agent_name: "second-agent".into(),
            },
        ];

        let matched = resolve_binding(&inbound, &inbound.session_key, &bindings).unwrap();
        assert_eq!(matched.agent_name(), "second-agent");
    }

    #[test]
    fn binding_resolution_falls_back_to_channel_then_default() {
        let inbound = inbound("ws", "unknown-scope", None, None);
        let bindings = vec![
            BindingRule::Default {
                agent_name: "default-agent".into(),
            },
            BindingRule::Channel {
                channel: "ws".into(),
                agent_name: "channel-agent".into(),
            },
        ];

        let matched = resolve_binding(&inbound, &inbound.session_key, &bindings).unwrap();
        assert_eq!(matched.agent_name(), "channel-agent");
    }

    #[test]
    fn binding_resolution_matches_specialist_team_id() {
        let inbound = inbound("specialist", "team-123:codex", None, Some("@codex"));
        let bindings = vec![BindingRule::Team {
            team_id: "team-123".into(),
            agent_name: "team-agent".into(),
        }];

        let matched = resolve_binding(&inbound, &inbound.session_key, &bindings).unwrap();
        assert_eq!(matched.agent_name(), "team-agent");
    }
}
