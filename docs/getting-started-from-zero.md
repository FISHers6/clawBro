# ClawBro 从零开始

这份文档面向第一次使用 `clawbro` / `ClawBro` 的开发者，目标是让你按当前代码实现，把系统从零配置到可运行，再逐步扩展到多 Agent、Team、Lark、DingTalk、Cron 和诊断面。

如果你优先想从用户视角快速安装和配置，先看：

- [`setup.md`](setup.md)

如果你优先想了解产品定位和案例，再看：

- [`../README.md`](../README.md)

本文基于当前代码路径：

- CLI 入口：[clawbro_cli.rs](/Users/fishers/Desktop/repo/quickai-openclaw/clawBro/crates/clawbro-server/src/bin/clawbro_cli.rs)
- 服务入口：[gateway_process.rs](/Users/fishers/Desktop/repo/quickai-openclaw/clawBro/crates/clawbro-server/src/gateway_process.rs)
- 配置结构：[config.rs](/Users/fishers/Desktop/repo/quickai-openclaw/clawBro/crates/clawbro-server/src/config.rs)
- Backend 家族：[runtime-backends.md](/Users/fishers/Desktop/repo/quickai-openclaw/clawBro/docs/runtime-backends.md)
- 上下文文件契约：[context-filesystem-contract.md](/Users/fishers/Desktop/repo/quickai-openclaw/clawBro/docs/context-filesystem-contract.md)
- 路由契约：[routing-contract.md](/Users/fishers/Desktop/repo/quickai-openclaw/clawBro/docs/routing-contract.md)
- 运维诊断面：[doctor-and-status.md](/Users/fishers/Desktop/repo/quickai-openclaw/clawBro/docs/operations/doctor-and-status.md)

## 1. 系统是什么

`ClawBro` 当前是一个统一的 AI Gateway 和控制面，负责：

- 接收外部消息：WebSocket、Lark、DingTalk、Cron
- 做会话、路由、绑定、memory、team orchestration
- 把实际执行分发给不同 backend family：
  - `claw_bro_native`
  - `acp`
  - `open_claw_gateway`
- 暴露统一诊断接口：
  - `/health`
  - `/status`
  - `/doctor`
  - `/diagnostics/*`

当前 Claude 路径使用 `npx @zed-industries/claude-agent-acp`。`clawbro-claude-agent` 仅保留为仓库内遗留实验产物，不属于标准产品接入路径。

建议的上手顺序不是直接接 IM，而是：

1. 先跑 `WS + claw_bro_native`
2. 再加 `agent_roster`
3. 再加 `binding`
4. 再加 `group/team`
5. 最后接 `Lark / DingTalk`

## 2. 目录和运行时文件

当前默认运行目录都在 `~/.clawbro/` 下。

关键文件和目录：

- `~/.clawbro/config.toml`
  - 主配置文件
- `~/.clawbro/sessions/`
  - 会话存储
- `~/.clawbro/shared/`
  - 共享 memory 存储
- `~/.clawbro/skills/`
  - skills 主目录
- `~/.clawbro/cron.db`
  - cron SQLite 存储
- `~/.clawbro/gateway.port`
  - 启动后写入的 gateway 端口
- `~/.clawbro/allowlist.json`
  - channel allowlist，可选

建议先创建：

```bash
mkdir -p ~/.clawbro
mkdir -p ~/.clawbro/sessions
mkdir -p ~/.clawbro/shared
mkdir -p ~/.clawbro/skills
mkdir -p ~/.clawbro/personas
```

## 3. 先准备什么

至少要有：

- Rust / Cargo
- 一个可用模型 API Key
- `clawbro` 二进制

推荐先编译：

```bash
cd /Users/fishers/Desktop/repo/quickai-openclaw/clawBro
cargo build -p clawbro --bin clawbro
```

如果你使用 `claw_bro_native` family 的 `bundled_command` 启动方式，gateway 会默认尝试执行 ClawBro 管理的捆绑 shell：

```bash
clawbro runtime-bridge
```

默认情况下不再依赖 `PATH`。如果你需要覆盖捆绑路径，再改成显式 `external_command` 启动。

兼容性说明：

- `embedded` 仍会被解析为 `bundled_command`
- `command` 仍会被解析为 `external_command`
- 文档和新配置示例统一使用新的 canonical 名称

## 4. 环境变量

### 4.1 模型环境变量

`clawbro` 内置 runtime bridge 当前读取优先级是：

1. `ANTHROPIC_API_KEY`
2. `OPENAI_API_KEY`
3. `DEEPSEEK_API_KEY`

可选附加：

- `OPENAI_API_BASE`
- `CLAWBRO_MODEL`
- `CLAWBRO_SYSTEM_PROMPT`

最常见的两种写法：

```bash
export OPENAI_API_KEY=sk-xxx
export CLAWBRO_MODEL=gpt-4o
```

或：

```bash
export OPENAI_API_KEY=sk-xxx
export OPENAI_API_BASE=https://api.deepseek.com
export CLAWBRO_MODEL=deepseek-chat
```

### 4.2 WebSocket 鉴权

如果配置了：

```toml
[auth]
ws_token = "dev-token"
```

那么访问 `/ws` 时必须带：

```text
Authorization: Bearer dev-token
```

### 4.3 Lark 环境变量

如果启用 Lark，需要：

- `LARK_APP_ID`
- `LARK_APP_SECRET`

### 4.4 DingTalk 环境变量

如果启用 DingTalk，需要：

- `DINGTALK_APP_KEY`
- `DINGTALK_APP_SECRET`

### 4.5 Allowlist 路径

如果你不想用默认路径，可以设：

- `CLAWBRO_ALLOWLIST_PATH`

## 5. 配置文件总规则

默认主配置文件路径：

- `~/.clawbro/config.toml`

用户入口 `clawbro serve` 支持覆盖：

- `--config /path/to/config.toml`
- `--port 18080`
- `CLAWBRO_CONFIG`
- `CLAWBRO_PORT`

启动前会先做拓扑校验。几个最重要的规则：

- 至少要有一个 `[[backend]]`
- 如果没有 `[[agent_roster]]`，就必须配置 `[agent].backend_id`
- `[[agent_roster]]` 的每个 `backend_id` 都必须存在于 `[[backend]]`
- `[[binding]]` 只能和 `[[agent_roster]]` 一起用
- `[[group]]` 里的 `front_bot` 和 `group.team.roster` 必须引用 `[[agent_roster]]` 里已存在的 agent 名

## 6. 最小可运行场景：单 Agent + WebSocket

这是最推荐的第一步。先不要接 IM。

写入 `~/.clawbro/config.toml`：

```toml
[gateway]
host = "127.0.0.1"
port = 8080
require_mention_in_groups = false
default_workspace = "/Users/yourname/work/demo"

[auth]
ws_token = "dev-token"

[agent]
backend_id = "native-main"

[[backend]]
id = "native-main"
family = "claw_bro_native"

[backend.launch]
type = "bundled_command"

[skills]
dir = "/Users/yourname/.clawbro/skills"

[session]
dir = "/Users/yourname/.clawbro/sessions"

[memory]
shared_dir = "/Users/yourname/.clawbro/shared"
distill_every_n = 20
distiller_binary = "clawbro"
```

启动：

```bash
cd /Users/fishers/Desktop/repo/quickai-openclaw/clawBro
cargo run -p clawbro --bin clawbro -- serve
```

如果你不想固定端口，也可以写：

```toml
[gateway]
port = 0
```

这样系统会让操作系统分配随机端口，并把最终端口写入：

- `~/.clawbro/gateway.port`

## 6.1 用 `clawbro setup` 快速初始化

如果你不想手写 `config.toml`，可以直接用 CLI 向导：

```bash
clawbro setup
```

最常见的非交互示例：

单 Agent：

```bash
clawbro setup \
  --lang en \
  --provider anthropic \
  --api-key sk-ant-xxx \
  --mode solo \
  --non-interactive
```

Team（DM 工作台）：

```bash
clawbro setup \
  --lang en \
  --provider anthropic \
  --api-key sk-ant-xxx \
  --mode team \
  --front-bot planner \
  --specialist coder \
  --specialist reviewer \
  --team-target direct-message \
  --team-scope user:ou_your_user_id \
  --team-name my-team \
  --non-interactive
```

Team（群聊协作）：

```bash
clawbro setup \
  --lang en \
  --provider anthropic \
  --api-key sk-ant-xxx \
  --mode team \
  --front-bot planner \
  --specialist coder \
  --specialist reviewer \
  --team-target group \
  --team-scope group:lark:chat-123 \
  --team-name ops-room \
  --non-interactive
```

交互模式下：

- `front_bot` 会单独询问
- specialist 会逐个输入，留空结束
- Team 的 `scope/name` 会在拿到 channel 后再询问

所有 setup 生成的 Team skeleton 都应直接通过：

```bash
clawbro config validate
```

### 启动后会发生什么

- 读取 `~/.clawbro/config.toml`
- 初始化 session 存储
- 初始化 skills
- 注册 runtime adapters
- 注册 backend catalog
- 启动 HTTP/WS gateway
- 写端口到 `~/.clawbro/gateway.port`
- 如果配置了 cron / channel / team，也会一并启动

### 第一次验证

先看：

```bash
curl http://127.0.0.1:8080/health
curl http://127.0.0.1:8080/status
curl http://127.0.0.1:8080/doctor
```

你应该至少能看到：

- backend 已注册
- health 为 `ok` 或至少不是配置错误
- 没有明显 topology 错误

## 7. 用 WebSocket 发第一条消息

当前 `InboundMsg` 结构最小例子：

```json
{
  "id": "msg-1",
  "session_key": {
    "channel": "ws",
    "scope": "user:test"
  },
  "content": {
    "type": "Text",
    "text": "hello"
  },
  "sender": "test-user",
  "channel": "ws",
  "timestamp": "2026-03-09T00:00:00Z",
  "thread_ts": null,
  "target_agent": null,
  "source": "human"
}
```

如果你配置了 `ws_token`，连接 `/ws` 时要带 Bearer Token。

服务会返回 `AgentEvent`。最重要的是看到：

- `Delta`
- `TurnComplete`
- 如有审批则可能看到 `ApprovalRequest`

## 8. 单 Agent 但不用 bundled_command

如果你不想使用 ClawBro 管理的捆绑路径，可以显式写成 `external_command`：

```toml
[agent]
backend_id = "native-main"

[[backend]]
id = "native-main"
family = "claw_bro_native"

[backend.launch]
type = "external_command"
command = "/Users/fishers/Desktop/repo/quickai-openclaw/clawBro/target/debug/clawbro"
args = []
```

`claw_bro_native` adapter 会在内部为 `clawbro` 注入 `runtime-bridge` 子命令，不需要你手动写。

## 9. 多 backend 场景

当前 backend family 有 3 类。

### 9.1 `claw_bro_native`

适合本地默认原生执行。

```toml
[[backend]]
id = "native-main"
family = "claw_bro_native"

[backend.launch]
type = "bundled_command"
```

如果你要给 `claw_bro_native` 挂外部 MCP SSE server，可以直接配在 backend 下：

```toml
[[backend.external_mcp_servers]]
name = "filesystem"
url = "http://127.0.0.1:3001/sse"

[[backend.external_mcp_servers]]
name = "github"
url = "http://127.0.0.1:3002/sse"
```

当前行为：

- 这些 server 会作为宿主级 `external_mcp_servers` 进入 `RuntimeSessionSpec`
- `clawbro runtime-bridge` 会在 native runtime 内部通过 `rig/rmcp` 连接并注册工具
- 这和 `team_tool_url` 是两条不同路径：
  - `team_tool_url` 负责 ClawBro 自己的 team tools
  - `external_mcp_servers` 负责用户配置的外部 MCP tools
- 名称必须唯一，且不能使用保留名 `team-tools`

### 9.2 `acp`

适合接支持 ACP 的 CLI agent 或 bridge。

`acp` family 内部支持多种不同的 ACP backend，使用可选的 `acp_backend` 字段标识。

**注意：**
- `acp_backend` 只在 `family = "acp"` 时有效，其他 family 会拒绝该字段
- `config.toml` 不支持 `${ENV_VAR}` 环境变量插值，所有值必须是字面字符串
- Gemini 尚未作为 `acp_backend` 值在 ClawBro 中验证

**Bridge-backed backends（需要 npx 适配器包）：**

```toml
# Claude
[[backend]]
id = "claude-main"
family = "acp"
acp_backend = "claude"

[backend.launch]
type = "external_command"
command = "npx"
args = ["--yes", "--prefer-offline", "@zed-industries/claude-agent-acp@0.18.0"]

[backend.launch.env]
ANTHROPIC_AUTH_TOKEN = "sk-ant-your-key-here"
```

```toml
# Codex
[[backend]]
id = "codex-main"
family = "acp"
acp_backend = "codex"

[backend.launch]
type = "external_command"
command = "npx"
args = ["--yes", "--prefer-offline", "@zed-industries/codex-acp@latest"]
```

**Generic ACP CLI backends（直接命令行启动）：**

```toml
# Qwen
[[backend]]
id = "qwen-main"
family = "acp"
acp_backend = "qwen"

[backend.launch]
type = "external_command"
command = "npx"
args = ["@qwen-code/qwen-code", "--acp"]
```

```toml
# Goose（使用子命令）
[[backend]]
id = "goose-main"
family = "acp"
acp_backend = "goose"

[backend.launch]
type = "external_command"
command = "goose"
args = ["acp"]
```

**Generic path（省略 acp_backend）：**

```toml
[[backend]]
id = "codex-main"
family = "acp"

[backend.launch]
type = "external_command"
command = "codex-acp"
args = ["--stdio"]
```

省略 `acp_backend` 时走通用 ACP CLI 路径，没有特殊策略。

如果你要给 ACP backend 挂外部 MCP SSE server，同样配在 backend 下：

```toml
[[backend.external_mcp_servers]]
name = "filesystem"
url = "http://127.0.0.1:3001/sse"

[[backend.external_mcp_servers]]
name = "github"
url = "http://127.0.0.1:3002/sse"
```

当前行为：

- gateway 会把 `team-tools` 和这些外部 MCP SSE servers 一起注册进 ACP session
- 这一阶段只支持 `SSE`
- 如果 ACP agent 本身不声明 `mcp_capabilities.sse = true`，这些 MCP server 不会生效
- 名称必须唯一，且不能使用保留名 `team-tools`

### 9.3 `open_claw_gateway`

适合接已经运行的 OpenClaw Gateway。

```toml
[[backend]]
id = "openclaw-main"
family = "open_claw_gateway"

[backend.launch]
type = "gateway_ws"
endpoint = "ws://127.0.0.1:18789"
agent_id = "main"
```

如果你希望它参与 Team helper：

```toml
team_helper_command = "/usr/local/bin/clawbro-team-cli"
team_helper_args = []
```

如果希望它可作为 Lead：

```toml
lead_helper_mode = true
```

注意：

- 这一阶段 `OpenClaw` 不支持像 `ACP / claw_bro_native` 那样的外部 MCP server parity
- 原因不是 ClawBro 偷懒，而是当前 OpenClaw gateway chat/client 路径没有对等的外部 MCP 注入面
- `OpenClaw` 当前仍然支持 team helper CLI bridge，这是另一条能力路径，不等同于外部 MCP

## 10. 多 Agent roster 场景

如果你要支持多个 Agent，就用 `[[agent_roster]]`，不要继续只靠 `[agent].backend_id`。

例子：

```toml
[[agent_roster]]
name = "claude"
mentions = ["@claude"]
backend_id = "native-main"
persona_dir = "/Users/yourname/.clawbro/personas/claude"
workspace_dir = "/Users/yourname/work/app1"

[[agent_roster]]
name = "reviewer"
mentions = ["@reviewer"]
backend_id = "codex-main"
persona_dir = "/Users/yourname/.clawbro/personas/reviewer"
workspace_dir = "/Users/yourname/work/app1"

[[agent_roster]]
name = "openclaw"
mentions = ["@openclaw"]
backend_id = "openclaw-main"
workspace_dir = "/Users/yourname/work/app1"
```

### roster 的作用

- 把 `@mention` 映射到 backend
- 定义 persona 目录
- 定义 workspace
- 定义每个 agent 的额外 skills 目录

### 为什么 external MCP 放在 `[[backend]]` 而不是 `[[agent_roster]]`

当前 ClawBro 把外部 MCP server 视为 backend execution capability，而不是 persona/roster 属性。

这意味着：

- 同一个 backend 下的多个 agent 共享同一组外部 MCP tools
- 这和 `backend.launch.env`、命令、协议能力属于同一层
- `agent_roster` 继续负责：
  - mention
  - persona_dir
  - workspace_dir
  - extra skills

这样做的好处是：

- 多 CLI backend 的 ownership 更清楚
- 不会把系统做成“backend 一套工具，agent 再覆盖一套工具”的双层混乱结构

### roster-only 模式

如果你不配置 `[agent].backend_id`，而是只配置 `[[agent_roster]]`，当前系统会默认落到 roster 第一个 agent。

## 11. Persona 和上下文文件

当前 filesystem-native context contract 支持这些文件：

- `SOUL.md`
- `IDENTITY.md`
- `USER.md`
- `MEMORY.md`
- `memory/<channel>_<scope>.md`
- `AGENTS.md`
- `CLAUDE.md`
- `HEARTBEAT.md`
- `TEAM.md`
- `CONTEXT.md`
- `TASKS.md`

最常见的 persona 目录示例：

```text
~/.clawbro/personas/claude/
  SOUL.md
  IDENTITY.md
  MEMORY.md
  USER.md
  memory/
```

最常见的 workspace 目录示例：

```text
/Users/yourname/work/app1/
  AGENTS.md
  CLAUDE.md
  HEARTBEAT.md
```

### 当前加载规则

- `persona_dir` 贡献 persona 文件
- `workspace_dir` 贡献 workspace 文件
- team mode 再额外叠加 team 根目录文件
- 可见文件会投影进 runtime context

### role 差异

- `Solo`
  - 收到 persona + workspace 文件
- `Lead`
  - 收到 persona + workspace + team 文件
- `Specialist`
  - 收到角色允许的 persona 文件和 team 文件
  - 不自动看到长期私有 `MEMORY.md`

## 12. 显式 routing：binding

如果你要 deterministic routing，就配置 `[[binding]]`。

最常用的是 `scope`：

```toml
[[binding]]
kind = "scope"
agent = "claude"
scope = "group:lark:chat-123"
channel = "lark"
```

表示：
这个 scope 下没有显式 `@mention` 时，默认走 `claude`。

还支持：

- `thread`
- `scope`
- `peer`
- `team`
- `channel`
- `default`

示例：

```toml
[[binding]]
kind = "thread"
agent = "reviewer"
scope = "group:lark:chat-123"
thread_id = "thread-001"
channel = "lark"

[[binding]]
kind = "channel"
agent = "claude"
channel = "ws"

[[binding]]
kind = "default"
agent = "claude"
```

注意：

- `[[binding]]` 必须依赖 `[[agent_roster]]`
- 同一优先级后注册的 binding 会覆盖先注册的 binding
- `@mention` 优先级仍高于 binding
- `/backend` 的 session override 优先级也高于 binding

## 13. Group 场景

你可以为特定群组配置专门的交互模式。

最简单的 group：

```toml
[[group]]
scope = "group:lark:chat-123"
name = "dev-group"

[group.mode]
interaction = "solo"
front_bot = "claude"
channel = "lark"
```

这表示：

- 这个群的默认 front bot 是 `claude`
- 没显式 mention 时，也可落到 front bot

### interaction 可选值

- `solo`
- `relay`
- `team`

### `auto_promote`

如果你开：

```toml
auto_promote = true
```

系统会对配置的 group 开启关键词驱动的自动升级能力。

## 14. Team 多 Agent 场景

这是第二阶段以后再开的模式。先确认单 Agent 和 roster 已经稳定。

示例：

```toml
[[group]]
scope = "group:lark:chat-123"
name = "team-group"

[group.mode]
interaction = "team"
front_bot = "claude"
channel = "lark"

[group.team]
roster = ["reviewer", "openclaw"]
public_updates = "minimal"
max_parallel = 3
```

如果你要在单聊里把一个用户视角的“个人工作台”挂成 Team，而不是群聊透明协作，可以用精确 scope 的 `[[team_scope]]`：

```toml
[[team_scope]]
scope = "user:ou_a0af70fd8fc139a5d7a4bf2926810b6f"
name = "dm-workbench"

[team_scope.mode]
interaction = "team"
front_bot = "claude"
channel = "lark"

[team_scope.team]
roster = ["codex", "researcher"]
public_updates = "minimal"
max_parallel = 2
```

`public_updates` 现在是 Team 生命周期通知的唯一外发开关：

- `minimal`
  - 只发 lead 显式 `post_update` / 最终答复
- `normal`
  - 在 `minimal` 基础上，再发关键自动通知：
    - `TaskBlocked`
    - `TaskFailed`
    - `AllTasksDone`
- `verbose`
  - 再额外发详细 Team 生命周期通知：
    - `TaskDispatched`
    - `TaskSubmitted`
    - `TaskDone`
    - `TasksUnlocked`
    - `TaskCheckpoint`

它和 channel 的 `presentation` 是独立的：

- `public_updates` 控制 Team 里程碑是否对外可见
- `presentation` 控制工具/进度提示是否发送

### Team 运行后会有什么

team runtime 会建立：

- team session root
- `TEAM.md`
- `CONTEXT.md`
- `TASKS.md`
- `events.jsonl`
- `tasks/<task-id>/meta.json`
- `tasks/<task-id>/spec.md`
- `tasks/<task-id>/progress.md`
- `tasks/<task-id>/result.md`

### Team 的前置要求

- `front_bot` 必须存在于 `[[agent_roster]]`
- `group.team.roster` 中每个 specialist 名字都必须存在于 `[[agent_roster]]`
- `team_scope.team.roster` 中每个 specialist 名字也必须存在于 `[[agent_roster]]`
- backend 必须支持其被分配的 role
- 如果 OpenClaw family 参与 Team，通常要配置 `team_helper_command`

## 15. Cron 场景

当前支持在配置里声明 `[[cron_jobs]]`，启动时会同步到 `~/.clawbro/cron.db`。

例子：

```toml
[[cron_jobs]]
name = "daily-standup"
expr = "0 9 * * 1-5"
prompt = "请总结今天的工作安排"
session_key = "lark:group:chat-123"
enabled = true
agent = "claude"
condition = "idle_gt_seconds:300"
```

### 字段说明

- `name`
  - 任务名
- `expr`
  - cron 表达式
- `prompt`
  - 触发时注入的消息
- `session_key`
  - 目标会话，格式是 `channel:scope`
- `enabled`
  - 是否启用
- `agent`
  - 可选，指定目标 agent
- `condition`
  - 可选，目前支持空闲条件

## 16. Lark 场景

如果你要接飞书，配置文件最少要开：

```toml
[channels.lark]
enabled = true
```

同时环境变量必须有：

```bash
export LARK_APP_ID=cli_xxx
export LARK_APP_SECRET=xxx
```

### Lark 运行行为

- 使用长连接 WebSocket 模式
- 默认 `presentation = "final_only"`，只发送最终结果
- 可选 `presentation = "progress_compact"`，先发送简化进度，再发送最终结果
- `presentation` 不决定 Team milestone 是否外发；那由 `group.team.public_updates` / `team_scope.team.public_updates` 控制

例如：

```toml
[channels.lark]
enabled = true
presentation = "progress_compact"
```

### Lark 群聊 scope

group scope 通常形如：

```text
group:lark:<chat_id>
```

所以 group 配置和 binding 也应使用这个格式。

## 17. DingTalk 场景

如果你要接钉钉，配置文件最少要开：

```toml
[channels.dingtalk]
enabled = true
presentation = "progress_compact"
```

同时环境变量必须有：

```bash
export DINGTALK_APP_KEY=dingxxx
export DINGTALK_APP_SECRET=xxx
```

### DingTalk 运行行为

- 使用 Stream Mode
- 默认 `presentation = "final_only"`
- 可选 `presentation = "progress_compact"`，会先发紧凑进度，再发最终结果
- 群聊 scope 形如 `group:<conversationId>`
- 私聊 scope 形如 `user:<senderId>`

如果你启用了：

```toml
[gateway]
require_mention_in_groups = true
```

那么群消息需要显式 mention 才会进入处理。

## 18. Allowlist 场景

如果你要限制谁可以用 channel，写：

- `~/.clawbro/allowlist.json`

示例：

```json
{
  "dingtalk": {
    "enabled": true,
    "mode": "allowlist",
    "users": ["user_staff_id_1", "user_staff_id_2"]
  },
  "lark": {
    "enabled": true,
    "mode": "allowlist",
    "open_ids": ["ou_abc123", "ou_xyz456"]
  }
}
```

规则：

- 文件不存在时是 open mode
- channel 未配置时也是 open mode
- `enabled = false` 时该 channel 全拒绝

## 19. 诊断和运维面

启动后可用的只读接口：

- `GET /health`
- `GET /status`
- `GET /doctor`
- `GET /diagnostics/backends`
- `GET /diagnostics/teams`
- `GET /diagnostics/channels`
- `GET /diagnostics/topology`

### 最常看的接口

```bash
curl http://127.0.0.1:8080/health
curl http://127.0.0.1:8080/status
curl http://127.0.0.1:8080/doctor
curl http://127.0.0.1:8080/diagnostics/backends
```

### 这些接口分别看什么

- `/health`
  - 适合 liveness / readiness
- `/status`
  - 适合看当前整体快照
- `/doctor`
  - 适合看 operator 级诊断结论
- `/diagnostics/backends`
  - backend 是否已注册、已 probe、是否健康
- `/diagnostics/teams`
  - team 状态、artifact 健康度、任务数量
- `/diagnostics/channels`
  - channel 是否配置、启用、是否有 wiring
- `/diagnostics/topology`
  - bindings、groups、team groups 摘要

## 20. 一个完整的多场景样例

下面这份配置覆盖：

- 默认 single-agent
- 多 backend
- 多 agent roster
- binding
- team group
- lark channel
- cron

```toml
[gateway]
host = "127.0.0.1"
port = 8080
require_mention_in_groups = false
default_workspace = "/Users/yourname/work/app1"

[auth]
ws_token = "dev-token"

[skills]
dir = "/Users/yourname/.clawbro/skills"
global_dirs = ["/Users/yourname/.clawbro/global-skills"]

[session]
dir = "/Users/yourname/.clawbro/sessions"

[memory]
shared_dir = "/Users/yourname/.clawbro/shared"
distill_every_n = 20
distiller_binary = "clawbro"

[[backend]]
id = "native-main"
family = "claw_bro_native"

[backend.launch]
type = "bundled_command"

[[backend]]
id = "codex-main"
family = "acp"

[backend.launch]
type = "external_command"
command = "codex-acp"
args = ["--stdio"]

[[backend]]
id = "openclaw-main"
family = "open_claw_gateway"

[backend.launch]
type = "gateway_ws"
endpoint = "ws://127.0.0.1:18789"
agent_id = "main"
team_helper_command = "/usr/local/bin/clawbro-team-cli"
lead_helper_mode = true

[[agent_roster]]
name = "claude"
mentions = ["@claude"]
backend_id = "native-main"
persona_dir = "/Users/yourname/.clawbro/personas/claude"
workspace_dir = "/Users/yourname/work/app1"

[[agent_roster]]
name = "reviewer"
mentions = ["@reviewer"]
backend_id = "codex-main"
persona_dir = "/Users/yourname/.clawbro/personas/reviewer"
workspace_dir = "/Users/yourname/work/app1"

[[agent_roster]]
name = "openclaw"
mentions = ["@openclaw"]
backend_id = "openclaw-main"
workspace_dir = "/Users/yourname/work/app1"

[[binding]]
kind = "scope"
agent = "claude"
scope = "group:lark:chat-123"
channel = "lark"

[[group]]
scope = "group:lark:chat-123"
name = "dev-team"

[group.mode]
interaction = "team"
front_bot = "claude"
channel = "lark"
auto_promote = true

[group.team]
roster = ["reviewer", "openclaw"]
public_updates = "minimal"
max_parallel = 3

[channels.lark]
enabled = true

[[cron_jobs]]
name = "daily-standup"
expr = "0 9 * * 1-5"
prompt = "请总结今天工作安排"
session_key = "lark:group:lark:chat-123"
enabled = true
agent = "claude"
```

注意：

- `session_key` 当前按 `channel:scope` 解析，如果 `scope` 自身也带冒号，建议你在实际环境里先验证它与目标会话一致
- 生产环境里更稳妥的做法，是先跑起来并观察实际 scope 形式，再写 cron 目标

## 21. 推荐的上手路径

### 场景 A：本地开发者第一次启动

只做这些：

- `claw_bro_native`
- `[agent].backend_id`
- `ws_token`
- 不接 IM
- 不配 team

目标：

- `/health` 正常
- WS 一条消息能得到 `TurnComplete`

### 场景 B：本地多 Agent

在场景 A 基础上加：

- `[[agent_roster]]`
- `persona_dir`
- `workspace_dir`
- `[[binding]]`

目标：

- `@mention` 正常切换 agent
- scope binding 正常默认路由

### 场景 C：群组 Team

在场景 B 基础上加：

- `[[group]]`
- `interaction = "team"`
- `front_bot`
- `group.team.roster`

目标：

- lead 可规划任务
- specialists 可执行
- task artifacts 落盘

### 场景 D：接 Lark / DingTalk

在场景 A/B/C 任一基础上再加：

- `[channels.lark]` 或 `[channels.dingtalk]`
- 对应环境变量
- 可选 allowlist

目标：

- 外部消息进入 gateway
- agent 处理后回写 channel

## 22. 常见故障

### 22.1 启动时报 `at least one [[backend]] entry is required`

原因：

- 没有配置任何 `[[backend]]`

处理：

- 至少加一个 backend catalog

### 22.2 启动时报 `agent.backend_id is required when no [[agent_roster]] is configured`

原因：

- 既没有 default backend，也没有 roster

处理：

- 二选一：
  - 配 `[agent].backend_id`
  - 或配置 `[[agent_roster]]`

### 22.3 启动时报 binding 相关错误

原因通常是：

- `[[binding]]` 使用了不存在的 agent
- 或没有 `[[agent_roster]]`

处理：

- 先定义 roster，再定义 binding

### 22.4 `claw_bro_native` 启动失败

常见原因：

- 捆绑路径下没有可执行的 `clawbro`
- 模型 API Key 没配
- `CLAWBRO_MODEL` 不可用

处理：

- 先单独运行 `clawbro runtime-bridge` 验证
- 或改成显式 `external_command` 启动

### 22.5 Lark / DingTalk 配了但没收到消息

优先检查：

- channel 是否 `enabled = true`
- 环境变量是否存在
- `/diagnostics/channels`
- 外部平台 webhook / stream / long connection 是否已正确配置

### 22.6 `/health` 是好的，但 backend 实际不可用

原因：

- `/health` 是只读健康摘要，不会每次都 live probe

处理：

- 看 `/status`
- 看 `/doctor`
- 看 `/diagnostics/backends`

### 22.7 Team 配了但没真正跑起来

优先检查：

- `front_bot` 是否存在于 roster
- `group.team.roster` 是否都是合法 agent
- `/diagnostics/teams`
- team artifact root 是否已创建

## 23. 从零开始的最短 checklist

如果你只想最快跑通，照这个做：

1. 编译 `clawbro`
2. 导出 `OPENAI_API_KEY`
3. 写最小 `~/.clawbro/config.toml`
4. 启动 `cargo run -p clawbro --bin clawbro -- serve`
5. 调 `curl /health`
6. 连 `/ws`
7. 发一条 `InboundMsg`
8. 确认收到 `TurnComplete`

只有这一步通过后，再继续加：

1. `agent_roster`
2. `binding`
3. `group/team`
4. `lark/dingtalk`
5. `cron`
6. `allowlist`

## 24. 相关文档

- [runtime-backends.md](/Users/fishers/Desktop/repo/quickai-openclaw/clawBro/docs/runtime-backends.md)
- [routing-contract.md](/Users/fishers/Desktop/repo/quickai-openclaw/clawBro/docs/routing-contract.md)
- [context-filesystem-contract.md](/Users/fishers/Desktop/repo/quickai-openclaw/clawBro/docs/context-filesystem-contract.md)
- [doctor-and-status.md](/Users/fishers/Desktop/repo/quickai-openclaw/clawBro/docs/operations/doctor-and-status.md)
