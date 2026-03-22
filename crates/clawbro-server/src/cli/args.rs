use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "clawbro",
    about = "ClawBro — AI Agent 配置与运行",
    version,
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 交互式初始化向导（首次使用请运行此命令）
    Setup(SetupArgs),
    /// API Key 管理（set / list / check）
    Auth(AuthArgs),
    /// 配置文件管理（show / validate / edit）
    Config(ConfigArgs),
    /// 启动 Gateway 服务
    Serve(ServeArgs),
    /// OpenClaw 团队工具内部辅助命令
    #[command(hide = true)]
    TeamHelper(TeamHelperArgs),
    /// 内部 native runtime bridge
    #[command(hide = true)]
    RuntimeBridge,
    /// 内部 ACP agent server
    #[command(hide = true)]
    AcpAgent,
    /// 诊断配置和运行环境
    Doctor,
    /// 显示当前配置摘要
    Status,
    /// 管理本地 runtime scheduler
    Schedule(ScheduleArgs),
    /// 管理本地 skills 目录
    Skill(SkillArgs),
    /// WeChat QR 码扫码登录
    WechatLogin,
    /// 生成 Shell 补全脚本
    Completions(CompletionsArgs),
}

#[derive(clap::Args, Debug)]
pub struct SkillArgs {
    #[command(subcommand)]
    pub command: SkillCommands,
}

#[derive(Subcommand, Debug)]
pub enum SkillCommands {
    /// 安装一个 skill 到指定 scope（优先委托 skills 生态）
    Add(SkillAddArgs),
    /// 检查默认 baseline skills 和 backend mirrors 状态
    Check(SkillCheckArgs),
    /// 委托 ClawHub 管理 workspace ./skills
    Hub(SkillHubArgs),
    /// 列出当前 skills 目录
    List(SkillListArgs),
    /// 从指定 scope 删除一个 skill
    Remove(SkillRemoveArgs),
    /// 同步默认 baseline skills 和 backend mirrors
    Sync(SkillSyncArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum SkillScopeArg {
    Managed,
    Project,
    Private,
    Agent,
}

#[derive(clap::Args, Debug)]
pub struct SkillAddArgs {
    /// 本地目录、git/url/repo 标识等 skills 生态 source
    #[arg(long)]
    pub source: PathBuf,
    #[arg(long, value_enum)]
    pub scope: SkillScopeArg,
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    #[arg(long)]
    pub agent: Option<String>,
    #[arg(long)]
    pub force: bool,
}

#[derive(clap::Args, Debug)]
pub struct SkillHubArgs {
    #[command(subcommand)]
    pub command: SkillHubCommands,
}

#[derive(Subcommand, Debug)]
pub enum SkillHubCommands {
    /// 在 ClawHub 中搜索 skills
    Search(SkillHubSearchArgs),
    /// 通过 ClawHub 安装到 project canonical skills dir（默认 <workspace>/.agents/skills）
    #[command(disable_version_flag = true)]
    Install(SkillHubInstallArgs),
    /// 列出当前 project canonical skills dir 的 ClawHub 记录
    List(SkillHubListArgs),
    /// 更新一个或全部已安装的 ClawHub skills
    #[command(disable_version_flag = true)]
    Update(SkillHubUpdateArgs),
    /// 扫描并同步本地 workspace skills 到 ClawHub
    Sync(SkillHubSyncArgs),
}

#[derive(clap::Args, Debug)]
pub struct SkillHubSearchArgs {
    pub query: String,
    #[arg(long)]
    pub limit: Option<u32>,
}

#[derive(clap::Args, Debug)]
pub struct SkillHubInstallArgs {
    pub slug: String,
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    #[arg(long)]
    pub dir: Option<PathBuf>,
    #[arg(long)]
    pub version: Option<String>,
    #[arg(long)]
    pub force: bool,
}

#[derive(clap::Args, Debug, Default)]
pub struct SkillHubListArgs {
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    #[arg(long)]
    pub dir: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
pub struct SkillHubUpdateArgs {
    pub slug: Option<String>,
    #[arg(long)]
    pub all: bool,
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    #[arg(long)]
    pub dir: Option<PathBuf>,
    #[arg(long)]
    pub version: Option<String>,
    #[arg(long)]
    pub force: bool,
}

#[derive(clap::Args, Debug, Default)]
pub struct SkillHubSyncArgs {
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    #[arg(long)]
    pub dir: Option<PathBuf>,
    #[arg(long = "root")]
    pub roots: Vec<PathBuf>,
    #[arg(long)]
    pub all: bool,
    #[arg(long = "dry-run")]
    pub dry_run: bool,
    #[arg(long)]
    pub bump: Option<String>,
    #[arg(long)]
    pub changelog: Option<String>,
    #[arg(long)]
    pub tags: Option<String>,
    #[arg(long)]
    pub concurrency: Option<u32>,
}

#[derive(clap::Args, Debug, Default)]
pub struct SkillCheckArgs {
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args, Debug, Default)]
pub struct SkillListArgs {
    #[arg(long, value_enum)]
    pub scope: Option<SkillScopeArg>,
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    #[arg(long)]
    pub agent: Option<String>,
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args, Debug)]
pub struct SkillRemoveArgs {
    #[arg(long)]
    pub name: String,
    #[arg(long, value_enum)]
    pub scope: SkillScopeArg,
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    #[arg(long)]
    pub agent: Option<String>,
}

#[derive(clap::Args, Debug, Default)]
pub struct SkillSyncArgs {
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args, Debug)]
pub struct ScheduleArgs {
    #[arg(long)]
    pub json: bool,
    #[command(subcommand)]
    pub command: ScheduleCommands,
}

#[derive(Subcommand, Debug)]
pub enum ScheduleCommands {
    AddCron(ScheduleAddCronArgs),
    AddAt(ScheduleAddAtArgs),
    AddEvery(ScheduleAddEveryArgs),
    AddDelay(ScheduleAddDelayArgs),
    List(ScheduleListArgs),
    Pause(ScheduleJobRefArgs),
    Resume(ScheduleJobRefArgs),
    Delete(ScheduleDeleteArgs),
    DeleteAll(ScheduleDeleteAllArgs),
    RunNow(ScheduleJobRefArgs),
    History(ScheduleHistoryArgs),
}

#[derive(clap::Args, Debug, Default)]
pub struct SetupArgs {
    /// 界面语言（跳过语言选择步骤）
    #[arg(long, value_enum)]
    pub lang: Option<LangArg>,
    /// 首次初始化拓扑预设
    #[arg(long, value_enum)]
    pub preset: Option<SetupPresetArg>,
    /// AI Provider
    #[arg(long, value_enum)]
    pub provider: Option<ProviderArg>,
    /// API Key（跳过交互式输入）
    #[arg(long, env = "CLAWBRO_SETUP_API_KEY")]
    pub api_key: Option<String>,
    /// 自定义 API Base URL
    #[arg(long)]
    pub api_base: Option<String>,
    /// 模型名称（覆盖 provider 默认值）
    #[arg(long)]
    pub model: Option<String>,
    /// 运行模式
    #[arg(long, value_enum)]
    pub mode: Option<ModeArg>,
    /// Team 模式下的 front bot 名称（默认: lead）
    #[arg(long)]
    pub front_bot: Option<String>,
    /// Team 模式下的目标类型（direct-message 或 group）
    #[arg(long, value_enum)]
    pub team_target: Option<TeamTargetArg>,
    /// Team 模式下的 specialist 名称，可重复传入
    #[arg(long)]
    pub specialist: Vec<String>,
    /// Team 模式下的 scope（例如 user:ou_xxx 或 group:lark:chat-123）
    #[arg(long)]
    pub team_scope: Option<String>,
    /// Team 模式下的可读名称
    #[arg(long)]
    pub team_name: Option<String>,
    /// WebSocket 认证 Token（留空 = 开放模式）
    #[arg(long)]
    pub ws_token: Option<String>,
    /// 备份旧配置后重新初始化
    #[arg(long)]
    pub reinit: bool,
    /// 非交互模式
    #[arg(long)]
    pub non_interactive: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum SetupPresetArg {
    Custom,
    WechatSolo,
    WechatDmTeam,
    LarkGroupTeam,
}

#[derive(clap::Args, Debug)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub command: AuthCommands,
}

#[derive(Subcommand, Debug)]
pub enum AuthCommands {
    /// 设置 API Key（写入 ~/.clawbro/.env）
    Set {
        /// provider 名称: anthropic | openai | deepseek | azure | ollama | custom
        provider: String,
        /// API Key 值
        key: String,
    },
    /// 列出已配置的 provider（key 脱敏显示）
    List,
    /// 检查 API Key 是否有效
    Check {
        /// 可选 provider 名称: anthropic | openai | deepseek | azure | ollama | custom
        provider: Option<String>,
    },
}

#[derive(clap::Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommands,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ConfigChannelArg {
    Wechat,
    Lark,
    Dingtalk,
    DingtalkWebhook,
}

impl ConfigChannelArg {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wechat => "wechat",
            Self::Lark => "lark",
            Self::Dingtalk => "dingtalk",
            Self::DingtalkWebhook => "dingtalk_webhook",
        }
    }
}

#[derive(clap::Args, Debug)]
pub struct ConfigChannelArgs {
    #[command(subcommand)]
    pub command: ConfigChannelCommands,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ConfigPresentationArg {
    FinalOnly,
    ProgressCompact,
}

#[derive(Subcommand, Debug)]
pub enum ConfigChannelCommands {
    /// 查看 channel 当前配置摘要
    Show { channel: ConfigChannelArg },
    /// 启用 channel
    Enable { channel: ConfigChannelArg },
    /// 禁用 channel
    Disable { channel: ConfigChannelArg },
    /// 执行 channel 登录流程（当前仅 WeChat）
    Login { channel: ConfigChannelArg },
    /// 设置 channel 的前台展示模式
    SetPresentation(ConfigChannelPresentationArgs),
    /// 一键把 channel 配成 solo 路由
    SetupSolo(ConfigChannelSetupSoloArgs),
    /// 一键把 channel 配成 team scope
    SetupTeam(ConfigChannelSetupTeamArgs),
}

#[derive(clap::Args, Debug)]
pub struct ConfigChannelPresentationArgs {
    #[arg(value_enum)]
    pub channel: ConfigChannelArg,
    #[arg(long, value_enum)]
    pub presentation: ConfigPresentationArg,
}

#[derive(clap::Args, Debug)]
pub struct ConfigChannelSetupSoloArgs {
    #[arg(value_enum)]
    pub channel: ConfigChannelArg,
    #[arg(long)]
    pub agent: String,
}

#[derive(clap::Args, Debug)]
pub struct ConfigChannelSetupTeamArgs {
    #[arg(value_enum)]
    pub channel: ConfigChannelArg,
    #[arg(long)]
    pub scope: String,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long = "front-bot")]
    pub front_bot: String,
    #[arg(long = "specialist")]
    pub specialists: Vec<String>,
    #[arg(long = "max-parallel", default_value_t = 1)]
    pub max_parallel: usize,
    #[arg(long = "public-updates", default_value = "minimal")]
    pub public_updates: String,
}

#[derive(clap::Args, Debug)]
pub struct ConfigProviderArgs {
    #[command(subcommand)]
    pub command: ConfigProviderCommands,
}

#[derive(Subcommand, Debug)]
pub enum ConfigProviderCommands {
    List,
    Show { id: String },
    Remove { id: String },
    AddOfficialSession { id: String },
    AddAnthropicCompatible(ConfigProviderAnthropicAddArgs),
    AddOpenaiCompatible(ConfigProviderOpenaiAddArgs),
}

#[derive(clap::Args, Debug)]
pub struct ConfigProviderAnthropicAddArgs {
    #[arg(long)]
    pub id: String,
    #[arg(long = "base-url")]
    pub base_url: String,
    #[arg(long = "auth-env")]
    pub auth_env: String,
    #[arg(long = "default-model")]
    pub default_model: String,
    #[arg(long = "small-fast-model")]
    pub small_fast_model: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct ConfigProviderOpenaiAddArgs {
    #[arg(long)]
    pub id: String,
    #[arg(long = "base-url")]
    pub base_url: String,
    #[arg(long = "auth-env")]
    pub auth_env: String,
    #[arg(long = "default-model")]
    pub default_model: String,
}

#[derive(clap::Args, Debug)]
pub struct ConfigBackendArgs {
    #[command(subcommand)]
    pub command: ConfigBackendCommands,
}

#[derive(Subcommand, Debug)]
pub enum ConfigBackendCommands {
    List,
    Show { id: String },
    Remove { id: String },
    Add(ConfigBackendAddArgs),
}

#[derive(clap::Args, Debug)]
pub struct ConfigBackendAddArgs {
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub family: String,
    #[arg(long = "acp-backend")]
    pub acp_backend: Option<String>,
    #[arg(long = "provider")]
    pub provider: Option<String>,
    #[arg(long = "launch", default_value = "bundled")]
    pub launch: String,
    #[arg(long)]
    pub command: Option<String>,
    #[arg(long = "arg")]
    pub args: Vec<String>,
    #[arg(long = "env")]
    pub env: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub struct ConfigAgentArgs {
    #[command(subcommand)]
    pub command: ConfigAgentCommands,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAgentCommands {
    List,
    Show { name: String },
    Remove { name: String },
    Add(ConfigAgentAddArgs),
}

#[derive(clap::Args, Debug)]
pub struct ConfigAgentAddArgs {
    #[arg(long)]
    pub name: String,
    #[arg(long = "mention")]
    pub mentions: Vec<String>,
    #[arg(long = "backend")]
    pub backend: String,
    #[arg(long = "persona-dir")]
    pub persona_dir: Option<PathBuf>,
    #[arg(long = "workspace-dir")]
    pub workspace_dir: Option<PathBuf>,
    #[arg(long = "extra-skills-dir")]
    pub extra_skills_dirs: Vec<PathBuf>,
}

#[derive(clap::Args, Debug)]
pub struct ConfigTeamScopeArgs {
    #[command(subcommand)]
    pub command: ConfigTeamScopeCommands,
}

#[derive(Subcommand, Debug)]
pub enum ConfigTeamScopeCommands {
    List,
    Show { channel: String, scope: String },
    Remove { channel: String, scope: String },
    Add(ConfigTeamScopeAddArgs),
}

#[derive(clap::Args, Debug)]
pub struct ConfigTeamScopeAddArgs {
    #[arg(long)]
    pub channel: String,
    #[arg(long)]
    pub scope: String,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long = "front-bot")]
    pub front_bot: String,
    #[arg(long = "specialist")]
    pub specialists: Vec<String>,
    #[arg(long = "max-parallel", default_value_t = 1)]
    pub max_parallel: usize,
    #[arg(long = "public-updates", default_value = "minimal")]
    pub public_updates: String,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ConfigBindingPeerKindArg {
    User,
    Group,
}

#[derive(clap::Args, Debug)]
pub struct ConfigBindingArgs {
    #[command(subcommand)]
    pub command: ConfigBindingCommands,
}

#[derive(Subcommand, Debug)]
pub enum ConfigBindingCommands {
    List,
    Show { id: String },
    Remove { id: String },
    AddThread(ConfigBindingAddThreadArgs),
    AddScope(ConfigBindingAddScopeArgs),
    AddPeer(ConfigBindingAddPeerArgs),
    AddTeam(ConfigBindingAddTeamArgs),
    AddChannelInstance(ConfigBindingAddChannelInstanceArgs),
    AddChannel(ConfigBindingAddChannelArgs),
    AddDefault(ConfigBindingAddDefaultArgs),
}

#[derive(clap::Args, Debug)]
pub struct ConfigBindingAddThreadArgs {
    #[arg(long)]
    pub agent: String,
    #[arg(long)]
    pub scope: String,
    #[arg(long = "thread-id")]
    pub thread_id: String,
    #[arg(long)]
    pub channel: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct ConfigBindingAddScopeArgs {
    #[arg(long)]
    pub agent: String,
    #[arg(long)]
    pub scope: String,
    #[arg(long)]
    pub channel: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct ConfigBindingAddPeerArgs {
    #[arg(long)]
    pub agent: String,
    #[arg(long = "peer-kind", value_enum)]
    pub peer_kind: ConfigBindingPeerKindArg,
    #[arg(long = "peer-id")]
    pub peer_id: String,
    #[arg(long)]
    pub channel: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct ConfigBindingAddTeamArgs {
    #[arg(long)]
    pub agent: String,
    #[arg(long = "team-id")]
    pub team_id: String,
}

#[derive(clap::Args, Debug)]
pub struct ConfigBindingAddChannelInstanceArgs {
    #[arg(long)]
    pub agent: String,
    #[arg(long)]
    pub channel: String,
    #[arg(long = "channel-instance")]
    pub channel_instance: String,
}

#[derive(clap::Args, Debug)]
pub struct ConfigBindingAddChannelArgs {
    #[arg(long)]
    pub agent: String,
    #[arg(long)]
    pub channel: String,
}

#[derive(clap::Args, Debug)]
pub struct ConfigBindingAddDefaultArgs {
    #[arg(long)]
    pub agent: String,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ConfigDeliveryPurposeArg {
    LeadFinal,
    LeadMessage,
    Milestone,
    Approval,
    BotMention,
    Cron,
}

#[derive(clap::Args, Debug)]
pub struct ConfigDeliverySenderArgs {
    #[command(subcommand)]
    pub command: ConfigDeliverySenderCommands,
}

#[derive(Subcommand, Debug)]
pub enum ConfigDeliverySenderCommands {
    List,
    Show { id: String },
    Remove { id: String },
    Add(ConfigDeliverySenderAddArgs),
}

#[derive(clap::Args, Debug)]
pub struct ConfigDeliverySenderAddArgs {
    #[arg(long, value_enum)]
    pub purpose: ConfigDeliveryPurposeArg,
    #[arg(long)]
    pub agent: Option<String>,
    #[arg(long)]
    pub channel: Option<String>,
    #[arg(long = "channel-instance")]
    pub channel_instance: String,
}

#[derive(clap::Args, Debug)]
pub struct ConfigDeliveryTargetArgs {
    #[command(subcommand)]
    pub command: ConfigDeliveryTargetCommands,
}

#[derive(Subcommand, Debug)]
pub enum ConfigDeliveryTargetCommands {
    List,
    Show { id: String },
    Remove { id: String },
    Add(ConfigDeliveryTargetAddArgs),
}

#[derive(clap::Args, Debug)]
pub struct ConfigDeliveryTargetAddArgs {
    #[arg(long, value_enum)]
    pub purpose: ConfigDeliveryPurposeArg,
    #[arg(long)]
    pub agent: Option<String>,
    #[arg(long)]
    pub channel: Option<String>,
    #[arg(long = "channel-instance")]
    pub channel_instance: Option<String>,
    #[arg(long)]
    pub scope: String,
    #[arg(long = "reply-to")]
    pub reply_to: Option<String>,
    #[arg(long = "thread-ts")]
    pub thread_ts: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// 打印当前配置（secrets 脱敏）
    Show,
    /// 验证 config.toml 语法和拓扑
    Validate,
    /// 用 $EDITOR 打开 config.toml
    Edit,
    /// 交互式配置中心
    Wizard,
    /// 管理 channel 配置
    Channel(ConfigChannelArgs),
    /// 管理 provider_profile 配置
    Provider(ConfigProviderArgs),
    /// 管理 backend 配置
    Backend(ConfigBackendArgs),
    /// 管理 agent_roster 配置
    Agent(ConfigAgentArgs),
    /// 管理 routing binding 配置
    Binding(ConfigBindingArgs),
    /// 管理 delivery sender 绑定
    DeliverySender(ConfigDeliverySenderArgs),
    /// 管理 delivery target override
    DeliveryTarget(ConfigDeliveryTargetArgs),
    /// 管理 team_scope 配置
    TeamScope(ConfigTeamScopeArgs),
}

#[derive(clap::Args, Debug, Default)]
pub struct ServeArgs {
    /// 配置文件路径（默认 ~/.clawbro/config.toml）
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// 覆盖监听端口
    #[arg(long, env = "CLAWBRO_PORT")]
    pub port: Option<u16>,
}

#[derive(clap::Args, Debug)]
#[command(trailing_var_arg = true)]
pub struct TeamHelperArgs {
    #[arg(long)]
    pub url: Option<String>,
    #[arg(long = "session-channel")]
    pub session_channel: Option<String>,
    #[arg(long = "session-scope")]
    pub session_scope: Option<String>,
    #[arg(value_name = "ARGS", allow_hyphen_values = true, num_args = 1..)]
    pub command: Vec<String>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct ScheduleTargetArgs {
    #[arg(long)]
    pub name: String,
    #[arg(long = "session-key")]
    pub session_key: Option<String>,
    #[arg(long = "current-session-key", hide = true)]
    pub current_session_key: Option<String>,
    #[arg(long)]
    pub prompt: String,
    #[arg(long)]
    pub agent: Option<String>,
    #[arg(long = "target-kind", value_enum, default_value_t = ScheduleTargetKindArg::Auto)]
    pub target_kind: ScheduleTargetKindArg,
    #[arg(long)]
    pub timezone: Option<String>,
    #[arg(long = "idle-gt-seconds")]
    pub idle_gt_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ScheduleTargetKindArg {
    Auto,
    AgentTurn,
    DeliveryMessage,
}

#[derive(clap::Args, Debug)]
pub struct ScheduleAddCronArgs {
    #[command(flatten)]
    pub target: ScheduleTargetArgs,
    #[arg(long)]
    pub expr: String,
}

#[derive(clap::Args, Debug)]
pub struct ScheduleAddAtArgs {
    #[command(flatten)]
    pub target: ScheduleTargetArgs,
    #[arg(long)]
    pub at: String,
}

#[derive(clap::Args, Debug)]
pub struct ScheduleAddEveryArgs {
    #[command(flatten)]
    pub target: ScheduleTargetArgs,
    #[arg(long)]
    pub every: String,
}

#[derive(clap::Args, Debug)]
pub struct ScheduleAddDelayArgs {
    #[command(flatten)]
    pub target: ScheduleTargetArgs,
    #[arg(long)]
    pub delay: String,
}

#[derive(clap::Args, Debug)]
pub struct ScheduleJobRefArgs {
    #[arg(long = "job-id")]
    pub job_id: String,
}

#[derive(clap::Args, Debug, Default, Clone)]
pub struct ScheduleSessionFilterArgs {
    #[arg(long = "session-key")]
    pub session_key: Option<String>,
    #[arg(long = "current-session-key", hide = true)]
    pub current_session_key: Option<String>,
}

#[derive(clap::Args, Debug, Default)]
pub struct ScheduleListArgs {
    #[command(flatten)]
    pub session: ScheduleSessionFilterArgs,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long = "name-contains")]
    pub name_contains: Option<String>,
}

#[derive(clap::Args, Debug, Default)]
pub struct ScheduleDeleteArgs {
    #[arg(long = "job-id")]
    pub job_id: Option<String>,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long = "name-contains")]
    pub name_contains: Option<String>,
    #[command(flatten)]
    pub session: ScheduleSessionFilterArgs,
    #[arg(long = "all-matches")]
    pub all_matches: bool,
}

#[derive(clap::Args, Debug, Default)]
pub struct ScheduleDeleteAllArgs {
    #[command(flatten)]
    pub session: ScheduleSessionFilterArgs,
}

#[derive(clap::Args, Debug)]
pub struct ScheduleHistoryArgs {
    #[arg(long = "job-id")]
    pub job_id: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct CompletionsArgs {
    #[arg(value_enum)]
    pub shell: ShellArg,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum LangArg {
    Zh,
    En,
    Ja,
    Ko,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum ProviderArg {
    Anthropic,
    Openai,
    Deepseek,
    Azure,
    Ollama,
    Custom,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum ModeArg {
    Solo,
    Multi,
    Team,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum TeamTargetArg {
    DirectMessage,
    Group,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum ShellArg {
    Bash,
    Zsh,
    Fish,
    PowerShell,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_skill_hub_install_with_version_flag() {
        let cli = Cli::try_parse_from([
            "clawbro",
            "skill",
            "hub",
            "install",
            "weather",
            "--workspace",
            "/tmp/ws",
            "--version",
            "1.2.3",
            "--force",
        ])
        .unwrap();

        match cli.command {
            Commands::Skill(SkillArgs {
                command:
                    SkillCommands::Hub(SkillHubArgs {
                        command: SkillHubCommands::Install(args),
                    }),
            }) => {
                assert_eq!(args.slug, "weather");
                assert_eq!(args.version.as_deref(), Some("1.2.3"));
                assert!(args.force);
            }
            _ => panic!("expected skill hub install command"),
        }
    }

    #[test]
    fn parses_skill_hub_update_with_version_flag() {
        let cli = Cli::try_parse_from([
            "clawbro",
            "skill",
            "hub",
            "update",
            "weather",
            "--workspace",
            "/tmp/ws",
            "--version",
            "2.0.0",
        ])
        .unwrap();

        match cli.command {
            Commands::Skill(SkillArgs {
                command:
                    SkillCommands::Hub(SkillHubArgs {
                        command: SkillHubCommands::Update(args),
                    }),
            }) => {
                assert_eq!(args.slug.as_deref(), Some("weather"));
                assert_eq!(args.version.as_deref(), Some("2.0.0"));
            }
            _ => panic!("expected skill hub update command"),
        }
    }
}
