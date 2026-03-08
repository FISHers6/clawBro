use crate::{
    adapter::{BackendAdapter, LaunchSpec},
    backend::{BackendFamily, CapabilityProfile},
    contract::{RuntimeSessionSpec, TurnResult},
    event_sink::RuntimeEventSink,
    native::probe::default_native_capability_profile,
    native::session_driver::{run_command_turn, NativeCommandConfig},
    registry::BackendSpec,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct QuickAiNativeBackendAdapter;

impl QuickAiNativeBackendAdapter {
    fn command_config(spec: &BackendSpec) -> anyhow::Result<NativeCommandConfig> {
        match &spec.launch {
            LaunchSpec::Embedded => Ok(NativeCommandConfig {
                command: "quickai-rust-agent".into(),
                args: vec!["--runtime-bridge".into()],
                env: vec![],
            }),
            LaunchSpec::Command { command, args, env } => {
                let mut args = args.clone();
                if !args.iter().any(|arg| arg == "--runtime-bridge") {
                    args.insert(0, "--runtime-bridge".into());
                }
                Ok(NativeCommandConfig {
                    command: command.clone(),
                    args,
                    env: env.clone(),
                })
            }
            other => anyhow::bail!(
                "native backend '{}' requires LaunchSpec::Embedded or LaunchSpec::Command, got {other:?}",
                spec.backend_id
            ),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl BackendAdapter for QuickAiNativeBackendAdapter {
    async fn probe(&self, spec: &BackendSpec) -> anyhow::Result<CapabilityProfile> {
        if spec.family != BackendFamily::QuickAiNative {
            anyhow::bail!(
                "native adapter requires QuickAiNative backend family, got {:?}",
                spec.family
            );
        }
        Ok(default_native_capability_profile())
    }

    async fn run_turn(
        &self,
        spec: &BackendSpec,
        session: RuntimeSessionSpec,
        sink: RuntimeEventSink,
    ) -> anyhow::Result<TurnResult> {
        if spec.family != BackendFamily::QuickAiNative {
            anyhow::bail!(
                "native adapter requires QuickAiNative backend family, got {:?}",
                spec.family
            );
        }
        let config = Self::command_config(spec)?;
        run_command_turn(&config, session, sink).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        adapter::LaunchSpec,
        contract::{RuntimeContext, RuntimeRole, ToolSurfaceSpec},
    };

    fn native_spec() -> BackendSpec {
        BackendSpec {
            backend_id: "quickai-native".into(),
            family: BackendFamily::QuickAiNative,
            adapter_key: "native".into(),
            launch: LaunchSpec::Embedded,
        }
    }

    #[tokio::test]
    async fn native_adapter_probe_reports_default_profile() {
        let adapter = QuickAiNativeBackendAdapter;
        let profile = adapter.probe(&native_spec()).await.unwrap();

        assert!(profile.workspace_native_contract);
        assert!(profile.role_eligibility.solo);
        assert!(profile.role_eligibility.specialist);
        assert!(profile.role_eligibility.lead);
    }

    #[tokio::test]
    async fn native_adapter_run_turn_rejects_unsupported_launch_spec() {
        let adapter = QuickAiNativeBackendAdapter;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let err = adapter
            .run_turn(
                &BackendSpec {
                    backend_id: "quickai-native".into(),
                    family: BackendFamily::QuickAiNative,
                    adapter_key: "native".into(),
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
                },
                RuntimeSessionSpec {
                    backend_id: "quickai-native".into(),
                    participant_name: None,
                    session_key: qai_protocol::SessionKey::new("ws", "native:test"),
                    role: RuntimeRole::Solo,
                    workspace_dir: None,
                    prompt_text: "hello".into(),
                    tool_surface: ToolSurfaceSpec::default(),
                    tool_bridge_url: None,
                    team_tool_url: None,
                    context: RuntimeContext::default(),
                },
                RuntimeEventSink::new(tx),
            )
            .await
            .unwrap_err();

        assert!(err
            .to_string()
            .contains("LaunchSpec::Embedded or LaunchSpec::Command"));
    }

    #[test]
    fn native_adapter_embedded_launch_maps_to_default_runtime_bridge() {
        let cfg = QuickAiNativeBackendAdapter::command_config(&native_spec()).unwrap();
        assert_eq!(cfg.command, "quickai-rust-agent");
        assert!(cfg.args.iter().any(|a| a == "--runtime-bridge"));
    }
}
