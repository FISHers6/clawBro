use anyhow::Result;
use qai_agent::roster::AgentEntry;
use qai_agent::selector::EngineConfig;
use serde::{Deserialize, Serialize};
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
    #[serde(default)]
    pub memory: MemorySection,
    #[serde(default)]
    pub cron_jobs: Vec<CronJobConfig>,
    /// 群组专项配置列表（`[[group]]` 段，可配置交互模式和 Team Mode 参数）
    #[serde(default, rename = "group")]
    pub groups: Vec<GroupConfig>,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentSection {
    pub engine: EngineConfig,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LarkSection {
    pub enabled: bool,
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
    pub public_updates: String,
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

fn default_public_updates() -> String {
    "minimal".to_string()
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

[agent_roster.engine]
type = "rust_agent"

[[agent_roster]]
name = "reviewer"
mentions = ["@reviewer"]

[agent_roster.engine]
type = "codex_acp"
"#;
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.agent_roster.len(), 2);
        assert_eq!(cfg.agent_roster[0].name, "mybot");
        assert_eq!(cfg.agent_roster[0].mentions, vec!["@mybot", "@dev"]);
        assert_eq!(cfg.agent_roster[1].name, "reviewer");
    }

    #[test]
    fn test_gateway_config_empty_roster_is_default() {
        let toml_str = "[gateway]\nhost = \"127.0.0.1\"\nport = 0";
        let cfg: GatewayConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.agent_roster.is_empty());
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
        assert_eq!(g.team.public_updates, "verbose");
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
        assert_eq!(g.team.public_updates, "minimal");
        assert_eq!(g.team.max_parallel, 3);
    }

    #[test]
    fn test_groups_empty_by_default() {
        let cfg = GatewayConfig::default();
        assert!(cfg.groups.is_empty());
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
}
