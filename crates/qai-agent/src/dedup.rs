use dashmap::DashMap;
use std::time::{Duration, Instant};

/// 消息去重（防止 DingTalk 等 channel 重复投递）
/// 使用内存 DashMap + TTL（5 分钟）
pub struct DedupStore {
    seen: DashMap<String, Instant>,
    ttl: Duration,
}

impl DedupStore {
    pub fn new() -> Self {
        Self {
            seen: DashMap::new(),
            ttl: Duration::from_secs(300),
        }
    }

    /// 如果已见过此 ID 返回 false（重复），否则记录并返回 true（新消息）
    pub fn check_and_insert(&self, id: &str) -> bool {
        let now = Instant::now();
        // 清理过期条目
        self.seen.retain(|_, v| now.duration_since(*v) < self.ttl);
        if self.seen.contains_key(id) {
            return false;
        }
        self.seen.insert(id.to_string(), now);
        true
    }
}

impl Default for DedupStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dedup() {
        let store = DedupStore::new();
        assert!(store.check_and_insert("msg1")); // 新消息
        assert!(!store.check_and_insert("msg1")); // 重复
        assert!(store.check_and_insert("msg2")); // 新消息
    }

    #[test]
    fn test_different_ids_are_independent() {
        let store = DedupStore::new();
        assert!(store.check_and_insert("a"));
        assert!(store.check_and_insert("b"));
        assert!(store.check_and_insert("c"));
        assert!(!store.check_and_insert("a"));
        assert!(!store.check_and_insert("b"));
    }
}
