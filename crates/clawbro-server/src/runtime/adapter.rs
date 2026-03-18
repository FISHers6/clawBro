use crate::runtime::{
    backend::CapabilityProfile,
    contract::{RuntimeSessionSpec, TurnResult},
    event_sink::RuntimeEventSink,
    registry::BackendSpec,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchSpec {
    BundledCommand,
    ExternalCommand {
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    GatewayWs {
        endpoint: String,
        token: Option<String>,
        password: Option<String>,
        role: Option<String>,
        scopes: Vec<String>,
        agent_id: Option<String>,
        team_helper_command: Option<String>,
        team_helper_args: Vec<String>,
        lead_helper_mode: bool,
    },
}

#[async_trait::async_trait(?Send)]
pub trait BackendAdapter: Send + Sync {
    async fn probe(&self, spec: &BackendSpec) -> anyhow::Result<CapabilityProfile>;

    async fn run_turn(
        &self,
        spec: &BackendSpec,
        session: RuntimeSessionSpec,
        sink: RuntimeEventSink,
    ) -> anyhow::Result<TurnResult>;
}
