# Slock.ai 功能全景与 clawBro 对标分析报告

> **作者:** 研究分析
> **日期:** 2026-03-24
> **信息来源:** Slock.ai 官网、社区复刻项目 `slock-daemon-ts`、clawBro 源码全量审计
> **目标读者:** clawBro 产品/工程团队

---

## 目录

1. [研究背景与方法](#一研究背景与方法)
2. [Slock.ai 产品定位](#二slockai-产品定位)
3. [Slock.ai 完整功能清单](#三slockai-完整功能清单)
4. [Slock.ai 架构原理深度解析](#四slockai-架构原理深度解析)
5. [clawBro 当前能力全景](#五clawbro-当前能力全景)
6. [功能对标矩阵](#六功能对标矩阵)
7. [架构模型对比](#七架构模型对比)
8. [差距分析与优劣势判断](#八差距分析与优劣势判断)
9. [clawBro 超越 Slock.ai 的领域](#九clawbro-超越-slockai-的领域)
10. [结论与建议](#十结论与建议)

---

## 一、研究背景与方法

本报告的目的是对 Slock.ai 进行完整的功能拆解，并与 clawBro 的当前实现逐项对标，得出客观的差距分析结论。

**信息来源分层：**

| 来源层级 | 内容 | 可信度 |
|---|---|---|
| Slock 官网 (`slock.ai`) | 产品定位、营销文案、功能描述 | 高（产品层） |
| 社区复刻 `slock-daemon-ts` | Daemon 协议、Agent 生命周期、MCP 工具实现 | 中高（行为对齐声明） |
| Anthropic 官方文档 | Claude Code stream-json 模式、MCP 协议 | 高（一手资料） |
| OpenAI 官方文档 | Codex CLI `--json` 事件流、MCP 配置 | 高（一手资料） |
| clawBro 源码 | clawBro 所有已实现能力 | 最高（直接代码审计） |

**重要注意：** Slock.ai 服务端实现未公开，本报告对服务端行为的部分描述为强推断，基于 daemon 协议和社区复刻行为推导。

---

## 二、Slock.ai 产品定位

Slock.ai 的核心产品主张是：

> **"让 AI Agent 成为你团队里的同事"** —— 在一个类 Slack 的协作空间里，人类和 Agent 是平等的参与者，可以在同一个频道里自然对话、分配任务、协同完成工作。

**三个核心产品承诺：**

1. **Agents That Remember** — Agent 跨会话保持记忆，不会每次"忘掉一切重新开始"
2. **Your Machines, Your Agents** — Agent 运行在用户本地机器上，不是云端黑盒
3. **Hibernate/Wake** — Agent 空闲时休眠节省资源，有新消息时自动唤醒

**产品本质：** 本地 Daemon 进程管理器 + 云端消息路由服务器 + MCP 工具注入的 CLI Agent 适配层。

---

## 三、Slock.ai 完整功能清单

### 3.1 通信与协作空间

| 功能 | 描述 | 实现方式 |
|---|---|---|
| **频道（Channel）** | 公开频道（如 #all、#dev），所有成员（人类+Agent）可见 | 云端服务器维护频道订阅关系 |
| **私信（DM）** | 人类与 Agent 之间、Agent 与 Agent 之间的私信 | `send_message(channel="DM:@name")` |
| **@mention 路由** | @某个 Agent 触发定向消息，Agent 感知到被 @ | 频道广播 + Agent 自行解析 @mention |
| **群组广播（Fan-out）** | 频道消息分发给所有订阅该频道的 Agent | `agent:deliver` 事件逐一投递 |
| **消息历史查询** | Agent 可查询频道或 DM 的历史消息，支持翻页 | `read_history(channel, before, after, limit)` |
| **人类参与** | 人类可在频道/DM 中直接发言，与 Agent 混合对话 | `sender_type: human/agent` 统一消息格式 |
| **One Conversation** | 所有 runtime 的发言聚合在同一对话空间里 | 所有发送均通过 `send_message` → 同一频道 |

### 3.2 Agent 生命周期管理

| 功能 | 描述 | 实现方式 |
|---|---|---|
| **Agent 启动（Start）** | 服务端下发 `agent:start` 指令，Daemon 启动对应 CLI 进程 | Daemon 进程管理器 + runtime driver |
| **Agent 停止（Stop）** | 服务端下发 `agent:stop`，Daemon 终止进程 | SIGTERM + 进程清理 |
| **Agent 休眠（Hibernate）** | 服务端下发 `agent:sleep`，Daemon 杀掉进程但**保留语义状态为 sleeping** | workspace 文件保留，进程销毁 |
| **Agent 唤醒（Wake）** | 新消息到达时 Daemon 重新拉起进程 | `agent:start` + workspace 恢复 |
| **Always-On 体验** | Agent 空闲休眠，有消息自动唤醒，用户感知不到断线 | Hibernate/Wake 无缝衔接 |

### 3.3 运行时适配（Runtime Driver）

| 功能 | 描述 | 实现方式 |
|---|---|---|
| **Claude Code 适配** | 以 `--output-format stream-json --input-format stream-json` 启动 Claude Code | 事件流解析：每行一个 JSON event |
| **Codex CLI 适配** | 以 `codex exec --json` 启动，解析 JSONL 事件流 | event types: thread.started, turn.completed 等 |
| **Gemini CLI 适配** | 推断适配 Gemini CLI（日志显示 detected） | 类似 driver 模式 |
| **Runtime 自动检测** | Daemon 启动时探测本机可用 runtime（claude/codex/gemini） | 路径探测 + `detected runtimes` 上报给 server |
| **多 Runtime 共存** | 同一机器可同时运行多个不同 runtime 的 Agent | 各自独立进程，独立 workspace |

### 3.4 MCP 工具注入（Chat Bridge）

这是 Slock.ai 的核心技术亮点，通过 MCP 把"社交能力"注入到任意 CLI Agent：

| MCP 工具 | 功能描述 | API 形态 |
|---|---|---|
| **`send_message`** | 向频道或 DM 发送消息 | `send_message(channel="#all", content="...")` 或 `send_message(channel="DM:@codex", ...)` |
| **`receive_message`** | 阻塞等待新消息（Agent 主循环） | `receive_message(block=true)` |
| **`list_server`** | 查询当前 server 的频道列表、在线 Agent 列表、在线人类列表 | 返回 `{ channels, agents, humans }` |
| **`read_history`** | 读取频道/DM 历史消息，支持 before/after 翻页 | `read_history(channel, before, after, limit)` |

**实现形式：** `chat-bridge` 是一个 MCP stdio server（`StdioServerTransport`），Daemon 在启动 CLI Agent 时将其注入为 MCP server，CLI Agent 的 tool use 系统自动识别并可调用。

### 3.5 记忆与 Workspace 系统

| 功能 | 描述 | 实现方式 |
|---|---|---|
| **每 Agent 独立 workspace** | `~/.slock/agents/<agentId>/` 目录 | Daemon 在启动前创建并设为 cwd |
| **MEMORY.md 跨会话记忆** | 每个 Agent 有自己的 `MEMORY.md`（role、key knowledge、active context 等 section） | System prompt 要求 Agent 启动时读取、更新 |
| **notes/ 细节存储** | 记忆细节写入 `notes/` 子目录 | MEMORY.md 作为索引，notes/ 存储详细内容 |
| **System Prompt 操作规程** | 启动步骤第 1 条："Read MEMORY.md (in your cwd)" | 直接写入 system prompt 强制执行 |
| **cwd 持久化声明** | "Your cwd is persistent across sessions" | System prompt 告知，配合 workspace 目录实现 |
| **记忆默认隔离** | 各 Agent 的 MEMORY/notes 互不可见，除非主动发布到频道 | 独立目录结构天然隔离 |

### 3.6 消息可靠性

| 功能 | 描述 | 实现方式 |
|---|---|---|
| **可靠投递队列** | `agent:deliver` 带 `seq` 序号，Daemon 回 `agent:deliver:ack` | 类消息队列 ACK 机制 |
| **断线重连** | Daemon 与 Server 的 WebSocket 连接断开后自动重连 | 指数退避（backoff）重连策略 |
| **本地消息队列** | Daemon 本地维护待投递消息队列 | 与 server 队列配合实现 at-least-once 语义 |

### 3.7 多机器/多 Daemon 支持

| 功能 | 描述 | 实现方式 |
|---|---|---|
| **Machine Key 认证** | 每台机器有唯一 `sk_machine_...` 密钥 | WS 连接时 querystring 携带 key |
| **能力声明** | Daemon 启动时上报本机能力（runtimes、hostname、OS、daemonVersion） | `ready` 消息 payload |
| **多机器同时接入** | 不同机器的 Agent 可以在同一 Server 协作空间中共存 | Server 端维护 machine ↔ agent 映射 |

### 3.8 产品层功能（推断）

| 功能 | 描述 | 可信度 |
|---|---|---|
| **Web/App 界面** | 类 Slack 的聊天界面，人类通过 UI 与 Agent 交流 | 推断（官网截图） |
| **Agent 配置管理** | 通过控制台创建/配置 Agent（名称、runtime、绑定机器等） | 推断（产品功能逻辑） |
| **团队协作空间** | 多人共享同一 Server，共同管理 Agent 团队 | 推断（官网描述） |
| **历史消息持久化** | 频道和 DM 历史在 Server 侧持久化存储 | 推断（`read_history` 需要服务端存储） |

---

## 四、Slock.ai 架构原理深度解析

### 4.1 系统总体架构

```
┌─────────────────────────────────────────────────────────┐
│               Slock Cloud Server (api.slock.ai)          │
│  ┌────────────┐  ┌───────────┐  ┌──────────────────┐   │
│  │ 频道/DM    │  │ 消息存储  │  │  Agent 配置管理  │   │
│  │ 成员管理   │  │ 历史查询  │  │  机器注册中心    │   │
│  └────────────┘  └───────────┘  └──────────────────┘   │
└─────────────────┬───────────────────────────────────────┘
                  │ WebSocket (ws(s)://api.slock.ai/daemon/connect?key=...)
                  │ 断线重连 + 心跳
┌─────────────────┴───────────────────────────────────────┐
│         Machine Daemon (npx @slock-ai/daemon)            │
│  ┌──────────────────────────────────────────────────┐   │
│  │ ready → 上报 {capabilities, runtimes, agents}    │   │
│  │ agent:start   → 启动 CLI Agent 进程              │   │
│  │ agent:stop    → 终止进程                         │   │
│  │ agent:sleep   → 休眠（kill 进程，保留 workspace）│   │
│  │ agent:deliver → 本地队列 + ACK                   │   │
│  └──────────────────────────────────────────────────┘   │
│  ┌──────────────────────────────────────────────────┐   │
│  │           进程管理（每个 Agent 一个子进程）       │   │
│  │  Claude Code │ Codex CLI │ Gemini CLI │ 其他     │   │
│  │  driver: stream-json  │  --json JSONL  │ ...      │   │
│  └──────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
         ↓ MCP stdio server 注入 (chat-bridge)
┌─────────────────────────────────────────────────────────┐
│            CLI Agent Runtime（如 Claude Code）           │
│  ┌──────────────────────────────────────────────────┐   │
│  │  System Prompt:                                  │   │
│  │   1. Read MEMORY.md                              │   │
│  │   2. receive_message(block=true) 等待消息        │   │
│  │   3. 处理消息，调用工具，回复                    │   │
│  │   4. 更新 MEMORY.md                              │   │
│  └──────────────────────────────────────────────────┘   │
│  Available MCP Tools:                                    │
│  • send_message    • receive_message                     │
│  • list_server     • read_history                        │
└─────────────────────────────────────────────────────────┘
```

### 4.2 消息分发机制

Slock 的多 Agent 消息分发是**去中心化**的：

1. 人类在 `#all` 频道发消息
2. Server 对该频道所有订阅 Agent 执行 `agent:deliver`（fan-out）
3. 每个 Agent 的 Daemon 收到消息，投入本地队列并 ACK
4. 每个 Agent 的 `receive_message(block=true)` 解除阻塞
5. 每个 Agent **自行决定是否回复**（由 system prompt 约定行为）

```
Human: "帮我分析这段代码"
    ↓ server fan-out
    ├── agent:deliver → Claude Agent → 自行判断是否回复
    ├── agent:deliver → Codex Agent → 自行判断是否回复
    └── agent:deliver → Gemini Agent → 自行判断是否回复
```

### 4.3 Agent 协作模型

Slock 的协作本质是**消息传递（Message Passing）**，不是共享内存：

- Agent A 知道 B 存在（通过 `list_server`）
- Agent A 向 B 发 DM（`send_message(channel="DM:@B", ...)`）
- B 的 `receive_message` 收到，处理，回复
- 全程对人类透明可见（因为消息在 Server 上，人类可以看到）

### 4.4 记忆模型

```
~/.slock/agents/
├── claude-agent-001/
│   ├── MEMORY.md          ← 跨会话记忆索引
│   ├── notes/             ← 细节记忆文件
│   │   ├── project-context.md
│   │   └── user-preferences.md
│   └── [工作文件...]      ← Agent 操作的代码/文档
├── codex-agent-001/
│   ├── MEMORY.md
│   └── notes/
└── gemini-agent-001/
    ├── MEMORY.md
    └── notes/
```

---

## 五、clawBro 当前能力全景

### 5.1 通信与消息处理

| 功能 | 实现状态 | 核心文件 |
|---|---|---|
| **钉钉（DingTalk）Channel** | ✅ 完整实现 | `channels_internal/dingtalk*.rs` |
| **飞书（Lark）Channel** | ✅ 完整实现 | `channels_internal/lark.rs` |
| **WebSocket 实时通信** | ✅ 完整实现 | `gateway/ws_handler.rs` |
| **Web Chat REST API** | ✅ Phase 3 实现 | `gateway/api/chat.rs` |
| **@mention 路由** | ✅ 完整实现 | `channels_internal/mention_trigger.rs` + `agent_core/roster.rs` |
| **群组消息处理** | ✅ 支持群聊/私聊 scope | `protocol/types.rs` |
| **消息去重** | ✅ 实现 | `agent_core/dedup.rs` |
| **DingTalk Webhook 去重** | ✅ 实现 | `channels_internal/dingtalk_webhook_dedup.rs` |
| **富文本消息** | ✅ DingTalk 富文本 | `channels_internal/dingtalk_webhook_richtext.rs` |

### 5.2 Agent 管理与路由

| 功能 | 实现状态 | 核心文件 |
|---|---|---|
| **AgentRoster 静态配置** | ✅ 完整实现 | `agent_core/roster.rs` |
| **按名称查找 Agent** | ✅ 实现 | `roster.find_by_name()` |
| **按 @mention 查找 Agent** | ✅ 实现 | `roster.find_by_mention()` |
| **Agent REST CRUD** | ✅ Phase 3 实现 | `gateway/api/agents.rs` + `agents_write.rs` |
| **Session 管理** | ✅ 完整实现 | `agent_core/registry.rs` |
| **Session 持久化（SQLite）** | ✅ 完整实现 | `session/lib.rs` + `store.rs` |
| **Session 历史清除** | ✅ Phase 3 实现 | `gateway/api/sessions_write.rs` |

### 5.3 多 Agent 团队协作

| 功能 | 实现状态 | 核心文件 |
|---|---|---|
| **Lead + Specialist 团队模式** | ✅ 完整实现 | `agent_core/team/orchestrator.rs` |
| **RELAY 引擎（同步委托）** | ✅ 完整实现 | `agent_core/relay.rs` |
| **任务状态机（TaskRegistry）** | ✅ 完整实现 | `agent_core/team/registry.rs` |
| **团队 Heartbeat 调度** | ✅ 完整实现 | `agent_core/team/heartbeat.rs` |
| **专才会话隔离** | ✅ 完整实现 | `agent_core/team/session.rs` |
| **里程碑交付系统** | ✅ 完整实现 | `agent_core/team/milestone.rs` |
| **Mode Selector 自动提升** | ✅ 完整实现 | `agent_core/mode_selector.rs` |
| **团队通知协议（TEAM_NOTIFY）** | ✅ 完整实现 | Lead system prompt 注入 |
| **任务认领锁（Semaphore 串行）** | ✅ 完整实现 | `agent_core/registry.rs` |

### 5.4 记忆系统

| 功能 | 实现状态 | 核心文件 |
|---|---|---|
| **MEMORY.md 跨会话记忆** | ✅ 完整实现 | `agent_core/memory/mod.rs` |
| **记忆上下文窗口检测（80%触发）** | ✅ 完整实现 | `agent_core/memory/cap.rs` |
| **Nightly 记忆蒸馏** | ✅ 完整实现 | `agent_core/memory/triggers/nightly.rs` |
| **空闲记忆蒸馏** | ✅ 完整实现 | `agent_core/memory/triggers/idle_distill.rs` |
| **N-turn 记忆蒸馏** | ✅ 完整实现 | `agent_core/memory/triggers/nturn_distill.rs` |
| **用户 /remember 命令** | ✅ 完整实现 | `agent_core/memory/triggers/user_remember.rs` |
| **Cron 结果记忆写入** | ✅ 完整实现 | `agent_core/memory/triggers/cron_result.rs` |
| **共享记忆（shared_memory）** | ✅ 完整实现 | `agent_core/memory/system.rs` |
| **Per-agent 独立记忆** | ✅ 通过 persona_dir 实现 | 配置 `persona_dir` 指向各自目录 |

### 5.5 Skills 与 Persona 系统

| 功能 | 实现状态 | 核心文件 |
|---|---|---|
| **Skill 加载器** | ✅ 完整实现 | `skills_internal/manifest.rs` |
| **Persona Skill（type: persona）** | ✅ 完整实现 | `skills_internal/persona_skill.rs` |
| **IDENTITY.md / SOUL.md 注入** | ✅ 完整实现 | `skills_internal/identity.rs` |
| **MBTI 认知栈注入** | ✅ 完整实现 | `skills_internal/mbti.rs` |
| **6层 System Prompt 构建** | ✅ 完整实现 | `agent_core/prompt_builder.rs` |
| **Agent 人格前缀（IM 消息）** | ✅ 完整实现 | Persona prefix in output_sink |
| **IM 前缀注入防注入检测** | ✅ 实现（warn-only） | prompt_builder |

### 5.6 运行时后端（Backend）

| 功能 | 实现状态 | 核心文件 |
|---|---|---|
| **Native Runtime（内置 rig-core）** | ✅ 完整实现 | `embedded_agent/native_runtime.rs` |
| **ACP 后端适配（Claude Code / Codex 等）** | ✅ 完整实现 | `runtime/acp/` |
| **OpenClaw 后端** | ✅ 完整实现 | `runtime/openclaw/` |
| **多 Provider 支持（Anthropic/OpenAI/DeepSeek）** | ✅ 完整实现 | `agent_sdk_internal/config.rs` |
| **Backend REST CRUD** | ✅ Phase 3 实现 | `gateway/api/backends*.rs` |
| **外部 MCP Server 接入（SSE）** | ✅ 完整实现 | `agent_sdk_internal/tools/mod.rs` |
| **工具审批系统（ApprovalMode）** | ✅ 完整实现 | `agent_core/approval.rs` |

### 5.7 内置工具（Native Runtime）

| 工具 | 功能 | 核心文件 |
|---|---|---|
| **BashTool** | 执行 Shell 命令（含安全策略） | `agent_sdk_internal/tools/bash.rs` |
| **ViewFileTool** | 查看文件内容 | `agent_sdk_internal/tools/fileio.rs` |
| **WriteFileTool** | 写入文件 | `agent_sdk_internal/tools/fileio.rs` |
| **EditFileTool** | 编辑文件 | `agent_sdk_internal/tools/fileio.rs` |
| **GlobTool** | 文件模式搜索 | `agent_sdk_internal/tools/search.rs` |
| **GrepTool** | 内容搜索 | `agent_sdk_internal/tools/search.rs` |
| **LsTool** | 目录列表 | `agent_sdk_internal/tools/search.rs` |
| **Team 工具（task_create 等）** | 团队协作工具 | `embedded_agent/team.rs` |
| **Schedule 工具** | 定时任务工具 | `embedded_agent/schedule.rs` |

### 5.8 定时任务系统（Scheduler/Cron）

| 功能 | 实现状态 | 核心文件 |
|---|---|---|
| **Cron 表达式调度** | ✅ 完整实现 | `scheduler/scheduler.rs` |
| **SQLite 持久化** | ✅ 完整实现 | `scheduler/store.rs` |
| **Job REST CRUD** | ✅ 完整实现 | `gateway/api/scheduler*.rs` |
| **立即执行（run-now）** | ✅ 完整实现 | `gateway/api/scheduler*.rs` |
| **Cron 触发结果写入记忆** | ✅ 完整实现 | `memory/triggers/cron_result.rs` |
| **Cron 触发 IM 发送** | ✅ 修复实现 | `main.rs` 中 `cron_channel_map` |

### 5.9 REST API（Phase 3）

| 端点组 | 端点数 | 实现状态 |
|---|---|---|
| **聊天** | 1 | ✅ `POST /api/chat` |
| **Agent 管理** | 6 | ✅ CRUD + skills |
| **配置管理** | 5 | ✅ 读/写/验证 |
| **Session 管理** | 5 | ✅ 查询/详情/消息/事件/删除 |
| **Backend 管理** | 2 | ✅ 列表/详情 |
| **Channel 管理** | 2 | ✅ 列表/详情 |
| **Skills** | 1 | ✅ 列表 |
| **Approvals** | 4 | ✅ 列表/详情/approve/deny |
| **Scheduler** | 4 | ✅ 列表/详情/runs/run-now |
| **Teams** | 4 | ✅ 列表/详情/artifacts/tasks |
| **Tasks** | 2 | ✅ 列表/详情 |
| **System** | 3 | ✅ /health /status /doctor |

### 5.10 WebSocket 协议

| 功能 | 实现状态 |
|---|---|
| **AgentEvent 实时流** | ✅ TextDelta/TurnComplete/ToolCallStart/ToolCallResult/ToolCallFailed/ApprovalRequest/Error |
| **Dashboard Topic 订阅** | ✅ approvals/backends/channels/session/scheduler/team/task |
| **WsVirtualChannel（Web Chat）** | ✅ Phase 3 实现 |
| **Session 事件广播** | ✅ 实现 |
| **Approval 实时推送** | ✅ 实现 |

### 5.11 Slash 命令系统

| 命令 | 功能 |
|---|---|
| `/memory` | 查看当前 Agent 记忆 |
| `/remember <内容>` | 强制写入记忆 |
| `/backend <name>` | 切换 Agent 后端 |
| `/status` | 显示系统状态 |
| `/team` | 显示团队状态 |
| 自定义 slash 命令 | 通过 skills 扩展 |

---

## 六、功能对标矩阵

### 6.1 核心能力对标

| 能力维度 | Slock.ai | clawBro | 状态 |
|---|---|---|---|
| **通信频道** | 云端 #all/#dev 等频道 | 钉钉/飞书群组 + Web Chat | ✅ 同等能力（不同载体） |
| **私信（DM）** | 人类↔Agent、Agent↔Agent DM | IM 私聊 + WS Chat | ✅ 人类↔Agent DM 支持 |
| **@mention 路由** | Agent 自行解析频道中的 @mention | Gateway 集中路由到对应 Agent | ✅ 实现（不同模型） |
| **群组广播（Fan-out）** | 所有订阅 Agent 均收到消息 | 仅路由到 front_bot 或 @mentioned | ❌ **差距：clawBro 不做广播** |
| **消息历史查询** | `read_history` MCP 工具 | Session 历史 REST API | ⚠️ REST 有，Agent 工具无 |
| **人类可见 Agent 对话** | 频道公开对话，人类全程可见 | IM 频道中 Agent 回复人类可见 | ✅ 支持 |

### 6.2 Agent 生命周期对标

| 能力维度 | Slock.ai | clawBro | 状态 |
|---|---|---|---|
| **进程启动** | Daemon `agent:start` 拉起 CLI 进程 | Gateway 每次 turn 按需启动子进程 | ✅ 同等效果（per-turn 模式） |
| **进程停止** | Daemon `agent:stop` | 子进程 turn 完成自动退出 | ✅ 自动管理 |
| **Agent 休眠/唤醒** | 显式 hibernate/wake，保留 workspace | 无显式休眠，Session 持久化在 SQLite | ❌ **差距：无显式休眠机制** |
| **Always-on 体验** | 休眠+唤醒模拟持续在线 | Session 持久化 + 记忆系统实现等效体验 | ✅ 实质等效（不同实现） |
| **Runtime 自动检测** | 探测 claude/codex/gemini 路径 | 静态 backend 配置 | ❌ **差距：无自动检测** |
| **多 Runtime 共存** | 同一机器多个不同 CLI Agent | 多 Backend 配置，可同时运行 | ✅ 支持 |

### 6.3 记忆系统对标

| 能力维度 | Slock.ai | clawBro | 状态 |
|---|---|---|---|
| **跨会话持久记忆** | MEMORY.md + notes/ | ✅ 完整记忆系统 + SQLite | ✅ **clawBro 更完善** |
| **Per-agent 隔离** | `~/.slock/agents/<id>/` 独立目录 | persona_dir + workspace_dir 隔离 | ✅ 支持 |
| **记忆自动更新** | System prompt 要求 Agent 自行维护 | 多触发器自动蒸馏（nightly/idle/nturn） | ✅ **clawBro 更自动化** |
| **记忆容量管理** | 无明确描述 | 80% 上下文窗口触发蒸馏 | ✅ **clawBro 独有** |
| **共享记忆** | 无（只有频道消息作为共享上下文） | shared_memory 机制 | ✅ **clawBro 独有** |

### 6.4 多 Agent 协作对标

| 能力维度 | Slock.ai | clawBro | 状态 |
|---|---|---|---|
| **Agent 感知同伴（list_server）** | ✅ MCP 工具 | ❌ Agent 不知道其他 Agent 存在 | ❌ **差距（计划中）** |
| **Agent 主动发消息给另一个 Agent** | ✅ `send_message(DM:@agent)` | ⚠️ RELAY 同步委托（有限制） | ⚠️ **部分差距（计划中）** |
| **Agent 间异步消息传递** | ✅ 通过频道/DM | ✅ Team 任务异步调度 | ✅ 不同实现，等效能力 |
| **Lead + Specialist 模式** | ❌ 无结构化任务分工 | ✅ 完整 Lead+Specialist 编排 | ✅ **clawBro 更完善** |
| **任务状态机** | ❌ 无（靠 Agent 自行协商） | ✅ 完整 TaskRegistry 状态机 | ✅ **clawBro 独有** |
| **任务超时重试** | ❌ 无 | ✅ Heartbeat 检测 + 3次重试 | ✅ **clawBro 独有** |
| **团队 Milestone 交付** | ❌ 无 | ✅ MilestoneDelivery 系统 | ✅ **clawBro 独有** |

### 6.5 MCP 工具对标

| 工具类别 | Slock.ai | clawBro | 状态 |
|---|---|---|---|
| **社交工具（send/receive/list/history）** | ✅ 4个 MCP 工具 | ❌ 无对应工具 | ❌ **差距（计划中）** |
| **文件操作工具** | ❌ 依赖 CLI Agent 自身能力 | ✅ View/Write/Edit/Glob/Grep/Ls | ✅ **clawBro 独有** |
| **Bash 执行工具** | ❌ 依赖 CLI Agent 自身能力 | ✅ BashTool（含安全策略） | ✅ **clawBro 独有** |
| **团队协作工具** | ❌ 无 | ✅ task_create/submit/claim 等 | ✅ **clawBro 独有** |
| **定时任务工具** | ❌ 无 | ✅ schedule_create/list/delete | ✅ **clawBro 独有** |
| **外部 MCP Server 接入** | ❌ 无（Agent 有 MCP 但不是 Gateway 提供） | ✅ 支持外部 SSE MCP Server | ✅ **clawBro 独有** |

### 6.6 运维与管理能力对标

| 能力维度 | Slock.ai | clawBro | 状态 |
|---|---|---|---|
| **REST 管理 API** | ❌ 无公开 API | ✅ 40+ 端点 Phase 3 API | ✅ **clawBro 远超** |
| **WebSocket 实时事件** | 云端推送（产品层） | ✅ 完整 WS 协议 + AgentEvent 流 | ✅ **clawBro 更可控** |
| **定时任务系统** | ❌ 无 | ✅ 完整 Cron + SQLite 持久化 | ✅ **clawBro 独有** |
| **工具调用审批** | ❌ 无 | ✅ 完整 Approval 系统（WS + REST） | ✅ **clawBro 独有** |
| **配置热更新（写 config.toml）** | ❌ 无 | ✅ PUT /api/config/raw | ✅ **clawBro 独有** |
| **Health / Doctor 端点** | ❌ 无 | ✅ /health /status /doctor | ✅ **clawBro 独有** |

---

## 七、架构模型对比

### 7.1 核心哲学差异

| 维度 | Slock.ai | clawBro |
|---|---|---|
| **协作模型** | 去中心化：每个 Agent 自主决定是否响应 | 集中路由：Gateway 决定消息给谁处理 |
| **协作空间** | 聊天室（chat-room）：人和 Agent 平等参与者 | 任务系统（task-system）：Gateway 编排，Agent 执行 |
| **Agent 感知** | Agent 感知整个协作空间（list_server） | Agent 只感知当前 session，不知道同伴 |
| **记忆模型** | 文件驱动（MEMORY.md 自维护） | 系统驱动（多触发器自动蒸馏） |
| **任务协调** | 靠对话协商（无结构化任务系统） | 结构化任务状态机（TaskRegistry） |
| **进程模型** | 长驻进程（hibernate/wake） | Per-turn 子进程（按需启动） |
| **部署模型** | 云端 Server + 本地 Daemon | 本地 Gateway（可自托管） |

### 7.2 信息流对比

**Slock.ai 信息流（去中心化）：**
```
Human 发消息
    → Server fan-out → 所有 Agent 收到
        → 每个 Agent 自行决定是否回复
            → Agent 可以 list_server 感知其他 Agent
            → Agent 可以 send_message 主动联系其他 Agent
```

**clawBro 信息流（集中路由）：**
```
Human 发消息
    → Gateway 解析 @mention / 判断 scope
        → 路由到一个特定 Agent
            → Agent 执行 turn
                → 如需协作：RELAY 同步委托 或 Team 任务调度
                → 结果返回 Human
```

### 7.3 各自优势场景

**Slock.ai 更适合：**
- 开放式探索型协作（多个 Agent 各自发表意见）
- 人类深度参与的半自动化工作流
- "群聊"感的人机协作体验
- 需要 Agent 主动观察频道并自主触发行为

**clawBro 更适合：**
- 结构化任务分解与并行执行
- 需要明确任务状态追踪的复杂工作流
- 企业 IM 集成（钉钉/飞书）
- 需要精细控制（审批、超时重试、里程碑）的 Agent 编排

---

## 八、差距分析与优劣势判断

### 8.1 clawBro 相对 Slock.ai 的真实差距

以下是经过深度分析后，clawBro 真正缺少的、且有实际用户价值的能力：

#### 差距 1：Agent 缺乏"社交感知"（高优先级）

**描述：** Agent 在执行 turn 时不知道其他 Agent 的存在，无法主动发现同伴、无法主动委托任务。

**Slock.ai 实现：** `list_server` MCP 工具 → 返回 `{ agents, humans, channels }`

**影响：** Agent 无法实现 Slock 式的"自主团队发现和协作"，只能通过预配置的 Team 模式被动参与编排。

**难度：** 低。已有计划（`docs/plans/2026-03-22-agent-social-awareness.md`）。

#### 差距 2：Agent 无法主动发消息给另一个 Agent（高优先级）

**描述：** Agent A 在 turn 中无法主动触发 Agent B 执行任务（除非通过 RELAY 的同步委托语法，且有约束）。

**Slock.ai 实现：** `send_message(channel="DM:@agentB", content="...")`

**影响：** 无法实现真正的"Agent 主动协作"，只能被动被 RELAY 或 Team Heartbeat 调度。

**难度：** 低。`POST /api/agents/{name}/message` + `send_to_agent` 工具，已有计划。

#### 差距 3：群组广播（Fan-out）缺失（中优先级）

**描述：** 当群组消息来临时，clawBro 只路由给一个 Agent（front_bot 或 @mentioned），其他 Agent 完全不知道发生了什么。

**Slock.ai 实现：** Server 对所有订阅 Agent 执行 `agent:deliver`，每个 Agent 独立决定是否响应。

**影响：** Agent 无法"旁听"群组对话，无法基于群组上下文主动插话或作出反应。

**难度：** 中。需要改变 `agent_core/registry.rs` 的核心路由逻辑，影响面较大。

#### 差距 4：Agent 休眠/唤醒机制缺失（低优先级）

**描述：** clawBro 没有显式的"休眠保留状态 + 唤醒恢复"机制。虽然 Session 在 SQLite 中持久化，但没有 `hibernate/wake` 语义。

**Slock.ai 实现：** `agent:sleep` = kill 进程 + 保留 workspace；`agent:start` = 重启进程 + 读取 workspace。

**影响：** 不影响实际使用（clawBro per-turn 模式本质上每次都是"唤醒+执行+休眠"），但缺乏显式的生命周期管理 API。

**难度：** 中。需要新增 lifecycle API + backend adapter 生命周期钩子。

#### 差距 5：Runtime 自动检测缺失（低优先级）

**描述：** Slock Daemon 启动时自动探测本机的 claude/codex/gemini 路径，clawBro 需要手动配置 backend。

**影响：** 首次配置体验较差，但功能等效（配置后效果相同）。

**难度：** 低。可通过 `clawbro setup` 向导改善体验。

### 8.2 差距严重程度评级

| 差距 | 产品影响 | 实现难度 | 优先级 |
|---|---|---|---|
| Agent 社交感知（list_roster） | 高：影响自主协作能力 | 低 | **P0 - 已计划** |
| Agent 主动发消息 | 高：影响 Agent 主动性 | 低 | **P0 - 已计划** |
| 群组广播（Fan-out） | 中：影响"群聊感" | 中 | P1 - 待计划 |
| Agent 休眠/唤醒 API | 低：体验优化 | 中 | P2 - 未来 |
| Runtime 自动检测 | 低：首次配置体验 | 低 | P2 - 未来 |

---

## 九、clawBro 超越 Slock.ai 的领域

这是报告中最重要的发现：**clawBro 在多个核心维度上已经显著超越 Slock.ai**。

### 9.1 结构化任务系统（clawBro 独有）

Slock.ai 完全没有任务状态机。它的"协作"靠 Agent 在频道里自由协商，没有任何约束和保证。

clawBro 有完整的 `TaskRegistry`（SQLite）：
- 任务状态：Pending → Claimed → Done / Failed
- 依赖关系（deps）
- 超时检测 + 3次自动重试
- 永久失败通知回调
- 任务认领者身份验证

**对用户的意义：** clawBro 的多 Agent 协作是**可预期、可追踪、可恢复**的；Slock 的协作是"尽力而为"的聊天式协商。

### 9.2 记忆系统自动化（clawBro 更完善）

Slock.ai 的记忆靠 Agent 自己维护（system prompt 约定），实际可靠性依赖模型遵循程度。

clawBro 有系统级多触发器：
- **Nightly：** 每天定时蒸馏
- **Idle：** 空闲超时触发
- **N-turn：** 每 N 轮对话触发
- **80% 上下文：** 容量即将耗尽时触发
- **/remember 命令：** 用户主动写入

**对用户的意义：** 记忆自动化程度更高，不依赖 Agent 自觉性。

### 9.3 Skills / Persona 系统（clawBro 独有）

Slock.ai 完全没有 Skills 概念。

clawBro 有完整的 Skills 生态：
- `quickai.plugin.json` 清单格式
- `type: persona` 支持完整 AI 人格包（SOUL.md + IDENTITY.md + MBTI 认知栈）
- 6层 System Prompt 构建（IDENTITY → Cognitive → soul-injection → SOUL → memory → skills）
- npx 风格安装（`npx skills add xxx`）

### 9.4 企业 IM 集成（clawBro 独有）

Slock.ai 自建了一个类 Slack 的协作空间，没有与现有企业通讯工具集成。

clawBro 直接接入：
- **钉钉（DingTalk）**：企业应用机器人 + Webhook + 富文本卡片
- **飞书（Lark）**：完整 API 集成
- 群聊 + 单聊 + @mention 全覆盖

**对用户的意义：** 不需要让团队成员切换到新的通讯工具，直接在钉钉/飞书里与 Agent 协作。

### 9.5 完整的管理 API（clawBro 独有）

Slock.ai 没有公开 REST API，所有配置通过其 Web 控制台完成。

clawBro 有 40+ REST 端点（Phase 3），涵盖：
- Agent / Backend / Channel CRUD
- Session 历史查询与清除
- 配置文件读写与验证
- Approval 审批流
- Scheduler/Cron 管理
- Teams/Tasks 查询
- Health/Status/Doctor 诊断

**对用户的意义：** clawBro 完全可以通过 API 自动化管理，可以集成到 CI/CD、监控系统、Dashboard。

### 9.6 工具调用审批（clawBro 独有）

Slock.ai 没有 Agent 工具调用审批机制。

clawBro 有完整的 Approval 系统：
- Agent 执行高危工具前请求人类审批
- WebSocket 实时推送审批请求
- REST API approve/deny
- 审批记录持久化

---

## 十、结论与建议

### 10.1 总体结论

**clawBro 在功能总量和专业深度上已经超越 Slock.ai**，特别是在：
- 结构化任务编排
- 记忆自动化
- 企业 IM 集成
- Skills/Persona 系统
- 管理 API 完整性

**Slock.ai 在以下方面有先发优势：**
- Agent "社交感知"（list_server）
- 去中心化的频道广播
- 用户体验（Web 界面、一键启动的 npx daemon）

### 10.2 优先补齐建议

#### 立即执行（P0）：Agent 社交感知

**已有计划：** `docs/plans/2026-03-22-agent-social-awareness.md`

4个任务，全部基于已有模式（CLAWBRO_TEAM_TOOL_URL 的镜像），预计工作量小：
1. `POST /api/agents/{name}/message` REST 端点
2. `CLAWBRO_GATEWAY_API_URL` 环境变量注入
3. `list_roster` + `send_to_agent` native rig 工具
4. 文档更新

实现后，native runtime Agent 将拥有 Slock.ai `list_server` + `send_message(DM:@agent)` 的等效能力。

#### 短期规划（P1）：群组广播

让群组消息广播给所有 roster 中的 Agent（可选配置 `broadcast: true`），每个 Agent 拿到消息但只有 front_bot/mentioned agent 回复人类。其余 Agent "静默观察"，更新自己的 Session 上下文，从而具备群聊情境感知能力。

#### 中期规划（P2）：内置 MCP 服务器

在 Gateway 内嵌一个 MCP SSE Server（`/mcp/sse`），将 `list_roster`、`send_to_agent` 等工具暴露给外部 ACP Backend（claude-code、codex 等），让所有类型的 Backend 都能使用社交感知工具，而不只是 native runtime。

### 10.3 差距关闭路线图

```
现状（2026-03-24）
├── ✅ 任务编排、记忆系统、Skills、企业 IM → 已超越 Slock.ai
├── ❌ Agent 社交感知（list_roster / send_to_agent）
│   └── → P0 计划已写好，立即执行
├── ❌ 群组广播（Fan-out）
│   └── → P1，需要单独计划
└── ❌ 内置 MCP 服务器（外部 Backend 社交工具）
    └── → P2，依赖 P0 完成后

目标（P0 完成后）
├── ✅ Native Agent 可 list_roster 查询同伴
├── ✅ Native Agent 可 send_to_agent 主动委托
├── ✅ 外部系统可通过 REST 向指定 Agent 发消息
└── → 基本实现 Slock.ai "Agent 社交感知" 能力对等
```

---

## 附录 A：Slock.ai MCP 工具接口规格

基于社区复刻分析，以下为 `chat-bridge` 暴露的 4 个 MCP 工具的接口规格：

```typescript
// 工具 1：发送消息
send_message({
  channel: string,    // "#channel-name" 或 "DM:@username"
  content: string,    // 消息内容
}): { ok: true }

// 工具 2：接收消息（阻塞等待）
receive_message({
  block?: boolean,    // true = 阻塞直到有新消息
  timeout?: number,  // 超时秒数
}): AgentMessage | null

// AgentMessage 结构：
{
  channel_type: "channel" | "dm",
  channel_name: string,
  sender_type: "human" | "agent",
  sender_name: string,
  content: string,
  timestamp: number,
}

// 工具 3：查询服务器状态
list_server(): {
  channels: { name: string, member_count: number }[],
  agents: { name: string, status: "online" | "sleeping" | "offline" }[],
  humans: { name: string, status: "online" | "offline" }[],
}

// 工具 4：读取历史消息
read_history({
  channel: string,   // "#channel-name" 或 "DM:@username"
  limit?: number,
  before?: number,   // timestamp
  after?: number,    // timestamp
}): AgentMessage[]
```

---

## 附录 B：clawBro 计划中的对等实现

| Slock.ai 工具 | clawBro 对等实现（计划中） |
|---|---|
| `list_server` | `list_roster` native rig 工具 → `GET /api/agents` |
| `send_message(DM:@agent)` | `send_to_agent` native rig 工具 → `POST /api/agents/{name}/message` |
| `send_message(#channel)` | 未计划（clawBro 通过 IM Channel 处理，不需要 Agent 主动发 IM） |
| `receive_message` | 不需要（clawBro 是被动响应模型，不是 Agent 主循环拉取模型） |
| `read_history` | 未计划（可通过 `GET /api/sessions/messages` REST API 访问，但尚无 Agent 工具） |

---

*报告完成于 2026-03-24，基于 clawBro 源码完整审计与 Slock.ai 公开信息综合分析。*
