use qai_protocol::SessionKey;
use uuid::Uuid;

pub type SessionId = Uuid;

/// 从 SessionKey 生成确定性 SessionId（UUID v5）
pub fn key_to_session_id(key: &SessionKey) -> Uuid {
    let namespace = Uuid::NAMESPACE_URL;
    let name = format!("{}:{}", key.channel, key.scope);
    Uuid::new_v5(&namespace, name.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_session_id() {
        let key = SessionKey::new("dingtalk", "user_123");
        let id1 = key_to_session_id(&key);
        let id2 = key_to_session_id(&key);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_different_keys_different_ids() {
        let k1 = SessionKey::new("dingtalk", "user_123");
        let k2 = SessionKey::new("dingtalk", "user_456");
        assert_ne!(key_to_session_id(&k1), key_to_session_id(&k2));
    }
}
