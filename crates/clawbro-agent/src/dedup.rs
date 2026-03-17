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
    ///
    /// check-and-insert 是原子的：使用 `DashMap::entry()` 在持有分片锁的情况下同时
    /// 完成存在性检查和插入，消除了旧实现中 contains_key → insert 之间的 TOCTOU 竞态窗口
    /// （两个并发调用者可能同时通过 contains_key 检查，各自执行 insert，导致同一消息被处理两次）。
    pub fn check_and_insert(&self, id: &str) -> bool {
        let now = Instant::now();
        // 清理过期条目（在 entry() 之前释放分片锁，避免持有两个分片锁）
        self.seen.retain(|_, v| now.duration_since(*v) < self.ttl);
        // entry() 持有分片锁，check + insert 原子完成
        use dashmap::mapref::entry::Entry;
        match self.seen.entry(id.to_string()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(e) => {
                e.insert(now);
                true
            }
        }
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
