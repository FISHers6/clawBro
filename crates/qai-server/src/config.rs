use anyhow::Result;
use qai_agent::bindings::{BindingPeerKind, BindingRule};
use qai_agent::roster::AgentEntry;
use qai_agent::team::milestone_delivery::TeamPublicUpdatesMode;
use qai_runtime::{AcpBackend, ApprovalMode, BackendFamily, BackendSpec, LaunchSpec};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthSection {
    /// Bearer token for /ws endpoint. None or empty string = open mode.
    pub ws_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySection {
    #[serde(default = "default_distill_n")]
    pub distill_every_n: u64,
    #[serde(default = "default_distiller_binary")]
    pub distiller_binary: String,
    #[serde(default = "default_shared_dir")]
    pub shared_dir: PathBuf,
    #[serde(default = "default_shared_max_words")]
    pub shared_memory_max_words: usize,
    #[serde(default = "default_agent_max_words")]
    pub agent_memory_max_words: usize,
}

fn default_distill_n() -> u64 {
    20
}
fn default_distiller_binary() -> String {
    "quickai-rust-agent".to_string()
}
fn default_shared_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".quickai")
        .join("shared")
}
fn default_shared_max_words() -> usize {
    300
}
fn default_agent_max_words() -> usize {
    500
}

impl Default for MemorySection {
    fn default() -> Self {
        Self {
            distill_every_n: default_distill_n(),
            distiller_binary: default_distiller_binary(),
            shared_dir: default_shared_dir(),
            shared_memory_max_words: default_shared_max_words(),
            agent_memory_max_words: default_agent_max_words(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobConfig {
    pub name: String,
    pub expr: String,
    pub prompt: String,
    pub session_key: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub condition: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GatewayConfig {
    #[serde(default)]
    pub gateway: GatewaySection,
    #[serde(default)]
    pub agent: AgentSection,
    #[serde(default)]
    pub auth: AuthSection,
    #[serde(default)]
    pub channels: ChannelsSection,
    #[serde(default)]
    pub skills: SkillsSection,
    #[serde(default)]
    pub session: SessionSection,
    #[serde(default)]
    pub agent_roster: Vec<AgentEntry>,
    /// Canonical backend catalog (`[[backend]]`).
    #[serde(default, rename = "backend")]
    pub backends: Vec<BackendCatalogEntry>,
    /// Canonical provider profile registry (`[[provider_profile]]`).
    #[serde(default, rename = "provider_profile")]
    pub provider_profiles: Vec<ProviderProfileConfig>,
    #[serde(default)]
    pub memory: MemorySection,
    #[serde(default)]
    pub cron_jobs: Vec<CronJobConfig>,
    /// 群组专项配置列表（`[[group]]` 段，可配置交互模式和 Team Mode 参数）
    #[serde(default, rename = "group")]
    pub groups: Vec<GroupConfig>,
    /// 精确 scope team 配置（`[[team_scope]]` 段），用于 DM 等非 group scope。
    #[serde(default, rename = "team_scope")]
    pub team_scopes: Vec<TeamScopeConfig>,
    /// Deterministic routing bindings (`[[binding]]`).
    #[serde(default, rename = "binding")]
    pub bindings: Vec<BindingConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewaySection {
    pub host: String,
    pub port: u16,
    /// When true, bot only responds in group chats if @-mentioned.
    /// In private chats this flag has no effect.
    #[serde(default)]
    pub require_mention_in_groups: bool,
    /// Default workspace directory for all agent sessions.
    #[serde(default)]
    pub default_workspace: Option<PathBuf>,
}

impl Default for GatewaySection {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 0,
            require_mention_in_groups: false,
            default_workspace: None,
        }
    }
}

impl GatewayConfig {
    pub fn resolved_default_backend_id(&self) -> Option<String> {
        let backend_id = self.agent.backend_id.trim();
        (!backend_id.is_empty()).then(|| backend_id.to_string())
    }

    pub fn normalized_team_scopes(&self) -> Vec<TeamScopeSpec> {
        let mut by_scope = BTreeMap::new();

        for group in self
            .groups
            .iter()
            .filter(|group| matches!(group.mode.interaction, InteractionMode::Team))
        {
            by_scope.insert(
                (group.mode.channel.clone(), group.scope.clone()),
                TeamScopeSpec {
                    scope: group.scope.clone(),
                    name: group.name.clone(),
                    mode: group.mode.clone(),
                    team: group.team.clone(),
                    source: TeamScopeSource::LegacyGroup,
                },
            );
        }

        for team_scope in &self.team_scopes {
            by_scope.insert(
                (team_scope.mode.channel.clone(), team_scope.scope.clone()),
                TeamScopeSpec {
                    scope: team_scope.scope.clone(),
                    name: team_scope.name.clone(),
                    mode: team_scope.mode.clone(),
                    team: team_scope.team.clone(),
                    source: TeamScopeSource::ExactScope,
                },
            );
        }

        by_scope.into_values().collect()
    }

    pub fn validate_runtime_topology(&self) -> Result<()> {
        if self.backends.is_empty() {
            anyhow::bail!("at least one [[backend]] entry is required");
        }

        for backend in &self.backends {
            if backend.acp_backend.is_some() && !matches!(backend.family, BackendFamilyConfig::Acp)
            {
                anyhow::bail!(
                    "backend `{}` sets acp_backend but family is not `acp`",
                    backend.id
                );
            }
            if backend.acp_auth_method.is_some()
                && !matches!(backend.family, BackendFamilyConfig::Acp)
            {
                anyhow::bail!(
                    "backend `{}` sets acp_auth_method but family is not `acp`",
                    backend.id
                );
            }
            if backend.acp_auth_method.is_some() && backend.acp_backend != Some(AcpBackend::Codex) {
                anyhow::bail!(
                    "backend `{}` sets acp_auth_method but only acp_backend = \"codex\" is supported in the current phase",
                    backend.id
                );
            }
            if backend.codex.is_some() && !matches!(backend.family, BackendFamilyConfig::Acp) {
                anyhow::bail!(
                    "backend `{}` sets [backend.codex] but family is not `acp`",
                    backend.id
                );
            }
            if backend.codex.is_some() && backend.acp_backend != Some(AcpBackend::Codex) {
                anyhow::bail!(
                    "backend `{}` sets [backend.codex] but only acp_backend = \"codex\" is supported in the current phase",
                    backend.id
                );
            }

            let mut seen_mcp_names = BTreeSet::new();
            for server in &backend.external_mcp_servers {
                let name = server.name.trim();
                if name.is_empty() {
                    anyhow::bail!(
                        "backend `{}` contains an external MCP server with an empty name",
                        backend.id
                    );
                }
                if name == "team-tools" {
                    anyhow::bail!(
                        "backend `{}` uses reserved external MCP server name `team-tools`",
                        backend.id
                    );
                }
                if !seen_mcp_names.insert(name.to_string()) {
                    anyhow::bail!(
                        "backend `{}` contains duplicate external MCP server name `{}`",
                        backend.id,
                        name
                    );
                }
            }
        }

        let mut seen_provider_ids = BTreeSet::new();
        for profile in &self.provider_profiles {
            let id = profile.id.trim();
            if id.is_empty() {
                anyhow::bail!("provider_profile id cannot be empty");
            }
            if !seen_provider_ids.insert(id.to_string()) {
                anyhow::bail!("duplicate provider_profile id `{id}`");
            }
            profile.validate()?;
        }

        let backend_ids: BTreeSet<&str> = self
            .backends
            .iter()
            .map(|backend| backend.id.as_str())
            .collect();

        if !self.agent.backend_id.trim().is_empty() {
            if !backend_ids.contains(self.agent.backend_id.as_str()) {
                anyhow::bail!(
                    "agent.backend_id `{}` is not present in [[backend]] catalog",
                    self.agent.backend_id
                );
            }
        } else if self.agent_roster.is_empty() {
            anyhow::bail!("agent.backend_id is required when no [[agent_roster]] is configured");
        }

        for entry in &self.agent_roster {
            let backend_id = entry.runtime_backend_id();
            if !backend_ids.contains(backend_id) {
                anyhow::bail!(
                    "agent_roster `{}` resolves to backend `{}` which is not present in [[backend]] catalog",
                    entry.name,
                    backend_id
                );
            }
        }

        let provider_ids: BTreeSet<&str> = self
            .provider_profiles
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        for backend in &self.backends {
            if let Some(profile_id) = backend.provider_profile.as_deref() {
                if !provider_ids.contains(profile_id) {
                    anyhow::bail!(
                        "backend `{}` references provider_profile `{}` which is not present in [[provider_profile]]",
                        backend.id,
                        profile_id
                    );
                }
            }
            if let Some(codex) = &backend.codex {
                if matches!(codex.projection, CodexProjectionModeConfig::LocalConfig) {
                    let profile_id = backend.provider_profile.as_deref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "backend `{}` sets codex projection = \"local_config\" but has no provider_profile",
                            backend.id
                        )
                    })?;
                    let profile = self
                        .provider_profiles
                        .iter()
                        .find(|profile| profile.id == profile_id)
                        .ok_or_else(|| {
                            anyhow::anyhow!("unknown provider_profile `{profile_id}`")
                        })?;
                    if !matches!(
                        profile.protocol,
                        ProviderProfileProtocolConfig::OpenaiCompatible { .. }
                    ) {
                        anyhow::bail!(
                            "backend `{}` sets codex projection = \"local_config\" but provider_profile `{}` is not openai_compatible",
                            backend.id,
                            profile_id
                        );
                    }
                }
            }
        }

        let roster_names: BTreeSet<&str> = self
            .agent_roster
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        if !self.bindings.is_empty() && roster_names.is_empty() {
            anyhow::bail!(
                "[[binding]] requires [[agent_roster]] so bindings can resolve to named agents"
            );
        }
        for group in &self.groups {
            if let Some(front_bot) = group.mode.front_bot.as_deref() {
                if !roster_names.contains(front_bot) {
                    anyhow::bail!(
                        "group `{}` references front_bot `{}` which is not present in [[agent_roster]]",
                        group.scope,
                        front_bot
                    );
                }
            }

            for specialist in &group.team.roster {
                if !roster_names.contains(specialist.as_str()) {
                    anyhow::bail!(
                        "group `{}` references team agent `{}` which is not present in [[agent_roster]]",
                        group.scope,
                        specialist
                    );
                }
            }
        }

        let mut seen_team_scope_names = BTreeSet::new();
        for team_scope in &self.team_scopes {
            if !seen_team_scope_names
                .insert((team_scope.mode.channel.clone(), team_scope.scope.clone()))
            {
                anyhow::bail!(
                    "duplicate team_scope `{}` for channel `{}`",
                    team_scope.scope,
                    team_scope.mode.channel.as_deref().unwrap_or("*")
                );
            }
            if !matches!(team_scope.mode.interaction, InteractionMode::Team) {
                anyhow::bail!(
                    "team_scope `{}` must set mode.interaction = \"team\"",
                    team_scope.scope
                );
            }
            if team_scope.mode.auto_promote {
                anyhow::bail!(
                    "team_scope `{}` sets mode.auto_promote = true, but [[team_scope]] entries always create a full team orchestrator and do not support keyword-based auto-promotion; remove auto_promote or use [[group]] instead",
                    team_scope.scope
                );
            }
            if let Some(front_bot) = team_scope.mode.front_bot.as_deref() {
                if !roster_names.contains(front_bot) {
                    anyhow::bail!(
                        "team_scope `{}` references front_bot `{}` which is not present in [[agent_roster]]",
                        team_scope.scope,
                        front_bot
                    );
                }
            }
            for specialist in &team_scope.team.roster {
                if !roster_names.contains(specialist.as_str()) {
                    anyhow::bail!(
                        "team_scope `{}` references team agent `{}` which is not present in [[agent_roster]]",
                        team_scope.scope,
                        specialist
                    );
                }
            }
        }

        for binding in &self.bindings {
            let agent_name = binding.agent_name();
            if !roster_names.contains(agent_name) {
                anyhow::bail!(
                    "binding references agent `{}` which is not present in [[agent_roster]]",
                    agent_name
                );
            }
        }

        Ok(())
    }
}

impl GatewayConfig {
    pub fn resolve_provider_profile(
        &self,
        provider_profile_id: Option<&str>,
    ) -> Result<Option<qai_runtime::ConfiguredProviderProfile>> {
        let Some(id) = provider_profile_id else {
            return Ok(None);
        };
        let profile = self
            .provider_profiles
            .iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| anyhow::anyhow!("unknown provider_profile `{id}`"))?;
        Ok(Some(profile.to_runtime_profile()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentSection {
    #[serde(default)]
    pub backend_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendFamilyConfig {
    Acp,
    OpenClawGateway,
    QuickAiNative,
}

impl BackendFamilyConfig {
    pub fn into_runtime_family(self) -> BackendFamily {
        match self {
            Self::Acp => BackendFamily::Acp,
            Self::OpenClawGateway => BackendFamily::OpenClawGateway,
            Self::QuickAiNative => BackendFamily::QuickAiNative,
        }
    }
}

/// Config-layer alias for the runtime ACP backend identity type.
/// Re-exported from `qai_runtime` so TOML deserialization and runtime use share the same type.
pub type AcpBackendConfig = AcpBackend;
pub type AcpAuthMethodConfig = qai_runtime::AcpAuthMethod;
pub type CodexProjectionModeConfig = qai_runtime::CodexProjectionMode;

/// Config-layer alias for backend approval mode.
pub type ApprovalModeConfig = ApprovalMode;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BackendLaunchConfig {
    Command {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    GatewayWs {
        endpoint: String,
        #[serde(default)]
        token: Option<String>,
        #[serde(default)]
        password: Option<String>,
        #[serde(default)]
        role: Option<String>,
        #[serde(default)]
        scopes: Vec<String>,
        #[serde(default)]
        agent_id: Option<String>,
        #[serde(default)]
        team_helper_command: Option<String>,
        #[serde(default)]
        team_helper_args: Vec<String>,
        #[serde(default)]
        lead_helper_mode: bool,
    },
    Embedded,
}

impl BackendLaunchConfig {
    pub fn into_launch_spec(self) -> LaunchSpec {
        match self {
            Self::Command { command, args, env } => LaunchSpec::Command {
                command,
                args,
                env: env.into_iter().collect(),
            },
            Self::GatewayWs {
                endpoint,
                token,
                password,
                role,
                scopes,
                agent_id,
                team_helper_command,
                team_helper_args,
                lead_helper_mode,
            } => LaunchSpec::GatewayWs {
                endpoint,
                token,
                password,
                role,
                scopes,
                agent_id,
                team_helper_command,
                team_helper_args,
                lead_helper_mode,
            },
            Self::Embedded => LaunchSpec::Embedded,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendCatalogEntry {
    pub id: String,
    pub family: BackendFamilyConfig,
    #[serde(default)]
    pub adapter_key: Option<String>,
    /// Optional ACP backend identity. Only valid when `family = "acp"`.
    /// When omitted, the backend is treated as a generic ACP CLI backend.
    #[serde(default)]
    pub acp_backend: Option<AcpBackend>,
    /// Optional ACP auth-method identity. Only valid for selected bridge-backed ACP backends.
    #[serde(default)]
    pub acp_auth_method: Option<AcpAuthMethodConfig>,
    /// Optional Codex-specific projection mode. Only valid when `family = "acp"` and `acp_backend = "codex"`.
    #[serde(default)]
    pub codex: Option<BackendCodexConfig>,
    /// Optional provider profile binding. Resolved against the top-level `[[provider_profile]]` registry.
    #[serde(default)]
    pub provider_profile: Option<String>,
    #[serde(default)]
    pub approval: BackendApprovalConfig,
    #[serde(default)]
    pub external_mcp_servers: Vec<ExternalMcpServerConfig>,
    pub launch: BackendLaunchConfig,
}

impl BackendCatalogEntry {
    pub fn adapter_key(&self) -> &str {
        self.adapter_key.as_deref().unwrap_or(match self.family {
            BackendFamilyConfig::Acp => "acp",
            BackendFamilyConfig::OpenClawGateway => "openclaw",
            BackendFamilyConfig::QuickAiNative => "native",
        })
    }

    pub fn to_backend_spec(
        &self,
        provider_profile: Option<qai_runtime::ConfiguredProviderProfile>,
    ) -> BackendSpec {
        BackendSpec {
            backend_id: self.id.clone(),
            family: self.family.clone().into_runtime_family(),
            adapter_key: self.adapter_key().to_string(),
            launch: self.launch.clone().into_launch_spec(),
            approval_mode: self.approval.mode,
            external_mcp_servers: self
                .external_mcp_servers
                .iter()
                .cloned()
                .map(ExternalMcpServerConfig::into_runtime_spec)
                .collect(),
            provider_profile,
            acp_backend: self.acp_backend,
            acp_auth_method: self.acp_auth_method,
            codex_projection: self.codex.as_ref().map(|cfg| cfg.projection),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendCodexConfig {
    pub projection: CodexProjectionModeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct BackendApprovalConfig {
    #[serde(default)]
    pub mode: ApprovalModeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderProfileConfig {
    pub id: String,
    #[serde(flatten)]
    pub protocol: ProviderProfileProtocolConfig,
}

impl ProviderProfileConfig {
    fn validate(&self) -> Result<()> {
        match &self.protocol {
            ProviderProfileProtocolConfig::OfficialSession => Ok(()),
            ProviderProfileProtocolConfig::AnthropicCompatible {
                base_url,
                auth_token_env,
                default_model,
                ..
            }
            | ProviderProfileProtocolConfig::OpenaiCompatible {
                base_url,
                auth_token_env,
                default_model,
            } => {
                if base_url.trim().is_empty() {
                    anyhow::bail!("provider_profile `{}` requires non-empty base_url", self.id);
                }
                if auth_token_env.trim().is_empty() {
                    anyhow::bail!(
                        "provider_profile `{}` requires non-empty auth_token_env",
                        self.id
                    );
                }
                if default_model.trim().is_empty() {
                    anyhow::bail!(
                        "provider_profile `{}` requires non-empty default_model",
                        self.id
                    );
                }
                Ok(())
            }
        }
    }

    fn to_runtime_profile(&self) -> qai_runtime::ConfiguredProviderProfile {
        qai_runtime::ConfiguredProviderProfile {
            id: self.id.clone(),
            protocol: match &self.protocol {
                ProviderProfileProtocolConfig::OfficialSession => {
                    qai_runtime::ConfiguredProviderProtocol::OfficialSession
                }
                ProviderProfileProtocolConfig::AnthropicCompatible {
                    base_url,
                    auth_token_env,
                    default_model,
                    small_fast_model,
                } => qai_runtime::ConfiguredProviderProtocol::AnthropicCompatible {
                    base_url: base_url.clone(),
                    auth_token_env: auth_token_env.clone(),
                    default_model: default_model.clone(),
                    small_fast_model: small_fast_model.clone(),
                },
                ProviderProfileProtocolConfig::OpenaiCompatible {
                    base_url,
                    auth_token_env,
                    default_model,
                } => qai_runtime::ConfiguredProviderProtocol::OpenaiCompatible {
                    base_url: base_url.clone(),
                    auth_token_env: auth_token_env.clone(),
                    default_model: default_model.clone(),
                },
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum ProviderProfileProtocolConfig {
    OfficialSession,
    AnthropicCompatible {
        base_url: String,
        auth_token_env: String,
        default_model: String,
        #[serde(default)]
        small_fast_model: Option<String>,
    },
    OpenaiCompatible {
        base_url: String,
        auth_token_env: String,
        default_model: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalMcpServerConfig {
    pub name: String,
    pub url: String,
}

impl ExternalMcpServerConfig {
    fn into_runtime_spec(self) -> qai_runtime::ExternalMcpServerSpec {
        qai_runtime::ExternalMcpServerSpec {
            name: self.name,
            transport: qai_runtime::ExternalMcpTransport::Sse { url: self.url },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChannelsSection {
    #[serde(default)]
    pub dingtalk: Option<DingTalkSection>,
    #[serde(default)]
    pub lark: Option<LarkSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DingTalkSection {
    pub enabled: bool,
    #[serde(default)]
    pub presentation: ProgressPresentationMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProgressPresentationMode {
    #[default]
    FinalOnly,
    ProgressCompact,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LarkSection {
    pub enabled: bool,
    #[serde(default)]
    pub presentation: ProgressPresentationMode,
    #[serde(default)]
    pub trigger_policy: Option<LarkTriggerPolicyConfig>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelTriggerModeConfig {
    #[default]
    AllMessages,
    MentionOnly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ScopeTriggerPolicyConfig {
    #[serde(default)]
    pub mode: ChannelTriggerModeConfig,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LarkTriggerPolicyConfig {
    #[serde(default)]
    pub group: ScopeTriggerPolicyConfig,
    #[serde(default)]
    pub dm: ScopeTriggerPolicyConfig,
}

impl LarkSection {
    pub fn resolved_trigger_policy(
        &self,
        gateway: &GatewaySection,
    ) -> qai_channels::LarkTriggerPolicy {
        let fallback = qai_channels::LarkTriggerPolicy::from_require_mention_in_groups(
            gateway.require_mention_in_groups,
        );
        let Some(policy) = self.trigger_policy else {
            return fallback;
        };

        qai_channels::LarkTriggerPolicy {
            group: match policy.group.mode {
                ChannelTriggerModeConfig::AllMessages => qai_channels::LarkTriggerMode::AllMessages,
                ChannelTriggerModeConfig::MentionOnly => qai_channels::LarkTriggerMode::MentionOnly,
            },
            dm: match policy.dm.mode {
                ChannelTriggerModeConfig::AllMessages => qai_channels::LarkTriggerMode::AllMessages,
                ChannelTriggerModeConfig::MentionOnly => qai_channels::LarkTriggerMode::MentionOnly,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsSection {
    /// 主 skills 目录（通过 `quickai-skill add` 安装的技能）
    pub dir: PathBuf,
    /// 全局附加 skills 目录（所有 agent 默认注入，含 skill-finder）
    /// 在 config.toml 中配置：global_dirs = ["~/.quickai/skills"]
    #[serde(default)]
    pub global_dirs: Vec<PathBuf>,
}

impl Default for SkillsSection {
    fn default() -> Self {
        let dir = dirs::home_dir()
            .unwrap_or_default()
            .join(".quickai")
            .join("skills");
        Self {
            dir,
            global_dirs: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSection {
    pub dir: PathBuf,
}

impl Default for SessionSection {
    fn default() -> Self {
        let dir = dirs::home_dir()
            .unwrap_or_default()
            .join(".quickai")
            .join("sessions");
        Self { dir }
    }
}

// ─── Group config ─────────────────────────────────────────────────────────────

/// 群组交互模式（Solo / Relay / Team）
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InteractionMode {
    /// 每个 @mention 独立响应，互不知晓
    #[default]
    Solo,
    /// Lead 同步透明委托给 Specialist（[RELAY:] 标记）
    Relay,
    /// Lead 规划任务，Specialist 异步并行执行（Team Mode）
    Team,
}

/// 群组级别的模式配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GroupModeConfig {
    /// 基础交互模式
    #[serde(default)]
    pub interaction: InteractionMode,
    /// true = 根据关键词自动升级模式（Solo→Relay→Team）
    #[serde(default)]
    pub auto_promote: bool,
    /// 负责接收用户消息并协调其他 Specialist 的 Lead agent 名称
    #[serde(default)]
    pub front_bot: Option<String>,
    /// IM channel name for lead_session_key (e.g. "lark", "dingtalk", "ws").
    /// When set, overrides the auto-detected channel from enabled channels config.
    #[serde(default)]
    pub channel: Option<String>,
    // TODO(consent_required): When true, Lead must call request_confirmation() and the user
    // must reply with a confirmation keyword before activate() proceeds. The registry.rs
    // AwaitingConfirm state machine already supports this flow; TeamOrchestrator.activate()
    // needs a consent_required flag, and main.rs needs to wire it from config.
    // Removed field to prevent silent misconfiguration (field parsed but never read).
    // pub consent_required: bool,
}

/// 群组级别的 Team Mode 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupTeamConfig {
    /// Team 中参与的 agent 名称列表
    #[serde(default)]
    pub roster: Vec<String>,
    /// 里程碑广播详细程度（minimal / normal / verbose）
    #[serde(default = "default_public_updates")]
    pub public_updates: TeamPublicUpdatesMode,
    /// 最大并行任务数
    #[serde(default = "default_max_parallel")]
    pub max_parallel: usize,
}

impl Default for GroupTeamConfig {
    fn default() -> Self {
        Self {
            roster: vec![],
            public_updates: default_public_updates(),
            max_parallel: default_max_parallel(),
        }
    }
}

fn default_public_updates() -> TeamPublicUpdatesMode {
    TeamPublicUpdatesMode::Minimal
}
fn default_max_parallel() -> usize {
    3
}

/// 单个群组的完整配置（对应 `[[group]]` 段）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupConfig {
    /// 匹配模式，如 `"group:lark:{chat_id}"` 或精确 scope
    pub scope: String,
    /// 可读名称（可选）
    #[serde(default)]
    pub name: Option<String>,
    /// 交互模式配置
    #[serde(default)]
    pub mode: GroupModeConfig,
    /// Team Mode 配置
    #[serde(default)]
    pub team: GroupTeamConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamScopeConfig {
    pub scope: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub mode: GroupModeConfig,
    #[serde(default)]
    pub team: GroupTeamConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamScopeSource {
    LegacyGroup,
    ExactScope,
}

#[derive(Debug, Clone)]
pub struct TeamScopeSpec {
    pub scope: String,
    pub name: Option<String>,
    pub mode: GroupModeConfig,
    pub team: GroupTeamConfig,
    pub source: TeamScopeSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BindingPeerKindConfig {
    User,
    Group,
}

impl BindingPeerKindConfig {
    fn into_agent_kind(self) -> BindingPeerKind {
        match self {
            Self::User => BindingPeerKind::User,
            Self::Group => BindingPeerKind::Group,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BindingConfig {
    Thread {
        agent: String,
        scope: String,
        thread_id: String,
        #[serde(default)]
        channel: Option<String>,
    },
    Scope {
        agent: String,
        scope: String,
        #[serde(default)]
        channel: Option<String>,
    },
    Peer {
        agent: String,
        peer_kind: BindingPeerKindConfig,
        peer_id: String,
        #[serde(default)]
        channel: Option<String>,
    },
    Team {
        agent: String,
        team_id: String,
    },
    Channel {
        agent: String,
        channel: String,
    },
    Default {
        agent: String,
    },
}

impl BindingConfig {
    pub fn agent_name(&self) -> &str {
        match self {
            Self::Thread { agent, .. }
            | Self::Scope { agent, .. }
            | Self::Peer { agent, .. }
            | Self::Team { agent, .. }
            | Self::Channel { agent, .. }
            | Self::Default { agent } => agent,
        }
    }

    pub fn to_binding_rule(&self) -> BindingRule {
        match self {
            Self::Thread {
                agent,
                scope,
                thread_id,
                channel,
            } => BindingRule::Thread {
                channel: channel.clone(),
                scope: scope.clone(),
                thread_id: thread_id.clone(),
                agent_name: agent.clone(),
            },
            Self::Scope {
                agent,
                scope,
                channel,
            } => BindingRule::Scope {
                channel: channel.clone(),
                scope: scope.clone(),
                agent_name: agent.clone(),
            },
            Self::Peer {
                agent,
                peer_kind,
                peer_id,
                channel,
            } => BindingRule::Peer {
                channel: channel.clone(),
                kind: peer_kind.clone().into_agent_kind(),
                id: peer_id.clone(),
                agent_name: agent.clone(),
            },
            Self::Team { agent, team_id } => BindingRule::Team {
                team_id: team_id.clone(),
                agent_name: agent.clone(),
            },
            Self::Channel { agent, channel } => BindingRule::Channel {
                channel: channel.clone(),
                agent_name: agent.clone(),
            },
            Self::Default { agent } => BindingRule::Default {
                agent_name: agent.clone(),
            },
        }
    }
}

impl GatewayConfig {
    /// 从 ~/.quickai/config.toml 加载，不存在则用默认值
    pub fn load() -> Result<Self> {
        let path = dirs::home_dir()
            .unwrap_or_default()
            .join(".quickai")
            .join("config.toml");

        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)?;
        Ok(toml::from_str(&content)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gateway_config_agent_roster_deserializes() {
        // agent_roster uses user-defined @mention names (not hardcoded "claude"/"codex")
        let toml_str = r#"
[gateway]
host = "127.0.0.1"
port = 8080

[[agent_roster]]
name = "mybot"
mentions = ["@mybot", "@dev"]
backend_id = "mybot-main"

[[agent_roster]]
name = "reviewer"
mentions = ["@reviewer"]
backend_id = "reviewer-main"
"#;
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.agent_roster.len(), 2);
        assert_eq!(cfg.agent_roster[0].name, "mybot");
        assert_eq!(cfg.agent_roster[0].mentions, vec!["@mybot", "@dev"]);
        assert_eq!(cfg.agent_roster[0].backend_id, "mybot-main");
        assert_eq!(cfg.agent_roster[1].name, "reviewer");
    }

    #[test]
    fn test_gateway_config_empty_roster_is_default() {
        let toml_str = "[gateway]\nhost = \"127.0.0.1\"\nport = 0";
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.agent_roster.is_empty());
        assert!(cfg.backends.is_empty());
    }

    #[test]
    fn test_backend_catalog_acp_deserializes() {
        let toml = r#"
[[backend]]
id = "codex-main"
family = "acp"
adapter_key = "acp"

[backend.launch]
type = "command"
command = "codex-acp"
args = ["--stdio"]
        "#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.backends.len(), 1);
        let spec = cfg.backends[0].to_backend_spec(None);
        assert_eq!(spec.backend_id, "codex-main");
        assert_eq!(spec.family, BackendFamily::Acp);
        assert_eq!(spec.adapter_key, "acp");
        assert!(matches!(spec.launch, LaunchSpec::Command { .. }));
    }

    #[test]
    fn test_backend_catalog_openclaw_deserializes() {
        let toml = r#"
[[backend]]
id = "openclaw-main"
family = "open_claw_gateway"

[backend.launch]
type = "gateway_ws"
endpoint = "ws://127.0.0.1:18789"
token = "test-token"
scopes = ["operator.admin"]
        "#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        let spec = cfg.backends[0].to_backend_spec(None);
        assert_eq!(spec.family, BackendFamily::OpenClawGateway);
        assert_eq!(spec.adapter_key, "openclaw");
        match spec.launch {
            LaunchSpec::GatewayWs {
                endpoint,
                token,
                password,
                role,
                scopes,
                agent_id,
                team_helper_command,
                team_helper_args,
                lead_helper_mode,
            } => {
                assert_eq!(endpoint, "ws://127.0.0.1:18789");
                assert_eq!(token.as_deref(), Some("test-token"));
                assert!(password.is_none());
                assert!(role.is_none());
                assert_eq!(scopes, vec!["operator.admin"]);
                assert!(agent_id.is_none());
                assert!(team_helper_command.is_none());
                assert!(team_helper_args.is_empty());
                assert!(!lead_helper_mode);
            }
            other => panic!("unexpected launch spec: {other:?}"),
        }
    }

    #[test]
    fn test_backend_catalog_openclaw_lead_helper_mode_deserializes() {
        let toml = r#"
[[backend]]
id = "openclaw-lead"
family = "open_claw_gateway"

[backend.launch]
type = "gateway_ws"
endpoint = "ws://127.0.0.1:18789"
team_helper_command = "/bin/qai_team_cli"
lead_helper_mode = true
        "#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        let spec = cfg.backends[0].to_backend_spec(None);
        match spec.launch {
            LaunchSpec::GatewayWs {
                team_helper_command,
                lead_helper_mode,
                ..
            } => {
                assert_eq!(team_helper_command.as_deref(), Some("/bin/qai_team_cli"));
                assert!(lead_helper_mode);
            }
            other => panic!("unexpected launch spec: {other:?}"),
        }
    }

    #[test]
    fn test_backend_catalog_native_deserializes() {
        let toml = r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"
        "#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        let spec = cfg.backends[0].to_backend_spec(None);
        assert_eq!(spec.family, BackendFamily::QuickAiNative);
        assert_eq!(spec.adapter_key, "native");
        assert!(matches!(spec.launch, LaunchSpec::Embedded));
    }

    #[test]
    fn parse_backend_with_external_mcp_servers() {
        let toml = r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[[backend.external_mcp_servers]]
name = "filesystem"
url = "http://127.0.0.1:3001/sse"
        "#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.backends.len(), 1);
        assert_eq!(cfg.backends[0].external_mcp_servers.len(), 1);
        assert_eq!(cfg.backends[0].external_mcp_servers[0].name, "filesystem");
        let spec = cfg.backends[0].to_backend_spec(None);
        assert_eq!(spec.external_mcp_servers.len(), 1);
        match &spec.external_mcp_servers[0].transport {
            qai_runtime::ExternalMcpTransport::Sse { url } => {
                assert_eq!(url, "http://127.0.0.1:3001/sse");
            }
        }
    }

    #[test]
    fn test_dingtalk_presentation_deserializes() {
        let toml = r#"
[channels.dingtalk]
enabled = true
presentation = "progress_compact"
        "#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.channels.dingtalk.unwrap().presentation,
            ProgressPresentationMode::ProgressCompact
        );
    }

    #[test]
    fn test_agent_backend_id_deserializes() {
        let toml = r#"
[agent]
backend_id = "native-main"
        "#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.agent.backend_id, "native-main");
    }

    #[test]
    fn test_validate_runtime_topology_accepts_backend_catalog_and_roster_binding() {
        let toml = r#"
[agent]
backend_id = "native-main"

[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[[agent_roster]]
name = "reviewer"
mentions = ["@reviewer"]
backend_id = "native-main"
        "#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        cfg.validate_runtime_topology().unwrap();
    }

    #[test]
    fn test_validate_runtime_topology_rejects_missing_backend_binding() {
        let toml = r#"
[agent]
backend_id = "missing-main"

[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"
        "#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        let err = cfg.validate_runtime_topology().unwrap_err();
        assert!(err
            .to_string()
            .contains("agent.backend_id `missing-main` is not present"));
    }

    #[test]
    fn test_validate_runtime_topology_accepts_group_front_bot_and_team_roster() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[agent]
backend_id = "native-main"

[[agent_roster]]
name = "claude"
mentions = ["@claude"]
backend_id = "native-main"

[[agent_roster]]
name = "codex"
mentions = ["@codex"]
backend_id = "native-main"

[[group]]
scope = "group:lark:abc"

[group.mode]
interaction = "team"
front_bot = "claude"

[group.team]
roster = ["codex"]
"#,
        )
        .unwrap();

        cfg.validate_runtime_topology().unwrap();
    }

    #[test]
    fn test_validate_runtime_topology_rejects_unknown_group_front_bot() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[agent]
backend_id = "native-main"

[[agent_roster]]
name = "codex"
mentions = ["@codex"]
backend_id = "native-main"

[[group]]
scope = "group:lark:abc"

[group.mode]
interaction = "team"
front_bot = "claude"
"#,
        )
        .unwrap();

        let err = cfg.validate_runtime_topology().unwrap_err();
        assert!(err.to_string().contains("front_bot `claude`"));
    }

    #[test]
    fn test_validate_runtime_topology_rejects_unknown_group_team_agent() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[agent]
backend_id = "native-main"

[[agent_roster]]
name = "claude"
mentions = ["@claude"]
backend_id = "native-main"

[[group]]
scope = "group:lark:abc"

[group.mode]
interaction = "team"
front_bot = "claude"

[group.team]
roster = ["codex"]
"#,
        )
        .unwrap();

        let err = cfg.validate_runtime_topology().unwrap_err();
        assert!(err.to_string().contains("team agent `codex`"));
    }

    #[test]
    fn test_resolved_default_backend_id_treats_blank_as_none() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[agent]
backend_id = "   "

[[agent_roster]]
name = "claude"
mentions = ["@claude"]
backend_id = "native-main"
"#,
        )
        .unwrap();

        assert_eq!(cfg.resolved_default_backend_id(), None);
    }

    #[test]
    fn test_resolved_default_backend_id_returns_trimmed_value() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[agent]
backend_id = " native-main "
"#,
        )
        .unwrap();

        assert_eq!(
            cfg.resolved_default_backend_id().as_deref(),
            Some("native-main")
        );
    }

    #[test]
    fn test_memory_config_deserializes_with_defaults() {
        let toml_str = "[memory]\ndistill_every_n = 5";
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.memory.distill_every_n, 5);
        assert_eq!(cfg.memory.distiller_binary, "quickai-rust-agent");
        assert_eq!(cfg.memory.shared_memory_max_words, 300);
        assert_eq!(cfg.memory.agent_memory_max_words, 500);
    }

    #[test]
    fn test_gateway_config_includes_memory_section() {
        let toml_str = "[memory]\ndistill_every_n = 10";
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.memory.distill_every_n, 10);
    }

    #[test]
    fn test_cron_jobs_config_deserializes() {
        let toml_str = r#"
[[cron_jobs]]
name = "daily-standup"
expr = "0 9 * * 1-5"
prompt = "站会摘要"
session_key = "dingtalk:group_xxx"
enabled = true

[[cron_jobs]]
name = "weekly-report"
expr = "0 18 * * 5"
prompt = "工作报告"
session_key = "lark:ou_xxx"
"#;
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.cron_jobs.len(), 2);
        assert_eq!(cfg.cron_jobs[0].name, "daily-standup");
        assert_eq!(cfg.cron_jobs[0].expr, "0 9 * * 1-5");
        assert!(cfg.cron_jobs[0].enabled);
        assert_eq!(cfg.cron_jobs[1].name, "weekly-report");
        // enabled defaults to true when omitted
        assert!(cfg.cron_jobs[1].enabled);
    }

    #[test]
    fn test_cron_jobs_empty_by_default() {
        let toml_str = "[gateway]\nhost = \"127.0.0.1\"\nport = 0";
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.cron_jobs.is_empty());
    }

    #[test]
    fn test_cron_job_config_with_agent() {
        let toml = r#"
[[cron_jobs]]
name = "digest"
expr = "0 0 8 * * *"
prompt = "Summarize today"
session_key = "lark:ou_xxx"
agent = "reviewer"
"#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.cron_jobs[0].agent, Some("reviewer".to_string()));
    }

    #[test]
    fn test_cron_job_config_agent_defaults_to_none() {
        let toml = r#"
[[cron_jobs]]
name = "digest"
expr = "0 0 8 * * *"
prompt = "Summarize today"
session_key = "lark:ou_xxx"
"#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.cron_jobs[0].agent, None);
    }

    #[test]
    fn test_cron_job_config_condition_deserializes() {
        let toml = r#"
[[cron_jobs]]
name = "heartbeat"
expr = "0 */30 * * * *"
prompt = "Check in with the user"
session_key = "lark:ou_xxx"
condition = "idle_gt_seconds = 3600"
"#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.cron_jobs[0].condition,
            Some("idle_gt_seconds = 3600".to_string())
        );
    }

    #[test]
    fn test_cron_job_config_condition_defaults_to_none() {
        let toml = r#"
[[cron_jobs]]
name = "ping"
expr = "0 * * * * *"
prompt = "Hello"
session_key = "lark:ou_xxx"
"#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.cron_jobs[0].condition, None);
    }

    #[test]
    fn test_gateway_require_mention_in_groups_defaults_to_false() {
        let toml_str = "[gateway]\nhost = \"127.0.0.1\"\nport = 8080";
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        assert!(!cfg.gateway.require_mention_in_groups);
    }

    #[test]
    fn test_gateway_require_mention_in_groups_can_be_set_true() {
        let toml_str =
            "[gateway]\nhost = \"127.0.0.1\"\nport = 8080\nrequire_mention_in_groups = true";
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.gateway.require_mention_in_groups);
    }

    #[test]
    fn test_gateway_default_workspace_deserialises() {
        let toml = r#"
[gateway]
host = "127.0.0.1"
port = 8080
default_workspace = "/home/user/workspace"
    "#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.gateway.default_workspace,
            Some(std::path::PathBuf::from("/home/user/workspace"))
        );
    }

    #[test]
    fn test_gateway_default_workspace_defaults_to_none() {
        let cfg = GatewayConfig::default();
        assert!(cfg.gateway.default_workspace.is_none());
    }

    #[test]
    fn lark_trigger_policy_defaults_are_group_compatible_and_dm_open() {
        let toml = r#"
[gateway]
host = "127.0.0.1"
port = 8080
require_mention_in_groups = true

[channels.lark]
enabled = true
"#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        let policy = cfg
            .channels
            .lark
            .as_ref()
            .unwrap()
            .resolved_trigger_policy(&cfg.gateway);
        assert_eq!(policy.group, qai_channels::LarkTriggerMode::MentionOnly);
        assert_eq!(policy.dm, qai_channels::LarkTriggerMode::AllMessages);
    }

    #[test]
    fn lark_trigger_policy_parses_group_and_dm_modes() {
        let toml = r#"
[gateway]
host = "127.0.0.1"
port = 8080

[channels.lark]
enabled = true

[channels.lark.trigger_policy.group]
mode = "mention_only"

[channels.lark.trigger_policy.dm]
mode = "all_messages"
"#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        let policy = cfg
            .channels
            .lark
            .as_ref()
            .unwrap()
            .resolved_trigger_policy(&cfg.gateway);
        assert_eq!(policy.group, qai_channels::LarkTriggerMode::MentionOnly);
        assert_eq!(policy.dm, qai_channels::LarkTriggerMode::AllMessages);
    }

    #[test]
    fn test_group_config_deserializes() {
        let toml_str = r#"
[[group]]
scope = "group:lark:abc123"
name = "后端研发群"

[group.mode]
interaction = "relay"
auto_promote = true
front_bot = "claude"

[group.team]
roster = ["codex", "researcher"]
public_updates = "verbose"
max_parallel = 5
"#;
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.groups.len(), 1);
        let g = &cfg.groups[0];
        assert_eq!(g.scope, "group:lark:abc123");
        assert_eq!(g.name.as_deref(), Some("后端研发群"));
        assert_eq!(g.mode.interaction, InteractionMode::Relay);
        assert!(g.mode.auto_promote);
        assert_eq!(g.mode.front_bot.as_deref(), Some("claude"));
        assert_eq!(g.team.roster, vec!["codex", "researcher"]);
        assert_eq!(g.team.public_updates, TeamPublicUpdatesMode::Verbose);
        assert_eq!(g.team.max_parallel, 5);
    }

    #[test]
    fn test_group_config_defaults() {
        let toml_str = r#"
[[group]]
scope = "group:lark:xyz"
"#;
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        let g = &cfg.groups[0];
        assert_eq!(g.mode.interaction, InteractionMode::Solo);
        assert!(!g.mode.auto_promote);
        assert!(g.mode.front_bot.is_none());
        assert!(g.team.roster.is_empty());
        assert_eq!(g.team.public_updates, TeamPublicUpdatesMode::Minimal);
        assert_eq!(g.team.max_parallel, 3);
    }

    #[test]
    fn test_groups_empty_by_default() {
        let cfg = GatewayConfig::default();
        assert!(cfg.groups.is_empty());
    }

    #[test]
    fn test_bindings_empty_by_default() {
        let cfg = GatewayConfig::default();
        assert!(cfg.bindings.is_empty());
    }

    #[test]
    fn test_group_mode_channel_field_deserializes() {
        let toml_str = r#"
[[group]]
scope = "group:lark:abc"

[group.mode]
interaction = "team"
front_bot = "claude"
channel = "lark"
"#;
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        let g = &cfg.groups[0];
        assert_eq!(g.mode.channel.as_deref(), Some("lark"));
    }

    #[test]
    fn test_group_mode_channel_defaults_to_none() {
        let toml_str = "[[group]]\nscope = \"group:lark:xyz\"";
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.groups[0].mode.channel.is_none());
    }

    #[test]
    fn test_team_scope_config_deserializes() {
        let toml_str = r#"
[[team_scope]]
scope = "user:ou_123"
name = "私聊工作台"

[team_scope.mode]
interaction = "team"
front_bot = "claude"
channel = "lark"

[team_scope.team]
roster = ["codex", "researcher"]
public_updates = "minimal"
max_parallel = 2
"#;
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.team_scopes.len(), 1);
        let team_scope = &cfg.team_scopes[0];
        assert_eq!(team_scope.scope, "user:ou_123");
        assert_eq!(team_scope.name.as_deref(), Some("私聊工作台"));
        assert_eq!(team_scope.mode.interaction, InteractionMode::Team);
        assert_eq!(team_scope.mode.front_bot.as_deref(), Some("claude"));
        assert_eq!(team_scope.mode.channel.as_deref(), Some("lark"));
        assert_eq!(team_scope.team.roster, vec!["codex", "researcher"]);
        assert_eq!(
            team_scope.team.public_updates,
            TeamPublicUpdatesMode::Minimal
        );
        assert_eq!(team_scope.team.max_parallel, 2);
    }

    #[test]
    fn test_validate_runtime_topology_accepts_team_scope_front_bot_and_roster() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[agent]
backend_id = "native-main"

[[agent_roster]]
name = "claude"
mentions = ["@claude"]
backend_id = "native-main"

[[agent_roster]]
name = "codex"
mentions = ["@codex"]
backend_id = "native-main"

[[team_scope]]
scope = "user:ou_abc"

[team_scope.mode]
interaction = "team"
front_bot = "claude"

[team_scope.team]
roster = ["codex"]
"#,
        )
        .unwrap();

        cfg.validate_runtime_topology().unwrap();
    }

    #[test]
    fn test_validate_runtime_topology_rejects_team_scope_non_team_interaction() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[agent]
backend_id = "native-main"

[[agent_roster]]
name = "claude"
mentions = ["@claude"]
backend_id = "native-main"

[[team_scope]]
scope = "user:ou_abc"

[team_scope.mode]
interaction = "solo"
front_bot = "claude"
"#,
        )
        .unwrap();

        let err = cfg.validate_runtime_topology().unwrap_err();
        assert!(err
            .to_string()
            .contains("team_scope `user:ou_abc` must set mode.interaction = \"team\""));
    }

    #[test]
    fn test_validate_runtime_topology_rejects_team_scope_with_auto_promote() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[agent]
backend_id = "native-main"

[[agent_roster]]
name = "claude"
mentions = ["@claude"]
backend_id = "native-main"

[[team_scope]]
scope = "user:ou_abc"

[team_scope.mode]
interaction = "team"
auto_promote = true
front_bot = "claude"
"#,
        )
        .unwrap();

        let err = cfg.validate_runtime_topology().unwrap_err();
        assert!(
            err.to_string().contains("auto_promote"),
            "expected auto_promote rejection, got: {err}"
        );
        assert!(
            err.to_string().contains("user:ou_abc"),
            "expected scope in error, got: {err}"
        );
    }

    #[test]
    fn test_validate_runtime_topology_rejects_duplicate_team_scope() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[agent]
backend_id = "native-main"

[[agent_roster]]
name = "claude"
mentions = ["@claude"]
backend_id = "native-main"

[[team_scope]]
scope = "user:ou_dup"

[team_scope.mode]
interaction = "team"
front_bot = "claude"

[[team_scope]]
scope = "user:ou_dup"

[team_scope.mode]
interaction = "team"
front_bot = "claude"
"#,
        )
        .unwrap();

        let err = cfg.validate_runtime_topology().unwrap_err();
        assert!(err
            .to_string()
            .contains("duplicate team_scope `user:ou_dup` for channel `*`"));
    }

    #[test]
    fn test_validate_runtime_topology_allows_same_team_scope_on_distinct_channels() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[agent]
backend_id = "native-main"

[[agent_roster]]
name = "claude"
mentions = ["@claude"]
backend_id = "native-main"

[[team_scope]]
scope = "user:ou_dup"

[team_scope.mode]
interaction = "team"
front_bot = "claude"
channel = "lark"

[[team_scope]]
scope = "user:ou_dup"

[team_scope.mode]
interaction = "team"
front_bot = "claude"
channel = "dingtalk"
"#,
        )
        .unwrap();

        cfg.validate_runtime_topology().unwrap();
    }

    #[test]
    fn test_normalized_team_scopes_explicit_team_scope_overrides_group_team_scope() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[agent]
backend_id = "native-main"

[[group]]
scope = "user:ou_same"
name = "legacy"

[group.mode]
interaction = "team"
front_bot = "legacy-front"

[group.team]
roster = ["legacy-worker"]

[[team_scope]]
scope = "user:ou_same"
name = "exact"

[team_scope.mode]
interaction = "team"
front_bot = "exact-front"

[team_scope.team]
roster = ["exact-worker"]
max_parallel = 7
"#,
        )
        .unwrap();

        let normalized = cfg.normalized_team_scopes();
        assert_eq!(normalized.len(), 1);
        let team_scope = &normalized[0];
        assert_eq!(team_scope.scope, "user:ou_same");
        assert_eq!(team_scope.name.as_deref(), Some("exact"));
        assert_eq!(team_scope.mode.front_bot.as_deref(), Some("exact-front"));
        assert_eq!(team_scope.team.roster, vec!["exact-worker"]);
        assert_eq!(team_scope.team.max_parallel, 7);
        assert!(matches!(team_scope.source, TeamScopeSource::ExactScope));
    }

    #[test]
    fn test_normalized_team_scopes_keeps_same_scope_on_distinct_channels() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[agent]
backend_id = "native-main"

[[team_scope]]
scope = "user:ou_same"
name = "lark-team"

[team_scope.mode]
interaction = "team"
front_bot = "lark-front"
channel = "lark"

[[team_scope]]
scope = "user:ou_same"
name = "ding-team"

[team_scope.mode]
interaction = "team"
front_bot = "ding-front"
channel = "dingtalk"
"#,
        )
        .unwrap();

        let normalized = cfg.normalized_team_scopes();
        assert_eq!(normalized.len(), 2);
        assert!(normalized.iter().any(|team_scope| {
            team_scope.scope == "user:ou_same"
                && team_scope.mode.channel.as_deref() == Some("lark")
                && team_scope.name.as_deref() == Some("lark-team")
        }));
        assert!(normalized.iter().any(|team_scope| {
            team_scope.scope == "user:ou_same"
                && team_scope.mode.channel.as_deref() == Some("dingtalk")
                && team_scope.name.as_deref() == Some("ding-team")
        }));
    }

    #[test]
    fn test_binding_config_deserializes_multiple_kinds() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[[agent_roster]]
name = "claude"
mentions = ["@claude"]
backend_id = "native-main"

[[agent_roster]]
name = "codex"
mentions = ["@codex"]
backend_id = "native-main"

[[binding]]
kind = "thread"
agent = "claude"
channel = "dingtalk"
scope = "group:cid_1"
thread_id = "webhook-1"

[[binding]]
kind = "peer"
agent = "codex"
channel = "lark"
peer_kind = "user"
peer_id = "ou_123"

[[binding]]
kind = "default"
agent = "claude"
"#,
        )
        .unwrap();

        assert_eq!(cfg.bindings.len(), 3);
        assert!(matches!(cfg.bindings[0], BindingConfig::Thread { .. }));
        assert!(matches!(cfg.bindings[1], BindingConfig::Peer { .. }));
        assert!(matches!(cfg.bindings[2], BindingConfig::Default { .. }));
    }

    #[test]
    fn test_validate_runtime_topology_rejects_unknown_binding_agent() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[[agent_roster]]
name = "claude"
mentions = ["@claude"]
backend_id = "native-main"

[[binding]]
kind = "channel"
agent = "codex"
channel = "lark"
"#,
        )
        .unwrap();

        let err = cfg.validate_runtime_topology().unwrap_err();
        assert!(err.to_string().contains("binding references agent `codex`"));
    }

    #[test]
    fn test_validate_runtime_topology_rejects_bindings_without_roster() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[agent]
backend_id = "native-main"

[[binding]]
kind = "default"
agent = "claude"
"#,
        )
        .unwrap();

        let err = cfg.validate_runtime_topology().unwrap_err();
        assert!(err
            .to_string()
            .contains("[[binding]] requires [[agent_roster]]"));
    }

    #[test]
    fn test_validate_runtime_topology_rejects_empty_external_mcp_name() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[[backend.external_mcp_servers]]
name = "   "
url = "http://127.0.0.1:3001/sse"

[agent]
backend_id = "native-main"
"#,
        )
        .unwrap();

        let err = cfg.validate_runtime_topology().unwrap_err();
        assert!(err
            .to_string()
            .contains("contains an external MCP server with an empty name"));
    }

    #[test]
    fn test_validate_runtime_topology_rejects_duplicate_external_mcp_names() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[[backend.external_mcp_servers]]
name = "filesystem"
url = "http://127.0.0.1:3001/sse"

[[backend.external_mcp_servers]]
name = "filesystem"
url = "http://127.0.0.1:3002/sse"

[agent]
backend_id = "native-main"
"#,
        )
        .unwrap();

        let err = cfg.validate_runtime_topology().unwrap_err();
        assert!(err
            .to_string()
            .contains("contains duplicate external MCP server name `filesystem`"));
    }

    #[test]
    fn test_validate_runtime_topology_rejects_reserved_external_mcp_name() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "embedded"

[[backend.external_mcp_servers]]
name = "team-tools"
url = "http://127.0.0.1:3001/sse"

[agent]
backend_id = "native-main"
"#,
        )
        .unwrap();

        let err = cfg.validate_runtime_topology().unwrap_err();
        assert!(err
            .to_string()
            .contains("uses reserved external MCP server name `team-tools`"));
    }

    #[test]
    fn acp_backend_accepts_claude() {
        let toml = r#"
[[backend]]
id = "claude-main"
family = "acp"
acp_backend = "claude"

[backend.launch]
type = "command"
command = "npx"
args = ["@zed-industries/claude-agent-acp"]

[agent]
backend_id = "claude-main"
        "#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.backends[0].acp_backend, Some(AcpBackendConfig::Claude));
        cfg.validate_runtime_topology().unwrap();
    }

    #[test]
    fn acp_backend_accepts_qwen() {
        let toml = r#"
[[backend]]
id = "qwen-main"
family = "acp"
acp_backend = "qwen"

[backend.launch]
type = "command"
command = "npx"
args = ["@qwen-code/qwen-code", "--acp"]

[agent]
backend_id = "qwen-main"
        "#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.backends[0].acp_backend, Some(AcpBackendConfig::Qwen));
        cfg.validate_runtime_topology().unwrap();
    }

    #[test]
    fn acp_backend_may_be_omitted() {
        let toml = r#"
[[backend]]
id = "generic-acp"
family = "acp"

[backend.launch]
type = "command"
command = "some-acp-tool"
args = ["--acp"]

[agent]
backend_id = "generic-acp"
        "#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.backends[0].acp_backend, None);
        cfg.validate_runtime_topology().unwrap();
    }

    #[test]
    fn non_acp_backend_rejects_acp_backend_field() {
        let toml = r#"
[[backend]]
id = "native-main"
family = "quick_ai_native"
acp_backend = "claude"

[backend.launch]
type = "embedded"

[agent]
backend_id = "native-main"
        "#;
        let cfg: GatewayConfig = toml::from_str(toml).unwrap();
        let err = cfg.validate_runtime_topology().unwrap_err();
        assert!(err.to_string().contains("acp_backend"));
        assert!(err.to_string().contains("native-main"));
    }

    #[test]
    fn acp_backend_rejects_unknown_value() {
        let toml = r#"
[[backend]]
id = "gemini-main"
family = "acp"
acp_backend = "gemini"

[backend.launch]
type = "command"
command = "gemini"
args = ["--acp"]
        "#;
        let result: Result<GatewayConfig, _> = toml::from_str(toml);
        assert!(
            result.is_err(),
            "unknown acp_backend value should be rejected at parse time"
        );
    }
}
