<div align="center">
  <img src="./assets/logo.png" alt="clawBro Logo" width="200">
  <h1>🦀 clawBro: Let CLI Coding Agents Work Like OpenClaw in Chat and Team Mode</h1>
  <p>
    <strong>Built around OpenClaw ideas, clawBro helps Claude Code, Codex, Qwen, Qoder, Gemini, and other CLI Coding Agents work together across WeChat, Lark, DingTalk, and long-running team workflows.</strong>
  </p>
  <p>
    <a href="./README_ZH.md"><strong>中文</strong></a> ·
    <a href="./README_JA.md"><strong>日本語</strong></a> ·
    <a href="./README_KO.md"><strong>한국어</strong></a>
  </p>
  <p>
    <a href="#-project-status">Project Status</a> ·
    <a href="#️-architecture">Architecture</a> ·
    <a href="#-use-cases">Use Cases</a> ·
    <a href="#-quick-start">Quick Start</a> ·
    <a href="#-team-modes">Team Modes</a> ·
    <a href="#-cli-coding-agent-integration">CLI Coding Agent Integration</a> ·
    <a href="./docs/setup.md">Setup Guide</a>
  </p>
  <p>
    <img src="https://img.shields.io/badge/version-0.1.9-blue" alt="Version">
    <img src="https://img.shields.io/badge/rust-1.90%2B-orange" alt="Rust">
    <img src="https://img.shields.io/badge/license-MIT-green" alt="License">
    <img src="https://img.shields.io/badge/agents-Claude%20%7C%20Codex%20%7C%20Qwen%20%7C%20Qoder%20%7C%20Gemini-111827" alt="Agents">
    <img src="https://img.shields.io/badge/channels-WeChat%20%7C%20Lark%20%7C%20DingTalk-4EA1FF" alt="Channels">
    <img src="https://img.shields.io/badge/runtime-Native%20%7C%20CLI%20Bridge%20%7C%20OpenClaw-8B5CF6" alt="Runtime">
    <img src="https://img.shields.io/badge/modes-Solo%20%7C%20Multi%20%7C%20Team-111827" alt="Modes">
  </p>
</div>

`clawBro` is a Rust-based system for making CLI Coding Agents work together across local workflows, chat apps, and long-running team collaboration.

It stays close to the OpenClaw spirit, but pushes toward practical teamwork: Claude Code, Codex, Qwen, Qoder, Gemini, and related coding agents can be organized into solo, role-based, and lead-plus-specialist workflows, then connected to WeChat, Lark, DingTalk, and WebSocket entrypoints.

It also supports the official WeChat lobster path: a WeChat front bot can lead shrimp soldiers and crab generals in Team mode, keep the user-facing conversation stable, and delegate work to specialists behind the scenes.

## 📢 Project Status

- **[03-20]** Durable scheduling is now built in: one-shot reminders, exact-time jobs, interval polling, cron schedules, chat-created reminders, and current-session cleanup all share the same runtime scheduler.
- **[03-19]** One `clawbro` surface now brings together multiple AI CLI Coding Agents instead of forcing one tool per workflow.
- **[03-19]** Team orchestration supports lead-driven workflows, specialist agents, milestone delivery, and named roles like `planner`, `coder`, `reviewer`, and `researcher`.
- **[03-19]** Group and direct-message usage now fit the same routing model, with WeChat DM, Lark, DingTalk Stream Mode, DingTalk custom robot webhook, and WebSocket entrypoints.
- **[03-19]** Multi-IM connectivity is now practical for always-on chat workflows: one runtime can stay online across WeChat, Lark, DingTalk, and team conversations at the same time.
- **[03-19]** Operational controls include approvals, allowlists, memory-aware sessions, `/health`, `/status`, `/doctor`, and diagnostics surfaces.

> `clawBro` is built for engineering, research, and workflow experimentation. It is meant for real agent collaboration, not just another chat wrapper.

## Key Features of clawBro:

🏛️ **Unified Control Plane**: One `clawbro` entrypoint for setup, routing, session management, diagnostics, and runtime dispatch.

🤖 **Unified CLI Coding Agents**: Bring Claude, Codex, Qwen, Qoder, Gemini, and other coding CLIs into one product surface instead of juggling separate entrypoints.

👥 **Team Orchestration**: Support `solo`, `multi`, and `team` interaction models with lead + specialists, scope-aware routing, and milestone-style collaboration.

💬 **Group Chat Collaboration**: Connect workflows to Lark and DingTalk, route group mentions to named agents, and turn chat rooms into AI workbenches.

📡 **Always-On Multi-IM**: Keep one `clawbro` runtime online across Lark, DingTalk Stream Mode, DingTalk custom robot webhook, and WebSocket, then keep chatting without switching tools.

🧠 **Memory and Habits**: Let agents accumulate working memory, repeated preferences, review standards, and recurring project context over time.

⏰ **Durable Scheduling**: Create reminders, one-time jobs, recurring polling loops, and agent-driven scheduled work through `delay`, `at`, `every`, and `cron`.

🛡️ **Operationally Controllable**: Built-in config validation, approval flow, allowlists, doctor/status commands, and health endpoints.

## 🏗️ Architecture

```text
User / Group / WebSocket / Scheduled Jobs
              |
              v
           clawbro
              |
              +--> Routing / Session / Memory / Bindings / Team
              |
              +--> ClawBro Native ------> runtime-bridge ------> clawbro-agent-sdk
              |
              +--> Coding CLI Bridge ---> Claude / Codex / Qwen / Qoder / Gemini / custom coding CLIs
              |
              +--> OpenClaw Gateway ----> remote agent runtime
              |
              +--> Channels ------------> Lark / DingTalk / WebSocket delivery
```

## Table of Contents

- [Project Status](#-project-status)
- [Key Features](#key-features-of-clawbro)
- [Architecture](#️-architecture)
- [Features](#-features)
- [Use Cases](#-use-cases)
- [Install](#-install)
- [Quick Start](#-quick-start)
- [Scheduled Tasks](#-scheduled-tasks)
- [Team Modes](#-team-modes)
- [CLI Coding Agent Integration](#-cli-coding-agent-integration)
- [Chat Channels](#-chat-channels)
- [Configuration & Operations](#️-configuration--operations)
- [Project Structure](#️-project-structure)
- [Documentation Map](#-documentation-map)
- [Positioning](#-positioning)

## ✨ Features

<table align="center">
  <tr align="center">
    <th><p align="center">🤖 Coding Agent Hub</p></th>
    <th><p align="center">👥 Team Coordination</p></th>
    <th><p align="center">🧠 Memory Habits</p></th>
  </tr>
  <tr>
    <td align="center">One control plane for Claude, Codex, Qwen, Qoder, Gemini, and other coding-focused agents.</td>
    <td align="center">Lead + specialists, scope-aware team routing, group mentions, milestone delivery, and task-oriented collaboration.</td>
    <td align="center">Shared memory and agent memory help the system keep long-running habits, context, and preferences.</td>
  </tr>
</table>

## 🌟 Use Cases

### 🚀 Full-Stack App Building

Turn one request into a coordinated build loop:

- `@planner` breaks the product request into milestones
- `@coder` implements API routes, UI flows, and data models
- `@reviewer` checks quality, risks, and regressions
- `@tester` fills in edge cases and missing validation

In Team mode, the lead can keep the user-facing conversation clean while specialists work in the background. In group chat, the same setup can feel like an AI project room instead of a single bot window.

### 📚 Deep Research and Report Writing

Use ClawBro as a research squad:

- `@researcher` collects source material
- `@critic` looks for gaps, counterexamples, and weak assumptions
- `@writer` turns the findings into a structured report
- the lead agent summarizes progress and final conclusions

This works especially well for technical reports, architecture comparisons, literature reviews, and long-form analysis that benefits from multiple perspectives before one final answer.

### 🧑‍💻 PR Review and Design Review

Drop a patch, PR, or design note into a chat and route it to the right mix of agents:

- `@coder` focuses on implementation details
- `@reviewer` checks correctness and maintainability
- `@researcher` verifies outside dependencies or competing approaches
- the lead returns a consolidated recommendation

This gives you something closer to an AI review room than a single one-shot answer.

### 💬 Group Chat With Multiple Named Agents

ClawBro is a natural fit for role-based group workflows:

- `@planner` for decomposition
- `@coder` for implementation
- `@reviewer` for criticism
- `@researcher` for evidence gathering

That pattern works for engineering teams, study groups, product discussions, and internal support rooms. Even when the strongest current Team path is still lead-driven, the group experience can already feel much more structured than a generic bot chat.

### 🧠 Memory-Driven Coding Habits

ClawBro is not just about one conversation at a time. Over repeated use, it can preserve working context such as:

- architecture preferences
- recurring review standards
- naming conventions
- project-specific workflows
- things a user repeatedly asks the system to remember

That makes it useful for building a long-running coding habit, where your agents gradually become more aligned with how you actually work instead of resetting to zero every day.

### 🎭 Fun Team Play: Werewolf, RPG, and Role Rooms

The same role system also works for playful group scenarios:

- a lead agent can act as the moderator in Werewolf
- specialist agents can play judge, narrator, analyst, or character roles
- role-based group chats can simulate product debates, mock trial rooms, or scripted multi-character conversations

This is one of the most distinctive parts of the project: the architecture is serious enough for engineering work, but flexible enough for entertainment and social experiments.

## 📦 Install

**Recommended**

```bash
cargo install clawbro
```

**GitHub Release** (no Rust required)

1. Download the archive for your platform from GitHub Releases
2. Unpack it
3. Run `./clawbro --version`

If you want a global command, move it into your `PATH`:

```bash
chmod +x clawbro
mv clawbro ~/.local/bin/clawbro
```

**npm** (binary installer)

```bash
npm install -g clawbro
clawbro --version
```

**Build from source** (developer path)

```bash
cd clawBro
cargo build -p clawbro --bin clawbro
```

## 🚀 Quick Start

> [!TIP]
> Start with one of these classic cases. For deeper topology design, use `clawbro config wizard` or read [Setup Guide](./docs/setup.md).

**Case 1: minimal local start**

```bash
cargo install clawbro
clawbro setup
clawbro config validate
source ~/.clawbro/.env
clawbro serve
```

**Case 2: official WeChat lobster, solo mode**

```bash
clawbro setup --preset wechat-solo
clawbro config channel login wechat
clawbro config channel setup-solo wechat --agent claw
clawbro config validate
clawbro serve
```

**Case 3: WeChat DM Team, front bot leads shrimp soldiers and crab generals**

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

## ⏰ Scheduled Tasks

`clawBro` can now keep one eye on the clock while your agents keep coding.

- **Four schedule styles**:
  - `delay`: “in 1 minute remind me to charge my phone”
  - `at`: “tomorrow at 09:00 remind me about the meeting”
  - `every`: “check service status every 30 minutes”
  - `cron`: “every workday at 18:00 summarize today’s issue progress”
- **Two execution styles**:
  - fixed reminders go straight back to chat as durable messages
  - dynamic work goes through scheduled agent turns
- **Natural-language chat flows**:
  - “Remind me in 1 minute to charge my phone”
  - “Tell me Beijing time every minute”
  - “Delete the charging reminder”
  - “Clear all reminders in this conversation”
- **Operator commands**:

```bash
clawbro schedule add-delay --name phone-charge --delay 1m --prompt "Reminder: charge your phone"
clawbro schedule add-every --name service-check --every 30m --target-kind agent-turn --prompt "Check service status and report anomalies."
clawbro schedule list
clawbro schedule delete --name phone-charge
clawbro schedule delete-all --current-session-key 'lark@alpha:user:ou_xxx'
```

The important split is simple: reminders are delivered directly, while scheduled agent jobs wake up later and do fresh work when the time comes.

## 📦 Distribution

`clawBro` now ships through three install paths:

- **Cargo** for Rust-first users:
  - `cargo install clawbro`
- **GitHub Release binaries** for quick download-and-run installs:
  - download the archive for macOS or Linux
  - unpack it and run `./clawbro`
  - move it into `PATH` later if you want a global command
- **npm binary installer** for JavaScript-heavy setups:
  - `npm install -g clawbro`
  - the installer downloads the matching GitHub Release binary for your machine

Phase 1 release targets:

- macOS Apple Silicon
- macOS Intel
- Linux x86_64

On macOS, the binary may show the usual first-run Gatekeeper prompt because it is not notarized yet.

## 👥 Team Modes

ClawBro supports multiple interaction styles from the same control plane.

| Mode | What it does | Current best fit |
| --- | --- | --- |
| **Solo** | Single-agent setup with one primary backend. | Personal assistant, local coding help, focused one-user workflows. |
| **Multi** | Generates a starting point for named-agent configuration and bindings. | Role-based group rooms where you want `@planner`, `@coder`, `@reviewer`, or other named agents. |
| **Team** | Lead agent delegates work to specialists and returns milestone-style output. | Engineering collaboration, deep research, review workflows, and group workbench patterns with one stable front door. |

> Today, the strongest product path is lead-driven Team mode: specialists execute as focused workers while the lead remains the stable user-facing output surface.

<details>
<summary><b>Team mode examples</b></summary>

<br>

**Direct message Team**

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

**Group Team**

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

## 🔌 CLI Coding Agent Integration

ClawBro separates the business control plane from the execution plane so you can bring multiple coding agents into the same system without turning the README into a protocol manual.

| Integration path | Current role | Notes |
| --- | --- | --- |
| **ClawBro Native** | Default native execution path | Uses the internal runtime bridge and supports canonical Team Tool RPC. |
| **Coding CLI bridge** | Compatibility layer for external coding CLIs | Unifies multiple AI CLI Coding Agents behind one control plane. Internally this is where ACP-style compatibility helps, but users mainly feel a single product surface. |
| **OpenClaw Gateway** | Remote runtime integration | Active backend family for OpenClaw WS-based execution with explicit helper constraints in Team mode. |

Current documented agent examples include:

- Claude
- Codex
- Qwen
- Qoder
- Gemini
- custom coding CLIs

For the implementation details inside `clawBro`, see:

- [Runtime Backends](./docs/runtime-backends.md)
- [Backend Support Matrix](./docs/backend-support-matrix.md)

## 💬 Chat Channels

ClawBro connects agent workflows to chat delivery surfaces while keeping transcript truth and runtime progress under host control.

| Channel | Current status | Notes |
| --- | --- | --- |
| **WeChat (official lobster)** | Structured | Supports official WeChat login, WeChat solo routing, and WeChat DM Team mode where the front bot leads specialist agents. |
| **Lark / Feishu** | Complete | Supports `final_only` and `progress_compact` presentation modes. |
| **DingTalk** | Structured | Supports both app/stream mode and custom robot group webhook mode. |
| **WebSocket** | Structured | Recommended first setup path before adding IM integrations. |

Typical deployment path:

1. Start with WebSocket or official WeChat lobster and one primary backend.
2. Add `agent_roster`, bindings, and named roles.
3. Add Team scope and routing.
4. Connect Lark or DingTalk if you need more channels.

For DingTalk, there are now two distinct integration styles:

- `dingtalk`
  - app / stream mode
  - uses `client_id`, `client_secret`, and optional `agent_id`
- `dingtalk_webhook`
  - custom robot group webhook mode
  - uses `secret_key`, your inbound `webhook_path`, and optional `access_token` fallback

## ⚙️ Configuration & Operations

The primary user entrypoint is:

```bash
clawbro
```

Common commands:

| Command | Purpose |
| --- | --- |
| `clawbro setup` | First-time initialization |
| `clawbro config wizard` | Continue configuring providers, backends, channels, routing, and Team scopes |
| `clawbro config validate` | Validate topology and config references |
| `clawbro config channel login wechat` | Log in to the official WeChat lobster channel |
| `clawbro config channel setup-solo wechat --agent claw` | Turn WeChat into a solo front door |
| `clawbro config channel setup-team wechat --scope user:o9cqxxxx@im.wechat --front-bot claude --specialist claw` | Turn a WeChat DM scope into a Team lead workspace |
| `clawbro serve` | Start the gateway |
| `clawbro status` | Show the active config summary |
| `clawbro doctor` | Diagnose environment and runtime issues |
| `clawbro auth list` | List configured auth material |

Default runtime layout:

- `~/.clawbro/config.toml`
- `~/.clawbro/.env`
- `~/.clawbro/sessions/`
- `~/.clawbro/shared/`
- `~/.clawbro/skills/`
- `~/.clawbro/personas/`

Operational surfaces mentioned in the current docs:

- `/health`
- `/status`
- `/doctor`
- `/diagnostics/*`

## 🗂️ Project Structure

```text
clawBro/
├── crates/clawbro-server/       # public clawbro CLI and gateway
├── crates/clawbro-agent/        # routing, context, memory, team orchestration
├── crates/clawbro-runtime/      # backend adapters and runtime contracts
├── crates/clawbro-channels/     # Lark and DingTalk channel integration
├── crates/clawbro-agent-sdk/    # runtime bridge and reusable agent shell
├── crates/clawbro-session/      # session storage and queueing
├── crates/clawbro-skills/       # skills and persona loading
├── crates/clawbro-server/src/scheduler/  # internal scheduler modules
└── docs/                        # setup, routing, backend, and operations docs
```

## 📚 Documentation Map

- [Setup Guide](./docs/setup.md)
- [Getting Started From Zero](./docs/getting-started-from-zero.md)
- [Runtime Backends](./docs/runtime-backends.md)
- [Backend Support Matrix](./docs/backend-support-matrix.md)
- [Routing Contract](./docs/routing-contract.md)
- [Doctor and Status Operations](./docs/operations/doctor-and-status.md)
- [Context Filesystem Contract](./docs/context-filesystem-contract.md)

## 🎯 Positioning

ClawBro currently fits best if you want one of these:

- An engineering control plane that brings multiple coding agents into group chat and workflow operations.
- A lead-plus-specialists setup for complex coding, review, research, and report-generation tasks.
- A unified surface that can sit above Claude, Codex, Qwen, Qoder, Gemini, and related coding CLIs without giving up operational control.

If you want a configurable, IM-connected, team-aware control plane, this project is aimed directly at that problem.
