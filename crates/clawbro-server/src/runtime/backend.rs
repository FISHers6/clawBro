use serde::{Deserialize, Serialize};

pub use crate::runtime::contract::ApprovalMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendFamily {
    Acp,
    OpenClawGateway,
    ClawBroNative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolBridgeKind {
    None,
    Mcp,
    BackendNative,
    Hybrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeTeamCapability {
    Unsupported,
    SupportedButDisabled,
    SupportedAndEnabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RoleEligibility {
    pub solo: bool,
    pub relay: bool,
    pub specialist: bool,
    pub lead: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityProfile {
    pub streaming: bool,
    pub workspace_native_contract: bool,
    pub tool_bridge: ToolBridgeKind,
    pub native_team: NativeTeamCapability,
    pub role_eligibility: RoleEligibility,
}

impl CapabilityProfile {
    pub fn solo_only() -> Self {
        Self {
            streaming: true,
            workspace_native_contract: false,
            tool_bridge: ToolBridgeKind::None,
            native_team: NativeTeamCapability::Unsupported,
            role_eligibility: RoleEligibility {
                solo: true,
                relay: true,
                specialist: false,
                lead: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_family_equality_does_not_imply_lead_eligibility() {
        let family = BackendFamily::OpenClawGateway;
        let profile = CapabilityProfile::solo_only();

        assert_eq!(family, BackendFamily::OpenClawGateway);
        assert!(profile.role_eligibility.solo);
        assert!(profile.role_eligibility.relay);
        assert!(!profile.role_eligibility.lead);
        assert!(!profile.role_eligibility.specialist);
    }
}
