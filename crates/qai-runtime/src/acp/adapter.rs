use crate::{
    acp::{
        probe::capability_profile_from_initialize,
        session_driver::{probe_command_backend, run_command_turn, AcpCommandConfig},
    },
    adapter::{BackendAdapter, LaunchSpec},
    approval::ApprovalBroker,
    backend::CapabilityProfile,
    contract::{RuntimeSessionSpec, TurnResult},
    event_sink::RuntimeEventSink,
    registry::BackendSpec,
};

#[derive(Debug, Clone, Default)]
pub struct AcpBackendAdapter {
    approvals: ApprovalBroker,
}

impl AcpBackendAdapter {
    pub fn new(approvals: ApprovalBroker) -> Self {
        Self { approvals }
    }

    fn command_config(spec: &BackendSpec) -> anyhow::Result<AcpCommandConfig> {
        match &spec.launch {
            LaunchSpec::Command { command, args, env } => Ok(AcpCommandConfig {
                command: command.clone(),
                args: args.clone(),
                env: env.clone(),
            }),
            other => Err(anyhow::anyhow!(
                "ACP backend '{}' requires LaunchSpec::Command, got {other:?}",
                spec.backend_id
            )),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl BackendAdapter for AcpBackendAdapter {
    async fn probe(&self, spec: &BackendSpec) -> anyhow::Result<CapabilityProfile> {
        let config = Self::command_config(spec)?;
        let init = probe_command_backend(&config).await?;
        Ok(capability_profile_from_initialize(&init, true))
    }

    async fn run_turn(
        &self,
        spec: &BackendSpec,
        session: RuntimeSessionSpec,
        sink: RuntimeEventSink,
    ) -> anyhow::Result<TurnResult> {
        let config = Self::command_config(spec)?;
        run_command_turn(&config, session, sink, self.approvals.clone()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{backend::BackendFamily, registry::BackendSpec};

    #[tokio::test]
    async fn acp_adapter_rejects_non_command_launch_specs() {
        let adapter = AcpBackendAdapter::default();
        let spec = BackendSpec {
            backend_id: "openclaw".into(),
            family: BackendFamily::OpenClawGateway,
            adapter_key: "acp".into(),
            launch: LaunchSpec::GatewayWs {
                endpoint: "ws://127.0.0.1:18789".into(),
                token: None,
                password: None,
                role: None,
                scopes: vec![],
                agent_id: None,
                team_helper_command: None,
                team_helper_args: vec![],
                lead_helper_mode: false,
            },
        };

        let err = adapter.probe(&spec).await.unwrap_err();
        assert!(err.to_string().contains("LaunchSpec::Command"));
    }
}
