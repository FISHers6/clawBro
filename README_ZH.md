<div align="center">
  <img src="./assets/logo.png" alt="clawBro Logo" width="200">
  <h1>🦀 clawBro：让 CLI Coding Agents 像 OpenClaw 一样在 IM 和团队里真正干活</h1>
  <p>
    <strong>围绕 OpenClaw 的思路继续往前走，让 Claude Code、Codex、Qwen、Qoder、Gemini 等 CLI Coding Agent 能协作干活，并接入微信、Lark、DingTalk 和团队工作流。</strong>
  </p>
  <p>
    <a href="./README.md"><strong>English</strong></a> ·
    <a href="./README_JA.md"><strong>日本語</strong></a> ·
    <a href="./README_KO.md"><strong>한국어</strong></a>
  </p>
  <p>
    <a href="#-项目状态">项目状态</a> ·
    <a href="#️-架构">架构</a> ·
    <a href="#-应用场景">应用场景</a> ·
    <a href="#-快速开始">快速开始</a> ·
    <a href="#-团队模式">团队模式</a> ·
    <a href="#-cli-coding-agent-接入">CLI Coding Agent 接入</a> ·
    <a href="./docs/setup.md">安装配置</a>
  </p>
  <p>
    <img src="https://img.shields.io/badge/version-0.1.10-blue" alt="Version">
    <img src="https://img.shields.io/badge/rust-1.90%2B-orange" alt="Rust">
    <img src="https://img.shields.io/badge/license-MIT-green" alt="License">
    <img src="https://img.shields.io/badge/agents-Claude%20%7C%20Codex%20%7C%20Qwen%20%7C%20Qoder%20%7C%20Gemini-111827" alt="Agents">
    <img src="https://img.shields.io/badge/channels-WeChat%20%7C%20Lark%20%7C%20DingTalk-4EA1FF" alt="Channels">
    <img src="https://img.shields.io/badge/runtime-Native%20%7C%20CLI%20Bridge%20%7C%20OpenClaw-8B5CF6" alt="Runtime">
    <img src="https://img.shields.io/badge/modes-Solo%20%7C%20Multi%20%7C%20Team-111827" alt="Modes">
  </p>
</div>

`clawBro` 是一个用 Rust 写的 AI 协作系统，目标不是只包一层聊天机器人，而是让多个 CLI Coding Agent 真正能一起工作。

它延续 OpenClaw 的方向，但更强调实际协作：Claude Code、Codex、Qwen、Qoder、Gemini 等都可以纳入同一套工作流，在微信、单聊、群聊、Lark、DingTalk、WebSocket 和 Team 模式里配合干活。

它也强调微信官方龙虾这条路径：前台 bot 可以在微信里带队养虾兵蟹将，对外稳定回复，对内把任务分发给 specialist。

## 📢 项目状态

- **[03-20]** 现在内置持久化定时任务：一次性提醒、指定时刻任务、固定轮询、cron 调度、聊天里直接创建提醒、按当前会话清理提醒，全部走同一套 runtime scheduler。
- **[03-19]** 把多个 AI CLI Coding Agent 放进同一套协作流程，而不是每个工具单独跑一套。
- **[03-19]** 支持Team协作模式： Lead 负责对外，specialist 在后台执行，再由 Lead 汇总 milestone。
- **[03-19]** 支持把工作流接入微信、Lark、DingTalk Stream Mode、DingTalk 自定义机器人 Webhook 和 WebSocket，适合从本地到群聊逐步扩展。
- **[03-19]** 现在可以稳定做多 IM 常驻：一个 `clawbro` 运行时可以同时在线接微信、Lark、DingTalk 和团队对话，随时聊天、随时继续任务。
- **[03-19]** 运行时提供 approvals、allowlist、memory-aware sessions、`/health`、`/status`、`/doctor` 和 diagnostics 能力。

> `clawBro` 适合工程协作、研究工作流、群聊 AI 助手和多 Agent 实验，而不是单纯做一个最轻量的对话 CLI。

## clawBro 的核心特点

🏛️ **统一入口**：一个 `clawbro` 命令，负责初始化、路由、会话、诊断和运行。

🤖 **统一 CLI Coding Agent**：Claude Code、Codex、Qwen、Qoder、Gemini 以及其他 CLI Coding Agent，可以放进同一套系统里协作。

👥 **团队协作模式**：支持 `solo`、`multi`、`team` 三种交互模式，适合从个人使用一路升级到 Lead + Specialists。

💬 **接入聊天场景**：工作流可以接进 Lark / DingTalk，也可以先从 WebSocket 起步。

📡 **多 IM 常驻在线**：同一个 `clawbro` 运行时可以同时连 Lark、DingTalk Stream、DingTalk Webhook 和 WebSocket，不需要为不同聊天入口分别起一套系统。

🧠 **记忆与习惯**：支持共享记忆、角色记忆、项目偏好沉淀，让 Agent 越用越贴近你的真实工作方式。

⏰ **定时与轮询**：支持 `delay`、`at`、`every`、`cron` 四类调度，既能做提醒，也能做定时触发的 Agent 工作流。

🛡️ **工程可控**：内置 config validate、doctor/status、审批和健康检查，不是黑盒跑完就结束。

## 🏗️ 架构

```text
用户 / 群聊 / WebSocket / 定时任务
              |
              v
           clawbro
              |
              +--> 路由 / 会话 / 记忆 / Bindings / Team
              |
              +--> ClawBro Native ------> runtime-bridge ------> clawbro-agent-sdk
              |
              +--> Coding CLI Bridge ---> Claude / Codex / Qwen / Qoder / Gemini / custom coding CLIs
              |
              +--> OpenClaw Gateway ----> remote agent runtime
              |
              +--> Channels ------------> Lark / DingTalk / WebSocket delivery
```

## 目录导航

- [项目状态](#-项目状态)
- [核心特点](#clawbro-的核心特点)
- [架构](#️-架构)
- [功能概览](#-功能概览)
- [应用场景](#-应用场景)
- [安装](#-安装)
- [快速开始](#-快速开始)
- [定时任务](#-定时任务)
- [团队模式](#-团队模式)
- [CLI Coding Agent 接入](#-cli-coding-agent-接入)
- [聊天渠道](#-聊天渠道)
- [配置与运维](#️-配置与运维)
- [项目结构](#️-项目结构)
- [文档地图](#-文档地图)
- [项目定位](#-项目定位)

## ✨ 功能概览

<table align="center">
  <tr align="center">
    <th><p align="center">🤖 Agent 中枢</p></th>
    <th><p align="center">👥 协作编排</p></th>
    <th><p align="center">🧠 长期记忆</p></th>
  </tr>
  <tr>
    <td align="center">把 Claude Code、Codex、Qwen、Qoder、Gemini 等 coding agent 放进一个系统里使用。</td>
    <td align="center">支持 Lead + Specialists、群聊角色路由、milestone 汇报和任务式协作。</td>
    <td align="center">共享记忆和角色记忆可以持续沉淀项目上下文、规范和习惯。</td>
  </tr>
</table>

## 🌟 应用场景

### 🚀 全栈应用开发

你可以把一个需求直接变成一套协作工作流：

- `@planner` 负责拆需求、定 milestone
- `@coder` 负责写 API、页面和数据结构
- `@reviewer` 负责挑风险、查回归、抓边界
- `@tester` 负责补测试和异常路径

在 Team 模式里，Lead 对外统一沟通，specialist 在后台分工推进；在群聊里，它又能像一个 AI 项目群一样自然工作。

### 📚 深度研究与报告生成

适合做“研究小队”：

- `@researcher` 找资料
- `@critic` 找漏洞和反例
- `@writer` 写成报告
- Lead 统一汇总阶段性进展和最终结论

特别适合技术调研、架构对比、论文综述、行业分析和长文报告。

### 🧑‍💻 PR 审查与方案评审

把 patch、PR、设计文档扔进对话里，就可以拉起一场像样的协作评审：

- `@coder` 看实现细节
- `@reviewer` 看可维护性和风险
- `@researcher` 查依赖、替代方案和外部信息
- Lead 最后汇总成一份能直接往下推进的结论

这种体验比“问一个 bot 拿一次性答案”更接近真实团队评审。

### 💬 群聊里的多 Agent 工作台

`clawBro` 很适合做角色化群聊：

- `@planner` 拆任务
- `@coder` 写实现
- `@reviewer` 提批评
- `@researcher` 查资料

它可以用在研发群、读书群、产品群、技术支持群。即使当前最稳的 Team 形态仍然是 Lead 对外统一输出，群聊体验也已经比普通 bot 更像“一个有分工的 AI 工作台”。

### 🧠 养成记忆型 Coding 习惯

`clawBro` 不是一次性问答工具。连续使用后，它可以逐步记住：

- 你的架构偏好
- 你习惯的 review 标准
- 命名风格
- 项目工作流
- 你反复强调要记住的事情

这样它更像一个越用越懂你的 coding 搭子，而不是每天重新认识你一次。

### 🎭 娱乐玩法：狼人杀 / 跑团 / 角色群聊

同一套角色机制也可以拿来做更有趣的玩法：

- Lead 当狼人杀法官
- specialist 扮演裁判、解说、复盘官或角色
- 群聊里做剧情杀、跑团、产品辩论、人格对话

这也是 `clawBro` 很有特色的一点：它既能做严肃工程协作，也适合做有互动感的多人娱乐玩法。

## 📦 安装

**推荐方式**

```bash
cargo install clawbro
```

**GitHub Release**（不需要 Rust）

1. 从 GitHub Releases 下载对应平台压缩包
2. 解压
3. 直接运行 `./clawbro --version`

如果你想像全局命令一样使用，也可以放进 `PATH`：

```bash
chmod +x clawbro
mv clawbro ~/.local/bin/clawbro
```

**npm**（二进制安装器）

```bash
npm install -g clawbro
clawbro --version
```

**从源码编译**（开发者路径）

```bash
cd clawBro
cargo build -p clawbro --bin clawbro
```

## 🚀 快速开始

> [!TIP]
> 这里保留 3 个经典案例。更完整的拓扑细节请看 [Setup Guide](./docs/setup.md) 或直接进入 `clawbro config wizard`。

**案例 1：本地最小启动**

```bash
cargo install clawbro
clawbro setup
clawbro config validate
source ~/.clawbro/.env
clawbro serve
```

**案例 2：微信官方龙虾，单兵模式**

```bash
clawbro setup --preset wechat-solo
clawbro config channel login wechat
clawbro config channel setup-solo wechat --agent claw
clawbro config validate
clawbro serve
```

**案例 3：微信单聊 Team，前台带队养虾兵蟹将**

```bash
clawbro setup --preset wechat-dm-team
clawbro config channel login wechat
clawbro config channel setup-team wechat \
  --scope user:o9cqxxxx@im.wechat \
  --front-bot claude \
  --specialist claw
clawbro config validate
clawbro serve
```

## ⏰ 定时任务

`clawBro` 现在不只是随叫随到，也能“到了时间自己开工”。

- **四种调度方式**：
  - `delay`：比如“1 分钟后提醒我给手机充电”
  - `at`：比如“明天早上 9 点提醒我开会”
  - `every`：比如“每 30 分钟检查一次服务状态”
  - `cron`：比如“工作日每天 18 点总结 issue 进展”
- **两种执行方式**：
  - 固定提醒会直接回到聊天会话
  - 动态任务会在到点后拉起 agent 再执行一轮
- **聊天里可以直接说**：
  - “一分钟后提醒我给手机充电”
  - “每分钟告诉我一下北京时间”
  - “把给手机充电的提醒删掉”
  - “清空这个会话里的所有提醒”
- **管理员也可以走正式命令**：

```bash
clawbro schedule add-delay --name phone-charge --delay 1m --prompt "提醒：请给手机充电"
clawbro schedule add-every --name service-check --every 30m --target-kind agent-turn --prompt "检查服务状态，有异常就汇报。"
clawbro schedule list
clawbro schedule delete --name phone-charge
clawbro schedule delete-all --current-session-key 'lark@alpha:user:ou_xxx'
```

简单说：提醒类任务负责准时冒泡，Agent 定时任务负责到点后重新干活，二者共用同一套 scheduler，但执行语义清楚分开。

## 📦 分发方式

现在 `clawBro` 有三条安装路径：

- **Cargo**：适合 Rust 用户
  - `cargo install clawbro`
- **GitHub Release 二进制**：适合想直接下载就跑的人
  - 下载对应平台压缩包
  - 解压后直接执行 `./clawbro`
  - 需要全局命令时再手动放进 `PATH`
- **npm 二进制安装器**：适合 JavaScript / Node 用户
  - `npm install -g clawbro`
  - 安装时会自动下载当前机器对应的 GitHub Release 二进制

第一阶段支持的平台：

- macOS Apple Silicon
- macOS Intel
- Linux x86_64

macOS 上首次运行时，系统可能会给出未公证二进制的常规提示，这是预期行为。

## 👥 团队模式

`clawBro` 支持多种交互方式：

| 模式 | 作用 | 当前最适合的用法 |
| --- | --- | --- |
| **Solo** | 单 Agent 运行，一个主要 backend。 | 个人助手、写码辅助、单人高频问答。 |
| **Multi** | 生成命名 Agent 和 bindings 的起始配置。 | 想在群聊里做 `@planner`、`@coder`、`@reviewer` 这类角色化房间。 |
| **Team** | Lead 调度 specialists，再由 Lead 统一输出。 | 工程协作、深度研究、评审流程、群聊工作台。 |

> 当前最稳的产品路径仍然是 Lead 驱动的 Team 模式：specialist 幕后执行，Lead 统一对外输出。

<details>
<summary><b>Team 模式示例</b></summary>

<br>

**单聊 Team**

```bash
./target/debug/clawbro setup \
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

**群聊 Team**

```bash
./target/debug/clawbro setup \
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

</details>

## 🔌 CLI Coding Agent 接入

`clawBro` 把业务协作和底层执行拆开，这样多个 coding agent 就能放进同一套系统里，不需要每个工具单独维护一套入口。

| 接入方式 | 当前角色 | 说明 |
| --- | --- | --- |
| **ClawBro Native** | 默认原生执行路径 | 使用内部 runtime bridge，并支持 Team Tool RPC。 |
| **Coding CLI bridge** | 外部 coding CLI 的兼容层 | 把多个 AI CLI Coding Agent 收敛到同一套使用方式里。 |
| **OpenClaw Gateway** | 远程运行时接入 | 用于 OpenClaw WS 路径和相关 helper 模式。 |

当前文档里明确覆盖到的 agent 示例包括：

- Claude
- Codex
- Qwen
- Qoder
- Gemini
- custom coding CLIs

如果你想看 `clawBro` 内部实现细节，可以继续读：

- [Runtime Backends](./docs/runtime-backends.md)
- [Backend Support Matrix](./docs/backend-support-matrix.md)

## 💬 聊天渠道

`clawBro` 可以把 agent 工作流接到聊天渠道，同时保持自身对 transcript 和运行状态的掌控。

| 渠道 | 当前状态 | 说明 |
| --- | --- | --- |
| **微信官方龙虾** | Structured | 支持官方微信登录、微信 solo 路由，以及微信单聊 Team 模式。 |
| **Lark / Feishu** | Complete | 已支持 `final_only` 和 `progress_compact`。 |
| **DingTalk** | Structured | 同方向能力已接入，当前标注仍偏结构化阶段。 |
| **WebSocket** | Structured | 推荐作为最小起步路径。 |

推荐的部署顺序：

1. 先用 WebSocket 或微信官方龙虾 + 一个主 backend 跑通。
2. 再补 `agent_roster`、bindings 和命名角色。
3. 再加 Team scope 和路由。
4. 最后接入 Lark 或 DingTalk。

## ⚙️ 配置与运维

用户主入口是：

```bash
clawbro
```

常用命令：

| 命令 | 作用 |
| --- | --- |
| `clawbro setup` | 首次初始化 |
| `clawbro config wizard` | 继续配置 provider、backend、channel、路由和 Team |
| `clawbro config validate` | 校验配置和拓扑引用 |
| `clawbro config channel login wechat` | 登录微信官方龙虾 |
| `clawbro config channel setup-solo wechat --agent claw` | 把微信配成 solo 前台入口 |
| `clawbro config channel setup-team wechat --scope user:o9cqxxxx@im.wechat --front-bot claude --specialist claw` | 把微信单聊配成 Team 前台 |
| `clawbro serve` | 启动服务 |
| `clawbro status` | 查看当前配置摘要 |
| `clawbro doctor` | 诊断环境和运行时问题 |
| `clawbro auth list` | 查看已配置认证信息 |

默认运行目录：

- `~/.clawbro/config.toml`
- `~/.clawbro/.env`
- `~/.clawbro/sessions/`
- `~/.clawbro/shared/`
- `~/.clawbro/skills/`
- `~/.clawbro/personas/`

当前文档里提到的运维接口包括：

- `/health`
- `/status`
- `/doctor`
- `/diagnostics/*`

## 🗂️ 项目结构

```text
clawBro/
├── crates/clawbro-server/       # 对外 clawbro CLI 和 gateway
├── crates/clawbro-agent/        # 路由、上下文、记忆、team 编排
├── crates/clawbro-runtime/      # backend adapters 和 runtime contract
├── crates/clawbro-channels/     # Lark / DingTalk 集成
├── crates/clawbro-agent-sdk/    # runtime bridge 和通用 agent shell
├── crates/clawbro-session/      # session 存储和排队
├── crates/clawbro-skills/       # skills / persona 加载
├── crates/clawbro-server/src/scheduler/  # 内部调度模块
└── docs/                        # 安装、路由、backend、运维文档
```

## 📚 文档地图

- [Setup Guide](./docs/setup.md)
- [Getting Started From Zero](./docs/getting-started-from-zero.md)
- [Runtime Backends](./docs/runtime-backends.md)
- [Backend Support Matrix](./docs/backend-support-matrix.md)
- [Routing Contract](./docs/routing-contract.md)
- [Doctor and Status Operations](./docs/operations/doctor-and-status.md)
- [Context Filesystem Contract](./docs/context-filesystem-contract.md)

## 🎯 项目定位

`clawBro` 当前最适合这几类用户：

- 想把多个 coding agent 接进群聊和工作流的工程团队
- 想用 Lead + Specialists 处理复杂编码、评审和研究任务的个人开发者
- 想把 OpenClaw、Claude Code、Codex、Qwen、Qoder、Gemini 等使用方式串起来的系统设计者

如果你想要一个可接聊天渠道、可做团队协作、可长期沉淀记忆和角色分工的系统，这个项目就是为这个方向做的。
