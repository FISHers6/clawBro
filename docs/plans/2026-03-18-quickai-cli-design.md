# ClawBro CLI 设计方案

> 日期：2026-03-18
> 参考：zeroclaw CLI 架构研究（`docs/research/zeroclaw-cli-architecture-research.md`）、nanoclaw Skills 研究

---

## 背景与目标

ClawBro Gateway 需要一个面向终端用户的 CLI，解决 onboarding 问题：
- 用户不知道如何创建 `~/.clawbro/config.toml`
- 手动编辑 TOML 容易出错且不友好
- 添加新功能（Channel、Agent、Backend）需要了解配置结构

**双轨并行**：
1. **CLI 命令**（非交互式/脚本友好）：`clawbro add channel lark`
2. **Claude Code Skills**（AI 引导式）：`/add-lark`（调用本文定义的 Skill）

两者生成**相同的配置结果**，用户可以选择喜欢的方式。

---

## CLI 名称与 Binary 策略

### Binary 名称

```
clawbro          — 主 CLI（配置管理 + 状态查询）
clawbro-gateway  — 纯服务端（保持现有行为，只负责 serve）
```

> 参考 zeroclaw：`zeroclaw` 一个 binary 搞定所有事（init/add/serve/status）。
> clawbro 也应该一个入口覆盖所有，`clawbro serve` 等同于 `clawbro-gateway`。

### 放哪里

选项 A（推荐）：在 `clawbro-server` crate 增加一个新 `[[bin]]`：
```toml
[[bin]]
name = "clawbro"
path = "src/bin/clawbro_cli.rs"
```

选项 B：新建 `crates/clawbro-cli` crate，专门承载 CLI 逻辑。
- 好处：职责分离，`clawbro-server` 不膨胀
- 坏处：增加一个 crate

**建议选 A（初期），代码量增大后再分离**。

---

## 命令树（完整设计）

```
clawbro
├── serve [--config <path>] [--port <port>]
│                     启动 Gateway 服务（等同于直接运行 clawbro-gateway）
│
├── init              交互式初始化向导（等同于 /setup skill 的 CLI 版本）
│   [--mode solo|multi|team]
│   [--provider anthropic|openai|deepseek]
│   [--non-interactive]  非交互模式，从参数直接生成配置
│
├── config
│   ├── show          打印当前配置（脱敏）
│   ├── validate      验证 config.toml 语法和完整性
│   ├── set <key> <value>    设置单个配置项
│   └── edit          用 $EDITOR 打开配置文件
│
├── add
│   ├── channel
│   │   ├── lark      添加飞书 Channel（等同于 /add-lark）
│   │   └── dingtalk  添加钉钉 Channel（等同于 /add-dingtalk）
│   │
│   ├── backend <id>  添加 ACP Backend（等同于 /add-acp-backend）
│   │   [--type acp|native]
│   │   [--command <cmd>]
│   │   [--args <args...>]
│   │
│   ├── agent <name>  向 agent_roster 追加 Agent（等同于 /add-agent）
│   │   [--backend <id>]
│   │   [--mention <mention>]
│   │   [--workspace <dir>]
│   │   [--persona <dir>]
│   │
│   └── team-mode     为 scope 启用 Team 模式（等同于 /add-team-mode）
│       [--scope <scope>]
│       [--lead <agent>]
│       [--specialists <a,b,c>]
│
├── remove
│   ├── channel <lark|dingtalk>
│   ├── backend <id>
│   └── agent <name>
│
├── list
│   ├── agents        列出所有 agent_roster
│   ├── backends      列出所有 backend
│   ├── channels      列出已配置的 channel
│   └── bindings      列出所有 binding
│
├── status            显示 Gateway 整体状态（进程、端口、配置摘要）
│
├── doctor            诊断配置和运行问题（等同于 /doctor skill）
│   [--fix]           自动修复可以修复的问题
│
├── key
│   ├── set <provider> <key>   设置 API Key 到 ~/.clawbro/.env
│   └── check                 检查所有 API Key 是否有效
│
└── team              Team 模式运维命令（需要 Gateway 正在运行）
    ├── status [--scope <scope>]  显示 team 任务状态
    ├── tasks [--scope <scope>]   列出所有任务
    └── cancel <task-id>          取消任务
```

---

## 与 Skills 的关系

| 操作 | CLI 命令 | Claude Code Skill | 结果 |
|------|---------|-----------------|------|
| 初始化 | `clawbro init` | `/setup` | 生成 `~/.clawbro/config.toml` |
| 添加飞书 | `clawbro add channel lark` | `/add-lark` | 追加 `[channels.lark]` |
| 添加钉钉 | `clawbro add channel dingtalk` | `/add-dingtalk` | 追加 `[channels.dingtalk]` |
| 添加 Backend | `clawbro add backend codex` | `/add-acp-backend` | 追加 `[[backend]]` |
| 添加 Agent | `clawbro add agent rex` | `/add-agent` | 追加 `[[agent_roster]]` |
| 开启 Team | `clawbro add team-mode` | `/add-team-mode` | 追加 `[[group]]` |
| 诊断 | `clawbro doctor` | `/doctor` | 检查报告 |

**Skills 是 AI 引导版 CLI**：
- 有更多解释、提示、上下文帮助
- 适合第一次配置、不熟悉配置结构的用户
- 在 Claude Code 环境中运行

**CLI 是脚本/自动化版 Skills**：
- 无需 Claude Code
- 支持非交互模式（`--non-interactive`）
- 适合 CI/CD、Dockerfile、自动化部署

---

## 实现优先级

### P0（MVP，onboarding 必需）

1. `clawbro init` — 最重要，解决"第一次怎么用"问题
2. `clawbro serve` — 等同于 clawbro-gateway，统一入口
3. `clawbro doctor` — 诊断问题
4. `clawbro key set` — 设置 API Key

### P1（常用功能）

5. `clawbro add channel lark`
6. `clawbro add channel dingtalk`
7. `clawbro config show / validate`
8. `clawbro status`
9. `clawbro list agents|backends`

### P2（进阶功能）

10. `clawbro add backend <type>`
11. `clawbro add agent <name>`
12. `clawbro add team-mode`
13. `clawbro team status/tasks`
14. `clawbro remove *`

---

## 技术实现

### 依赖

```toml
# 在 clawbro-server/Cargo.toml 中追加
clap = { version = "4", features = ["derive", "env", "color"] }
dialoguer = "0.11"        # 交互式提示（select/input/confirm）
indicatif = "0.17"        # 进度条
console = "0.15"          # 终端颜色和格式化
toml_edit = "0.22"        # 无损 TOML 编辑（保留注释和格式）
```

> 关键：用 `toml_edit`（不是 `toml`）修改 config.toml，这样可以保留用户的注释和格式。

### 代码结构

```
crates/clawbro-server/src/bin/clawbro_cli.rs   — bin 入口
crates/clawbro-server/src/cli/
  mod.rs          — CLI 模块导出
  args.rs         — clap Args 结构（derive 宏定义所有命令）
  init.rs         — clawbro init 实现
  add/
    channel_lark.rs
    channel_dingtalk.rs
    backend.rs
    agent.rs
    team_mode.rs
  config.rs       — clawbro config show/validate/set
  doctor.rs       — clawbro doctor
  key.rs          — clawbro key set/check
  status.rs       — clawbro status
  team.rs         — clawbro team status/tasks
  serve.rs        — clawbro serve（调用现有 main 逻辑）
  utils/
    config_editor.rs  — toml_edit 包装，读写 config.toml
    env_editor.rs     — 读写 ~/.clawbro/.env
    terminal.rs       — 颜色、表格、spinners
```

### Args 定义示例（clap derive）

```rust
// args.rs
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "clawbro", about = "ClawBro Gateway CLI", version)]
pub struct Cli {
    #[arg(short, long, global = true, help = "配置文件路径")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 启动 Gateway 服务
    Serve {
        #[arg(short, long)]
        port: Option<u16>,
    },
    /// 交互式初始化向导
    Init {
        #[arg(long, value_enum)]
        mode: Option<InitMode>,
        #[arg(long)]
        non_interactive: bool,
    },
    /// 配置管理
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// 添加组件
    Add {
        #[command(subcommand)]
        command: AddCommands,
    },
    /// 诊断配置和运行问题
    Doctor {
        #[arg(long, help = "自动修复可修复的问题")]
        fix: bool,
    },
    /// API Key 管理
    Key {
        #[command(subcommand)]
        command: KeyCommands,
    },
    /// 查看当前状态
    Status,
    /// 列出配置的组件
    List {
        #[command(subcommand)]
        command: ListCommands,
    },
    /// Team 模式运维
    Team {
        #[command(subcommand)]
        command: TeamCommands,
    },
}

// ... 其他子命令定义
```

### config_editor.rs 关键设计

```rust
// 使用 toml_edit 无损编辑
use toml_edit::{Document, value, table, array};

pub struct ConfigEditor {
    doc: Document,
    path: PathBuf,
}

impl ConfigEditor {
    pub fn load() -> anyhow::Result<Self> { ... }
    pub fn save(&self) -> anyhow::Result<()> { /* 备份 + 写入 */ }
    pub fn append_lark_channel(&mut self, cfg: &LarkConfig) -> anyhow::Result<()> { ... }
    pub fn append_backend(&mut self, cfg: &BackendConfig) -> anyhow::Result<()> { ... }
    pub fn append_agent_roster(&mut self, cfg: &AgentConfig) -> anyhow::Result<()> { ... }
    pub fn append_group(&mut self, cfg: &GroupConfig) -> anyhow::Result<()> { ... }
}
```

---

## `clawbro init` 交互流程（CLI 版）

```
$ clawbro init

  ██████  ██    ██ ██  ██████ ██   ██  █████  ██
 ██    ██ ██    ██ ██ ██      ██  ██  ██   ██ ██
 ██    ██ ██    ██ ██ ██      █████   ███████ ██
 ██ ▄▄ ██ ██    ██ ██ ██      ██  ██  ██   ██ ██
  ██████   ██████  ██  ██████ ██   ██ ██   ██ ██

  ClawBro Gateway — 初始化向导

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

✓ 已找到 clawbro-gateway
✓ 已找到 clawbro-rust-agent

? 选择 AI Provider: ›
  ❯ Anthropic（Claude）
    OpenAI（GPT）
    DeepSeek
    其他 OpenAI 兼容端点

? 输入 ANTHROPIC_API_KEY: › sk-ant-***（输入时隐藏）

✓ API Key 已写入 ~/.clawbro/.env

? 选择运行模式: ›
  ❯ Solo（单 Agent，适合个人使用）
    Multi-agent（多 Agent 切换）
    Team（Lead + Specialists 协作）

? 监听端口 [8080]: ›

? 工作目录（留空使用当前目录）: ›

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  配置预览

  [gateway]
  host = "127.0.0.1"
  port = 8080

  [agent]
  backend_id = "native-main"
  ...

? 写入 ~/.clawbro/config.toml？ › (Y/n)

✓ 配置已写入！

? 接入 IM Channel？ ›
  ❯ 暂不接入，只用 WebSocket
    飞书（运行 clawbro add channel lark）
    钉钉（运行 clawbro add channel dingtalk）

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  🎉 初始化完成！

  启动命令：source ~/.clawbro/.env && clawbro serve
  添加飞书：clawbro add channel lark
  诊断问题：clawbro doctor
```

---

## 非交互模式（CI/Docker）

```bash
# 一行完成初始化（适合 Dockerfile）
clawbro init \
  --non-interactive \
  --mode solo \
  --provider anthropic

# 添加飞书（非交互，所有参数通过 flag 提供）
clawbro add channel lark \
  --app-id cli_xxxx \
  --app-secret xxxx \
  --verification-token xxxx

# 设置 API Key（非交互）
clawbro key set anthropic sk-ant-xxxx
```

---

## 安装方式（面向用户）

### 方式 1：从 GitHub Releases 下载（推荐）

```bash
# macOS
curl -L https://github.com/fishers/clawbro-openclaw/releases/latest/download/clawbro-darwin-aarch64.tar.gz | tar xz
mv clawbro ~/.local/bin/
mv clawbro-gateway ~/.local/bin/
mv clawbro-rust-agent ~/.local/bin/
```

### 方式 2：从源码编译

```bash
git clone https://github.com/fishers/clawbro-openclaw
cd clawbro-openclaw/clawbro-gateway
cargo build -r -p clawbro-server --bin clawbro
cargo build -r -p clawbro-server --bin clawbro-gateway
cargo build -r -p clawbro-rust-agent
```

### 方式 3：Claude Code Skills 用户

无需特别安装，克隆仓库 + cargo build 后，用 `/setup` skill 引导配置即可。

---

## Release 打包策略

一个 Release 包含三个 binary：

```
clawbro-gateway-v0.x.x-darwin-aarch64.tar.gz
  ├── clawbro           — 主 CLI（P0 实现后）
  ├── clawbro-gateway   — 服务端（现有）
  └── clawbro-rust-agent — 内置 ACP Agent（现有）
```

GitHub Actions workflow 目标平台：
- `aarch64-apple-darwin`（macOS M 系列）
- `x86_64-apple-darwin`（macOS Intel）
- `x86_64-unknown-linux-musl`（Linux，静态链接）
- `aarch64-unknown-linux-musl`（Linux ARM，树莓派等）

---

## 实现计划

### Task 1：骨架 + serve 子命令（0.5 天）
- `clawbro-server/src/bin/clawbro_cli.rs`
- `clap` Args 定义（Commands 枚举）
- `serve` 子命令复用 main 逻辑

### Task 2：config_editor + key 子命令（0.5 天）
- `toml_edit` 包装
- `clawbro key set <provider> <key>`
- `clawbro config show / validate`

### Task 3：init 交互向导（1 天）
- `dialoguer` 交互提示
- Solo / Multi-agent 配置生成
- 写入 config.toml

### Task 4：add channel lark / dingtalk（0.5 天）
- 复用 config_editor
- 交互收集 app_id / app_secret 等

### Task 5：add agent + add backend（0.5 天）

### Task 6：doctor（0.5 天）
- 复用已有的诊断逻辑

### Task 7：GitHub Actions CI + Release 打包（1 天）
- cross-compile workflow
- 打包 tar.gz / zip

**总估算：4-5 天工程量**
