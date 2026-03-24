pub mod adapter;
pub mod policy;
pub mod probe;
pub mod session_driver;

pub use adapter::AcpBackendAdapter;

/// Identifies which ACP backend is used within the `family = "acp"` runtime family.
/// Optional — when omitted, the backend is treated as a generic ACP CLI backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpBackend {
    Claude,
    Codex,
    Codebuddy,
    Qwen,
    Iflow,
    Goose,
    Kimi,
    Opencode,
    Qoder,
    Vibe,
    Gemini,
    Custom,
}

/// Optional ACP auth-method identity for bridge-backed backends that advertise
/// multiple authentication methods during `initialize()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpAuthMethod {
    Chatgpt,
    OpenaiApiKey,
    CodexApiKey,
}

impl AcpAuthMethod {
    pub fn protocol_id(self) -> &'static str {
        match self {
            Self::Chatgpt => "chatgpt",
            Self::OpenaiApiKey => "openai-api-key",
            Self::CodexApiKey => "codex-api-key",
        }
    }
}

/// Codex-specific provider projection mode within the ACP family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexProjectionMode {
    /// Use ACP authenticate() negotiation (`chatgpt`, `openai_api_key`, `codex_api_key`).
    AcpAuth,
    /// Materialize local Codex config/auth files and point the process at an isolated CODEX_HOME.
    LocalConfig,
}

pub fn supports_native_local_skills(backend: Option<AcpBackend>) -> bool {
    matches!(
        backend,
        Some(
            AcpBackend::Claude
                | AcpBackend::Codex
                | AcpBackend::Codebuddy
                | AcpBackend::Qwen
                | AcpBackend::Iflow
                | AcpBackend::Goose
                | AcpBackend::Kimi
                | AcpBackend::Opencode
                | AcpBackend::Qoder
                | AcpBackend::Vibe
                | AcpBackend::Gemini
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_native_local_skills_gemini_returns_true() {
        assert!(supports_native_local_skills(Some(AcpBackend::Gemini)));
    }

    #[test]
    fn supports_native_local_skills_custom_returns_false() {
        assert!(!supports_native_local_skills(Some(AcpBackend::Custom)));
    }

    #[test]
    fn supports_native_local_skills_none_returns_false() {
        assert!(!supports_native_local_skills(None));
    }
}
