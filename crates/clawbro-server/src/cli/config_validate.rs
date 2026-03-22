use crate::channels_internal::wechat::WeChatConfig;
use crate::cli::config_model::ConfigGraph;
use crate::cli::env::load_user_dot_env;
use crate::config::{
    BackendLaunchConfig, GatewayConfig, InteractionMode, LarkSection, ProviderProfileProtocolConfig,
};
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub severity: ValidationSeverity,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Default, Clone)]
pub struct ValidationReport {
    pub issues: Vec<ValidationIssue>,
}

impl ValidationReport {
    pub fn push_error(&mut self, code: &'static str, message: impl Into<String>) {
        self.issues.push(ValidationIssue {
            severity: ValidationSeverity::Error,
            code,
            message: message.into(),
        });
    }

    pub fn push_warning(&mut self, code: &'static str, message: impl Into<String>) {
        self.issues.push(ValidationIssue {
            severity: ValidationSeverity::Warning,
            code,
            message: message.into(),
        });
    }

    pub fn has_errors(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.severity == ValidationSeverity::Error)
    }

    pub fn error_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.severity == ValidationSeverity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.severity == ValidationSeverity::Warning)
            .count()
    }
}

pub fn validate_config_path(path: &Path) -> Result<ValidationReport> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    let cfg = GatewayConfig::from_toml_str(&content).context("parse GatewayConfig from TOML")?;
    Ok(validate_gateway_config(&cfg))
}

pub fn validate_gateway_config(cfg: &GatewayConfig) -> ValidationReport {
    load_user_dot_env();

    let mut report = ValidationReport::default();
    validate_structural(cfg, &mut report);
    validate_topology(cfg, &mut report);
    validate_runtime_preflight(cfg, &mut report);
    report
}

pub fn validate_gateway_config_static(cfg: &GatewayConfig) -> ValidationReport {
    let mut report = ValidationReport::default();
    validate_structural(cfg, &mut report);
    validate_topology(cfg, &mut report);
    report
}

pub fn validate_graph(graph: &ConfigGraph) -> ValidationReport {
    validate_gateway_config(&graph.to_gateway_config())
}

pub fn validate_graph_static(graph: &ConfigGraph) -> ValidationReport {
    validate_gateway_config_static(&graph.to_gateway_config())
}

fn validate_structural(cfg: &GatewayConfig, report: &mut ValidationReport) {
    validate_unique(
        cfg.provider_profiles.iter().map(|p| p.id.as_str()),
        "provider_profile.id",
        "duplicate_provider_profile",
        report,
    );
    validate_unique(
        cfg.backends.iter().map(|b| b.id.as_str()),
        "backend.id",
        "duplicate_backend",
        report,
    );
    validate_unique(
        cfg.agent_roster.iter().map(|a| a.name.as_str()),
        "agent_roster.name",
        "duplicate_agent",
        report,
    );
    validate_unique(
        cfg.groups.iter().map(|g| g.scope.as_str()),
        "group.scope",
        "duplicate_group_scope",
        report,
    );
    let mut seen_team_scopes = BTreeSet::new();
    for team_scope in &cfg.team_scopes {
        let channel = team_scope.mode.channel.as_deref().unwrap_or("*").trim();
        let scope = team_scope.scope.trim();
        if scope.is_empty() {
            report.push_error(
                "duplicate_team_scope_scope",
                "team_scope.scope contains an empty or whitespace-only value",
            );
            continue;
        }
        if !seen_team_scopes.insert((channel.to_string(), scope.to_string())) {
            report.push_error(
                "duplicate_team_scope_scope",
                format!("duplicate team_scope `{scope}` for channel `{channel}`"),
            );
        }
    }

    if let Some(lark) = cfg.channels.lark.as_ref() {
        validate_lark(lark, report);
    }
}

fn validate_topology(cfg: &GatewayConfig, report: &mut ValidationReport) {
    if let Err(err) = cfg.validate_runtime_topology() {
        report.push_error("runtime_topology", err.to_string());
    }

    for team_scope in &cfg.team_scopes {
        if team_scope.mode.channel.as_deref() == Some("wechat")
            && !team_scope.scope.starts_with("user:")
        {
            report.push_error(
                "wechat_team_scope_scope_family",
                format!(
                    "team_scope `{}` uses channel `wechat` but scope is not a DM user scope; WeChat currently supports only `user:*` scopes",
                    team_scope.scope
                ),
            );
        }
    }

    for group in &cfg.groups {
        if matches!(group.mode.interaction, InteractionMode::Team)
            && group.mode.channel.as_deref() == Some("wechat")
        {
            report.push_error(
                "wechat_group_team_unsupported",
                format!(
                    "group `{}` uses channel `wechat` with team mode, but WeChat team orchestration currently supports DM user scopes only",
                    group.scope
                ),
            );
        }
    }
}

fn validate_runtime_preflight(cfg: &GatewayConfig, report: &mut ValidationReport) {
    let referenced_provider_ids: BTreeSet<&str> = cfg
        .backends
        .iter()
        .filter_map(|backend| backend.provider_profile.as_deref())
        .collect();

    for profile in &cfg.provider_profiles {
        if !referenced_provider_ids.contains(profile.id.as_str()) {
            continue;
        }

        match &profile.protocol {
            ProviderProfileProtocolConfig::OfficialSession => {}
            ProviderProfileProtocolConfig::AnthropicCompatible { auth_token_env, .. }
            | ProviderProfileProtocolConfig::OpenaiCompatible { auth_token_env, .. } => {
                let missing = std::env::var(auth_token_env)
                    .ok()
                    .is_none_or(|value| value.trim().is_empty());
                if missing {
                    report.push_error(
                        "missing_provider_env",
                        format!(
                            "provider_profile `{}` requires environment variable `{}` to be set (shell or ~/.clawbro/.env)",
                            profile.id, auth_token_env
                        ),
                    );
                }
            }
        }
    }

    for backend in &cfg.backends {
        if let BackendLaunchConfig::ExternalCommand { command, .. } = &backend.launch {
            if which::which(command).is_err() {
                report.push_error(
                    "missing_launch_command",
                    format!(
                        "backend `{}` external command `{}` is not available in PATH",
                        backend.id, command
                    ),
                );
            }
        }
    }

    if cfg
        .channels
        .wechat
        .as_ref()
        .is_some_and(|wechat| wechat.enabled)
        && std::env::var("WECHAT_BOT_TOKEN")
            .ok()
            .is_none_or(|value| value.trim().is_empty())
    {
        let primary = WeChatConfig::credentials_path();
        let claude_fallback = fallback_claude_wechat_credentials_path();
        validate_wechat_credentials(report, &primary, &claude_fallback);
    }
}

fn validate_unique<'a>(
    values: impl Iterator<Item = &'a str>,
    label: &str,
    code: &'static str,
    report: &mut ValidationReport,
) {
    let mut seen = BTreeSet::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            report.push_error(
                code,
                format!("{label} contains an empty or whitespace-only value"),
            );
            continue;
        }
        if !seen.insert(trimmed.to_string()) {
            report.push_error(code, format!("duplicate {label} `{trimmed}`"));
        }
    }
}

fn validate_lark(lark: &LarkSection, report: &mut ValidationReport) {
    let mut seen_ids = BTreeSet::new();
    for instance in &lark.instances {
        let id = instance.id.trim();
        if id.is_empty() {
            report.push_error("empty_lark_instance_id", "lark instance id cannot be empty");
            continue;
        }
        if !seen_ids.insert(id.to_string()) {
            report.push_error(
                "duplicate_lark_instance_id",
                format!("duplicate lark instance id `{id}`"),
            );
        }
    }

    if lark.enabled && lark.instances.is_empty() {
        report.push_warning(
            "lark_no_instances",
            "lark is enabled but no [[channels.lark.instances]] are configured; runtime will rely on environment fallback only",
        );
    }
}

fn fallback_claude_wechat_credentials_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".claude")
        .join("channels")
        .join("wechat")
        .join("account.json")
}

fn validate_wechat_credentials(
    report: &mut ValidationReport,
    primary: &Path,
    claude_fallback: &Path,
) {
    if !primary.exists() && !claude_fallback.exists() {
        report.push_error(
            "missing_wechat_credentials",
            format!(
                "WeChat is enabled but neither WECHAT_BOT_TOKEN nor credential files exist at {} or {}",
                primary.display(),
                claude_fallback.display()
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_graph_flags_missing_provider_env() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
[[provider_profile]]
id = "deepseek-anthropic"
protocol = "anthropic_compatible"
base_url = "https://api.deepseek.com/anthropic"
auth_token_env = "CLAWBRO_TEST_MISSING_ENV"
default_model = "deepseek-chat"

[[backend]]
id = "claude-main"
family = "acp"
acp_backend = "claude"
provider_profile = "deepseek-anthropic"

[backend.launch]
type = "bundled_command"

[agent]
backend_id = "claude-main"
"#,
        )
        .unwrap();

        let report = validate_gateway_config(&cfg);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.code == "missing_provider_env"));
    }

    #[test]
    fn validate_graph_flags_missing_wechat_credentials() {
        let mut report = ValidationReport::default();
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("primary.json");
        let fallback = dir.path().join("fallback.json");
        validate_wechat_credentials(&mut report, &primary, &fallback);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.code == "missing_wechat_credentials"));
    }

    #[test]
    fn validate_graph_rejects_wechat_team_scope_group_style_scope() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
[[backend]]
id = "codex-main"
family = "acp"
acp_backend = "codex"

[backend.launch]
type = "bundled_command"

[[agent_roster]]
name = "claw"
mentions = ["@claw"]
backend_id = "codex-main"

[[team_scope]]
scope = "group:wechat:demo"

[team_scope.mode]
interaction = "team"
channel = "wechat"
front_bot = "claw"

[team_scope.team]
roster = ["claw"]
"#,
        )
        .unwrap();

        let report = validate_gateway_config(&cfg);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.code == "wechat_team_scope_scope_family"));
    }

    #[test]
    fn validate_graph_flags_missing_external_command() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
[[backend]]
id = "custom-main"
family = "acp"
acp_backend = "claude"

[backend.launch]
type = "external_command"
command = "clawbro-this-command-should-not-exist"

[agent]
backend_id = "custom-main"
"#,
        )
        .unwrap();

        let report = validate_gateway_config(&cfg);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.code == "missing_launch_command"));
    }
}
