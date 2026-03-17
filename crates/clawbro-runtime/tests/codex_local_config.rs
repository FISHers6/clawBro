use clawbro_runtime::codex_local_config::prepare_isolated_codex_home;
use clawbro_runtime::{RuntimeProviderProfile, RuntimeProviderProtocol};

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

#[test]
fn codex_local_config_projection_writes_official_chatgpt_auth_projection() {
    let temp = tempfile::tempdir().unwrap();
    let profile = RuntimeProviderProfile {
        id: "openai-official".into(),
        protocol: RuntimeProviderProtocol::OfficialSession,
    };

    let codex_home =
        prepare_isolated_codex_home(temp.path(), "codex-openai", &profile, None).unwrap();
    let auth = std::fs::read_to_string(codex_home.join("auth.json")).unwrap();
    let config = std::fs::read_to_string(codex_home.join("config.toml")).unwrap();

    assert!(auth.contains("\"OPENAI_API_KEY\": null"));
    assert!(auth.contains("\"auth_mode\": \"chatgpt\""));
    assert!(config.contains("model_provider = \"openai\""));
    assert!(config.contains("model = \"gpt-5.4\""));
    assert!(config.contains("model_reasoning_effort = \"high\""));
    assert!(config.contains("enableRouteSelection = true"));
    assert!(config.contains("requires_openai_auth = true"));
    assert!(!config.contains("base_url = "));
}

#[test]
fn codex_local_config_projection_copies_global_chatgpt_tokens() {
    let temp = tempfile::tempdir().unwrap();
    let global_codex_dir = temp.path().join(".codex");
    std::fs::create_dir_all(&global_codex_dir).unwrap();
    std::fs::write(
        global_codex_dir.join("auth.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "OPENAI_API_KEY": "ignored",
            "auth_mode": "chatgpt",
            "last_refresh": "2026-03-16T12:34:56Z",
            "tokens": {
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "id_token": "id-token",
                "account_id": "acct_123",
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let profile = RuntimeProviderProfile {
        id: "openai-official".into(),
        protocol: RuntimeProviderProtocol::OfficialSession,
    };

    let codex_home =
        prepare_isolated_codex_home(temp.path(), "codex-openai", &profile, None).unwrap();
    let auth: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(codex_home.join("auth.json")).unwrap())
            .unwrap();

    assert_eq!(auth.get("OPENAI_API_KEY"), Some(&serde_json::Value::Null));
    assert_eq!(
        auth.pointer("/tokens/access_token"),
        Some(&serde_json::Value::String("access-token".to_string()))
    );
    assert_eq!(
        auth.pointer("/tokens/account_id"),
        Some(&serde_json::Value::String("acct_123".to_string()))
    );
}
