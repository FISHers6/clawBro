use crate::adapter::LaunchSpec;
use crate::provider_profiles::ConfiguredProviderProtocol;
use crate::registry::BackendSpec;
use serde::Serialize;

const BACKEND_RESUME_FINGERPRINT_VERSION: u8 = 1;

#[derive(Debug, Serialize)]
struct BackendResumeFingerprint<'a> {
    version: u8,
    backend_id: &'a str,
    family: &'static str,
    adapter_key: &'a str,
    launch: LaunchFingerprint<'a>,
    acp_backend: Option<&'static str>,
    acp_auth_method: Option<&'static str>,
    codex_projection: Option<&'static str>,
    provider_profile: Option<ProviderFingerprint<'a>>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LaunchFingerprint<'a> {
    Embedded,
    Command {
        command: &'a str,
        args: &'a [String],
        env_keys: Vec<&'a str>,
    },
    GatewayWs {
        endpoint: &'a str,
        role: Option<&'a str>,
        scopes: &'a [String],
        agent_id: Option<&'a str>,
        team_helper_command: Option<&'a str>,
        team_helper_args: &'a [String],
        lead_helper_mode: bool,
    },
}

#[derive(Debug, Serialize)]
struct ProviderFingerprint<'a> {
    id: &'a str,
    protocol: ProviderProtocolFingerprint<'a>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ProviderProtocolFingerprint<'a> {
    OfficialSession,
    AnthropicCompatible {
        base_url: &'a str,
        default_model: &'a str,
        small_fast_model: Option<&'a str>,
    },
    OpenaiCompatible {
        base_url: &'a str,
        default_model: &'a str,
    },
}

pub fn fingerprint_backend_spec(spec: &BackendSpec) -> anyhow::Result<String> {
    let payload = BackendResumeFingerprint {
        version: BACKEND_RESUME_FINGERPRINT_VERSION,
        backend_id: &spec.backend_id,
        family: backend_family_name(spec.family),
        adapter_key: &spec.adapter_key,
        launch: launch_fingerprint(&spec.launch),
        acp_backend: spec.acp_backend.map(acp_backend_name),
        acp_auth_method: spec.acp_auth_method.map(acp_auth_method_name),
        codex_projection: spec.codex_projection.map(codex_projection_name),
        provider_profile: spec.provider_profile.as_ref().map(provider_fingerprint),
    };
    Ok(format!("resume:v1:{}", serde_json::to_string(&payload)?))
}

fn backend_family_name(family: crate::backend::BackendFamily) -> &'static str {
    match family {
        crate::backend::BackendFamily::Acp => "acp",
        crate::backend::BackendFamily::OpenClawGateway => "openclaw_gateway",
        crate::backend::BackendFamily::QuickAiNative => "quick_ai_native",
    }
}

fn launch_fingerprint(launch: &LaunchSpec) -> LaunchFingerprint<'_> {
    match launch {
        LaunchSpec::Embedded => LaunchFingerprint::Embedded,
        LaunchSpec::Command { command, args, env } => {
            let mut env_keys: Vec<&str> = env.iter().map(|(key, _)| key.as_str()).collect();
            env_keys.sort_unstable();
            LaunchFingerprint::Command {
                command,
                args,
                env_keys,
            }
        }
        LaunchSpec::GatewayWs {
            endpoint,
            role,
            scopes,
            agent_id,
            team_helper_command,
            team_helper_args,
            lead_helper_mode,
            ..
        } => LaunchFingerprint::GatewayWs {
            endpoint,
            role: role.as_deref(),
            scopes,
            agent_id: agent_id.as_deref(),
            team_helper_command: team_helper_command.as_deref(),
            team_helper_args,
            lead_helper_mode: *lead_helper_mode,
        },
    }
}

fn provider_fingerprint(
    profile: &crate::provider_profiles::ConfiguredProviderProfile,
) -> ProviderFingerprint<'_> {
    ProviderFingerprint {
        id: &profile.id,
        protocol: match &profile.protocol {
            ConfiguredProviderProtocol::OfficialSession => {
                ProviderProtocolFingerprint::OfficialSession
            }
            ConfiguredProviderProtocol::AnthropicCompatible {
                base_url,
                default_model,
                small_fast_model,
                ..
            } => ProviderProtocolFingerprint::AnthropicCompatible {
                base_url,
                default_model,
                small_fast_model: small_fast_model.as_deref(),
            },
            ConfiguredProviderProtocol::OpenaiCompatible {
                base_url,
                default_model,
                ..
            } => ProviderProtocolFingerprint::OpenaiCompatible {
                base_url,
                default_model,
            },
        },
    }
}

fn acp_backend_name(backend: crate::acp::AcpBackend) -> &'static str {
    match backend {
        crate::acp::AcpBackend::Claude => "claude",
        crate::acp::AcpBackend::Codex => "codex",
        crate::acp::AcpBackend::Codebuddy => "codebuddy",
        crate::acp::AcpBackend::Qwen => "qwen",
        crate::acp::AcpBackend::Iflow => "iflow",
        crate::acp::AcpBackend::Goose => "goose",
        crate::acp::AcpBackend::Kimi => "kimi",
        crate::acp::AcpBackend::Opencode => "opencode",
        crate::acp::AcpBackend::Qoder => "qoder",
        crate::acp::AcpBackend::Vibe => "vibe",
        crate::acp::AcpBackend::Custom => "custom",
    }
}

fn acp_auth_method_name(method: crate::acp::AcpAuthMethod) -> &'static str {
    method.protocol_id()
}

fn codex_projection_name(mode: crate::acp::CodexProjectionMode) -> &'static str {
    match mode {
        crate::acp::CodexProjectionMode::AcpAuth => "acp_auth",
        crate::acp::CodexProjectionMode::LocalConfig => "local_config",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::{AcpAuthMethod, CodexProjectionMode};
    use crate::backend::{ApprovalMode, BackendFamily};
    use crate::provider_profiles::{ConfiguredProviderProfile, ConfiguredProviderProtocol};
    use crate::registry::BackendSpec;

    fn base_codex_spec() -> BackendSpec {
        BackendSpec {
            backend_id: "codex-main".into(),
            family: BackendFamily::Acp,
            adapter_key: "acp".into(),
            launch: LaunchSpec::Command {
                command: "npx".into(),
                args: vec!["@zed-industries/codex-acp".into()],
                env: vec![
                    ("HOME".into(), "/Users/fishers".into()),
                    ("OPENAI_API_KEY".into(), "secret".into()),
                ],
            },
            approval_mode: ApprovalMode::AutoAllow,
            external_mcp_servers: vec![],
            provider_profile: Some(ConfiguredProviderProfile {
                id: "openai-official".into(),
                protocol: ConfiguredProviderProtocol::OfficialSession,
            }),
            acp_backend: Some(crate::acp::AcpBackend::Codex),
            acp_auth_method: Some(AcpAuthMethod::Chatgpt),
            codex_projection: Some(CodexProjectionMode::LocalConfig),
        }
    }

    #[test]
    fn fingerprint_ignores_secret_env_values_but_changes_on_provider_identity() {
        let mut first = base_codex_spec();
        let mut second = base_codex_spec();
        if let LaunchSpec::Command { env, .. } = &mut second.launch {
            env[1].1 = "different-secret".into();
        }
        assert_eq!(
            fingerprint_backend_spec(&first).unwrap(),
            fingerprint_backend_spec(&second).unwrap()
        );

        first.provider_profile = Some(ConfiguredProviderProfile {
            id: "openai-official".into(),
            protocol: ConfiguredProviderProtocol::OfficialSession,
        });
        second.provider_profile = Some(ConfiguredProviderProfile {
            id: "aicodewith".into(),
            protocol: ConfiguredProviderProtocol::OpenaiCompatible {
                base_url: "https://api.aicodewith.com/chatgpt/v1".into(),
                auth_token_env: "OPENAI_API_KEY".into(),
                default_model: "gpt-5.3-codex".into(),
            },
        });
        assert_ne!(
            fingerprint_backend_spec(&first).unwrap(),
            fingerprint_backend_spec(&second).unwrap()
        );
    }

    #[test]
    fn fingerprint_changes_when_auth_method_changes() {
        let first = base_codex_spec();
        let mut second = base_codex_spec();
        second.acp_auth_method = Some(AcpAuthMethod::OpenaiApiKey);
        assert_ne!(
            fingerprint_backend_spec(&first).unwrap(),
            fingerprint_backend_spec(&second).unwrap()
        );
    }
}
