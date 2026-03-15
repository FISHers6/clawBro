use qai_runtime::codex_local_config::prepare_isolated_codex_home;
use qai_runtime::{RuntimeProviderProfile, RuntimeProviderProtocol};

#[test]
fn codex_local_config_projection_writes_openai_compatible_projection() {
    let temp = tempfile::tempdir().unwrap();
    let profile = RuntimeProviderProfile {
        id: "openrouter-openai".into(),
        protocol: RuntimeProviderProtocol::OpenaiCompatible {
            base_url: "https://openrouter.ai/api/v1".into(),
            api_key: "sk-openrouter".into(),
            default_model: "openai/gpt-4.1".into(),
        },
    };

    let codex_home =
        prepare_isolated_codex_home(temp.path(), "codex-openrouter", &profile, None).unwrap();
    let auth = std::fs::read_to_string(codex_home.join("auth.json")).unwrap();
    let config = std::fs::read_to_string(codex_home.join("config.toml")).unwrap();

    assert!(auth.contains("sk-openrouter"));
    assert!(config.contains("base_url = \"https://openrouter.ai/api/v1\""));
    assert!(config.contains("wire_api = \"responses\""));
    assert!(config.contains("model_provider = \"openrouter_openai\""));
    assert!(config.contains("preferred_auth_method = \"apikey\""));
    assert!(config.contains("enableRouteSelection = true"));
    assert!(config.contains("requires_openai_auth = false"));
}
