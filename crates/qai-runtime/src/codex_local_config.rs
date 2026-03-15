use crate::provider_profiles::{RuntimeProviderProfile, RuntimeProviderProtocol};
use anyhow::{bail, Context, Result};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

const MODEL_REASONING_EFFORT: &str = "low";

pub fn prepare_isolated_codex_home(
    home_dir: &Path,
    backend_id: &str,
    profile: &RuntimeProviderProfile,
    workspace_dir: Option<&Path>,
) -> Result<PathBuf> {
    let RuntimeProviderProtocol::OpenaiCompatible {
        base_url,
        api_key,
        default_model,
    } = &profile.protocol
    else {
        bail!(
            "codex local_config projection requires openai_compatible provider profile, got `{}`",
            profile.id
        );
    };

    let codex_home = codex_home_root_for(home_dir, backend_id);
    fs::create_dir_all(&codex_home)
        .with_context(|| format!("creating isolated CODEX_HOME at {}", codex_home.display()))?;

    let auth_payload = json!({
        "OPENAI_API_KEY": api_key,
    });
    let auth_path = codex_home.join("auth.json");
    let config_path = codex_home.join("config.toml");
    let provider_name = sanitize_provider_name(&profile.id);
    let config_text = render_codex_config(&provider_name, base_url, default_model, workspace_dir);

    fs::write(
        &auth_path,
        serde_json::to_vec_pretty(&auth_payload).context("serializing Codex auth payload")?,
    )
    .with_context(|| format!("writing {}", auth_path.display()))?;
    fs::write(&config_path, config_text)
        .with_context(|| format!("writing {}", config_path.display()))?;

    Ok(codex_home)
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
    let trust_section = workspace_dir
        .and_then(|p| p.to_str())
        .map(|p| format!("\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n", p))
        .unwrap_or_default();

    format!(
        r#"model_provider = "{provider_name}"
model = "{default_model}"
model_reasoning_effort = "{MODEL_REASONING_EFFORT}"
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
    fn prepare_rejects_non_openai_profiles() {
        let temp = tempdir().unwrap();
        let profile = RuntimeProviderProfile {
            id: "claude-official".into(),
            protocol: RuntimeProviderProtocol::OfficialSession,
        };

        let err =
            prepare_isolated_codex_home(temp.path(), "codex-main", &profile, None).unwrap_err();
        assert!(err
            .to_string()
            .contains("requires openai_compatible provider profile"));
    }
}
