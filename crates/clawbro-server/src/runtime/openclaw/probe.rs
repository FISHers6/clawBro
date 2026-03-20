use crate::runtime::backend::{
    CapabilityProfile, NativeTeamCapability, RoleEligibility, ToolBridgeKind,
};

pub fn default_openclaw_capability_profile() -> CapabilityProfile {
    CapabilityProfile {
        streaming: true,
        workspace_native_contract: true,
        native_local_skills: true,
        tool_bridge: ToolBridgeKind::BackendNative,
        native_team: NativeTeamCapability::SupportedButDisabled,
        role_eligibility: RoleEligibility {
            solo: true,
            relay: true,
            specialist: false,
            lead: false,
        },
    }
}

pub fn upgraded_openclaw_team_profile() -> CapabilityProfile {
    CapabilityProfile {
        native_team: NativeTeamCapability::SupportedButDisabled,
        role_eligibility: RoleEligibility {
            solo: true,
            relay: true,
            specialist: true,
            lead: false,
        },
        ..default_openclaw_capability_profile()
    }
}

pub fn upgraded_openclaw_lead_profile() -> CapabilityProfile {
    CapabilityProfile {
        native_team: NativeTeamCapability::SupportedButDisabled,
        role_eligibility: RoleEligibility {
            solo: true,
            relay: true,
            specialist: true,
            lead: true,
        },
        ..default_openclaw_capability_profile()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openclaw_defaults_to_solo_and_relay_with_native_team_disabled() {
        let profile = default_openclaw_capability_profile();
        assert!(profile.role_eligibility.solo);
        assert!(profile.role_eligibility.relay);
        assert!(!profile.role_eligibility.specialist);
        assert!(!profile.role_eligibility.lead);
        assert_eq!(
            profile.native_team,
            NativeTeamCapability::SupportedButDisabled
        );
    }

    #[test]
    fn upgraded_openclaw_profile_can_become_specialist_only_team_eligible() {
        let profile = upgraded_openclaw_team_profile();
        assert!(profile.role_eligibility.specialist);
        assert!(!profile.role_eligibility.lead);
        assert_eq!(
            profile.native_team,
            NativeTeamCapability::SupportedButDisabled
        );
    }

    #[test]
    fn upgraded_openclaw_lead_profile_is_explicit_and_native_team_disabled() {
        let default_profile = default_openclaw_capability_profile();
        assert!(!default_profile.role_eligibility.lead);

        let profile = upgraded_openclaw_lead_profile();
        assert!(profile.role_eligibility.specialist);
        assert!(profile.role_eligibility.lead);
        assert_eq!(
            profile.native_team,
            NativeTeamCapability::SupportedButDisabled
        );
    }
}
