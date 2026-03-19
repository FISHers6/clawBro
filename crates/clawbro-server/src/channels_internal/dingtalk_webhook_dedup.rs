use std::collections::{HashSet, VecDeque};

/// Small ingress-scoped dedup helper for webhook event/message IDs.
/// Keeps a bounded in-memory window to avoid creating duplicate turns
/// when DingTalk retries the same callback.
#[derive(Debug)]
pub struct DingTalkWebhookDedup {
    max_entries: usize,
    order: VecDeque<String>,
    seen: HashSet<String>,
}

impl DingTalkWebhookDedup {
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            order: VecDeque::new(),
            seen: HashSet::new(),
        }
    }

    /// Returns true when the event id is new and was inserted.
    /// Returns false when the event id was already present.
    pub fn record_if_new(&mut self, event_id: &str) -> bool {
        let event_id = event_id.trim();
        if event_id.is_empty() {
            return false;
        }
        if self.seen.contains(event_id) {
            return false;
        }
        let owned = event_id.to_string();
        self.seen.insert(owned.clone());
        self.order.push_back(owned);
        while self.order.len() > self.max_entries {
            if let Some(oldest) = self.order.pop_front() {
                self.seen.remove(&oldest);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::DingTalkWebhookDedup;

    #[test]
    fn record_if_new_rejects_duplicates() {
        let mut dedup = DingTalkWebhookDedup::new(16);
        assert!(dedup.record_if_new("msg-1"));
        assert!(!dedup.record_if_new("msg-1"));
        assert!(dedup.record_if_new("msg-2"));
    }

    #[test]
    fn record_if_new_evicts_old_entries_when_bounded() {
        let mut dedup = DingTalkWebhookDedup::new(2);
        assert!(dedup.record_if_new("msg-1"));
        assert!(dedup.record_if_new("msg-2"));
        assert!(dedup.record_if_new("msg-3"));
        assert!(dedup.record_if_new("msg-1"));
    }
}
