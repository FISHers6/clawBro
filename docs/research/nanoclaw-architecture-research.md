# NanoClaw 架构研究报告

> 研究目的：深度分析 nanoclaw 的设计哲学、Skills 配置体系、Channel 架构，作为 quickai-gateway 设计参考。
>
> 研究日期：2026-03-18
> 研究源码：`/Users/fishers/Desktop/repo/quickai-openclaw/nanoclaw`
> NanoClaw 版本：1.2.12

---

## 一、项目定位

NanoClaw 是一个**个人 AI 消息机器人守护进程**，核心理念：

> **"小到能看懂（~1000 行核心代码）、安全靠 OS 级隔离、定制靠 Skills 而非配置文件"**

它解决的问题：让 Claude 通过 WhatsApp/Telegram/Slack 等消息应用可达，而不需要理解几十万行代码。

**不是 CLI** — nanoclaw 是守护进程（daemon），用户通过 Claude Code skills 和消息 App 内发指令来交互。

---

## 二、整体架构

```
HOST（macOS / Linux）
│
├── Node.js 主进程 (src/index.ts)
│   ├── Channel 注册表 (自动检测已安装 Channel)
│   ├── 消息轮询循环（每 2s）
│   ├── 调度器循环（每 60s）
│   ├── IPC Watcher（每 1s）
│   └── GroupQueue（每组并发队列，全局上限 5）
│
├── SQLite DB (store/messages.db)
│   ├── messages       — 消息历史
│   ├── registered_groups — 已注册群组
│   ├── sessions       — 每组 Claude 会话
│   ├── scheduled_tasks — 定时任务
│   └── task_run_logs  — 任务执行日志
│
├── Credential Proxy (localhost:3001)
│   └── 拦截 API 调用，注入真实 Key（容器永远看不到 secrets）
│
└── 每条消息/任务 → 独立 Linux 容器
    ├── Mount: groups/{name}/ ← CLAUDE.md + 上下文
    ├── Mount: groups/global/ ← 全局 CLAUDE.md（共享记忆）
    ├── Mount: data/sessions/{name}/.claude/ ← 会话状态
    └── ANTHROPIC_BASE_URL → localhost:3001（Credential Proxy）
```

---

## 三、Skills 体系——核心设计亮点

### 3.1 核心理念：Skills 代替配置文件

NanoClaw **明确拒绝配置文件膨胀**。

普通项目的做法：
> "用户要加 Telegram？合并 PR #123，用户在 config.yaml 里配 bot_token。"

NanoClaw 的做法：
> "创建一个 `/add-telegram` skill，让 Claude Code 来执行：合并代码、收集 token、安装依赖、运行测试。"

### 3.2 Skill 的结构

```
.claude/skills/
├── setup/
│   └── SKILL.md        ← 安装引导（8 步）
├── add-whatsapp/
│   └── SKILL.md        ← 安装 WhatsApp channel
├── add-telegram/
│   └── SKILL.md        ← 安装 Telegram channel
├── add-slack/
│   └── SKILL.md
├── add-discord/
│   └── SKILL.md
├── add-gmail/
│   └── SKILL.md
├── update-nanoclaw/
│   └── SKILL.md        ← 把上游更新 merge 进定制 fork
├── customize/
│   └── SKILL.md        ← 交互式定制引导
├── debug/
│   └── SKILL.md        ← 故障排查
├── convert-to-apple-container/
│   └── SKILL.md
├── add-pdf-reader/
│   └── SKILL.md
├── add-image-vision/
│   └── SKILL.md
├── add-reactions/
│   └── SKILL.md
├── add-voice-transcription/
│   └── SKILL.md
├── add-ollama-tool/
│   └── SKILL.md        ← 本地 LLM 集成
├── add-parallel/
│   └── SKILL.md
├── add-compact/
│   └── SKILL.md
├── add-telegram-swarm/
│   └── SKILL.md        ← Agent Swarm（首个个人 AI 支持 Swarm）
├── update-skills/
│   └── SKILL.md
└── x-integration/
    └── SKILL.md        ← X/Twitter 集成
```

### 3.3 Skill 文件格式

```markdown
---
name: add-telegram
description: 添加 Telegram 作为消息 Channel
---

# Add Telegram Channel

## Phase 1: Pre-flight
检查 Node.js 版本、确认项目结构...

## Phase 2: Apply Code Changes
- 执行 git merge origin/skill-add-telegram
- 安装依赖

## Phase 3: Authentication
收集 TELEGRAM_BOT_TOKEN，写入 .env...

## Phase 4: Validation
运行 npm run build && npm test...

## Rollback
如有失败：git reset --hard <backup-tag>
```

### 3.4 Skill 执行流程

```
用户在 Claude Code 中输入 /add-telegram
    ↓
Claude Code 读取 .claude/skills/add-telegram/SKILL.md
    ↓
Claude 按步骤执行：
  1. git merge origin/skill-add-telegram（合并 Channel 代码）
  2. 询问用户："请提供 Telegram Bot Token"
  3. 写入 .env
  4. npm install（安装依赖）
  5. npm run build && npm test（验证）
    ↓
失败时：自动回滚到备份 tag
```

### 3.5 `update-nanoclaw` skill（重要设计）

解决问题：用户 fork 了 nanoclaw 并做了定制，如何同步上游更新而不丢失定制？

流程：
1. **Preflight**：检查 git 干净、检测 upstream remote
2. **Backup**：创建带时间戳的备份 branch/tag
3. **Preview**：`git log` 和 `git diff` 对比 merge base，分类显示变更
4. **更新方式选择**：merge / cherry-pick / rebase / abort
5. **冲突处理**：只打开冲突文件，保留用户定制
6. **验证**：`npm run build && npm test`
7. **Breaking changes 检查**：扫描 CHANGELOG.md 中的 `[BREAKING]`，提示运行迁移 skill
8. **回滚**：打印 `git reset --hard pre-update-<hash>-<timestamp>`

---

## 四、配置体系（不靠配置文件）

### 4.1 三层配置

| 层级 | 位置 | 内容 |
|------|------|------|
| 环境变量 | `.env` 文件 | API Key、Bot Token、性能参数 |
| 文件配置 | `~/.config/nanoclaw/` | Mount 白名单、Sender 白名单（项目外，防篡改） |
| 代码修改 | `src/config.ts` 等 | 行为定制（推荐直接改代码，fork 模式）|

### 4.2 完整环境变量列表

| 变量 | 默认值 | 用途 |
|------|--------|------|
| `ASSISTANT_NAME` | `Andy` | 触发词（如 `@Andy`）|
| `ASSISTANT_HAS_OWN_NUMBER` | `false` | WhatsApp 是否使用独立号码 |
| `ANTHROPIC_API_KEY` | — | Anthropic API Key |
| `CLAUDE_CODE_OAUTH_TOKEN` | — | Claude Code OAuth Token |
| `ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | 自定义 API 端点 |
| `CONTAINER_TIMEOUT` | `1800000` | 容器最长运行时间（ms，30 分钟）|
| `MAX_CONCURRENT_CONTAINERS` | `5` | 全局容器并发上限 |
| `IDLE_TIMEOUT` | `1800000` | 容器空闲超时（ms）|
| `CONTAINER_IMAGE` | `nanoclaw-agent:latest` | 容器镜像名 |
| `CREDENTIAL_PROXY_PORT` | `3001` | Credential Proxy 端口 |
| `POLL_INTERVAL` | `2000` | 消息轮询间隔（ms）|
| `SCHEDULER_POLL_INTERVAL` | `60000` | 调度器轮询间隔（ms）|
| `IPC_POLL_INTERVAL` | `1000` | IPC Watcher 轮询间隔（ms）|
| `TZ` | 系统 | Cron 表达式时区 |
| `LOG_LEVEL` | `info` | 日志级别 |
| `TELEGRAM_BOT_TOKEN` | — | Telegram Bot Token（安装 skill 后）|
| `SLACK_BOT_TOKEN` | — | Slack Bot Token（安装 skill 后）|
| `DISCORD_BOT_TOKEN` | — | Discord Bot Token（安装 skill 后）|

### 4.3 Mount 白名单（项目外，防篡改）

**路径**：`~/.config/nanoclaw/mount-allowlist.json`

```json
{
  "allowedRoots": [
    {
      "path": "~/projects",
      "allowReadWrite": true,
      "description": "开发项目目录"
    },
    {
      "path": "~/Documents/work",
      "allowReadWrite": false,
      "description": "工作文档（只读）"
    }
  ],
  "blockedPatterns": [".ssh", ".gnupg", "password", "secret", "token"],
  "nonMainReadOnly": true
}
```

规则：
- 非 main 组只能 read-only mount（除非 `nonMainReadOnly: false`）
- 阻止包含敏感关键词的路径 mount
- 只允许 allowedRoots 下的目录被 mount

### 4.4 Sender 白名单（可选）

**路径**：`~/.config/nanoclaw/sender-allowlist.json`

```json
{
  "allowMode": "allow",
  "groups": {
    "12345@g.us": {
      "allowedSenders": ["+8613800138000"]
    }
  },
  "logDenied": true
}
```

---

## 五、Channel 架构

### 5.1 自注册模式

Channel 通过 barrel import 自动注册，安装 skill 后自动生效：

```typescript
// src/channels/registry.ts — 注册表
const registry = new Map<string, ChannelFactory>();

export function registerChannel(name: string, factory: ChannelFactory) {
  registry.set(name, factory);
}

// src/channels/whatsapp.ts（add-whatsapp skill 安装后才有这个文件）
registerChannel('whatsapp', (opts) => {
  if (!fs.existsSync(authPath)) return null; // 没有认证，跳过
  return new WhatsAppChannel(opts);
});

// src/channels/index.ts — barrel，每个 skill 在这里加一行 import
import './whatsapp.js';
import './telegram.js';
// ...
```

启动时：
```typescript
for (const [name, factory] of registry.entries()) {
  const channel = factory(channelOpts);
  if (!channel) {
    logger.warn(`${name}: 缺少凭据，跳过`);
    continue;
  }
  await channel.connect();
}
```

### 5.2 支持的 Channel（通过 skills 安装）

| Channel | Skill | 认证方式 |
|---------|-------|---------|
| WhatsApp | `/add-whatsapp` | QR 码或配对码 |
| Telegram | `/add-telegram` | Bot Token |
| Discord | `/add-discord` | Bot Token |
| Slack | `/add-slack` | Socket Mode（Bot Token + App Token）|
| Gmail | `/add-gmail` | OAuth2 |
| X/Twitter | `/x-integration` | 浏览器自动化 |

**核心（默认零 Channel）**：nanoclaw 开箱没有任何 Channel，全靠 skill 安装。

---

## 六、LLM 支持

### 主要：Claude Agent SDK

使用 `@anthropic-ai/claude-agent-sdk`（v0.2.29），运行在容器内。

### 支持自定义端点

```bash
ANTHROPIC_BASE_URL=https://your-compatible-endpoint.com
ANTHROPIC_AUTH_TOKEN=your-token
```

可接入：
- Ollama（本地，skill: `/add-ollama-tool`）
- Together AI、Fireworks（Anthropic 兼容端点）
- 任何 Anthropic API 兼容服务

### Credential Proxy

- 运行在 `localhost:3001`
- 容器只知道这个地址，不知道真实 Key
- 支持 API Key 模式和 OAuth Token 模式

---

## 七、消息流程

### 7.1 收到消息

```
1. Channel 收到消息（如 WhatsApp）
2. 存入 SQLite messages 表
3. 消息循环（2s）拉取新消息
4. 检测触发词（`@Andy`，非 main 组必须有）
5. GroupQueue 排队（全局 5 并发上限）
6. spawn 容器（Mount 工作目录、全局记忆、会话）
7. 容器内 Claude 处理消息（读 CLAUDE.md、调工具）
8. Agent 通过 IPC 写回响应
9. IPC Watcher 检测到响应文件，路由到 Channel
10. Channel 发送到用户
```

### 7.2 IPC 通信

容器内 Agent 通过**写文件**向主进程通信：

```json
// data/ipc/{groupFolder}/messages/{uuid}.json
{
  "type": "message",
  "chatJid": "1234567890@s.whatsapp.net",
  "text": "这是 Agent 的回复"
}
```

IPC Watcher（每 1s）扫描目录，找到文件后发送，然后删除。

---

## 八、定时任务

### 创建方式

用户在消息中告诉 Agent：
> `@Andy 每个工作日早上 9 点给我发今日简报`

Agent 写入 SQLite `scheduled_tasks` 表，调度器自动执行。

### 支持的任务类型

| 类型 | `schedule_type` | `schedule_value` 格式 | 示例 |
|------|----------------|----------------------|------|
| Cron | `cron` | 5 字段 cron 表达式 | `0 9 * * 1-5` |
| 间隔 | `interval` | 毫秒数 | `60000` |
| 一次性 | `once` | ISO 8601 时间戳 | `2026-04-01T09:00:00Z` |

### 执行上下文

- 在对应 group 的容器里运行
- 有访问 group `CLAUDE.md` 的权限
- 可以调用 `send_message` 工具主动发消息给用户

---

## 九、记忆体系

### 三层记忆

| 层级 | 位置 | 范围 | 写权限 |
|------|------|------|--------|
| Group 记忆 | `groups/{name}/CLAUDE.md` | 当前组 | 当前组 Agent |
| 全局记忆 | `groups/CLAUDE.md` | 所有组 | 仅 main 组 |
| 会话状态 | `data/sessions/{name}/.claude/` | 当前组 | 当前组 Agent |

### main 组特权

main 组（通常是用户与自己的单聊）：
- 无需触发词
- 可写全局记忆
- 可管理所有组的任务
- 可配置其他组的 mount

---

## 十、安全模型

### OS 级隔离（不是权限检查）

Agent **在 Linux 容器里运行**，不是靠权限检查：
- 只能看到显式 mount 的目录
- 网络请求经过 Credential Proxy（API Key 不暴露给容器）
- PID、网络、文件系统全部隔离

### Credential Proxy

```
容器 → ANTHROPIC_BASE_URL=http://localhost:3001
    ↓
Credential Proxy 注入真实 x-api-key header
    ↓
api.anthropic.com
```

容器看到的是：
```
Authorization: Bearer placeholder-token
```

代理替换为：
```
x-api-key: sk-ant-real-key-here
```

---

## 十一、安装流程（`/setup` skill）

```bash
# 1. Fork 并 clone
gh repo fork qwibitai/nanoclaw --clone
cd nanoclaw

# 2. 运行 Claude Code
claude

# 3. 在 Claude Code 中执行 setup skill
/setup
```

`/setup` 自动完成 8 步：

| 步骤 | 内容 |
|------|------|
| 0 | Git/Fork 检查，配置 upstream |
| 1 | Bootstrap：检查 Node.js 20+，`npm ci` |
| 2 | 检查运行环境（Docker/Apple Container 可用性）|
| 3 | 构建容器镜像 |
| 4 | Claude 认证（API Key 或 OAuth Token）|
| 5 | Channel 选择与安装（多选，每个调用对应 skill）|
| 6 | Mount 白名单配置 |
| 7 | OS 服务注册（launchd/systemd）|
| 8 | 健康验证 |

### 启动服务

```bash
npm start              # 直接启动
npm run dev            # 开发模式（tsx，热重载）
# 或通过 OS 服务自动启动（/setup 已注册）
```

---

## 十二、对比 zeroclaw

| 维度 | NanoClaw | ZeroClaw |
|------|----------|----------|
| 语言 | Node.js + TypeScript | Rust |
| 定位 | 个人消息机器人 | 零开销 Agent 运行时 |
| 配置方式 | Skills + 代码改动（无配置文件）| TOML 配置文件（丰富字段）|
| Channel 安装 | Skill 合并代码（零默认 Channel）| 配置文件配置（多 Channel 内置）|
| CLI | 无（daemon + skills）| 完整 clap CLI（25+ 子命令）|
| 隔离 | Linux 容器（OS 级）| 进程级 trait 隔离 |
| 记忆 | CLAUDE.md 文件（per-group + 全局）| SQLite + Markdown + 向量 |
| LLM | Claude Agent SDK + 自定义端点 | 25+ Provider（openrouter/anthropic/...）|
| 硬件目标 | 标准 macOS/Linux | $10 SBC，<5MB RAM |
| 代码量 | ~1000 行核心 | 数万行 |
| 核心哲学 | "能看懂，通过 AI 定制" | "零开销，所有功能内置" |

---

## 十三、对 quickai-gateway 的设计参考

### 最值得学习的设计

| NanoClaw 设计 | 可借鉴之处 |
|-------------|----------|
| Skills = 自注册 Channel（合并代码）| Channel 配置复杂度降低；按需安装减少维护负担 |
| `update-nanoclaw` skill | 定制 fork + 上游同步，解决"个人定制 vs 社区更新"矛盾 |
| Credential Proxy | 多 backend 凭据安全注入（gateway 已有类似思路，但可以更显式）|
| 文件夹即上下文（`groups/{name}/CLAUDE.md`）| quickai-gateway 的 persona/workspace 契约已有，值得对齐 |
| IPC via 文件系统 | 简单可靠，适合跨进程通信（当前 gateway 用 WebSocket，不同场景）|
| main 组特权设计 | quickai-gateway 的 Solo/Lead 角色设计与此类似 |

### quickai-gateway 有而 NanoClaw 没有的

| quickai-gateway 特有 | 说明 |
|--------------------|------|
| Team/Lead/Specialist 编排 | NanoClaw 只有 Swarm（实验性）|
| ACP 协议（codex/claude-code CLI 接入）| NanoClaw 只接 Claude API |
| DingTalk/Lark 企业 IM | NanoClaw 无企业 IM Channel |
| Cron → IM Channel 发送 | NanoClaw 的 cron 也发消息，但接口更简单 |
| 多 backend 路由（roster/binding）| NanoClaw 单 Claude 实例 |

---

*参考文件：*
- `nanoclaw/src/index.ts`（主编排器）
- `nanoclaw/src/config.ts`（环境变量配置）
- `nanoclaw/src/channels/registry.ts`（Channel 注册表）
- `nanoclaw/.claude/skills/`（全部 Skills）
- `nanoclaw/docs/SPEC.md`（架构规范）
- `nanoclaw/docs/nanoclaw-architecture-final.md`（Skills 架构深度分析）
- `nanoclaw/README.md` & `README_zh.md`
