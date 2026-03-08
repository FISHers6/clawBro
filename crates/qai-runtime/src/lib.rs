pub mod acp;
pub mod adapter;
pub mod approval;
pub mod backend;
pub mod conductor;
pub mod contract;
pub mod event_sink;
pub mod helper_contract;
pub mod native;
pub mod observability;
pub mod openclaw;
pub mod registry;
pub mod tool_bridge;

pub use adapter::{BackendAdapter, LaunchSpec};
pub use approval::{ApprovalBroker, ApprovalDecision};
pub use backend::{
    BackendFamily, CapabilityProfile, NativeTeamCapability, RoleEligibility, ToolBridgeKind,
};
pub use conductor::RuntimeConductor;
pub use contract::{
    render_runtime_prompt, PermissionRequest, RuntimeContext, RuntimeEvent, RuntimeRole,
    RuntimeSessionSpec, TeamCallback, ToolSurfaceSpec, TurnIntent, TurnMode, TurnResult,
};
pub use event_sink::RuntimeEventSink;
pub use helper_contract::{
    optional_string_field, render_team_helper_failure, render_team_helper_success,
    required_string_field, ParsedTeamHelperResult, TEAM_HELPER_CONTRACT, TEAM_HELPER_VERSION,
};
pub use native::QuickAiNativeBackendAdapter;
pub use observability::{backend_family_name, runtime_role_name, team_id_from_scope, turn_mode_name};
pub use openclaw::OpenClawBackendAdapter;
pub use registry::{BackendRegistry, BackendSpec};
pub use tool_bridge::{
    visible_team_tools_for_role, TeamTool, TeamToolCall, TeamToolRequest, TeamToolResponse,
    TeamToolVisibility,
};
