use crate::{
    backend::BackendFamily,
    contract::{RuntimeRole, TurnMode},
};

pub fn backend_family_name(family: BackendFamily) -> &'static str {
    match family {
        BackendFamily::Acp => "acp",
        BackendFamily::OpenClawGateway => "openclaw_gateway",
        BackendFamily::QuickAiNative => "clawbro_native",
    }
}

pub fn runtime_role_name(role: RuntimeRole) -> &'static str {
    match role {
        RuntimeRole::Solo => "solo",
        RuntimeRole::Leader => "leader",
        RuntimeRole::Specialist => "specialist",
    }
}

pub fn turn_mode_name(mode: TurnMode) -> &'static str {
    match mode {
        TurnMode::Solo => "solo",
        TurnMode::Relay => "relay",
        TurnMode::Team => "team",
    }
}

pub fn team_id_from_scope(scope: &str) -> Option<&str> {
    let team_id = scope.split(':').next()?;
    (!team_id.is_empty() && scope.contains(':')).then_some(team_id)
}
