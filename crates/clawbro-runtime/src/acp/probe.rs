use crate::backend::{CapabilityProfile, NativeTeamCapability, RoleEligibility, ToolBridgeKind};
use agent_client_protocol as acp;

pub fn capability_profile_from_initialize(
    init: &acp::InitializeResponse,
    workspace_native_contract: bool,
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
        tool_bridge,
        native_team: NativeTeamCapability::SupportedButDisabled,
        role_eligibility: RoleEligibility {
            solo: true,
            relay: true,
            specialist: mcp.sse || mcp.http,
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

        let profile = capability_profile_from_initialize(&init, true);
        assert!(profile.streaming);
        assert!(profile.workspace_native_contract);
        assert_eq!(profile.tool_bridge, ToolBridgeKind::Mcp);
        assert!(profile.role_eligibility.specialist);
        assert!(profile.role_eligibility.lead);
    }

    #[test]
    fn capability_profile_without_mcp_is_solo_and_relay_only() {
        let init = acp::InitializeResponse::new(acp::ProtocolVersion::V1);
        let profile = capability_profile_from_initialize(&init, false);

        assert_eq!(profile.tool_bridge, ToolBridgeKind::None);
        assert!(profile.role_eligibility.solo);
        assert!(profile.role_eligibility.relay);
        assert!(!profile.role_eligibility.specialist);
    }
}
