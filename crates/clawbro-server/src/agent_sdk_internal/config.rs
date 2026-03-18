//! Configuration loaded from environment variables.
//! No Tauri dependencies — pure environment variable configuration.

use anyhow::{bail, Result};

use crate::agent_sdk_internal::bridge::{RuntimeProviderProfile, RuntimeProviderProtocol};

#[derive(Debug, Clone)]
pub enum Provider {
    Anthropic {
        base_url: Option<String>,
    },
    /// OpenAI-compatible provider. `base_url` is None for api.openai.com, Some(url) for custom.
    OpenAI {
        base_url: Option<String>,
    },
    DeepSeek,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub provider: Provider,
    pub api_key: String,
    pub model: String,
    pub system_prompt: String,
}

impl AgentConfig {
    /// Load configuration from environment variables.
    ///
    /// Priority: ANTHROPIC_API_KEY > OPENAI_API_KEY > DEEPSEEK_API_KEY
    /// Model: CLAWBRO_MODEL (defaults vary by provider)
    /// System prompt: CLAWBRO_SYSTEM_PROMPT
    pub fn from_env() -> Result<Self> {
        let system_prompt = std::env::var("CLAWBRO_SYSTEM_PROMPT")
            .unwrap_or_else(|_| "You are a helpful AI coding assistant.".to_string());

        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            return Ok(Self {
                provider: Provider::Anthropic { base_url: None },
                api_key: key,
                model: std::env::var("CLAWBRO_MODEL")
                    .unwrap_or_else(|_| "claude-opus-4-6".to_string()),
                system_prompt,
            });
        }
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            // If OPENAI_API_BASE is set, use it as a custom base URL (e.g. DeepSeek, local LLM)
            let base_url = std::env::var("OPENAI_API_BASE").ok();
            let default_model = if base_url.as_deref().unwrap_or("").contains("deepseek") {
                "deepseek-chat".to_string()
            } else {
                "gpt-4o".to_string()
            };
            return Ok(Self {
                provider: Provider::OpenAI { base_url },
                api_key: key,
                model: std::env::var("CLAWBRO_MODEL").unwrap_or(default_model),
                system_prompt,
            });
        }
        if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
            return Ok(Self {
                provider: Provider::DeepSeek,
                api_key: key,
                model: std::env::var("CLAWBRO_MODEL")
                    .unwrap_or_else(|_| "deepseek-chat".to_string()),
                system_prompt,
            });
        }
        bail!("No API key found. Set ANTHROPIC_API_KEY, OPENAI_API_KEY, or DEEPSEEK_API_KEY")
    }

    /// Returns true if this config uses a non-streaming provider (OpenAI / DeepSeek fallback).
    pub fn is_openai_or_deepseek(&self) -> bool {
        matches!(self.provider, Provider::OpenAI { .. } | Provider::DeepSeek)
    }

    pub fn with_runtime_provider_profile(
        &self,
        profile: Option<&RuntimeProviderProfile>,
    ) -> Result<Self> {
        let Some(profile) = profile else {
            return Ok(self.clone());
        };

        match &profile.protocol {
            RuntimeProviderProtocol::OfficialSession => Ok(self.clone()),
            RuntimeProviderProtocol::AnthropicCompatible {
                base_url,
                auth_token,
                default_model,
                small_fast_model: _,
            } => Ok(Self {
                provider: Provider::Anthropic {
                    base_url: Some(base_url.clone()),
                },
                api_key: auth_token.clone(),
                model: default_model.clone(),
                system_prompt: self.system_prompt.clone(),
            }),
            RuntimeProviderProtocol::OpenaiCompatible {
                base_url,
                api_key,
                default_model,
            } => Ok(Self {
                provider: Provider::OpenAI {
                    base_url: Some(base_url.clone()),
                },
                api_key: api_key.clone(),
                model: default_model.clone(),
                system_prompt: self.system_prompt.clone(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize env-mutating tests to avoid race conditions
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_config_from_env_missing_key() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Clear all API key vars to ensure error
        unsafe {
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("DEEPSEEK_API_KEY");
        }
        assert!(AgentConfig::from_env().is_err());
    }

    #[test]
    fn test_config_anthropic() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("OPENAI_API_BASE");
            std::env::remove_var("DEEPSEEK_API_KEY");
            std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
            std::env::remove_var("CLAWBRO_MODEL");
        }
        let config = AgentConfig::from_env().unwrap();
        assert!(matches!(config.provider, Provider::Anthropic { .. }));
        assert_eq!(config.api_key, "sk-test");
        assert_eq!(config.model, "claude-opus-4-6"); // default
        unsafe {
            std::env::remove_var("ANTHROPIC_API_KEY");
        }
    }

    #[test]
    fn test_config_openai_base_url() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("DEEPSEEK_API_KEY");
            std::env::set_var("OPENAI_API_KEY", "sk-ds-test");
            std::env::set_var("OPENAI_API_BASE", "https://api.deepseek.com");
            std::env::set_var("CLAWBRO_MODEL", "deepseek-chat");
        }
        let config = AgentConfig::from_env().unwrap();
        assert!(matches!(
            config.provider,
            Provider::OpenAI { base_url: Some(_) }
        ));
        assert_eq!(config.model, "deepseek-chat");
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("OPENAI_API_BASE");
            std::env::remove_var("CLAWBRO_MODEL");
        }
    }

    #[test]
    fn runtime_provider_profile_overrides_to_anthropic_compatible() {
        let cfg = AgentConfig {
            provider: Provider::OpenAI { base_url: None },
            api_key: "sk-openai".into(),
            model: "gpt-4o".into(),
            system_prompt: "base".into(),
        };
        let profile = RuntimeProviderProfile {
            id: "claude-deepseek".into(),
            protocol: RuntimeProviderProtocol::AnthropicCompatible {
                base_url: "https://api.deepseek.com/anthropic".into(),
                auth_token: "sk-deepseek".into(),
                default_model: "deepseek-chat".into(),
                small_fast_model: Some("deepseek-chat".into()),
            },
        };
        let projected = cfg.with_runtime_provider_profile(Some(&profile)).unwrap();
        assert!(matches!(
            projected.provider,
            Provider::Anthropic { base_url: Some(_) }
        ));
        assert_eq!(projected.api_key, "sk-deepseek");
        assert_eq!(projected.model, "deepseek-chat");
    }
}
