use crate::acp_engine::{AcpEngine, AcpEngineConfig};
use crate::traits::BoxEngine;
use std::sync::Arc;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineConfig {
    /// quickai-rust-agent: rig-core Rust Agent (Anthropic/OpenAI/DeepSeek)
    RustAgent {
        #[serde(default)]
        binary: Option<String>,
    },
    /// quickai-claude-agent: claude-agent-sdk wrapper (calls claude-code CLI)
    ClaudeAgent {
        #[serde(default)]
        binary: Option<String>,
    },
    /// codex-acp: OpenAI Codex CLI via ACP
    CodexAcp {
        #[serde(default)]
        binary: Option<String>,
    },
    /// 任意自定义 ACP server
    CustomAcp {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self::RustAgent { binary: None }
    }
}

pub struct EngineSelector;

impl EngineSelector {
    pub fn build(config: &EngineConfig) -> BoxEngine {
        let acp_config = match config {
            EngineConfig::RustAgent { binary } => AcpEngineConfig {
                command: binary
                    .clone()
                    .unwrap_or_else(|| "quickai-rust-agent".to_string()),
                args: vec![],
                env: vec![],
                workspace_dir: None,
            },
            EngineConfig::ClaudeAgent { binary } => AcpEngineConfig {
                command: binary
                    .clone()
                    .unwrap_or_else(|| "quickai-claude-agent".to_string()),
                args: vec![],
                env: vec![],
                workspace_dir: None,
            },
            EngineConfig::CodexAcp { binary } => AcpEngineConfig {
                command: binary.clone().unwrap_or_else(|| "codex-acp".to_string()),
                args: vec![],
                env: vec![],
                workspace_dir: None,
            },
            EngineConfig::CustomAcp { command, args } => AcpEngineConfig {
                command: command.clone(),
                args: args.clone(),
                env: vec![],
                workspace_dir: None,
            },
        };
        Arc::new(AcpEngine::new(acp_config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selector_builds_rust_agent() {
        let config = EngineConfig::RustAgent { binary: None };
        let engine = EngineSelector::build(&config);
        assert_eq!(engine.name(), "quickai-rust-agent");
    }

    #[test]
    fn test_selector_custom_binary() {
        let config = EngineConfig::RustAgent {
            binary: Some("/usr/local/bin/my-agent".to_string()),
        };
        let engine = EngineSelector::build(&config);
        assert_eq!(engine.name(), "/usr/local/bin/my-agent");
    }

    #[test]
    fn test_engine_config_deserialize() {
        let json = r#"{"type":"rust_agent","binary":null}"#;
        let c: EngineConfig = serde_json::from_str(json).unwrap();
        matches!(c, EngineConfig::RustAgent { .. });
    }
}
