pub mod executor;
pub mod projection;
pub mod prompt_contract;
pub mod schema;
pub mod transcript_policy;
pub mod visibility;

pub use executor::{execute_team_contract_call, resolve_claimed_agent, resolve_team_tool_role};
pub use prompt_contract::{render_canonical_team_skill_injection, render_team_host_contract};
pub use schema::{
    canonical_progress_tools, canonical_terminal_tools, is_legacy_alias, tool_for_call, TeamTool,
    TeamToolCall, TeamToolRequest, TeamToolResponse,
};
pub use visibility::{ensure_team_call_allowed, visible_team_tools_for_role, TeamToolVisibility};
