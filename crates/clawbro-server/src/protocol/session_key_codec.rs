use crate::protocol::SessionKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionScopeClass {
    User,
    Group,
    Other,
}

pub fn classify_session_scope(session_key: &SessionKey) -> SessionScopeClass {
    if session_key.scope.starts_with("user:") {
        SessionScopeClass::User
    } else if session_key.scope.starts_with("group:") {
        SessionScopeClass::Group
    } else {
        SessionScopeClass::Other
    }
}

pub fn normalize_channel_instance(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub fn normalize_runtime_session_identity(session_key: &SessionKey) -> SessionKey {
    let mut normalized = session_key.clone();
    normalized.channel_instance =
        normalize_channel_instance(session_key.channel_instance.as_deref());
    normalized
}

pub fn normalize_conversation_identity(session_key: &SessionKey) -> SessionKey {
    let mut normalized = session_key.clone();
    normalized.channel_instance = match classify_session_scope(session_key) {
        SessionScopeClass::Group => None,
        SessionScopeClass::User | SessionScopeClass::Other => {
            normalize_channel_instance(session_key.channel_instance.as_deref())
        }
    };
    normalized
}

fn escape_storage_component(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('#', "%23")
        .replace('=', "%3D")
}

pub fn render_session_key_text(session_key: &SessionKey) -> String {
    let channel = session_key.channel.trim();
    let scope = session_key.scope.trim();
    match normalize_channel_instance(session_key.channel_instance.as_deref()) {
        Some(instance) => format!("{channel}@{instance}:{scope}"),
        None => format!("{channel}:{scope}"),
    }
}

pub fn parse_session_key_text(text: &str) -> Result<SessionKey, String> {
    let text = text.trim();
    let (channel_head, scope) = text
        .split_once(':')
        .ok_or_else(|| format!("invalid session key `{text}`: missing scope separator"))?;
    if scope.trim().is_empty() {
        return Err(format!("invalid session key `{text}`: empty scope"));
    }
    let (channel, channel_instance) = match channel_head.split_once('@') {
        Some((channel, instance)) => (channel.trim(), normalize_channel_instance(Some(instance))),
        None => (channel_head.trim(), None),
    };
    if channel.is_empty() {
        return Err(format!("invalid session key `{text}`: empty channel"));
    }
    Ok(SessionKey {
        channel: channel.to_string(),
        scope: scope.trim().to_string(),
        channel_instance,
    })
}

pub fn render_scope_storage_key(session_key: &SessionKey) -> String {
    let normalized = normalize_conversation_identity(session_key);
    match normalize_channel_instance(normalized.channel_instance.as_deref()) {
        Some(instance) => format!(
            "c={}#i={}#s={}",
            escape_storage_component(&normalized.channel),
            escape_storage_component(&instance),
            escape_storage_component(&normalized.scope)
        ),
        None => format!(
            "c={}#s={}",
            escape_storage_component(&normalized.channel),
            escape_storage_component(&normalized.scope)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_legacy_session_key_text_supports_scope_colons() {
        let session_key = parse_session_key_text("lark:user:ou_1").unwrap();
        assert_eq!(session_key.channel, "lark");
        assert_eq!(session_key.scope, "user:ou_1");
        assert_eq!(session_key.channel_instance, None);
    }

    #[test]
    fn parse_and_render_instance_aware_session_key_text_round_trip() {
        let session_key = parse_session_key_text("lark@beta:user:ou_1").unwrap();
        assert_eq!(render_session_key_text(&session_key), "lark@beta:user:ou_1");
    }

    #[test]
    fn parse_session_key_text_rejects_missing_scope_separator() {
        let err = parse_session_key_text("lark").unwrap_err();
        assert!(err.contains("missing scope separator"));
    }

    #[test]
    fn normalize_conversation_identity_keeps_dm_instances() {
        let session_key = SessionKey {
            channel: "lark".into(),
            scope: "user:ou_1".into(),
            channel_instance: Some("beta".into()),
        };
        assert_eq!(
            normalize_conversation_identity(&session_key)
                .channel_instance
                .as_deref(),
            Some("beta")
        );
    }

    #[test]
    fn normalize_conversation_identity_drops_group_instances() {
        let session_key = SessionKey {
            channel: "lark".into(),
            scope: "group:oc_x".into(),
            channel_instance: Some("beta".into()),
        };
        assert_eq!(
            normalize_conversation_identity(&session_key),
            SessionKey {
                channel: "lark".into(),
                scope: "group:oc_x".into(),
                channel_instance: None,
            }
        );
    }

    #[test]
    fn normalize_runtime_session_identity_keeps_group_instances() {
        let session_key = SessionKey {
            channel: "lark".into(),
            scope: "group:oc_x".into(),
            channel_instance: Some("beta".into()),
        };
        assert_eq!(
            normalize_runtime_session_identity(&session_key),
            SessionKey {
                channel: "lark".into(),
                scope: "group:oc_x".into(),
                channel_instance: Some("beta".into()),
            }
        );
    }

    #[test]
    fn render_scope_storage_key_uses_normalized_identity() {
        let group = SessionKey {
            channel: "lark".into(),
            scope: "group:oc_x".into(),
            channel_instance: Some("beta".into()),
        };
        let dm = SessionKey {
            channel: "lark".into(),
            scope: "user:ou_1".into(),
            channel_instance: Some("beta".into()),
        };
        assert_eq!(render_scope_storage_key(&group), "c=lark#s=group:oc_x");
        assert_eq!(render_scope_storage_key(&dm), "c=lark#i=beta#s=user:ou_1");
    }

    #[test]
    fn render_scope_storage_key_avoids_legacy_separator_collisions() {
        let plain = SessionKey::new("lark_beta", "user:ou_1");
        let instanced = SessionKey::with_instance("lark", "beta", "user:ou_1");
        assert_ne!(
            render_scope_storage_key(&plain),
            render_scope_storage_key(&instanced)
        );
    }
}
