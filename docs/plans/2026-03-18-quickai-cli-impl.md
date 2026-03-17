# QuickAI CLI Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan.

**Goal:** Build a `quickai` CLI binary — interactive `setup` wizard + `auth`, `config`, `serve`, `doctor`, `status`, `completions` subcommands. Full custom provider/model/URL/auth support.

**Architecture:** New `[[bin]] quickai` in `qai-server`. `serve` exec-replaces into `quickai-gateway` binary (no code duplication). Config written via string building (not toml_edit — avoids extra dep). Provider credentials stored in `~/.quickai/.env`.

**Tech Stack:** clap 4 (derive + env), dialoguer 0.11, console 0.15, which 6, existing qai-server config types (read-only for status/doctor)

---

## Command Tree

```
quickai
├── setup               交互式向导：语言→provider→apikey→模式→auth→channel→写配置
│   --lang zh|en|ja|ko  跳过语言步骤
│   --provider <name>   anthropic|openai|deepseek|azure|ollama|custom
│   --api-key <KEY>     跳过 key 输入
│   --api-base <URL>    自定义端点
│   --model <MODEL>     覆盖默认模型
│   --mode solo|multi|team
│   --reinit            备份后重新配置
│   --non-interactive   全参数非交互模式（CI/脚本）
│
├── auth
│   ├── set <provider> <key>   写入 ~/.quickai/.env
│   ├── list                   显示已配置的 provider（key 脱敏）
│   └── check                  探活各 provider API Key
│
├── config
│   ├── show            打印 config.toml（脱敏 secrets）
│   ├── validate        语法+拓扑校验（调用 GatewayConfig::validate_runtime_topology）
│   └── edit            用 $EDITOR 打开 config.toml
│
├── serve               exec 替换为 quickai-gateway 进程
│   --config <path>     覆盖配置路径
│   --port <port>       覆盖端口
│
├── doctor              检查 binary/config/env/dirs/gateway 进程
│
├── status              配置摘要（port/mode/backends/channels/key 状态）
│
└── completions <shell> 输出 shell 补全脚本（bash/zsh/fish）
```

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `Cargo.toml` (workspace) | Modify | 添加 clap/dialoguer/console/which |
| `crates/qai-server/Cargo.toml` | Modify | 新增 `[[bin]] quickai`，引用新依赖 |
| `crates/qai-server/src/bin/quickai_cli.rs` | Create | binary 入口，clap parse → dispatch |
| `crates/qai-server/src/cli/mod.rs` | Create | pub mod 导出 |
| `crates/qai-server/src/cli/args.rs` | Create | 全部 clap derive 结构 |
| `crates/qai-server/src/cli/i18n.rs` | Create | Language enum + 4语言 Messages |
| `crates/qai-server/src/cli/setup/mod.rs` | Create | setup 向导主流程 |
| `crates/qai-server/src/cli/setup/provider.rs` | Create | provider 选择 + API key + base URL |
| `crates/qai-server/src/cli/setup/mode.rs` | Create | solo/multi/team 模式 + port + workspace |
| `crates/qai-server/src/cli/setup/auth_cfg.rs` | Create | ws_token 配置步骤 |
| `crates/qai-server/src/cli/setup/channel.rs` | Create | lark/dingtalk 凭证收集 |
| `crates/qai-server/src/cli/setup/writer.rs` | Create | ConfigInput → TOML 字符串 → 写磁盘 |
| `crates/qai-server/src/cli/auth.rs` | Create | `quickai auth` 子命令实现 |
| `crates/qai-server/src/cli/config_cmd.rs` | Create | `quickai config` 子命令实现 |
| `crates/qai-server/src/cli/serve.rs` | Create | exec 替换为 quickai-gateway |
| `crates/qai-server/src/cli/doctor.rs` | Create | 诊断检查列表 |
| `crates/qai-server/src/cli/status.rs` | Create | 配置摘要展示 |
| `crates/qai-server/src/lib.rs` | Modify | 追加 `pub mod cli` |

---

## Task 1 — 依赖 + binary 骨架 + clap Args + i18n

**Files:** Cargo.toml (workspace), qai-server/Cargo.toml, quickai_cli.rs, cli/mod.rs, cli/args.rs, cli/i18n.rs, lib.rs

### Steps

- [ ] **1.1 Workspace Cargo.toml — 追加新依赖**

在 `[workspace.dependencies]` 末尾添加：
```toml
clap      = { version = "4", features = ["derive", "env", "color"] }
dialoguer = "0.11"
console   = "0.15"
which     = "6"
```

- [ ] **1.2 qai-server/Cargo.toml — 新 binary + 依赖引用**

新增 `[[bin]]`（在现有 `[[bin]]` 后）：
```toml
[[bin]]
name = "quickai"
path = "src/bin/quickai_cli.rs"
```

在 `[dependencies]` 末尾追加：
```toml
clap.workspace      = true
dialoguer.workspace = true
console.workspace   = true
which.workspace     = true
```

- [ ] **1.3 创建 `src/bin/quickai_cli.rs`**

```rust
use anyhow::Result;
use clap::Parser;
use qai_server::cli::args::{Cli, Commands};

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Setup(args)        => qai_server::cli::setup::run(args).await,
        Commands::Auth(args)         => qai_server::cli::auth::run(args).await,
        Commands::Config(args)       => qai_server::cli::config_cmd::run(args).await,
        Commands::Serve(args)        => qai_server::cli::serve::run(args).await,
        Commands::Doctor             => qai_server::cli::doctor::run().await,
        Commands::Status             => qai_server::cli::status::run().await,
        Commands::Completions(args)  => qai_server::cli::completions::run(args),
    }
}
```

- [ ] **1.4 创建 `src/cli/args.rs`**

```rust
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "quickai",
    about = "QuickAI Gateway — AI Agent 配置与运行",
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
    /// 诊断配置和运行环境
    Doctor,
    /// 显示当前配置摘要
    Status,
    /// 生成 Shell 补全脚本
    Completions(CompletionsArgs),
}

// ── setup ──────────────────────────────────────────────────────────────────
#[derive(clap::Args, Debug, Default)]
pub struct SetupArgs {
    /// 界面语言（跳过语言选择步骤）
    #[arg(long, value_enum)]
    pub lang: Option<LangArg>,

    /// AI Provider
    #[arg(long, value_enum)]
    pub provider: Option<ProviderArg>,

    /// API Key（跳过交互式输入）
    #[arg(long, env = "QUICKAI_SETUP_API_KEY")]
    pub api_key: Option<String>,

    /// 自定义 API Base URL（OpenAI/Anthropic 兼容端点，或 Ollama 地址）
    #[arg(long)]
    pub api_base: Option<String>,

    /// 模型名称（覆盖 provider 默认值）
    #[arg(long)]
    pub model: Option<String>,

    /// 运行模式
    #[arg(long, value_enum)]
    pub mode: Option<ModeArg>,

    /// WebSocket 认证 Token（留空 = 开放模式）
    #[arg(long)]
    pub ws_token: Option<String>,

    /// 备份旧配置后重新初始化
    #[arg(long)]
    pub reinit: bool,

    /// 非交互模式（所有必填项从 flag/env 提供，缺少则报错退出）
    #[arg(long)]
    pub non_interactive: bool,
}

// ── auth ───────────────────────────────────────────────────────────────────
#[derive(clap::Args, Debug)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub command: AuthCommands,
}

#[derive(Subcommand, Debug)]
pub enum AuthCommands {
    /// 设置 API Key（写入 ~/.quickai/.env）
    Set {
        /// provider 名称: anthropic | openai | deepseek | azure | ollama | custom
        provider: String,
        /// API Key 值
        key: String,
    },
    /// 列出已配置的 provider（key 脱敏显示）
    List,
    /// 检查 API Key 是否有效（发送探活请求）
    Check,
}

// ── config ─────────────────────────────────────────────────────────────────
#[derive(clap::Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommands,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// 打印当前配置（secrets 脱敏）
    Show,
    /// 验证 config.toml 语法和拓扑
    Validate,
    /// 用 $EDITOR 打开 config.toml
    Edit,
}

// ── serve ──────────────────────────────────────────────────────────────────
#[derive(clap::Args, Debug, Default)]
pub struct ServeArgs {
    /// 配置文件路径（默认 ~/.quickai/config.toml）
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// 覆盖监听端口
    #[arg(long, env = "QUICKAI_PORT")]
    pub port: Option<u16>,
}

// ── completions ────────────────────────────────────────────────────────────
#[derive(clap::Args, Debug)]
pub struct CompletionsArgs {
    /// Shell 类型
    #[arg(value_enum)]
    pub shell: ShellArg,
}

// ── value enums ────────────────────────────────────────────────────────────
#[derive(Debug, Clone, ValueEnum)]
pub enum LangArg { Zh, En, Ja, Ko }

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
pub enum ModeArg { Solo, Multi, Team }

#[derive(Debug, Clone, ValueEnum)]
pub enum ShellArg { Bash, Zsh, Fish, PowerShell }
```

- [ ] **1.5 创建 `src/cli/i18n.rs`** (4 语言字符串表，结构见 Task 2 详细内容)

- [ ] **1.6 创建 `src/cli/mod.rs`**

```rust
pub mod args;
pub mod auth;
pub mod completions;
pub mod config_cmd;
pub mod doctor;
pub mod i18n;
pub mod serve;
pub mod setup;
pub mod status;
```

- [ ] **1.7 创建各模块占位文件**（编译通过用，后续任务覆盖）

每个模块创建最小骨架：
```rust
// auth.rs
use anyhow::Result;
use crate::cli::args::AuthArgs;
pub async fn run(_args: AuthArgs) -> Result<()> { todo!() }
```

类似地：`config_cmd.rs`（`ConfigArgs`），`serve.rs`（`ServeArgs`），`doctor.rs`，`status.rs`（后两个签名 `pub async fn run() -> Result<()>`），`completions.rs`（`pub fn run(_args: CompletionsArgs) -> Result<()>`）。

- [ ] **1.8 在 `src/lib.rs` 末尾追加**

```rust
pub mod cli;
```

- [ ] **1.9 验证编译 + --help**

```bash
cd /Users/fishers/Desktop/repo/quickai-openclaw/quickai-gateway
cargo build -p qai-server --bin quickai 2>&1 | grep -E "^error" | head -20
cargo run -p qai-server --bin quickai -- --help 2>&1
```

预期：0 errors，help 显示 7 个子命令。

---

## Task 2 — i18n + setup provider 步骤

**Files:** cli/i18n.rs, cli/setup/provider.rs

### Steps

- [ ] **2.1 实现 `cli/i18n.rs`**（4语言，30+ 字段）

```rust
use crate::cli::args::LangArg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language { Zh, En, Ja, Ko }

impl Language {
    pub fn from_arg(arg: Option<&LangArg>) -> Self {
        match arg {
            Some(LangArg::En) => Language::En,
            Some(LangArg::Ja) => Language::Ja,
            Some(LangArg::Ko) => Language::Ko,
            _ => Language::Zh,
        }
    }
}

pub struct Messages {
    pub welcome: &'static str,
    pub select_language: &'static str,
    pub select_provider: &'static str,
    pub enter_api_key: &'static str,
    pub enter_api_key_hint: &'static str,
    pub enter_api_base: &'static str,
    pub enter_model: &'static str,
    pub select_mode: &'static str,
    pub mode_solo: &'static str,
    pub mode_multi: &'static str,
    pub mode_team: &'static str,
    pub enter_port: &'static str,
    pub enter_workspace: &'static str,
    pub enter_ws_token: &'static str,
    pub enter_ws_token_hint: &'static str,
    pub select_channel: &'static str,
    pub channel_none: &'static str,
    pub channel_lark: &'static str,
    pub channel_dingtalk: &'static str,
    pub enter_lark_app_id: &'static str,
    pub enter_lark_app_secret: &'static str,
    pub enter_lark_verify_token: &'static str,
    pub enter_lark_bot_name: &'static str,
    pub enter_dingtalk_client_id: &'static str,
    pub enter_dingtalk_client_secret: &'static str,
    pub enter_dingtalk_agent_id: &'static str,
    pub enter_dingtalk_bot_name: &'static str,
    pub confirm_write: &'static str,
    pub written_config: &'static str,
    pub written_env: &'static str,
    pub backed_up: &'static str,
    pub done: &'static str,
    pub next_steps: &'static str,
}

impl Messages {
    pub fn for_lang(lang: Language) -> &'static Self {
        match lang {
            Language::Zh => &ZH,
            Language::En => &EN,
            Language::Ja => &JA,
            Language::Ko => &KO,
        }
    }
}

static ZH: Messages = Messages {
    welcome: "════════════════════════════════════\n  QuickAI Gateway 初始化向导\n════════════════════════════════════",
    select_language: "请选择界面语言 / Select Language",
    select_provider: "选择 AI Provider",
    enter_api_key: "请输入 API Key",
    enter_api_key_hint: "（输入内容不会显示）",
    enter_api_base: "自定义 API Base URL（可选，留空使用默认）",
    enter_model: "模型名称（留空使用默认值）",
    select_mode: "选择运行模式",
    mode_solo: "Solo — 单 Agent，适合个人使用",
    mode_multi: "Multi-agent — 多 Agent，通过 @mention 切换",
    mode_team: "Team — Lead + Specialists 编排协作",
    enter_port: "Gateway 监听端口（0 = 随机，默认 8080）",
    enter_workspace: "默认工作目录（留空 = 不设置）",
    enter_ws_token: "WebSocket 认证 Token（留空 = 开放模式，无需鉴权）",
    enter_ws_token_hint: "建议生产环境设置 Token 保护 /ws 端点",
    select_channel: "接入 IM Channel（可选）",
    channel_none: "暂不接入，只用 WebSocket",
    channel_lark: "飞书 / Feishu / Lark",
    channel_dingtalk: "钉钉 / DingTalk",
    enter_lark_app_id: "飞书 App ID（格式: cli_xxxx）",
    enter_lark_app_secret: "飞书 App Secret",
    enter_lark_verify_token: "飞书 Verification Token",
    enter_lark_bot_name: "Bot 名称（群消息 @mention 识别，可留空）",
    enter_dingtalk_client_id: "钉钉 AppKey / client_id",
    enter_dingtalk_client_secret: "钉钉 AppSecret / client_secret",
    enter_dingtalk_agent_id: "AgentId（数字，在钉钉开放平台基础信息页，可留空）",
    enter_dingtalk_bot_name: "Bot 名称（可留空）",
    confirm_write: "确认写入配置文件？",
    written_config: "✓ 配置已写入 ~/.quickai/config.toml",
    written_env: "✓ API Key 已写入 ~/.quickai/.env",
    backed_up: "✓ 旧配置已备份",
    done: "🎉 初始化完成！",
    next_steps: "启动 Gateway：\n  source ~/.quickai/.env && quickai serve\n\n其他命令：\n  quickai doctor      — 诊断问题\n  quickai status      — 查看配置\n  quickai auth list   — 查看 API Key\n  quickai setup       — 重新配置",
};

static EN: Messages = Messages {
    welcome: "════════════════════════════════════\n  QuickAI Gateway Setup Wizard\n════════════════════════════════════",
    select_language: "Select Language / 选择语言",
    select_provider: "Select AI Provider",
    enter_api_key: "Enter API Key",
    enter_api_key_hint: "(input is hidden)",
    enter_api_base: "Custom API Base URL (optional, leave empty for default)",
    enter_model: "Model name (leave empty for default)",
    select_mode: "Select operation mode",
    mode_solo: "Solo — single agent, great for personal use",
    mode_multi: "Multi-agent — multiple agents, switch via @mention",
    mode_team: "Team — Lead + Specialists for complex tasks",
    enter_port: "Gateway port (0 = random, default 8080)",
    enter_workspace: "Default workspace directory (leave empty to skip)",
    enter_ws_token: "WebSocket auth token (leave empty = open mode, no auth)",
    enter_ws_token_hint: "Recommended to set a token in production",
    select_channel: "Connect an IM Channel (optional)",
    channel_none: "Skip — WebSocket only",
    channel_lark: "Feishu / Lark",
    channel_dingtalk: "DingTalk",
    enter_lark_app_id: "Lark App ID (format: cli_xxxx)",
    enter_lark_app_secret: "Lark App Secret",
    enter_lark_verify_token: "Lark Verification Token",
    enter_lark_bot_name: "Bot name (for @mention in groups, optional)",
    enter_dingtalk_client_id: "DingTalk AppKey / client_id",
    enter_dingtalk_client_secret: "DingTalk AppSecret / client_secret",
    enter_dingtalk_agent_id: "AgentId (number, from DingTalk app basic info, optional)",
    enter_dingtalk_bot_name: "Bot name (optional)",
    confirm_write: "Write configuration files?",
    written_config: "✓ Config written to ~/.quickai/config.toml",
    written_env: "✓ API Key written to ~/.quickai/.env",
    backed_up: "✓ Old config backed up",
    done: "Setup complete!",
    next_steps: "Start Gateway:\n  source ~/.quickai/.env && quickai serve\n\nOther commands:\n  quickai doctor      — diagnose issues\n  quickai status      — show config\n  quickai auth list   — view API keys\n  quickai setup       — reconfigure",
};

static JA: Messages = Messages {
    welcome: "════════════════════════════════════\n  QuickAI Gateway セットアップ\n════════════════════════════════════",
    select_language: "言語を選択 / Select Language",
    select_provider: "AIプロバイダーを選択",
    enter_api_key: "APIキーを入力",
    enter_api_key_hint: "（入力内容は非表示）",
    enter_api_base: "カスタムAPI Base URL（任意、空欄でデフォルト使用）",
    enter_model: "モデル名（空欄でデフォルト）",
    select_mode: "動作モードを選択",
    mode_solo: "Solo — シングルエージェント",
    mode_multi: "Multi-agent — 複数エージェント（@mentionで切替）",
    mode_team: "Team — リード＋スペシャリスト編成",
    enter_port: "Gatewayポート（0=ランダム、デフォルト8080）",
    enter_workspace: "デフォルト作業ディレクトリ（任意）",
    enter_ws_token: "WebSocket認証トークン（空欄=認証なし）",
    enter_ws_token_hint: "本番環境では設定を推奨",
    select_channel: "IMチャンネル接続（任意）",
    channel_none: "スキップ — WebSocketのみ",
    channel_lark: "Feishu / Lark",
    channel_dingtalk: "DingTalk",
    enter_lark_app_id: "Lark App ID（形式: cli_xxxx）",
    enter_lark_app_secret: "Lark App Secret",
    enter_lark_verify_token: "Lark Verification Token",
    enter_lark_bot_name: "Bot名（@mention識別用、任意）",
    enter_dingtalk_client_id: "DingTalk AppKey / client_id",
    enter_dingtalk_client_secret: "DingTalk AppSecret / client_secret",
    enter_dingtalk_agent_id: "AgentId（数字、任意）",
    enter_dingtalk_bot_name: "Bot名（任意）",
    confirm_write: "設定ファイルを書き込みますか？",
    written_config: "✓ ~/.quickai/config.toml に設定を書き込みました",
    written_env: "✓ APIキーを ~/.quickai/.env に書き込みました",
    backed_up: "✓ 旧設定をバックアップしました",
    done: "セットアップ完了！",
    next_steps: "起動：\n  source ~/.quickai/.env && quickai serve\n\nその他：\n  quickai doctor      — 問題診断\n  quickai status      — 設定確認",
};

static KO: Messages = Messages {
    welcome: "════════════════════════════════════\n  QuickAI Gateway 설정 마법사\n════════════════════════════════════",
    select_language: "언어 선택 / Select Language",
    select_provider: "AI 공급자 선택",
    enter_api_key: "API 키 입력",
    enter_api_key_hint: "（입력 내용 비표시）",
    enter_api_base: "커스텀 API Base URL（선택사항）",
    enter_model: "모델 이름（기본값 사용 시 빈칸）",
    select_mode: "실행 모드 선택",
    mode_solo: "Solo — 단일 에이전트",
    mode_multi: "Multi-agent — 다중 에이전트（@mention 전환）",
    mode_team: "Team — Lead + Specialists 협업",
    enter_port: "Gateway 포트（0=랜덤, 기본 8080）",
    enter_workspace: "기본 작업 디렉토리（선택사항）",
    enter_ws_token: "WebSocket 인증 토큰（빈칸=인증 없음）",
    enter_ws_token_hint: "프로덕션 환경에서는 설정 권장",
    select_channel: "IM 채널 연결（선택사항）",
    channel_none: "건너뛰기 — WebSocket만",
    channel_lark: "Feishu / Lark",
    channel_dingtalk: "DingTalk",
    enter_lark_app_id: "Lark App ID（형식: cli_xxxx）",
    enter_lark_app_secret: "Lark App Secret",
    enter_lark_verify_token: "Lark Verification Token",
    enter_lark_bot_name: "봇 이름（선택사항）",
    enter_dingtalk_client_id: "DingTalk AppKey / client_id",
    enter_dingtalk_client_secret: "DingTalk AppSecret / client_secret",
    enter_dingtalk_agent_id: "AgentId（숫자, 선택사항）",
    enter_dingtalk_bot_name: "봇 이름（선택사항）",
    confirm_write: "설정 파일을 저장하시겠습니까?",
    written_config: "✓ ~/.quickai/config.toml 저장 완료",
    written_env: "✓ API 키가 ~/.quickai/.env에 저장되었습니다",
    backed_up: "✓ 이전 설정이 백업되었습니다",
    done: "설정 완료！",
    next_steps: "시작：\n  source ~/.quickai/.env && quickai serve\n\n기타：\n  quickai doctor      — 문제 진단\n  quickai status      — 설정 확인",
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_languages_non_empty() {
        for lang in [Language::Zh, Language::En, Language::Ja, Language::Ko] {
            let m = Messages::for_lang(lang);
            assert!(!m.welcome.is_empty());
            assert!(!m.select_provider.is_empty());
            assert!(!m.done.is_empty());
        }
    }
    #[test]
    fn from_arg_defaults_to_zh() {
        assert_eq!(Language::from_arg(None), Language::Zh);
        assert_eq!(Language::from_arg(Some(&LangArg::En)), Language::En);
    }
}
```

- [ ] **2.2 创建 `src/cli/setup/mod.rs`** (仅导出，详见 Task 4)

```rust
pub mod auth_cfg;
pub mod channel;
pub mod mode;
pub mod provider;
pub mod writer;

use anyhow::Result;
use crate::cli::args::SetupArgs;

pub async fn run(args: SetupArgs) -> Result<()> {
    todo!("implemented in Task 4")
}
```

- [ ] **2.3 实现 `src/cli/setup/provider.rs`**

```rust
use crate::cli::{args::{ProviderArg, SetupArgs}, i18n::{Language, Messages}};
use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Input, Password, Select};

/// 内置 provider 预设
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderKind {
    Anthropic,
    OpenAI,
    DeepSeek,
    Azure,
    Ollama,
    Custom,
}

impl ProviderKind {
    pub fn display_name(&self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "Anthropic（Claude）",
            ProviderKind::OpenAI    => "OpenAI（GPT）",
            ProviderKind::DeepSeek  => "DeepSeek",
            ProviderKind::Azure     => "Azure OpenAI",
            ProviderKind::Ollama    => "Ollama（本地模型）",
            ProviderKind::Custom    => "其他 OpenAI 兼容端点",
        }
    }
    /// 环境变量名
    pub fn env_var(&self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
            ProviderKind::Ollama    => "",  // Ollama 通常不需要 key
            _                       => "OPENAI_API_KEY",
        }
    }
    /// 协议类型：用于 [[provider_profile]]
    pub fn protocol_tag(&self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic_compatible",
            _                       => "openai_compatible",
        }
    }
    /// 内置默认 base_url（None = 无需配置）
    pub fn default_base_url(&self) -> Option<&'static str> {
        match self {
            ProviderKind::Anthropic => Some("https://api.anthropic.com"),
            ProviderKind::OpenAI    => Some("https://api.openai.com"),
            ProviderKind::DeepSeek  => Some("https://api.deepseek.com"),
            ProviderKind::Ollama    => Some("http://localhost:11434"),
            _                       => None,
        }
    }
    /// 默认模型名
    pub fn default_model(&self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "claude-sonnet-4-6",
            ProviderKind::OpenAI    => "gpt-4o",
            ProviderKind::DeepSeek  => "deepseek-chat",
            ProviderKind::Azure     => "gpt-4o",
            ProviderKind::Ollama    => "llama3",
            ProviderKind::Custom    => "gpt-4o",
        }
    }
    /// Ollama 和自定义 provider 不需要 key
    pub fn needs_api_key(&self) -> bool {
        !matches!(self, ProviderKind::Ollama)
    }
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub api_key: String,        // empty if Ollama
    pub base_url: String,       // always populated (either default or user-provided)
    pub model: String,          // always populated
    pub profile_id: String,     // e.g. "anthropic-main", "openai-main"
}

impl ProviderConfig {
    pub fn env_var(&self) -> &str {
        self.kind.env_var()
    }
}

pub fn collect(args: &SetupArgs, lang: Language) -> Result<ProviderConfig> {
    let m = Messages::for_lang(lang);
    let theme = ColorfulTheme::default();

    // Provider 选择
    let kind = if let Some(p) = &args.provider {
        match p {
            ProviderArg::Anthropic => ProviderKind::Anthropic,
            ProviderArg::Openai    => ProviderKind::OpenAI,
            ProviderArg::Deepseek  => ProviderKind::DeepSeek,
            ProviderArg::Azure     => ProviderKind::Azure,
            ProviderArg::Ollama    => ProviderKind::Ollama,
            ProviderArg::Custom    => ProviderKind::Custom,
        }
    } else {
        let choices = [
            ProviderKind::Anthropic,
            ProviderKind::OpenAI,
            ProviderKind::DeepSeek,
            ProviderKind::Azure,
            ProviderKind::Ollama,
            ProviderKind::Custom,
        ];
        let names: Vec<&str> = choices.iter().map(|c| c.display_name()).collect();
        let idx = Select::with_theme(&theme)
            .with_prompt(m.select_provider)
            .items(&names)
            .default(0)
            .interact()?;
        choices[idx].clone()
    };

    // API Key（Ollama 跳过）
    let api_key = if !kind.needs_api_key() {
        String::new()
    } else if let Some(k) = &args.api_key {
        k.clone()
    } else {
        println!("  {}", m.enter_api_key_hint);
        Password::with_theme(&theme)
            .with_prompt(m.enter_api_key)
            .interact()?
    };

    // Base URL
    let base_url = if let Some(b) = &args.api_base {
        b.clone()
    } else if matches!(kind, ProviderKind::Azure | ProviderKind::Custom) {
        // Azure/Custom 必须让用户填
        let default = kind.default_base_url().unwrap_or("").to_string();
        let entered: String = Input::with_theme(&theme)
            .with_prompt(m.enter_api_base)
            .default(default)
            .allow_empty(false)
            .interact_text()?;
        entered.trim().to_string()
    } else {
        // 其他 provider 有内置默认值，仍允许用户覆盖
        let default = kind.default_base_url().unwrap_or("").to_string();
        if !args.non_interactive {
            let entered: String = Input::with_theme(&theme)
                .with_prompt(m.enter_api_base)
                .default(default.clone())
                .allow_empty(true)
                .interact_text()?;
            if entered.trim().is_empty() { default } else { entered.trim().to_string() }
        } else {
            default
        }
    };

    // 模型
    let model = if let Some(m_override) = &args.model {
        m_override.clone()
    } else {
        let default_m = kind.default_model().to_string();
        if !args.non_interactive {
            let entered: String = Input::with_theme(&theme)
                .with_prompt(m.enter_model)
                .default(default_m.clone())
                .allow_empty(true)
                .interact_text()?;
            if entered.trim().is_empty() { default_m } else { entered.trim().to_string() }
        } else {
            default_m
        }
    };

    let profile_id = format!("{}-main", match kind {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::OpenAI    => "openai",
        ProviderKind::DeepSeek  => "deepseek",
        ProviderKind::Azure     => "azure",
        ProviderKind::Ollama    => "ollama",
        ProviderKind::Custom    => "custom",
    });

    Ok(ProviderConfig { kind, api_key, base_url, model, profile_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_defaults() {
        let k = ProviderKind::Anthropic;
        assert_eq!(k.env_var(), "ANTHROPIC_API_KEY");
        assert_eq!(k.default_model(), "claude-sonnet-4-6");
        assert_eq!(k.protocol_tag(), "anthropic_compatible");
        assert!(k.needs_api_key());
    }
    #[test]
    fn deepseek_base_url() {
        assert_eq!(
            ProviderKind::DeepSeek.default_base_url(),
            Some("https://api.deepseek.com")
        );
    }
    #[test]
    fn ollama_no_key() {
        assert!(!ProviderKind::Ollama.needs_api_key());
        assert_eq!(ProviderKind::Ollama.default_base_url(), Some("http://localhost:11434"));
    }
    #[test]
    fn profile_id_format() {
        let cfg = ProviderConfig {
            kind: ProviderKind::DeepSeek,
            api_key: "key".into(),
            base_url: "https://api.deepseek.com".into(),
            model: "deepseek-chat".into(),
            profile_id: "deepseek-main".into(),
        };
        assert_eq!(cfg.profile_id, "deepseek-main");
    }
}
```

- [ ] **2.4 运行测试**

```bash
cargo test -p qai-server cli 2>&1 | tail -20
```

预期：i18n + provider 测试全部 PASS。

---

## Task 3 — setup mode + auth_cfg + channel + writer

**Files:** setup/mode.rs, setup/auth_cfg.rs, setup/channel.rs, setup/writer.rs

### Steps

- [ ] **3.1 实现 `setup/mode.rs`**

```rust
use crate::cli::{args::{ModeArg, SetupArgs}, i18n::{Language, Messages}};
use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Mode { Solo, Multi, Team }

#[derive(Debug, Clone)]
pub struct ModeConfig {
    pub mode: Mode,
    pub port: u16,
    pub workspace: Option<PathBuf>,
}

pub fn collect(args: &SetupArgs, lang: Language) -> Result<ModeConfig> {
    let m = Messages::for_lang(lang);
    let theme = ColorfulTheme::default();

    let mode = if let Some(a) = &args.mode {
        match a { ModeArg::Solo => Mode::Solo, ModeArg::Multi => Mode::Multi, ModeArg::Team => Mode::Team }
    } else {
        let items = [m.mode_solo, m.mode_multi, m.mode_team];
        let idx = Select::with_theme(&theme).with_prompt(m.select_mode).items(&items).default(0).interact()?;
        match idx { 1 => Mode::Multi, 2 => Mode::Team, _ => Mode::Solo }
    };

    let port_str: String = Input::with_theme(&theme)
        .with_prompt(m.enter_port).default("8080".into()).interact_text()?;
    let port: u16 = port_str.trim().parse().unwrap_or(8080);

    let ws_str: String = Input::with_theme(&theme)
        .with_prompt(m.enter_workspace).allow_empty(true).interact_text()?;
    let workspace = if ws_str.trim().is_empty() { None } else { Some(PathBuf::from(ws_str.trim())) };

    Ok(ModeConfig { mode, port, workspace })
}
```

- [ ] **3.2 实现 `setup/auth_cfg.rs`**

```rust
use crate::cli::{args::SetupArgs, i18n::{Language, Messages}};
use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Input};

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub ws_token: Option<String>,
}

pub fn collect(args: &SetupArgs, lang: Language) -> Result<AuthConfig> {
    let m = Messages::for_lang(lang);
    let theme = ColorfulTheme::default();

    let ws_token = if let Some(t) = &args.ws_token {
        if t.is_empty() { None } else { Some(t.clone()) }
    } else if args.non_interactive {
        None
    } else {
        println!("  {}", m.enter_ws_token_hint);
        let entered: String = Input::with_theme(&theme)
            .with_prompt(m.enter_ws_token)
            .allow_empty(true)
            .interact_text()?;
        if entered.trim().is_empty() { None } else { Some(entered.trim().to_string()) }
    };

    Ok(AuthConfig { ws_token })
}
```

- [ ] **3.3 实现 `setup/channel.rs`**

```rust
use crate::cli::i18n::{Language, Messages};
use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Input, Password, Select};

#[derive(Debug, Clone)]
pub enum ChannelConfig { None, Lark(LarkCfg), DingTalk(DingTalkCfg) }

#[derive(Debug, Clone)]
pub struct LarkCfg {
    pub app_id: String,
    pub app_secret: String,
    pub verification_token: String,
    pub bot_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DingTalkCfg {
    pub client_id: String,
    pub client_secret: String,
    pub agent_id: Option<u64>,
    pub bot_name: Option<String>,
}

pub fn collect(lang: Language) -> Result<ChannelConfig> {
    let m = Messages::for_lang(lang);
    let theme = ColorfulTheme::default();
    let items = [m.channel_none, m.channel_lark, m.channel_dingtalk];
    let idx = Select::with_theme(&theme).with_prompt(m.select_channel).items(&items).default(0).interact()?;
    match idx {
        1 => {
            let app_id: String = Input::with_theme(&theme).with_prompt(m.enter_lark_app_id).interact_text()?;
            let app_secret = Password::with_theme(&theme).with_prompt(m.enter_lark_app_secret).interact()?;
            let verification_token: String = Input::with_theme(&theme).with_prompt(m.enter_lark_verify_token).interact_text()?;
            let bot_raw: String = Input::with_theme(&theme).with_prompt(m.enter_lark_bot_name).allow_empty(true).interact_text()?;
            let bot_name = if bot_raw.trim().is_empty() { None } else { Some(bot_raw.trim().to_string()) };
            Ok(ChannelConfig::Lark(LarkCfg { app_id: app_id.trim().into(), app_secret, verification_token: verification_token.trim().into(), bot_name }))
        }
        2 => {
            let client_id: String = Input::with_theme(&theme).with_prompt(m.enter_dingtalk_client_id).interact_text()?;
            let client_secret = Password::with_theme(&theme).with_prompt(m.enter_dingtalk_client_secret).interact()?;
            let agent_raw: String = Input::with_theme(&theme).with_prompt(m.enter_dingtalk_agent_id).allow_empty(true).interact_text()?;
            let agent_id = agent_raw.trim().parse::<u64>().ok();
            let bot_raw: String = Input::with_theme(&theme).with_prompt(m.enter_dingtalk_bot_name).allow_empty(true).interact_text()?;
            let bot_name = if bot_raw.trim().is_empty() { None } else { Some(bot_raw.trim().to_string()) };
            Ok(ChannelConfig::DingTalk(DingTalkCfg { client_id: client_id.trim().into(), client_secret, agent_id, bot_name }))
        }
        _ => Ok(ChannelConfig::None),
    }
}
```

- [ ] **3.4 实现 `setup/writer.rs`（含测试）**

```rust
use super::{auth_cfg::AuthConfig, channel::ChannelConfig, mode::{Mode, ModeConfig}, provider::ProviderConfig};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct WriteInputs<'a> {
    pub provider: &'a ProviderConfig,
    pub mode: &'a ModeConfig,
    pub auth: &'a AuthConfig,
    pub channel: &'a ChannelConfig,
}

/// 生成完整 config.toml 内容字符串
pub fn build_config_toml(input: &WriteInputs) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let qdir = home.join(".quickai");
    let mut s = String::new();

    // [gateway]
    s.push_str("[gateway]\n");
    s.push_str("host = \"127.0.0.1\"\n");
    s.push_str(&format!("port = {}\n", input.mode.port));
    let require_mention = !matches!(input.mode.mode, Mode::Solo);
    s.push_str(&format!("require_mention_in_groups = {}\n", require_mention));
    if let Some(ws) = &input.mode.workspace {
        s.push_str(&format!("default_workspace = {:?}\n", ws.to_string_lossy().as_ref()));
    }
    s.push('\n');

    // [auth]
    if let Some(tok) = &input.auth.ws_token {
        s.push_str("[auth]\n");
        s.push_str(&format!("ws_token = {:?}\n", tok));
        s.push('\n');
    }

    // [[provider_profile]]
    // Only write if provider has a base_url (always true for our kinds)
    s.push_str("[[provider_profile]]\n");
    s.push_str(&format!("id = {:?}\n", input.provider.profile_id));
    s.push_str(&format!("protocol = {:?}\n", input.provider.kind.protocol_tag()));
    s.push_str(&format!("base_url = {:?}\n", input.provider.base_url));
    s.push_str(&format!("auth_token_env = {:?}\n", input.provider.kind.env_var()));
    s.push_str(&format!("default_model = {:?}\n", input.provider.model));
    s.push('\n');

    // [[backend]]
    s.push_str("[[backend]]\n");
    s.push_str("id = \"native-main\"\n");
    s.push_str("family = \"quick_ai_native\"\n");
    s.push_str(&format!("provider_profile = {:?}\n", input.provider.profile_id));
    s.push('\n');
    s.push_str("[backend.launch]\n");
    s.push_str("type = \"bundled_command\"\n");
    s.push('\n');

    // [agent] or comment
    match input.mode.mode {
        Mode::Solo => {
            s.push_str("[agent]\n");
            s.push_str("backend_id = \"native-main\"\n");
            s.push('\n');
        }
        Mode::Multi | Mode::Team => {
            s.push_str("# 在下方添加 [[agent_roster]] 配置多个 Agent\n");
            s.push_str("# 示例:\n");
            s.push_str("# [[agent_roster]]\n");
            s.push_str("# name = \"claude\"\n");
            s.push_str("# mentions = [\"@claude\"]\n");
            s.push_str("# backend_id = \"native-main\"\n");
            s.push('\n');
        }
    }

    // [session]
    s.push_str("[session]\n");
    s.push_str(&format!("dir = {:?}\n", qdir.join("sessions").to_string_lossy().as_ref()));
    s.push('\n');

    // [memory]
    s.push_str("[memory]\n");
    s.push_str(&format!("shared_dir = {:?}\n", qdir.join("shared").to_string_lossy().as_ref()));
    s.push_str("distill_every_n = 20\n");
    s.push_str("distiller_binary = \"quickai-rust-agent\"\n");
    s.push('\n');

    // [skills]
    s.push_str("[skills]\n");
    s.push_str(&format!("dir = {:?}\n", qdir.join("skills").to_string_lossy().as_ref()));
    s.push('\n');

    // [channels.*]
    match input.channel {
        ChannelConfig::Lark(l) => {
            s.push_str("[channels.lark]\n");
            s.push_str("enabled = true\n");
            s.push('\n');
            s.push_str("[[channels.lark.instances]]\n");
            s.push_str("id = \"default\"\n");
            s.push_str(&format!("app_id = {:?}\n", l.app_id));
            s.push_str(&format!("app_secret = {:?}\n", l.app_secret));
            if let Some(bn) = &l.bot_name {
                s.push_str(&format!("bot_name = {:?}\n", bn));
            }
            s.push('\n');
            // verification_token goes in .env
        }
        ChannelConfig::DingTalk(d) => {
            s.push_str("[channels.dingtalk]\n");
            s.push_str("enabled = true\n");
            if let Some(aid) = d.agent_id {
                s.push_str(&format!("agent_id = {}\n", aid));
            }
            if let Some(bn) = &d.bot_name {
                s.push_str(&format!("bot_name = {:?}\n", bn));
            }
            s.push('\n');
        }
        ChannelConfig::None => {}
    }

    s
}

/// 生成 .env 文件内容
pub fn build_env_content(provider: &ProviderConfig, channel: &ChannelConfig) -> String {
    let mut lines = Vec::new();
    let env_var = provider.kind.env_var();
    if !env_var.is_empty() && !provider.api_key.is_empty() {
        lines.push(format!("export {}={}", env_var, provider.api_key));
    }
    // Channel secrets go into env too (gateway reads from env)
    match channel {
        ChannelConfig::Lark(l) => {
            lines.push(format!("export LARK_APP_ID={}", l.app_id));
            lines.push(format!("export LARK_APP_SECRET={}", l.app_secret));
            lines.push(format!("export LARK_VERIFICATION_TOKEN={}", l.verification_token));
        }
        ChannelConfig::DingTalk(d) => {
            lines.push(format!("export DINGTALK_APP_KEY={}", d.client_id));
            lines.push(format!("export DINGTALK_APP_SECRET={}", d.client_secret));
        }
        ChannelConfig::None => {}
    }
    lines.join("\n") + "\n"
}

/// 写 config.toml（先备份）
pub fn write_config(input: &WriteInputs) -> Result<Option<PathBuf>> {
    let path = config_path();
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    let backup = if path.exists() {
        let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
        let bak = path.with_extension(format!("toml.bak.{ts}"));
        std::fs::copy(&path, &bak).context("backup failed")?;
        Some(bak)
    } else { None };
    std::fs::write(&path, build_config_toml(input)).context("write config.toml")?;
    Ok(backup)
}

/// 写 .env 文件
pub fn write_env(provider: &ProviderConfig, channel: &ChannelConfig) -> Result<()> {
    let path = env_path();
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    std::fs::write(&path, build_env_content(provider, channel)).context("write .env")?;
    Ok(())
}

pub fn config_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".quickai").join("config.toml")
}
pub fn env_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".quickai").join(".env")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::setup::{
        auth_cfg::AuthConfig,
        channel::{ChannelConfig, LarkCfg, DingTalkCfg},
        mode::{Mode, ModeConfig},
        provider::{ProviderConfig, ProviderKind},
    };
    use tempfile::NamedTempFile;
    use std::io::Write;

    fn anthropic_provider() -> ProviderConfig {
        ProviderConfig {
            kind: ProviderKind::Anthropic,
            api_key: "sk-ant-test".into(),
            base_url: "https://api.anthropic.com".into(),
            model: "claude-sonnet-4-6".into(),
            profile_id: "anthropic-main".into(),
        }
    }
    fn solo_mode() -> ModeConfig { ModeConfig { mode: Mode::Solo, port: 8080, workspace: None } }
    fn no_auth() -> AuthConfig { AuthConfig { ws_token: None } }

    #[test]
    fn toml_has_gateway_section() {
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic_provider(), mode: &solo_mode(),
            auth: &no_auth(), channel: &ChannelConfig::None,
        });
        assert!(t.contains("[gateway]"), "missing [gateway]");
        assert!(t.contains("port = 8080"), "missing port");
    }
    #[test]
    fn toml_has_provider_profile() {
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic_provider(), mode: &solo_mode(),
            auth: &no_auth(), channel: &ChannelConfig::None,
        });
        assert!(t.contains("[[provider_profile]]"), "missing [[provider_profile]]");
        assert!(t.contains("anthropic_compatible"), "missing protocol");
        assert!(t.contains("anthropic-main"), "missing profile id");
    }
    #[test]
    fn toml_auth_section_when_token_set() {
        let auth = AuthConfig { ws_token: Some("my-secret".into()) };
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic_provider(), mode: &solo_mode(),
            auth: &auth, channel: &ChannelConfig::None,
        });
        assert!(t.contains("[auth]"), "missing [auth]");
        assert!(t.contains("my-secret"), "missing token");
    }
    #[test]
    fn toml_no_auth_section_when_empty() {
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic_provider(), mode: &solo_mode(),
            auth: &no_auth(), channel: &ChannelConfig::None,
        });
        assert!(!t.contains("[auth]"), "should not have [auth] when no token");
    }
    #[test]
    fn toml_lark_channel() {
        let lark = ChannelConfig::Lark(LarkCfg {
            app_id: "cli_abc".into(), app_secret: "sec".into(),
            verification_token: "tok".into(), bot_name: Some("AI".into()),
        });
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic_provider(), mode: &solo_mode(),
            auth: &no_auth(), channel: &lark,
        });
        assert!(t.contains("[channels.lark]"), "missing lark");
        assert!(t.contains("cli_abc"), "missing app_id");
    }
    #[test]
    fn toml_dingtalk_agent_id() {
        let dt = ChannelConfig::DingTalk(DingTalkCfg {
            client_id: "dingxxxx".into(), client_secret: "sec".into(),
            agent_id: Some(12345), bot_name: None,
        });
        let t = build_config_toml(&WriteInputs {
            provider: &anthropic_provider(), mode: &solo_mode(),
            auth: &no_auth(), channel: &dt,
        });
        assert!(t.contains("[channels.dingtalk]"), "missing dingtalk");
        assert!(t.contains("agent_id = 12345"), "missing agent_id");
    }
    #[test]
    fn env_file_anthropic() {
        let e = build_env_content(&anthropic_provider(), &ChannelConfig::None);
        assert!(e.contains("ANTHROPIC_API_KEY=sk-ant-test"));
    }
    #[test]
    fn env_file_lark_credentials() {
        let lark = ChannelConfig::Lark(LarkCfg {
            app_id: "cli_abc".into(), app_secret: "sec".into(),
            verification_token: "vtok".into(), bot_name: None,
        });
        let e = build_env_content(&anthropic_provider(), &lark);
        assert!(e.contains("LARK_APP_ID=cli_abc"));
        assert!(e.contains("LARK_VERIFICATION_TOKEN=vtok"));
    }
    #[test]
    fn deepseek_no_env_var_written_when_empty_key() {
        let provider = ProviderConfig {
            kind: ProviderKind::Ollama,
            api_key: String::new(),
            base_url: "http://localhost:11434".into(),
            model: "llama3".into(),
            profile_id: "ollama-main".into(),
        };
        let e = build_env_content(&provider, &ChannelConfig::None);
        // Ollama has empty env_var, nothing should be written
        assert!(!e.contains("=\n") && !e.contains("export ="), "empty env var written");
    }
}
```

- [ ] **3.5 运行 writer 测试**

```bash
cargo test -p qai-server setup 2>&1 | tail -20
```

预期：10 个测试 PASS。

---

## Task 4 — setup 向导主流程 + auth + config + serve + doctor + status + completions

**Files:** setup/mod.rs (rewrite), auth.rs, config_cmd.rs, serve.rs, doctor.rs, status.rs, completions.rs

### Steps

- [ ] **4.1 实现 `setup/mod.rs`（主流程）**

```rust
pub mod auth_cfg;
pub mod channel;
pub mod mode;
pub mod provider;
pub mod writer;

use crate::cli::{args::SetupArgs, i18n::{Language, Messages}};
use anyhow::Result;
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Select};

pub async fn run(args: SetupArgs) -> Result<()> {
    let theme = ColorfulTheme::default();

    // ── Step 1: 语言 ──────────────────────────────────────────
    let lang = if let Some(l) = &args.lang {
        Language::from_arg(Some(l))
    } else {
        let choices = ["中文", "English", "日本語", "한국어"];
        let idx = Select::with_theme(&theme)
            .with_prompt("请选择语言 / Select Language")
            .items(&choices).default(0).interact()?;
        match idx { 1 => Language::En, 2 => Language::Ja, 3 => Language::Ko, _ => Language::Zh }
    };
    let m = Messages::for_lang(lang);
    println!("\n{}\n", style(m.welcome).bold().cyan());

    // ── Step 2: 检查已有配置 ──────────────────────────────────
    let config_path = writer::config_path();
    if config_path.exists() && !args.reinit {
        let overwrite = if args.non_interactive { true } else {
            Confirm::with_theme(&theme).with_prompt(m.confirm_write).default(true).interact()?
        };
        if !overwrite {
            println!("已取消。重新配置请运行：quickai setup --reinit");
            return Ok(());
        }
    }

    // ── Step 3: Provider + API Key ────────────────────────────
    let provider_cfg = provider::collect(&args, lang)?;

    // ── Step 4: 运行模式 + 端口 + 工作目录 ──────────────────
    let mode_cfg = mode::collect(&args, lang)?;

    // ── Step 5: Auth（ws_token）──────────────────────────────
    let auth_cfg = auth_cfg::collect(&args, lang)?;

    // ── Step 6: Channel（可选）───────────────────────────────
    let channel_cfg = if args.non_interactive {
        channel::ChannelConfig::None
    } else {
        channel::collect(lang)?
    };

    // ── Step 7: 创建运行时目录 ────────────────────────────────
    let qdir = dirs::home_dir().unwrap_or_default().join(".quickai");
    for sub in ["sessions", "shared", "skills", "personas"] {
        std::fs::create_dir_all(qdir.join(sub))?;
    }

    // ── Step 8: 写入文件 ──────────────────────────────────────
    let inputs = writer::WriteInputs {
        provider: &provider_cfg,
        mode: &mode_cfg,
        auth: &auth_cfg,
        channel: &channel_cfg,
    };

    let backup = writer::write_config(&inputs)?;
    if let Some(bak) = &backup {
        println!("{} ({})", style(m.backed_up).dim(), bak.display());
    }
    println!("{}", style(m.written_config).green());

    writer::write_env(&provider_cfg, &channel_cfg)?;
    println!("{}", style(m.written_env).green());

    // ── Step 9: 完成 ─────────────────────────────────────────
    println!("\n{}", style(m.done).bold().green());
    println!("\n{}", m.next_steps);
    Ok(())
}
```

- [ ] **4.2 实现 `auth.rs`**

```rust
use crate::cli::args::{AuthArgs, AuthCommands};
use anyhow::Result;
use console::style;
use std::collections::HashMap;

pub async fn run(args: AuthArgs) -> Result<()> {
    match args.command {
        AuthCommands::Set { provider, key } => cmd_set(&provider, &key),
        AuthCommands::List => cmd_list(),
        AuthCommands::Check => cmd_check().await,
    }
}

fn env_path() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_default().join(".quickai").join(".env")
}

fn load_env_map() -> HashMap<String, String> {
    let Ok(content) = std::fs::read_to_string(env_path()) else { return HashMap::new() };
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

fn save_env_map(map: &HashMap<String, String>) -> Result<()> {
    let path = env_path();
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    let content = map.iter()
        .map(|(k, v)| format!("export {}={}", k, v))
        .collect::<Vec<_>>()
        .join("\n") + "\n";
    std::fs::write(&path, content)?;
    Ok(())
}

fn provider_env_var(provider: &str) -> &'static str {
    match provider.to_lowercase().as_str() {
        "anthropic" | "claude"        => "ANTHROPIC_API_KEY",
        "openai" | "gpt"              => "OPENAI_API_KEY",
        "deepseek"                    => "OPENAI_API_KEY",
        "azure"                       => "OPENAI_API_KEY",
        "ollama"                      => "",
        _                             => "OPENAI_API_KEY",
    }
}

fn cmd_set(provider: &str, key: &str) -> Result<()> {
    let var = provider_env_var(provider);
    if var.is_empty() {
        println!("{} Ollama 通常不需要 API Key", style("ℹ").cyan());
        return Ok(());
    }
    let mut map = load_env_map();
    map.insert(var.to_string(), key.to_string());
    save_env_map(&map)?;
    println!("{} {} 已更新 (~/.quickai/.env)", style("✓").green(), var);
    println!("  生效命令：source ~/.quickai/.env");
    Ok(())
}

fn cmd_list() -> Result<()> {
    println!("{}", style("已配置的 API Key：").bold());
    let map = load_env_map();
    let keys = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "LARK_APP_ID", "DINGTALK_APP_KEY"];
    let mut any = false;
    for var in keys {
        if let Some(val) = map.get(var) {
            let masked = mask_key(val);
            println!("  {} {} = {}", style("✓").green(), var, masked);
            any = true;
        }
    }
    if !any {
        println!("  {} 未找到任何 API Key（~/.quickai/.env 不存在或为空）", style("–").yellow());
        println!("  运行 quickai auth set anthropic <key> 设置");
    }
    Ok(())
}

async fn cmd_check() -> Result<()> {
    println!("{}", style("检查 API Key 有效性…").bold());
    let map = load_env_map();

    // Anthropic
    if let Some(key) = map.get("ANTHROPIC_API_KEY") {
        let ok = check_anthropic(key).await;
        let icon = if ok { style("✓").green() } else { style("✗").red() };
        println!("  {} Anthropic: {}", icon, if ok { "有效" } else { "无效或网络问题" });
    }

    // OpenAI
    if let Some(key) = map.get("OPENAI_API_KEY") {
        let ok = check_openai(key).await;
        let icon = if ok { style("✓").green() } else { style("✗").red() };
        println!("  {} OpenAI: {}", icon, if ok { "有效" } else { "无效或网络问题" });
    }

    Ok(())
}

async fn check_anthropic(key: &str) -> bool {
    let client = reqwest::Client::new();
    client.get("https://api.anthropic.com/v1/models")
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .send().await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn check_openai(key: &str) -> bool {
    let client = reqwest::Client::new();
    client.get("https://api.openai.com/v1/models")
        .bearer_auth(key)
        .send().await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn mask_key(val: &str) -> String {
    if val.len() <= 8 { return "****".into(); }
    format!("{}…{}", &val[..6], &val[val.len()-3..])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mask_short_key() { assert_eq!(mask_key("abc"), "****"); }
    #[test]
    fn mask_long_key() {
        let m = mask_key("sk-ant-api03-abc123xyz");
        assert!(m.starts_with("sk-ant"), "should start with prefix: {m}");
        assert!(m.contains("…"), "should have ellipsis: {m}");
    }
    #[test]
    fn provider_env_var_anthropic() { assert_eq!(provider_env_var("anthropic"), "ANTHROPIC_API_KEY"); }
    #[test]
    fn provider_env_var_deepseek_uses_openai() { assert_eq!(provider_env_var("deepseek"), "OPENAI_API_KEY"); }
    #[test]
    fn provider_env_var_ollama_empty() { assert_eq!(provider_env_var("ollama"), ""); }
}
```

- [ ] **4.3 实现 `config_cmd.rs`**

```rust
use crate::cli::args::{ConfigArgs, ConfigCommands};
use anyhow::{Context, Result};
use console::style;

pub async fn run(args: ConfigArgs) -> Result<()> {
    match args.command {
        ConfigCommands::Show     => cmd_show(),
        ConfigCommands::Validate => cmd_validate(),
        ConfigCommands::Edit     => cmd_edit(),
    }
}

fn config_path() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_default().join(".quickai").join("config.toml")
}

fn cmd_show() -> Result<()> {
    let path = config_path();
    if !path.exists() {
        println!("{} config.toml 不存在 — 请先运行 quickai setup", style("✗").red());
        return Ok(());
    }
    let content = std::fs::read_to_string(&path)?;
    // 脱敏：隐藏包含 secret/token/key/password 的值
    let redacted = redact_secrets(&content);
    println!("{}", redacted);
    Ok(())
}

fn redact_secrets(toml: &str) -> String {
    toml.lines().map(|line| {
        let lower = line.to_lowercase();
        if (lower.contains("secret") || lower.contains("token") || lower.contains("password"))
            && line.contains('=')
        {
            if let Some(eq) = line.find('=') {
                return format!("{} = \"<redacted>\"", &line[..eq].trim());
            }
        }
        line.to_string()
    }).collect::<Vec<_>>().join("\n")
}

fn cmd_validate() -> Result<()> {
    let path = config_path();
    if !path.exists() {
        anyhow::bail!("config.toml 不存在：{}", path.display());
    }
    // TOML 语法检查
    let content = std::fs::read_to_string(&path)?;
    let _: toml::Value = toml::from_str(&content)
        .context("TOML 语法错误")?;
    // 拓扑检查（加载并调用 validate_runtime_topology）
    let cfg = qai_server::config::GatewayConfig::load()
        .context("配置加载失败")?;
    cfg.validate_runtime_topology()
        .context("拓扑校验失败")?;
    println!("{} 配置语法和拓扑均正常", style("✓").green());
    Ok(())
}

fn cmd_edit() -> Result<()> {
    let path = config_path();
    if !path.exists() {
        println!("{} config.toml 不存在，请先运行 quickai setup", style("✗").red());
        return Ok(());
    }
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("无法启动编辑器 {:?}", editor))?;
    if !status.success() {
        anyhow::bail!("编辑器退出码: {:?}", status.code());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redact_removes_secret_values() {
        let toml = "app_secret = \"real-secret-value\"\nport = 8080\ntoken = \"abc123\"";
        let out = redact_secrets(toml);
        assert!(!out.contains("real-secret-value"), "secret not redacted: {out}");
        assert!(out.contains("port = 8080"), "port should be preserved: {out}");
        assert!(!out.contains("abc123"), "token not redacted: {out}");
    }
    #[test]
    fn redact_preserves_non_secret_lines() {
        let toml = "[gateway]\nhost = \"127.0.0.1\"\nport = 8080";
        let out = redact_secrets(toml);
        assert_eq!(out, toml);
    }
}
```

- [ ] **4.4 实现 `serve.rs`（exec 模式）**

```rust
use crate::cli::args::ServeArgs;
use anyhow::{Context, Result};

pub async fn run(args: ServeArgs) -> Result<()> {
    // 确定配置路径
    let config_path = args.config.unwrap_or_else(|| {
        dirs::home_dir().unwrap_or_default().join(".quickai").join("config.toml")
    });
    if !config_path.exists() {
        anyhow::bail!(
            "配置文件不存在: {}\n请先运行: quickai setup",
            config_path.display()
        );
    }

    // 加载 .env（当前进程，然后 exec 子进程继承）
    load_dot_env();

    // 找到 quickai-gateway binary
    let gateway_bin = which::which("quickai-gateway")
        .context("找不到 quickai-gateway，请确认已安装并在 PATH 中")?;

    // 设置环境变量传递给子进程
    std::env::set_var("QUICKAI_CONFIG", config_path.to_string_lossy().as_ref());
    if let Some(port) = args.port {
        std::env::set_var("QUICKAI_PORT", port.to_string());
    }

    // exec 替换当前进程（Unix）
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&gateway_bin).exec();
        return Err(anyhow::anyhow!("exec failed: {err}"));
    }

    // Windows fallback
    #[cfg(not(unix))]
    {
        let status = std::process::Command::new(&gateway_bin)
            .status()
            .context("quickai-gateway 启动失败")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn load_dot_env() {
    let env_path = dirs::home_dir().unwrap_or_default().join(".quickai").join(".env");
    let Ok(content) = std::fs::read_to_string(&env_path) else { return };
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() { continue; }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=') {
            if std::env::var(k.trim()).is_err() {
                std::env::set_var(k.trim(), v.trim());
            }
        }
    }
}
```

- [ ] **4.5 实现 `doctor.rs`**

```rust
use anyhow::Result;
use console::style;

pub async fn run() -> Result<()> {
    println!("{}", style("QuickAI Doctor").bold().cyan());
    println!("{}", "─".repeat(40));
    let mut issues = 0usize;

    // 1. Binary
    println!("\n[1] Binary");
    for bin in ["quickai-gateway", "quickai-rust-agent"] {
        match which::which(bin) {
            Ok(p) => println!("  {} {} ({})", style("✓").green(), bin, p.display()),
            Err(_) => { println!("  {} {} 未找到 — 请检查 PATH 或重新编译", style("✗").red(), bin); issues += 1; }
        }
    }

    // 2. Config
    println!("\n[2] 配置文件");
    let cfg_path = dirs::home_dir().unwrap_or_default().join(".quickai").join("config.toml");
    if cfg_path.exists() {
        let content = std::fs::read_to_string(&cfg_path).unwrap_or_default();
        match toml::from_str::<toml::Value>(&content) {
            Ok(_) => println!("  {} ~/.quickai/config.toml (TOML 语法正常)", style("✓").green()),
            Err(e) => { println!("  {} config.toml 语法错误: {e}", style("✗").red()); issues += 1; }
        }
    } else {
        println!("  {} config.toml 不存在 — 请运行: quickai setup", style("✗").red());
        issues += 1;
    }

    // 3. API Key
    println!("\n[3] API Key");
    let env_path = dirs::home_dir().unwrap_or_default().join(".quickai").join(".env");
    if env_path.exists() {
        println!("  {} ~/.quickai/.env 存在", style("✓").green());
    } else {
        println!("  {} ~/.quickai/.env 不存在", style("–").yellow());
    }
    for var in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"] {
        match std::env::var(var) {
            Ok(v) if !v.is_empty() => println!("  {} {} 已设置（当前 shell）", style("✓").green(), var),
            _ => println!("  {} {} 未设置（运行 source ~/.quickai/.env）", style("–").yellow(), var),
        }
    }

    // 4. 运行时目录
    println!("\n[4] 运行时目录");
    for sub in ["sessions", "shared", "skills"] {
        let p = dirs::home_dir().unwrap_or_default().join(".quickai").join(sub);
        if p.exists() {
            println!("  {} ~/.quickai/{}", style("✓").green(), sub);
        } else {
            println!("  {} ~/.quickai/{} 不存在 (mkdir -p ~/.quickai/{})", style("✗").red(), sub, sub);
            issues += 1;
        }
    }

    // 5. Gateway 进程
    println!("\n[5] Gateway");
    let port_file = dirs::home_dir().unwrap_or_default().join(".quickai").join("gateway.port");
    if port_file.exists() {
        let port = std::fs::read_to_string(&port_file).unwrap_or_default();
        println!("  {} 运行中 (port: {})", style("✓").green(), port.trim());
    } else {
        println!("  {} 未运行 (gateway.port 不存在)", style("–").yellow());
    }

    // 结果
    println!("\n{}", "─".repeat(40));
    if issues == 0 {
        println!("{}", style("✓ 所有检查通过").bold().green());
    } else {
        println!("{}", style(format!("发现 {issues} 个问题，请按上方提示修复")).bold().yellow());
    }
    Ok(())
}
```

- [ ] **4.6 实现 `status.rs`**

```rust
use anyhow::Result;
use console::style;

pub async fn run() -> Result<()> {
    let cfg_path = dirs::home_dir().unwrap_or_default().join(".quickai").join("config.toml");
    println!("{}", style("QuickAI Gateway — 配置摘要").bold().cyan());
    println!("{}", "─".repeat(40));
    if !cfg_path.exists() {
        println!("{} config.toml 不存在 — 请先运行: quickai setup", style("⚠").yellow());
        return Ok(());
    }
    let content = std::fs::read_to_string(&cfg_path)?;
    let val: toml::Value = toml::from_str(&content)
        .unwrap_or(toml::Value::Table(toml::map::Map::new()));

    let port = val.get("gateway").and_then(|g| g.get("port"))
        .and_then(|p| p.as_integer()).unwrap_or(0);
    println!("  端口        {}", if port == 0 { "随机".into() } else { port.to_string() });

    let roster_n = val.get("agent_roster").and_then(|r| r.as_array()).map(|a| a.len()).unwrap_or(0);
    let mode_str = if roster_n > 0 { format!("Multi-agent ({} agents)", roster_n) } else { "Solo".into() };
    println!("  模式        {}", mode_str);

    let backends: Vec<&str> = val.get("backend").and_then(|b| b.as_array())
        .map(|arr| arr.iter().filter_map(|b| b.get("id").and_then(|id| id.as_str())).collect())
        .unwrap_or_default();
    println!("  Backends    {}", if backends.is_empty() { "（未配置）".into() } else { backends.join(", ") });

    let provider_profiles: Vec<&str> = val.get("provider_profile").and_then(|p| p.as_array())
        .map(|arr| arr.iter().filter_map(|p| p.get("id").and_then(|id| id.as_str())).collect())
        .unwrap_or_default();
    println!("  Providers   {}", if provider_profiles.is_empty() { "（未配置）".into() } else { provider_profiles.join(", ") });

    let lark = val.get("channels").and_then(|c| c.get("lark")).is_some();
    let dt   = val.get("channels").and_then(|c| c.get("dingtalk")).is_some();
    let ch_str = match (lark, dt) {
        (true, true)  => "Lark + DingTalk",
        (true, false) => "Lark",
        (false, true) => "DingTalk",
        _             => "WebSocket only",
    };
    println!("  Channel     {}", ch_str);

    let has_auth = val.get("auth").and_then(|a| a.get("ws_token"))
        .and_then(|t| t.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
    println!("  WS Auth     {}", if has_auth { style("已启用").green().to_string() } else { "开放模式（无 token）".into() });

    let has_key = std::env::var("ANTHROPIC_API_KEY").or_else(|_| std::env::var("OPENAI_API_KEY")).is_ok();
    println!("  API Key     {}", if has_key { style("已设置").green().to_string() } else { style("未设置（source ~/.quickai/.env）").yellow().to_string() });

    let port_file = dirs::home_dir().unwrap_or_default().join(".quickai").join("gateway.port");
    println!("  Gateway     {}", if port_file.exists() { style("运行中").green().to_string() } else { style("未运行").dim().to_string() });

    println!("\n配置文件: {}", cfg_path.display());
    Ok(())
}
```

- [ ] **4.7 实现 `completions.rs`**

```rust
use crate::cli::args::{Cli, CompletionsArgs, ShellArg};
use anyhow::Result;
use clap::CommandFactory;
use clap_complete::{generate, shells};

pub fn run(args: CompletionsArgs) -> Result<()> {
    let mut cmd = Cli::command();
    let bin_name = "quickai";
    match args.shell {
        ShellArg::Bash       => generate(shells::Bash,       &mut cmd, bin_name, &mut std::io::stdout()),
        ShellArg::Zsh        => generate(shells::Zsh,        &mut cmd, bin_name, &mut std::io::stdout()),
        ShellArg::Fish       => generate(shells::Fish,       &mut cmd, bin_name, &mut std::io::stdout()),
        ShellArg::PowerShell => generate(shells::PowerShell, &mut cmd, bin_name, &mut std::io::stdout()),
    }
    Ok(())
}
```

注意：需要在 `Cargo.toml` 中添加 `clap_complete = "4"` 依赖。

- [ ] **4.8 添加 clap_complete 依赖**

在 workspace `Cargo.toml`：
```toml
clap_complete = "4"
```
在 `qai-server/Cargo.toml`：
```toml
clap_complete.workspace = true
```

- [ ] **4.9 全量编译**

```bash
cargo build -p qai-server --bin quickai 2>&1 | grep -E "^error" | head -30
```

预期：0 errors。

- [ ] **4.10 全量测试**

```bash
cargo test -p qai-server -- cli 2>&1 | tail -30
```

预期：所有 cli 测试 PASS。

- [ ] **4.11 手动验收**

```bash
# help
./target/debug/quickai --help
./target/debug/quickai setup --help
./target/debug/quickai auth --help
./target/debug/quickai config --help

# doctor（无需 TTY）
./target/debug/quickai doctor

# status
./target/debug/quickai status

# auth list
./target/debug/quickai auth list

# 非交互 setup（端到端）
./target/debug/quickai setup \
  --lang zh --provider anthropic \
  --api-key "sk-ant-test" \
  --mode solo --non-interactive

cat ~/.quickai/config.toml
cat ~/.quickai/.env

# completions
./target/debug/quickai completions zsh | head -5

# config show（脱敏验证）
./target/debug/quickai config show

# config validate
./target/debug/quickai config validate
```

预期：
- setup 生成包含 `[[provider_profile]]`、`[[backend]]`、`[gateway]` 的配置
- `.env` 包含 `ANTHROPIC_API_KEY=sk-ant-test`
- `config show` 不显示 api_key 原文
- `completions zsh` 输出 zsh 补全脚本开头
