use super::AcpBackend;

/// How the ACP backend is launched — bridge package (npx adapter) or raw CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapStyle {
    /// Launched via a dedicated adapter package (e.g., `npx @zed-industries/claude-agent-acp`).
    /// The adapter wraps the underlying AI system and speaks ACP on its behalf.
    BridgeBacked,
    /// Launched as a raw CLI tool that natively speaks ACP (e.g., `qwen --acp`).
    Generic,
}

/// ACP backend session resume 策略。
///
/// 实际生效需要在 `initialize()` 响应中确认 `AgentCapabilities.load_session == true`。
/// 若 backend 未声明该能力，runtime 自动退化为 `new_session()`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeStrategy {
    /// 使用 ACP 标准 `session/load` 方法恢复后端 session。
    /// 支持：claude-agent-acp（已验证）、codex-acp（已验证 load_session: true）。
    AcpLoadSession,
    /// 不尝试 resume（generic CLI 或能力未知的 bridge）。
    None,
}

/// Lightweight per-backend compatibility policy.
/// Derived from `AcpBackend` identity — not a second config surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpBackendPolicy {
    pub bootstrap_style: BootstrapStyle,
    /// Whether this backend requires extra MCP config loading at startup.
    /// Currently only codebuddy loads `~/.codebuddy/mcp.json`.
    pub special_mcp_loading: bool,
    /// Session resume 策略：bridge-backed backend 设为 AcpLoadSession。
    pub resume_strategy: ResumeStrategy,
}

impl AcpBackendPolicy {
    /// Returns the policy for the given ACP backend identity.
    /// `None` means generic ACP CLI (same as `Generic` policy).
    pub fn for_backend(backend: Option<AcpBackend>) -> Self {
        match backend {
            Some(AcpBackend::Claude) => Self {
                bootstrap_style: BootstrapStyle::BridgeBacked,
                special_mcp_loading: false,
                // Verified: claude-agent-acp implements load_session(), reads ~/.claude/ JSONL
                resume_strategy: ResumeStrategy::AcpLoadSession,
            },
            Some(AcpBackend::Codebuddy) => Self {
                bootstrap_style: BootstrapStyle::BridgeBacked,
                special_mcp_loading: true,
                // Expected: bridge-backed; degrades gracefully if capability=false at runtime
                resume_strategy: ResumeStrategy::AcpLoadSession,
            },
            Some(AcpBackend::Codex) => Self {
                bootstrap_style: BootstrapStyle::BridgeBacked,
                special_mcp_loading: false,
                // Verified: codex-acp declares load_session: true, uses resume_thread_from_rollout()
                resume_strategy: ResumeStrategy::AcpLoadSession,
            },
            // All other named backends + None (generic) use the shared generic path.
            // Kimi, Opencode, Qoder, Vibe, Custom etc. → unverified, use None for safety.
            _ => Self {
                bootstrap_style: BootstrapStyle::Generic,
                special_mcp_loading: false,
                resume_strategy: ResumeStrategy::None,
            },
        }
    }

    pub fn is_bridge_backed(&self) -> bool {
        self.bootstrap_style == BootstrapStyle::BridgeBacked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_maps_to_bridge_backed_policy() {
        let policy = AcpBackendPolicy::for_backend(Some(AcpBackend::Claude));
        assert_eq!(policy.bootstrap_style, BootstrapStyle::BridgeBacked);
        assert!(!policy.special_mcp_loading);
    }

    #[test]
    fn codebuddy_maps_to_bridge_backed_with_special_mcp_loading() {
        let policy = AcpBackendPolicy::for_backend(Some(AcpBackend::Codebuddy));
        assert_eq!(policy.bootstrap_style, BootstrapStyle::BridgeBacked);
        assert!(policy.special_mcp_loading);
    }

    #[test]
    fn codex_maps_to_bridge_backed_policy() {
        let policy = AcpBackendPolicy::for_backend(Some(AcpBackend::Codex));
        assert_eq!(policy.bootstrap_style, BootstrapStyle::BridgeBacked);
        assert!(!policy.special_mcp_loading);
    }

    #[test]
    fn qwen_maps_to_generic_acp_cli_policy() {
        let policy = AcpBackendPolicy::for_backend(Some(AcpBackend::Qwen));
        assert_eq!(policy.bootstrap_style, BootstrapStyle::Generic);
        assert!(!policy.special_mcp_loading);
    }

    #[test]
    fn omitted_acp_backend_maps_to_generic_acp_cli_policy() {
        let policy = AcpBackendPolicy::for_backend(None);
        assert_eq!(policy.bootstrap_style, BootstrapStyle::Generic);
        assert!(!policy.special_mcp_loading);
    }

    #[test]
    fn claude_uses_acp_load_session() {
        let policy = AcpBackendPolicy::for_backend(Some(AcpBackend::Claude));
        assert_eq!(policy.resume_strategy, ResumeStrategy::AcpLoadSession);
    }

    #[test]
    fn codex_uses_acp_load_session() {
        // codex-acp explicitly declares load_session: true in AgentCapabilities
        let policy = AcpBackendPolicy::for_backend(Some(AcpBackend::Codex));
        assert_eq!(policy.resume_strategy, ResumeStrategy::AcpLoadSession);
    }

    #[test]
    fn codebuddy_uses_acp_load_session() {
        let policy = AcpBackendPolicy::for_backend(Some(AcpBackend::Codebuddy));
        assert_eq!(policy.resume_strategy, ResumeStrategy::AcpLoadSession);
    }

    #[test]
    fn generic_backends_have_no_resume_strategy() {
        assert_eq!(
            AcpBackendPolicy::for_backend(Some(AcpBackend::Qwen)).resume_strategy,
            ResumeStrategy::None
        );
        assert_eq!(
            AcpBackendPolicy::for_backend(None).resume_strategy,
            ResumeStrategy::None
        );
    }
}
