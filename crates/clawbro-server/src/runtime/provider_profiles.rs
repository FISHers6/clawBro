use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

pub use crate::runtime::contract::{RuntimeProviderProfile, RuntimeProviderProtocol};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfiguredProviderProfile {
    pub id: String,
    pub protocol: ConfiguredProviderProtocol,
}

impl ConfiguredProviderProfile {
    pub fn resolve_from_env(&self) -> Result<RuntimeProviderProfile> {
        Ok(RuntimeProviderProfile {
            id: self.id.clone(),
            protocol: match &self.protocol {
                ConfiguredProviderProtocol::OfficialSession => {
                    RuntimeProviderProtocol::OfficialSession
                }
                ConfiguredProviderProtocol::AnthropicCompatible {
                    base_url,
                    auth_token_env,
                    default_model,
                    small_fast_model,
                } => RuntimeProviderProtocol::AnthropicCompatible {
                    base_url: base_url.clone(),
                    auth_token: required_env(auth_token_env)?,
                    default_model: default_model.clone(),
                    small_fast_model: small_fast_model.clone(),
                },
                ConfiguredProviderProtocol::OpenaiCompatible {
                    base_url,
                    auth_token_env,
                    default_model,
                } => RuntimeProviderProtocol::OpenaiCompatible {
                    base_url: base_url.clone(),
                    api_key: required_env(auth_token_env)?,
                    default_model: default_model.clone(),
                },
            },
        })
    }
}

fn required_env(name: &str) -> Result<String> {
    std::env::var(name).map_err(|_| {
        anyhow!("required environment variable `{name}` is not set for provider profile")
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum ConfiguredProviderProtocol {
    OfficialSession,
    AnthropicCompatible {
        base_url: String,
        auth_token_env: String,
        default_model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        small_fast_model: Option<String>,
    },
    OpenaiCompatible {
        base_url: String,
        auth_token_env: String,
        default_model: String,
    },
}
