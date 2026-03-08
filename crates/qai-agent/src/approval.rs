use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    AllowOnce,
    AllowAlways,
    Deny,
}

impl ApprovalDecision {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "allow-once" | "allow_once" => Some(Self::AllowOnce),
            "allow-always" | "allow_always" => Some(Self::AllowAlways),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllowOnce => "allow-once",
            Self::AllowAlways => "allow-always",
            Self::Deny => "deny",
        }
    }
}

#[async_trait]
pub trait ApprovalResolver: Send + Sync {
    async fn resolve(&self, approval_id: &str, decision: ApprovalDecision) -> anyhow::Result<bool>;
}
