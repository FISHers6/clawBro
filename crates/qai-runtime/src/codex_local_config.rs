use crate::provider_profiles::{RuntimeProviderProfile, RuntimeProviderProtocol};
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

const API_KEY_MODEL_REASONING_EFFORT: &str = "low";
const OFFICIAL_MODEL_REASONING_EFFORT: &str = "high";
const OFFICIAL_MODEL_PROVIDER: &str = "openai";
const OFFICIAL_MODEL: &str = "gpt-5.4";
const OFFICIAL_AUTH_MODE: &str = "chatgpt";

pub fn prepare_isolated_codex_home(
    home_dir: &Path,
    backend_id: &str,
    profile: &RuntimeProviderProfile,
    workspace_dir: Option<&Path>,
) -> Result<PathBuf> {
    let codex_home = codex_home_root_for(home_dir, backend_id);
    fs::create_dir_all(&codex_home)
        .with_context(|| format!("creating isolated CODEX_HOME at {}", codex_home.display()))?;

    let auth_path = codex_home.join("auth.json");
    let config_path = codex_home.join("config.toml");
    let (auth_payload, config_text) = match &profile.protocol {
        RuntimeProviderProtocol::OpenaiCompatible {
            base_url,
            api_key,
            default_model,
        } => {
            let provider_name = sanitize_provider_name(&profile.id);
            (
                json!({
                    "OPENAI_API_KEY": api_key,
                }),
                render_openai_compatible_codex_config(
                    &provider_name,
                    base_url,
                    default_model,
                    workspace_dir,
                ),
            )
        }
        RuntimeProviderProtocol::OfficialSession => (
            render_official_auth_payload(home_dir)?,
            render_official_codex_config(workspace_dir),
        ),
        RuntimeProviderProtocol::AnthropicCompatible { .. } => {
            bail!(
                "codex local_config projection requires openai_compatible or official_session provider profile, got `{}`",
                profile.id
            );
        }
    };

    fs::write(
        &auth_path,
        serde_json::to_vec_pretty(&auth_payload).context("serializing Codex auth payload")?,
    )
    .with_context(|| format!("writing {}", auth_path.display()))?;
    fs::write(&config_path, config_text)
        .with_context(|| format!("writing {}", config_path.display()))?;

    Ok(codex_home)
}

fn render_official_auth_payload(home_dir: &Path) -> Result<Value> {
    let mut auth =
        read_global_official_auth_payload(home_dir)?.unwrap_or_else(minimal_official_auth_payload);
    auth.insert("OPENAI_API_KEY".to_string(), Value::Null);
    auth.insert(
        "auth_mode".to_string(),
        Value::String(OFFICIAL_AUTH_MODE.to_string()),
    );
    Ok(Value::Object(auth))
}

fn read_global_official_auth_payload(home_dir: &Path) -> Result<Option<Map<String, Value>>> {
    let auth_path = home_dir.join(".codex").join("auth.json");
    let auth_text = match fs::read_to_string(&auth_path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| {
                format!("reading global Codex auth state at {}", auth_path.display())
            })
        }
    };

    let auth_value: Value = serde_json::from_str(&auth_text)
        .with_context(|| format!("parsing global Codex auth state at {}", auth_path.display()))?;
    let auth_object = auth_value.as_object().cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "global Codex auth state at {} must be a JSON object",
            auth_path.display()
        )
    })?;
    Ok(Some(auth_object))
}

fn minimal_official_auth_payload() -> Map<String, Value> {
    let mut auth = Map::new();
    auth.insert("OPENAI_API_KEY".to_string(), Value::Null);
    auth.insert(
        "auth_mode".to_string(),
        Value::String(OFFICIAL_AUTH_MODE.to_string()),
    );
    auth
}

pub fn codex_home_root_for(home_dir: &Path, backend_id: &str) -> PathBuf {
    home_dir
        .join(".quickai")
        .join("runtime")
        .join("codex")
        .join(backend_id)
}

pub fn render_codex_config(
    provider_name: &str,
    base_url: &str,
    default_model: &str,
    workspace_dir: Option<&Path>,
) -> String {
    render_openai_compatible_codex_config(provider_name, base_url, default_model, workspace_dir)
}

fn render_openai_compatible_codex_config(
    provider_name: &str,
    base_url: &str,
    default_model: &str,
    workspace_dir: Option<&Path>,
) -> String {
    let trust_section = workspace_dir
        .and_then(|p| p.to_str())
        .map(|p| format!("\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n", p))
        .unwrap_or_default();

    format!(
        r#"model_provider = "{provider_name}"
model = "{default_model}"
model_reasoning_effort = "{API_KEY_MODEL_REASONING_EFFORT}"
disable_response_storage = true
preferred_auth_method = "apikey"
enableRouteSelection = true

[model_providers.{provider_name}]
name = "{provider_name}"
base_url = "{base_url}"
wire_api = "responses"
requires_openai_auth = false
{trust_section}"#
    )
}

fn render_official_codex_config(workspace_dir: Option<&Path>) -> String {
    let trust_section = workspace_dir
        .and_then(|p| p.to_str())
        .map(|p| format!("\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n", p))
        .unwrap_or_default();

    format!(
        r#"model_provider = "{OFFICIAL_MODEL_PROVIDER}"
model = "{OFFICIAL_MODEL}"
model_reasoning_effort = "{OFFICIAL_MODEL_REASONING_EFFORT}"
disable_response_storage = true
enableRouteSelection = true

[model_providers.{OFFICIAL_MODEL_PROVIDER}]
name = "{OFFICIAL_MODEL_PROVIDER}"
wire_api = "responses"
requires_openai_auth = true
{trust_section}"#
    )
}

fn sanitize_provider_name(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "provider".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn codex_home_is_scoped_under_quickai_runtime() {
        let path = codex_home_root_for(Path::new("/tmp/home"), "codex-main");
        assert_eq!(
            path,
            PathBuf::from("/tmp/home/.quickai/runtime/codex/codex-main")
        );
    }

    #[test]
    fn prepare_writes_auth_and_config_files() {
        let temp = tempdir().unwrap();
        let profile = RuntimeProviderProfile {
            id: "deepseek-openai".into(),
            protocol: RuntimeProviderProtocol::OpenaiCompatible {
                base_url: "https://api.deepseek.com/v1".into(),
                api_key: "sk-test".into(),
                default_model: "deepseek-chat".into(),
            },
        };

        let codex_home =
            prepare_isolated_codex_home(temp.path(), "codex-main", &profile, None).unwrap();
        let auth = fs::read_to_string(codex_home.join("auth.json")).unwrap();
        let config = fs::read_to_string(codex_home.join("config.toml")).unwrap();

        assert!(auth.contains("\"OPENAI_API_KEY\": \"sk-test\""));
        assert!(config.contains("base_url = \"https://api.deepseek.com/v1\""));
        assert!(config.contains("wire_api = \"responses\""));
        assert!(config.contains("model = \"deepseek-chat\""));
        assert!(config.contains("preferred_auth_method = \"apikey\""));
        assert!(config.contains("enableRouteSelection = true"));
        assert!(config.contains("requires_openai_auth = false"));
    }

    #[test]
    fn prepare_writes_trust_when_workspace_provided() {
        let temp = tempdir().unwrap();
        let profile = RuntimeProviderProfile {
            id: "aicodewith-openai".into(),
            protocol: RuntimeProviderProtocol::OpenaiCompatible {
                base_url: "https://api.aicodewith.com/chatgpt/v1".into(),
                api_key: "sk-acw-test".into(),
                default_model: "gpt-5.3-codex".into(),
            },
        };
        let workspace = Path::new("/Users/fishers/Desktop/repo/project");
        let codex_home =
            prepare_isolated_codex_home(temp.path(), "codex-main", &profile, Some(workspace))
                .unwrap();
        let config = fs::read_to_string(codex_home.join("config.toml")).unwrap();
        assert!(
            config.contains("[projects.\"/Users/fishers/Desktop/repo/project\"]"),
            "trust section missing: {config}"
        );
        assert!(config.contains("trust_level = \"trusted\""));
        assert!(config.contains("requires_openai_auth = false"));
    }

    #[test]
    fn prepare_no_trust_when_workspace_none() {
        let temp = tempdir().unwrap();
        let profile = RuntimeProviderProfile {
            id: "deepseek-openai".into(),
            protocol: RuntimeProviderProtocol::OpenaiCompatible {
                base_url: "https://api.deepseek.com/v1".into(),
                api_key: "sk-test".into(),
                default_model: "deepseek-chat".into(),
            },
        };
        let codex_home =
            prepare_isolated_codex_home(temp.path(), "codex-main", &profile, None).unwrap();
        let config = fs::read_to_string(codex_home.join("config.toml")).unwrap();
        assert!(
            !config.contains("[projects."),
            "unexpected trust section: {config}"
        );
        assert!(config.contains("requires_openai_auth = false"));
    }

    #[test]
    fn prepare_writes_official_openai_auth_projection() {
        let temp = tempdir().unwrap();
        let profile = RuntimeProviderProfile {
            id: "openai-official".into(),
            protocol: RuntimeProviderProtocol::OfficialSession,
        };

        let codex_home =
            prepare_isolated_codex_home(temp.path(), "codex-main", &profile, None).unwrap();
        let auth = fs::read_to_string(codex_home.join("auth.json")).unwrap();
        let config = fs::read_to_string(codex_home.join("config.toml")).unwrap();

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
    fn prepare_copies_global_official_auth_tokens_into_isolated_codex_home() {
        let temp = tempdir().unwrap();
        let global_codex_dir = temp.path().join(".codex");
        fs::create_dir_all(&global_codex_dir).unwrap();
        fs::write(
            global_codex_dir.join("auth.json"),
            serde_json::to_vec_pretty(&json!({
                "OPENAI_API_KEY": "should-be-null",
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
            prepare_isolated_codex_home(temp.path(), "codex-main", &profile, None).unwrap();
        let auth: Value =
            serde_json::from_str(&fs::read_to_string(codex_home.join("auth.json")).unwrap())
                .unwrap();

        assert_eq!(auth.get("OPENAI_API_KEY"), Some(&Value::Null));
        assert_eq!(
            auth.get("auth_mode"),
            Some(&Value::String("chatgpt".to_string()))
        );
        assert_eq!(
            auth.pointer("/tokens/access_token"),
            Some(&Value::String("access-token".to_string()))
        );
        assert_eq!(
            auth.pointer("/tokens/account_id"),
            Some(&Value::String("acct_123".to_string()))
        );
        assert_eq!(
            auth.get("last_refresh"),
            Some(&Value::String("2026-03-16T12:34:56Z".to_string()))
        );
    }

    #[test]
    fn prepare_rejects_non_supported_profiles() {
        let temp = tempdir().unwrap();
        let profile = RuntimeProviderProfile {
            id: "claude-official".into(),
            protocol: RuntimeProviderProtocol::AnthropicCompatible {
                base_url: "https://api.deepseek.com/anthropic".into(),
                auth_token: "sk-test".into(),
                default_model: "deepseek-chat".into(),
                small_fast_model: None,
            },
        };

        let err =
            prepare_isolated_codex_home(temp.path(), "codex-main", &profile, None).unwrap_err();
        assert!(err
            .to_string()
            .contains("requires openai_compatible or official_session provider profile"));
    }
}
