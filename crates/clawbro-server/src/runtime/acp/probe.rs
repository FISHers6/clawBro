use crate::runtime::backend::{
    CapabilityProfile, NativeTeamCapability, RoleEligibility, ToolBridgeKind,
};
use agent_client_protocol as acp;

pub fn capability_profile_from_initialize(
    init: &acp::InitializeResponse,
    workspace_native_contract: bool,
    native_local_skills: bool,
) -> CapabilityProfile {
    let mcp = &init.agent_capabilities.mcp_capabilities;
    let tool_bridge = if mcp.sse || mcp.http {
        ToolBridgeKind::Mcp
    } else {
        ToolBridgeKind::None
    };

    CapabilityProfile {
        streaming: true,
        workspace_native_contract,
        native_local_skills,
        tool_bridge,
        native_team: NativeTeamCapability::SupportedButDisabled,
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
    fn capability_profile_marks_mcp_backends_as_tool_capable() {
        let init = acp::InitializeResponse::new(acp::ProtocolVersion::V1).agent_capabilities(
            acp::AgentCapabilities::default()
                .mcp_capabilities(acp::McpCapabilities::new().sse(true)),
        );

        let profile = capability_profile_from_initialize(&init, true, true);
        assert!(profile.streaming);
        assert!(profile.workspace_native_contract);
        assert!(profile.native_local_skills);
        assert_eq!(profile.tool_bridge, ToolBridgeKind::Mcp);
        assert!(profile.role_eligibility.specialist);
        assert!(profile.role_eligibility.lead);
    }

    #[test]
    fn capability_profile_without_mcp_still_supports_team_roles() {
        let init = acp::InitializeResponse::new(acp::ProtocolVersion::V1);
        let profile = capability_profile_from_initialize(&init, false, false);

        assert_eq!(profile.tool_bridge, ToolBridgeKind::None);
        assert!(!profile.native_local_skills);
        assert!(profile.role_eligibility.solo);
        assert!(profile.role_eligibility.relay);
        assert!(profile.role_eligibility.specialist);
        assert!(profile.role_eligibility.lead);
    }
}
