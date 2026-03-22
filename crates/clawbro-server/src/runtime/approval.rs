use crate::protocol::DashboardEvent;
use crate::runtime::contract::PermissionRequest;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast;
use tokio::sync::oneshot;
use tokio::time::{timeout, Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalDecision {
    AllowOnce,
    AllowAlways,
    Deny,
}

impl ApprovalDecision {
    pub fn as_openclaw_str(self) -> &'static str {
        match self {
            Self::AllowOnce => "allow-once",
            Self::AllowAlways => "allow-always",
            Self::Deny => "deny",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "allow-once" => Some(Self::AllowOnce),
            "allow-always" => Some(Self::AllowAlways),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

#[derive(Clone, Default)]
pub struct ApprovalBroker {
    pending: Arc<DashMap<String, oneshot::Sender<ApprovalDecision>>>,
    requests: Arc<DashMap<String, PermissionRequest>>,
    dashboard_tx: Arc<OnceLock<broadcast::Sender<DashboardEvent>>>,
}

impl std::fmt::Debug for ApprovalBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalBroker")
            .field("pending_count", &self.pending.len())
            .finish()
    }
}

impl ApprovalBroker {
    pub fn set_dashboard_sender(&self, tx: broadcast::Sender<DashboardEvent>) {
        let _ = self.dashboard_tx.set(tx);
    }

    fn emit_dashboard_event(&self, event: DashboardEvent) {
        if let Some(tx) = self.dashboard_tx.get() {
            let _ = tx.send(event);
        }
    }

    pub fn register(&self, request: &PermissionRequest) -> PendingApproval {
        let (tx, rx) = oneshot::channel();
        if let Some((_, stale)) = self.pending.remove(&request.id) {
            let _ = stale.send(ApprovalDecision::Deny);
        }
        self.requests.insert(request.id.clone(), request.clone());
        self.pending.insert(request.id.clone(), tx);
        self.emit_dashboard_event(DashboardEvent::ApprovalPending {
            request: request.clone(),
        });
        PendingApproval {
            broker: self.clone(),
            approval_id: request.id.clone(),
            rx,
            expires_at_ms: request.expires_at_ms,
        }
    }

    pub fn resolve(&self, approval_id: &str, decision: ApprovalDecision) -> bool {
        self.requests.remove(approval_id);
        let resolved = self
            .pending
            .remove(approval_id)
            .map(|(_, tx)| tx.send(decision).is_ok())
            .unwrap_or(false);
        if resolved {
            self.emit_dashboard_event(DashboardEvent::ApprovalResolved {
                approval_id: approval_id.to_string(),
                decision,
                resolved,
            });
        }
        resolved
    }

    pub fn contains(&self, approval_id: &str) -> bool {
        self.pending.contains_key(approval_id)
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn pending_requests(&self) -> Vec<PermissionRequest> {
        let mut requests: Vec<_> = self
            .requests
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        requests.sort_by(|a, b| a.id.cmp(&b.id));
        requests
    }

    pub fn get_request(&self, approval_id: &str) -> Option<PermissionRequest> {
        self.requests
            .get(approval_id)
            .map(|entry| entry.value().clone())
    }

    fn clear_if_pending(&self, approval_id: &str) {
        self.pending.remove(approval_id);
        self.requests.remove(approval_id);
    }
}

pub struct PendingApproval {
    broker: ApprovalBroker,
    approval_id: String,
    rx: oneshot::Receiver<ApprovalDecision>,
    expires_at_ms: Option<u64>,
}

impl PendingApproval {
    pub async fn wait(self) -> ApprovalDecision {
        let Self {
            broker,
            approval_id,
            mut rx,
            expires_at_ms,
        } = self;

        let result = if let Some(expires_at_ms) = expires_at_ms {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let timeout_ms = expires_at_ms.saturating_sub(now_ms);
            timeout(Duration::from_millis(timeout_ms.max(1)), &mut rx).await
        } else {
            timeout(Duration::from_secs(30), &mut rx).await
        };

        match result {
            Ok(Ok(decision)) => decision,
            Ok(Err(_)) | Err(_) => {
                broker.clear_if_pending(&approval_id);
                ApprovalDecision::Deny
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    #[tokio::test]
    async fn approval_broker_resolves_pending_request() {
        let broker = ApprovalBroker::default();
        let pending = broker.register(&PermissionRequest {
            id: "approval-1".into(),
            prompt: "allow?".into(),
            command: Some("git status".into()),
            cwd: None,
            host: None,
            agent_id: None,
            expires_at_ms: None,
        });

        assert!(broker.contains("approval-1"));
        assert!(broker.resolve("approval-1", ApprovalDecision::AllowOnce));
        let decision = pending.wait().await;
        assert_eq!(decision, ApprovalDecision::AllowOnce);
    }

    #[tokio::test]
    async fn approval_broker_times_out_to_deny() {
        let broker = ApprovalBroker::default();
        let pending = broker.register(&PermissionRequest {
            id: "approval-2".into(),
            prompt: "allow?".into(),
            command: None,
            cwd: None,
            host: None,
            agent_id: None,
            expires_at_ms: Some(0),
        });

        let decision = pending.wait().await;
        assert_eq!(decision, ApprovalDecision::Deny);
        assert!(!broker.contains("approval-2"));
    }

    #[tokio::test]
    async fn approval_broker_emits_dashboard_events() {
        let broker = ApprovalBroker::default();
        let (tx, mut rx) = broadcast::channel(8);
        broker.set_dashboard_sender(tx);
        let request = PermissionRequest {
            id: "approval-3".into(),
            prompt: "allow?".into(),
            command: Some("git status".into()),
            cwd: None,
            host: None,
            agent_id: Some("claude".into()),
            expires_at_ms: None,
        };

        let pending = broker.register(&request);
        match rx.recv().await.unwrap() {
            DashboardEvent::ApprovalPending { request: seen } => {
                assert_eq!(seen.id, "approval-3");
                assert_eq!(seen.agent_id.as_deref(), Some("claude"));
            }
            other => panic!("unexpected event: {other:?}"),
        }

        assert!(broker.resolve("approval-3", ApprovalDecision::AllowOnce));
        match rx.recv().await.unwrap() {
            DashboardEvent::ApprovalResolved {
                approval_id,
                decision,
                resolved,
            } => {
                assert_eq!(approval_id, "approval-3");
                assert_eq!(decision, ApprovalDecision::AllowOnce);
                assert!(resolved);
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let decision = pending.wait().await;
        assert_eq!(decision, ApprovalDecision::AllowOnce);
    }

    #[tokio::test]
    async fn approval_broker_does_not_emit_resolved_for_unknown_id() {
        let broker = ApprovalBroker::default();
        let (tx, mut rx) = broadcast::channel(8);
        broker.set_dashboard_sender(tx);

        assert!(!broker.resolve("missing-approval", ApprovalDecision::Deny));
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(25), rx.recv()).await,
            Err(_)
        ));
    }
}
