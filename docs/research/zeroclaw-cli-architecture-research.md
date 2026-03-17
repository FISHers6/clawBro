# ZeroClaw CLI 架构研究报告

> 研究目的：学习 zeroclaw 的 CLI 设计、配置体系、Channel/Provider 支持，作为 quickai-gateway 二进制分发和用户体验设计的参考。
>
> 研究日期：2026-03-18
> 研究源码：`/Users/fishers/Desktop/repo/quickai-openclaw/zeroclaw`
> ZeroClaw 版本：0.4.3

---

## 一、整体架构

ZeroClaw 是一个 **单 crate，lib + multi-bin 设计**：

```
zeroclawlabs (crate 名)
├── [[bin]] zeroclaw       ← 主 CLI 入口 (src/main.rs，~2300 行)
├── [lib]   zeroclaw       ← 公开库 (src/lib.rs，~541 行)
└── workspace member: crates/robot-kit
```

用户安装：
```bash
cargo install zeroclawlabs   # 获得 zeroclaw 命令
```

CLI 框架：**clap 4.5（derive 宏）**，25+ 顶级子命令。

---

## 二、顶级命令树

| 命令 | 功能 |
|------|------|
| `onboard` | 初始化配置（交互/非交互） |
| `agent` | 启动 AI Agent 会话（交互/单次） |
| `gateway` | HTTP/WebSocket 网关管理 |
| `daemon` | 全功能守护进程（网关+所有 Channel+调度器）|
| `service` | OS 服务管理（systemd/launchd） |
| `doctor` | 诊断（模型探活、trace 查询）|
| `status` | 系统状态总览 |
| `estop` | 紧急停止（分级） |
| `cron` | 定时任务管理 |
| `models` | Provider 模型目录管理 |
| `providers` | 查看支持的 Provider 列表 |
| `channel` | Channel 管理（增删改查+发送测试消息）|
| `integrations` | 第三方集成（Composio，50+）|
| `skills` | Skills 包管理（npm 风格）|
| `migrate` | 从其他 Agent Runtime 迁移数据 |
| `auth` | Provider OAuth/API Key 认证管理 |
| `hardware` | USB 硬件发现和探测 |
| `peripheral` | 外设管理（GPIO、串口、STM32、RPi）|
| `memory` | 记忆管理（列出/清除/统计）|
| `config` | 配置管理（dump JSON Schema）|
| `completions` | Shell 补全脚本生成 |

---

## 三、CLI 设计细节

### 3.1 全局参数

```
zeroclaw [--config-dir <PATH>] <SUBCOMMAND>
```

`--config-dir` 覆盖配置目录，等价于设置 `ZEROCLAW_CONFIG_DIR` 环境变量。

### 3.2 `onboard` — 初始化

```bash
zeroclaw onboard                                          # 交互式向导（TTY）
zeroclaw onboard --api-key sk-... --provider openrouter  # 脚本/CI 快速模式
zeroclaw onboard --channels-only                         # 只重配 Channel
zeroclaw onboard --reinit                                # 备份并全部重置
zeroclaw onboard --force                                 # 强制覆盖不确认
```

参数：

| 参数 | 类型 | 说明 |
|------|------|------|
| `--api-key <KEY>` | String | Provider API Key |
| `--provider <NAME>` | String | Provider（默认 `openrouter`） |
| `--model <MODEL_ID>` | String | 模型 ID |
| `--memory <BACKEND>` | String | 记忆后端（sqlite/lucid/markdown/none，默认 sqlite）|
| `--force` | Flag | 跳过确认覆盖 |
| `--reinit` | Flag | 备份后全部重置 |
| `--channels-only` | Flag | 快速修复，只重配 Channel |

### 3.3 `agent` — 启动 Agent

```bash
zeroclaw agent                                    # 交互式会话
zeroclaw agent -m "总结今天的日志"                 # 单次消息模式
zeroclaw agent -p anthropic --model claude-sonnet-4-20250514
zeroclaw agent -t 0.3                             # 自定义温度
zeroclaw agent --peripheral nucleo-f401re:/dev/ttyACM0  # 挂硬件外设
```

参数：

| 参数 | 类型 | 说明 |
|------|------|------|
| `-m, --message <TEXT>` | String | 单次模式（不进入交互循环）|
| `--session-state-file <PATH>` | Path | 加载/保存会话状态 JSON |
| `-p, --provider <NAME>` | String | 覆盖 Provider |
| `--model <MODEL>` | String | 覆盖模型 |
| `-t, --temperature <FLOAT>` | f64 | 温度（0.0–2.0）|
| `--peripheral <BOARD:PATH>` | String | 挂载硬件外设 |

### 3.4 `gateway` — 网关管理

```bash
zeroclaw gateway start [--port 8080] [--host 0.0.0.0]
zeroclaw gateway restart
zeroclaw gateway get-paircode [--new]
```

默认端口：`42617`，默认 host：`127.0.0.1`。

### 3.5 `daemon` — 守护进程（生产模式）

```bash
zeroclaw daemon
zeroclaw daemon -p 9090
zeroclaw daemon --host 0.0.0.0
```

集成：网关 + 所有 Channel + Heartbeat + Cron 调度器。

### 3.6 `estop` — 紧急停止

```bash
zeroclaw estop                                              # 默认 kill-all
zeroclaw estop --level domain-block --domain "*.chase.com"
zeroclaw estop --level tool-freeze --tool shell --tool browser
zeroclaw estop status
zeroclaw estop resume --network
zeroclaw estop resume --domain "*.sensitive.com"
zeroclaw estop resume --tool shell --otp 123456
```

级别：`kill-all` | `network-kill` | `domain-block` | `tool-freeze`

### 3.7 `cron` — 定时任务

```bash
zeroclaw cron list
zeroclaw cron add '0 9 * * 1-5' '早报' --tz Asia/Shanghai --agent
zeroclaw cron add '*/30 * * * *' '健康检查' --agent
zeroclaw cron add-at 2026-04-01T09:00:00Z '愚人节消息' --agent
zeroclaw cron add-every 60000 '心跳检测'
zeroclaw cron once 30m '30分钟后提醒' --agent
zeroclaw cron pause <task-id>
zeroclaw cron update <task-id> --expression '0 8 * * *' --tz Europe/London
```

时区：IANA 时区字符串，默认 UTC。

### 3.8 `auth` — 认证管理

```bash
zeroclaw auth login --provider openai-codex --device-code
zeroclaw auth paste-redirect --provider openai-codex --input <URL>
zeroclaw auth paste-token --provider anthropic --token <TOKEN>
zeroclaw auth list
zeroclaw auth status
zeroclaw auth use --provider openai-codex --profile main
zeroclaw auth logout --provider openai-codex
zeroclaw auth refresh --provider openai-codex
```

### 3.9 `completions` — Shell 补全

```bash
source <(zeroclaw completions bash)
zeroclaw completions zsh > ~/.zfunc/_zeroclaw
zeroclaw completions fish > ~/.config/fish/completions/zeroclaw.fish
```

支持：bash、fish、zsh、powershell、elvish。

---

## 四、配置文件体系

### 4.1 配置文件路径解析顺序

1. `ZEROCLAW_CONFIG_DIR` 环境变量
2. `ZEROCLAW_WORKSPACE` 环境变量
3. `~/.zeroclaw/active_workspace.toml` 中的激活工作区
4. 默认：`~/.zeroclaw/config.toml`

### 4.2 API Key 优先级（最高到最低）

1. `ZEROCLAW_API_KEY`（最高）
2. `API_KEY`
3. Provider 专属环境变量（如 `ANTHROPIC_API_KEY`、`OPENAI_API_KEY`）
4. config.toml 中 `api_key = "..."` 字段
5. 命名 Profile 的 key

### 4.3 顶层配置字段（核心）

```toml
# Provider & Model
api_key = "..."                         # 全局 API Key
api_url = "..."                         # 自定义 Base URL（Ollama 远端等）
api_path = "/v1/chat/completions"       # 自定义 Path
default_provider = "openrouter"         # 默认 Provider
default_model = "anthropic/claude-sonnet-4-6"
default_temperature = 0.7               # 0.0–2.0
provider_timeout_secs = 120             # HTTP 超时

# 命名 Provider Profile（Codex app-server 兼容格式）
[model_providers."my-profile"]
name = "openai"
base_url = "https://api.openai.com"
api_key = "sk-..."
model = "gpt-4"

# 模型路由提示
[[model_routes]]
hint = "fast"
provider = "groq"
model = "mixtral-8x7b-32768"

[[model_routes]]
hint = "vision"
provider = "openai"
model = "gpt-4-vision"
```

### 4.4 Gateway 配置

```toml
[gateway]
port = 42617                            # 默认端口
host = "127.0.0.1"
require_pairing = true                  # 需要配对码验证
allow_public_bind = false               # 禁止绑定非 localhost（除非有 tunnel）
pair_rate_limit_per_minute = 10
webhook_rate_limit_per_minute = 60
trust_forwarded_headers = false
idempotency_ttl_secs = 300
```

### 4.5 Channel 配置

```toml
[channels_config]
cli = true                              # CLI 频道始终开启
message_timeout_secs = 300             # 每轮预算（LLM + 工具）
ack_reactions = true                   # 添加 👀/✅/⚠️ 确认反应
show_tool_calls = false                 # 是否在频道显示工具调用
session_persistence = true
session_backend = "sqlite"             # "jsonl"（旧）或 "sqlite"（默认）
session_ttl_hours = 0                  # 0 = 永不归档

# Telegram
[channels_config.telegram]
bot_token = "..."
allowed_users = ["user123"]            # 空 = 拒绝所有
stream_mode = "off"                    # progressive edits
draft_update_interval_ms = 500
interrupt_on_new_message = false
mention_only = false                   # 群组中只响应 @mention

# Discord
[channels_config.discord]
bot_token = "..."
guild_id = "123456789"                 # 可选，限单 guild
allowed_users = []
listen_to_bots = false
mention_only = false

# Slack
[channels_config.slack]
bot_token = "xoxb-..."
app_token = "xapp-..."                 # Socket Mode 用
channel_id = "#general"
allowed_users = []
mention_only = false

# Matrix
[channels_config.matrix]
homeserver = "https://matrix.org"
access_token = "..."
room_id = "!abc123:matrix.org"
allowed_users = []

# Lark（飞书国际版）
[channels_config.lark]
# ...

# Feishu（飞书）
[channels_config.feishu]
# ...

# DingTalk（钉钉）
[channels_config.dingtalk]
# ...

# Webhook（通用 HTTP）
[channels_config.webhook]
port = 8090
listen_path = "/hook"
send_url = "https://..."
send_method = "POST"
auth_header = "X-Secret: ..."
secret = "..."
```

### 4.6 Memory 配置

```toml
[memory]
backend = "sqlite"                     # sqlite / markdown / lucid / embeddings / qdrant / none
auto_save = true
search_enabled = true
max_memory_entries = 10000

[memory.sqlite]
db_path = "~/.zeroclaw/memory.db"

[memory.markdown]
dir = "~/.zeroclaw/memory"

[memory.embeddings]
model = "text-embedding-3-small"
provider = "openai"

[memory.qdrant]
url = "http://localhost:6333"          # 或 QDRANT_URL 环境变量
collection = "zeroclaw_memories"       # 或 QDRANT_COLLECTION
api_key = "..."                        # 或 QDRANT_API_KEY
```

### 4.7 自主性 & 安全配置

```toml
[autonomy]
level = "supervised"                   # disabled / manual / supervised / autonomous
workspace_only = true
allowed_roots = ["/home/user/work"]
allowed_commands = ["curl", "cat", "grep"]
max_actions_per_hour = 100
max_cost_per_day_cents = 10000         # 每天 $100 预算上限

[security.otp]
enabled = false

[security.estop]
enabled = true
require_otp_to_resume = false
```

### 4.8 可观测性

```toml
[observability]
backend = "console"                    # console / file / otel
runtime_trace_mode = "jsonl"           # jsonl / structured / none
runtime_trace_path = "~/.zeroclaw/traces"
```

---

## 五、支持的 Provider（25+）

| Provider | 类型 | Config Key | 认证方式 | 备注 |
|----------|------|-----------|---------|------|
| OpenRouter | Cloud | `openrouter` | API Key | 默认 Provider，聚合 50+ 模型 |
| OpenAI | Cloud | `openai` | API Key | GPT-4 系列 |
| Anthropic | Cloud | `anthropic` | API Key / OAuth | Claude 系列 |
| Google Gemini | Cloud | `gemini` | API Key | Gemini Pro/Flash |
| Groq | Cloud | `groq` | API Key | 快速推理 |
| Mistral | Cloud | `mistral` | API Key | |
| DeepSeek | Cloud | `deepseek` | API Key | |
| X.AI | Cloud | `xai` | API Key | Grok 系列 |
| Together.AI | Cloud | `together` | API Key | 开源模型聚合 |
| Fireworks | Cloud | `fireworks` | API Key | |
| Perplexity | Cloud | `perplexity` | API Key | Sonar 系列 |
| Cohere | Cloud | `cohere` | API Key | |
| Moonshot (Kimi) | Cloud | `moonshot` | API Key | 中文 LLM |
| GLM (智谱) | Cloud | `glm` | API Key | GLM-4 系列 |
| Minimax | Cloud | `minimax` | OAuth/API Key | 中文 LLM |
| 百度千帆 | Cloud | `qianfan` | API Key | |
| 阿里 Dashscope | Cloud | `dashscope` | API Key | 通义千问 |
| Z.AI | Cloud | `zai` | API Key | GLM 兼容网关 |
| Venice | Cloud | `venice` | API Key | 隐私优先 |
| Ollama | Local | `ollama` | 无（本地）| 本地或远端 |
| OpenAI Codex | Cloud | `openai-codex` | OAuth Device Flow | |
| GitHub Copilot | OAuth | `copilot` | OAuth | |
| Claude Code | OAuth | `claude-code` | OAuth/Token | |
| AWS Bedrock | Cloud | `bedrock` | AWS Credentials | |
| Azure OpenAI | Cloud | `azure_openai` | API Key | |
| 自定义 OpenAI 兼容 | Custom | `custom:<URL>` | API Key | 任何 OpenAI 兼容端点 |
| 自定义 Anthropic 兼容 | Custom | `anthropic-custom:<URL>` | API Key | |

---

## 六、支持的 Channel（25+）

| Channel | Config Key | 主要字段 |
|---------|-----------|---------|
| Telegram | `telegram` | `bot_token`, `allowed_users`, `mention_only` |
| Discord | `discord` | `bot_token`, `guild_id`, `mention_only` |
| Slack | `slack` | `bot_token`, `app_token`, `channel_id` |
| Matrix | `matrix` | `homeserver`, `access_token`, `room_id` |
| Signal | `signal` | Signal 专属配置 |
| WhatsApp | `whatsapp` | Cloud API / Web 模式 |
| Lark（飞书国际）| `lark` | Bot 配置 |
| Feishu（飞书）| `feishu` | Bot 配置 |
| DingTalk（钉钉）| `dingtalk` | 企业 IM |
| Email | `email` | SMTP/IMAP 凭据 |
| IRC | `irc` | 服务器、nick、频道 |
| iMessage | `imessage` | macOS 专属，`allowed_contacts` |
| Mattermost | `mattermost` | `url`, `bot_token`, `channel_id` |
| Nextcloud Talk | `nextcloud_talk` | Nextcloud 服务器配置 |
| Webhook（通用 HTTP）| `webhook` | `port`, `listen_path`, `send_url`, `secret` |
| Notion | `notion` | Notion API Key |
| QQ | `qq` | QQ 官方 Bot 配置 |
| Twitter/X | `twitter` | API 凭据 |
| Reddit | `reddit` | OAuth2 凭据 |
| Bluesky | `bluesky` | AT Protocol，handle/password |
| WeCom（企业微信）| `wecom` | Bot Webhook |
| WATI | `wati` | WhatsApp Business API |
| Mochat | `mochat` | 客服平台 |
| ClawdTalk | `clawdtalk` | 语音 Channel |
| CLI | 内置 | 始终开启，无需配置 |

---

## 七、环境变量完整列表

### 核心配置变量

```bash
ZEROCLAW_API_KEY=sk-...                  # 全局 API Key（最高优先级）
ZEROCLAW_PROVIDER=openrouter            # Provider
ZEROCLAW_MODEL=anthropic/claude-sonnet  # 模型
ZEROCLAW_TEMPERATURE=0.7               # 温度
ZEROCLAW_CONFIG_DIR=~/.zeroclaw        # 配置目录
ZEROCLAW_WORKSPACE=~/work              # 工作区
```

### Provider 专属

```bash
OPENROUTER_API_KEY=sk-or-v1-...
ANTHROPIC_API_KEY=sk-ant-...
ANTHROPIC_OAUTH_TOKEN=...
OPENAI_API_KEY=sk-...
GEMINI_API_KEY=...
GOOGLE_API_KEY=...
GROQ_API_KEY=...
MISTRAL_API_KEY=...
DEEPSEEK_API_KEY=...
XAI_API_KEY=...
TOGETHER_API_KEY=...
FIREWORKS_API_KEY=...
PERPLEXITY_API_KEY=...
COHERE_API_KEY=...
MOONSHOT_API_KEY=...
DASHSCOPE_API_KEY=...
ZAI_API_KEY=...
```

### 网关

```bash
ZEROCLAW_GATEWAY_PORT=42617
ZEROCLAW_GATEWAY_HOST=127.0.0.1
ZEROCLAW_ALLOW_PUBLIC_BIND=false
```

### Web 搜索

```bash
ZEROCLAW_WEB_SEARCH_ENABLED=true
ZEROCLAW_WEB_SEARCH_PROVIDER=duckduckgo  # duckduckgo / brave
ZEROCLAW_BRAVE_API_KEY=...
ZEROCLAW_WEB_SEARCH_MAX_RESULTS=5
```

### 代理

```bash
HTTP_PROXY=http://proxy:8080
HTTPS_PROXY=http://proxy:8080
NO_PROXY=localhost,127.0.0.1
```

### 推理（支持推理的模型）

```bash
ZEROCLAW_REASONING_ENABLED=true
ZEROCLAW_REASONING_EFFORT=medium        # low / medium / high
```

### 存储

```bash
ZEROCLAW_STORAGE_PROVIDER=sqlite
ZEROCLAW_STORAGE_DB_URL=sqlite:///path/to/db
```

### Memory 后端（Qdrant）

```bash
QDRANT_URL=http://localhost:6333
QDRANT_COLLECTION=zeroclaw_memories
QDRANT_API_KEY=...
```

---

## 八、快速上手最短路径

```bash
# 1. 安装
cargo install zeroclawlabs

# 2. 初始化（交互式）
zeroclaw onboard

# 3. 或脚本化初始化
zeroclaw onboard --api-key $ANTHROPIC_API_KEY --provider anthropic --model claude-sonnet-4-6

# 4. 启动交互式 Agent
zeroclaw agent

# 5. 单次消息
zeroclaw agent -m "帮我写一个 hello world"

# 6. 完整守护进程（Gateway + Channel）
zeroclaw daemon

# 7. 健康检查
zeroclaw status
zeroclaw doctor
```

---

## 九、对 quickai-gateway 的设计参考

### 9.1 可以直接借鉴的

| zeroclaw 设计 | quickai-gateway 对应 |
|-------------|-------------------|
| `onboard` 命令（交互+非交互） | 目前缺失，用户只能手写 config.toml |
| `status` 命令（系统状态总览）| 有 `/status` HTTP 接口，但无 CLI |
| `doctor` 命令 | 有 `/doctor` HTTP 接口，但无 CLI |
| `channel` 命令管理 Channel | 目前只有 config.toml 配置 |
| `completions` Shell 补全 | 缺失 |
| `ZEROCLAW_API_KEY` 等环境变量覆盖 | 部分支持（backend env 字段） |
| API Key / URL / Model 可在 CLI 中指定 | 目前只有 config.toml |

### 9.2 quickai-gateway 特有优势（不需要照搬的）

- **Team/Specialist 模式**：zeroclaw 没有 Lead/Specialist 多 Agent 编排
- **Persona + Skills 系统**：结构比 zeroclaw 更深
- **ACP 协议**：zeroclaw 不走 ACP，直接调 LLM API
- **Cron → IM Channel**：zeroclaw 的 cron 结果不原生发到 IM

### 9.3 最值得学习的差距

1. **`onboard` 命令**：zeroclaw 有完整引导初始化，降低用户门槛极大
2. **CLI 覆盖参数**：zeroclaw 的 `--api-key`、`--provider`、`--model` 可以在 CLI 上覆盖 config，quickai-gateway 全靠 config.toml，调试麻烦
3. **Shell 补全**：clap 支持免费生成，一行代码的事
4. **`config schema` 命令**：dump JSON Schema 帮助用户了解所有可配字段

---

## 十、性能数据

| 指标 | 数值 |
|------|------|
| Release Binary 大小 | ~8.8 MB（静态 binary）|
| 启动时间（`--help`）| ~20 ms |
| 启动时间（`status`）| ~10 ms |
| 峰值内存（帮助）| ~3.9 MB |
| 峰值内存（status）| ~4.1 MB |
| 目标硬件 | $10 SBC，<5 MB RAM |

---

*参考文件：*
- `zeroclaw/src/main.rs`（CLI 入口，~2300 行）
- `zeroclaw/src/lib.rs`（库入口，~541 行）
- `zeroclaw/src/config/`（配置 Schema）
- `zeroclaw/src/channels/`（25+ Channel 后端）
- `zeroclaw/src/providers/`（25+ Provider 后端）
- `zeroclaw/src/gateway/`（HTTP/WS 网关）
