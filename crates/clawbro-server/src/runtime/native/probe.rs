use crate::runtime::backend::{
    CapabilityProfile, NativeTeamCapability, RoleEligibility, ToolBridgeKind,
};

pub fn default_native_capability_profile() -> CapabilityProfile {
    CapabilityProfile {
        streaming: true,
        workspace_native_contract: true,
        native_local_skills: false,
        tool_bridge: ToolBridgeKind::BackendNative,
        native_team: NativeTeamCapability::Unsupported,
        role_eligibility: RoleEligibility {
            solo: true,
            relay: true,
            specialist: true,
            lead: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_native_profile_starts_solo_and_relay_only() {
        let profile = default_native_capability_profile();
        assert!(profile.streaming);
        assert!(profile.workspace_native_contract);
        assert_eq!(profile.tool_bridge, ToolBridgeKind::BackendNative);
        assert!(profile.role_eligibility.solo);
        assert!(profile.role_eligibility.relay);
        assert!(profile.role_eligibility.specialist);
        assert!(profile.role_eligibility.lead);
    }
}
