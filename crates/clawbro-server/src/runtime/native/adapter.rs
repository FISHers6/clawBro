use crate::runtime::{
    adapter::{BackendAdapter, LaunchSpec},
    backend::{BackendFamily, CapabilityProfile},
    contract::{RuntimeSessionSpec, TurnResult},
    event_sink::RuntimeEventSink,
    native::probe::default_native_capability_profile,
    native::session_driver::{run_command_turn, NativeCommandConfig},
    registry::BackendSpec,
};
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Default, Clone, Copy)]
pub struct ClawBroNativeBackendAdapter;

impl ClawBroNativeBackendAdapter {
    fn resolve_bundled_shell_path() -> PathBuf {
        if let Ok(current_exe) = std::env::current_exe() {
            if current_exe.exists() {
                return current_exe;
            }
        }

        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
        let dev_debug = repo_root.join("clawBro/target/debug/clawbro");
        if dev_debug.exists() {
            return dev_debug;
        }
        let dev_release = repo_root.join("clawBro/target/release/clawbro");
        if dev_release.exists() {
            return dev_release;
        }

        repo_root.join("clawBro/target/debug/clawbro")
    }

    fn is_clawbro_command(command: &str) -> bool {
        Path::new(command)
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name == "clawbro" || name == "clawbro.exe")
            .unwrap_or(false)
    }

    fn command_config(spec: &BackendSpec) -> anyhow::Result<NativeCommandConfig> {
        match &spec.launch {
            LaunchSpec::BundledCommand => {
                let command = Self::resolve_bundled_shell_path()
                    .to_string_lossy()
                    .into_owned();
                tracing::debug!(
                    backend_id = %spec.backend_id,
                    command = %command,
                    "resolved bundled native shell path"
                );
                Ok(NativeCommandConfig {
                    command,
                    args: vec!["runtime-bridge".into()],
                    env: vec![],
                })
            }
            LaunchSpec::ExternalCommand { command, args, env } => {
                let mut args = args.clone();
                if !args
                    .iter()
                    .any(|arg| arg == "--runtime-bridge" || arg == "runtime-bridge")
                {
                    if Self::is_clawbro_command(command) {
                        args.insert(0, "runtime-bridge".into());
                    } else {
                        args.insert(0, "--runtime-bridge".into());
                    }
                }
                Ok(NativeCommandConfig {
                    command: command.clone(),
                    args,
                    env: env.clone(),
                })
            }
            other => anyhow::bail!(
                "native backend '{}' requires LaunchSpec::BundledCommand or LaunchSpec::ExternalCommand, got {other:?}",
                spec.backend_id
            ),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl BackendAdapter for ClawBroNativeBackendAdapter {
    async fn probe(&self, spec: &BackendSpec) -> anyhow::Result<CapabilityProfile> {
        if spec.family != BackendFamily::ClawBroNative {
            anyhow::bail!(
                "native adapter requires ClawBroNative backend family, got {:?}",
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
        if spec.family != BackendFamily::ClawBroNative {
            anyhow::bail!(
                "native adapter requires ClawBroNative backend family, got {:?}",
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
    use crate::runtime::{
        adapter::LaunchSpec,
        contract::{RuntimeContext, RuntimeRole, ToolSurfaceSpec},
    };

    fn native_spec() -> BackendSpec {
        BackendSpec {
            backend_id: "clawbro-native".into(),
            family: BackendFamily::ClawBroNative,
            adapter_key: "native".into(),
            launch: LaunchSpec::BundledCommand,
            approval_mode: Default::default(),
            external_mcp_servers: vec![],
            provider_profile: None,
            acp_backend: None,
            acp_auth_method: None,
            codex_projection: None,
        }
    }

    #[tokio::test]
    async fn native_adapter_probe_reports_default_profile() {
        let adapter = ClawBroNativeBackendAdapter;
        let profile = adapter.probe(&native_spec()).await.unwrap();

        assert!(profile.workspace_native_contract);
        assert!(profile.role_eligibility.solo);
        assert!(profile.role_eligibility.specialist);
        assert!(profile.role_eligibility.lead);
    }

    #[tokio::test]
    async fn native_adapter_run_turn_rejects_unsupported_launch_spec() {
        let adapter = ClawBroNativeBackendAdapter;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let err = adapter
            .run_turn(
                &BackendSpec {
                    backend_id: "clawbro-native".into(),
                    family: BackendFamily::ClawBroNative,
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
                    approval_mode: Default::default(),
                    external_mcp_servers: vec![],
                    provider_profile: None,
                    acp_backend: None,
                    acp_auth_method: None,
                    codex_projection: None,
                },
                RuntimeSessionSpec {
                    backend_id: "clawbro-native".into(),
                    participant_name: None,
                    session_key: crate::protocol::SessionKey::new("ws", "native:test"),
                    role: RuntimeRole::Solo,
                    workspace_dir: None,
                    prompt_text: "hello".into(),
                    tool_surface: ToolSurfaceSpec::default(),
                    approval_mode: Default::default(),
                    external_mcp_servers: vec![],
                    team_tool_url: None,
                    provider_profile: None,
                    backend_session_id: None,
                    context: RuntimeContext::default(),
                },
                RuntimeEventSink::new(tx),
            )
            .await
            .unwrap_err();

        assert!(err
            .to_string()
            .contains("LaunchSpec::BundledCommand or LaunchSpec::ExternalCommand"));
    }

    #[test]
    fn native_adapter_bundled_launch_maps_to_default_runtime_bridge() {
        let cfg = ClawBroNativeBackendAdapter::command_config(&native_spec()).unwrap();
        assert!(!cfg.command.is_empty());
        assert!(cfg.args.iter().any(|a| a == "runtime-bridge"));
    }
}
