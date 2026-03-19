# ClawBro Setup Guide

这份文档专门讲一件事：

- 如何从用户视角安装、配置、校验并启动 ClawBro

如果你想先看项目定位和案例，先读：

- [`../README.md`](../README.md)

如果你想看更完整的运行时和配置背景，继续读：

- [`getting-started-from-zero.md`](getting-started-from-zero.md)

---

## 1. Prerequisites

至少准备这些：

- Rust toolchain
- Cargo
- 一个 provider API Key
- 本仓库源码

当前 workspace 使用的 Rust 版本锁在：

- `rust-toolchain.toml`

建议先确认：

```bash
rustc --version
cargo --version
```

---

## 2. Build

在仓库根内进入 gateway 子项目：

```bash
cd clawBro
```

编译用户入口：

```bash
cargo build -p clawbro --bin clawbro
```

如果你要 release 产物：

```bash
cargo build -p clawbro --bin clawbro --release
```

---

## 3. Runtime Layout

默认运行目录在：

- `~/.clawbro/config.toml`
- `~/.clawbro/.env`
- `~/.clawbro/sessions/`
- `~/.clawbro/shared/`
- `~/.clawbro/skills/`
- `~/.clawbro/personas/`

`clawbro setup` 会自动创建这些目录。

---

## 4. First-Time Setup

### Interactive

最简单的方式：

```bash
clawbro setup
```

向导会依次引导你：

- 选择语言
- 选择 provider
- 输入 API key
- 选择模式
- 可选输入 WebSocket token
- 可选接入 Lark / DingTalk

### Non-interactive

适合脚本、CI、批量部署。

```bash
clawbro setup \
  --lang en \
  --provider anthropic \
  --api-key sk-ant-xxx \
  --mode solo \
  --non-interactive
```

---

## 5. Setup Modes

### Solo

最轻量的单 Agent 模式。

```bash
clawbro setup \
  --lang en \
  --provider anthropic \
  --api-key sk-ant-xxx \
  --mode solo \
  --non-interactive
```

生成结果重点：

- `[agent] backend_id = "native-main"`
- `[[backend]] family = "claw_bro_native"`

适合：

- 个人助理
- 本地开发辅助
- 单用户问答

### Multi

当前 `setup --mode multi` 主要生成多 agent 配置注释模板，不会自动推断完整 roster。

适合：

- 你已经知道自己要配置哪些 `agent_roster`
- 准备手工补充多 agent 名称和 binding

### Team

这是当前 setup 最完整的一条配置链。

支持：

- `front_bot`
- specialist 列表
- `team_target`
- `team_scope`
- `team_name`

#### Team 参数

| 参数 | 说明 |
| --- | --- |
| `--front-bot` | Lead 名称，默认 `lead` |
| `--specialist` | specialist 名称，可重复传入 |
| `--team-target` | `direct-message` 或 `group` |
| `--team-scope` | 目标 scope |
| `--team-name` | 可读名称 |

#### 交互模式体验

Team 模式下：

- 会询问 `front_bot`
- specialist 逐个输入，空输入结束
- 拿到 channel 后才询问 `scope/name`

#### Team: Direct Message

```bash
clawbro setup \
  --lang en \
  --provider anthropic \
  --api-key sk-ant-xxx \
  --mode team \
  --team-target direct-message \
  --front-bot planner \
  --specialist coder \
  --specialist reviewer \
  --team-scope user:ou_your_user_id \
  --team-name my-team \
  --non-interactive
```

会生成：

- `[[agent_roster]]`
- `[[team_scope]]`
- `[team_scope.mode] interaction = "team"`
- `[team_scope.team] roster = [...]`

#### Team: Group

```bash
clawbro setup \
  --lang en \
  --provider anthropic \
  --api-key sk-ant-xxx \
  --mode team \
  --team-target group \
  --front-bot planner \
  --specialist coder \
  --specialist reviewer \
  --team-scope group:lark:chat-123 \
  --team-name ops-room \
  --non-interactive
```

会生成：

- `[[group]]`
- `[group.mode] interaction = "team"`
- `[group.team] roster = [...]`

#### Team defaults

如果你不显式传 `team_scope/team_name`，会按 channel 和 target 生成默认值。

Direct Message:

- 无 channel: `user:default`
- Lark: `user:ou_your_user_id`
- DingTalk: `user:ding_your_user_id`

Group:

- 无 channel: `group:default`
- Lark: `group:lark:chat-123`
- DingTalk: `group:dingtalk:conversation-123`

---

## 6. Provider Setup

当前原生 runtime 最常见的 key 读取优先级：

1. `ANTHROPIC_API_KEY`
2. `OPENAI_API_KEY`
3. `DEEPSEEK_API_KEY`

setup 会把 key 写入：

- `~/.clawbro/.env`

启动前加载：

```bash
source ~/.clawbro/.env
```

也可以后续用 auth 子命令更新：

```bash
clawbro auth set anthropic sk-ant-xxx
clawbro auth list
clawbro auth check anthropic
```

---

## 7. Lark / DingTalk

setup 支持在向导里直接录入 channel 配置。

### Lark

需要：

- App ID
- App Secret
- 可选 bot name

### DingTalk

需要：

- client_id / AppKey
- client_secret / AppSecret
- 可选 agent_id
- 可选 bot name

这些配置会直接写入 `config.toml`，对应 key 也会写进 `.env`。

### DingTalk Custom Robot Webhook

这是和 stream mode 并行的另一条接入方式，不会替代现有 `dingtalk`。

当前 phase 1 支持面：

- 自定义机器人群聊 webhook 入站
- 仅处理群聊 `@` 机器人消息
- 复用现有 `InboundMsg -> spawn_im_turn -> Channel::send()` 主链
- 回复优先走钉钉提供的 `sessionWebhook`
- richText 文本与图片消息的异步解析

当前 phase 1 非目标：

- 不支持 1:1 私聊 webhook
- 不在 webhook handler 内同步跑重逻辑
- 不把 webhook mode 扩展成完整的通用主动出站系统

`setup` 现在支持在交互向导里选择 DingTalk `stream` 或 `webhook` 模式。

如果你在向导里选择 `Webhook Mode`，会生成和下面等价的配置：

```toml
[channels.dingtalk_webhook]
enabled = true
secret_key = "SECxxxxxxxx"
webhook_path = "/dingtalk-channel/message"
access_token = "dt_access_token_xxx"
presentation = "final_only"
```

说明：

- `stream` mode
  - 使用 `client_id / client_secret / agent_id`
  - 适合开放平台 app / stream 路径
- `webhook` mode
  - 使用 `secret_key`
  - 可选 `access_token`
  - 适合自定义机器人群聊 webhook 路径
- `secret_key`
  - 钉钉自定义机器人安全密钥
- `webhook_path`
  - 你的公网回调路径，默认建议独立出来，不和 stream mode 复用
- `presentation`
  - 和其他 channel 一样，控制 IM 进度展示风格
- `access_token`
  - 可选
  - 用于 `sessionWebhook` 过期后的 robot-level fallback 出站
  - 当前 phase 2 只把它作为 fallback，不把它扩展成完整主动出站能力

回调地址示例：

```text
https://your-domain.example/dingtalk-channel/message
```

allowlist 仍然复用全局：

- `~/.clawbro/allowlist.json`

当前 webhook mode 继续按 `dingtalk` channel key 做 allowlist 判断，不会引入第二套来源。

`clawbro doctor` 也会检查：

- `dingtalk_webhook.secret_key`
- `dingtalk_webhook.webhook_path`
- `access_token` 是否配置了 fallback

当前 reply 语义：

- 优先使用 webhook 入站携带的 `sessionWebhook`
- `clawbro` 会按 `session_key.scope` 缓存最近的 `sessionWebhook + expired_time`
- 如果即时回复缺少 `thread_ts`，但 scope 下还有未过期 lease，仍可继续回发
- 如果 lease 已过期且配置了 `access_token`，会退化到 DingTalk custom robot 的 `robot/send` fallback

这意味着 phase 2 现在已经支持：

- 即时回复
- 短时间延迟回复
- 过期后的 bot-level fallback
- richText 中文本和图片占位的异步补全

但仍然不等价于完整的主动出站系统。

---

## 8. Validate

无论是手写配置还是 setup 生成，下一步都应该先校验：

```bash
clawbro config validate
```

这个检查会验证：

- TOML 语法
- backend topology
- roster / front_bot / specialist 引用关系
- Team scope / group 配置一致性

如果这一步不过，不要直接启动服务。

---

## 9. Start

### Standard

```bash
source ~/.clawbro/.env
clawbro serve
```

### Override config or port

```bash
clawbro serve --config /path/to/config.toml --port 18080
```

也支持环境变量：

```bash
export CLAWBRO_CONFIG=/path/to/config.toml
export CLAWBRO_PORT=18080
clawbro serve
```

### Random port

如果配置中：

```toml
[gateway]
port = 0
```

最终端口会写到：

- `~/.clawbro/gateway.port`

---

## 10. Basic Health Checks

启动后建议立刻检查：

```bash
curl http://127.0.0.1:8080/health
curl http://127.0.0.1:8080/status
```

也可以用 CLI：

```bash
clawbro status
clawbro doctor
```

---

## 11. Recommended Onboarding Path

最稳的上手顺序：

1. `setup --mode solo`
2. `config validate`
3. `serve`
4. 再切到 `team`
5. 再接 Lark / DingTalk
6. 最后再引入 ACP backends / OpenClaw

不要一开始同时做：

- Team
- IM
- ACP
- 自定义 bindings

这样排错成本会明显更低。

---

## 12. Common Commands

| 命令 | 作用 |
| --- | --- |
| `clawbro setup` | 初始化配置 |
| `clawbro serve` | 启动服务 |
| `clawbro config show` | 查看配置 |
| `clawbro config validate` | 校验配置 |
| `clawbro config edit` | 编辑配置 |
| `clawbro auth set` | 更新 API key |
| `clawbro auth list` | 查看 key 列表 |
| `clawbro auth check [provider]` | 在线检查 key |
| `clawbro status` | 查看当前状态 |
| `clawbro doctor` | 做故障诊断 |

---

## 13. Where To Go Next

- Runtime backend 设计：
  - [`runtime-backends.md`](runtime-backends.md)
- 更完整的从零部署：
  - [`getting-started-from-zero.md`](getting-started-from-zero.md)
- 路由和 Team contract：
  - [`routing-contract.md`](routing-contract.md)
- 运维接口：
  - [`operations/doctor-and-status.md`](operations/doctor-and-status.md)
