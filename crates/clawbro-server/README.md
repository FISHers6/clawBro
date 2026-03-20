<div align="center">
  <h1>🦀 clawBro: Let Coding CLI Agents Work Like OpenClaw in Chat and collaborating as a team at all times</h1>
  <p>
    <strong>Built around OpenClaw ideas, clawBro helps Claude Code, Codex, Qwen, Qoder, Gemini, and other coding agent CLIs work together and connect to Lark, DingTalk, and team workflows.</strong>
  </p>
  <p>
    <a href="https://github.com/fishers6/clawbro/blob/main/README_ZH.md"><strong>中文</strong></a> ·
    <a href="https://github.com/fishers6/clawbro/blob/main/README_JA.md"><strong>日本語</strong></a> ·
    <a href="https://github.com/fishers6/clawbro/blob/main/README_KO.md"><strong>한국어</strong></a>
  </p>
  <p>
    <a href="#-project-status">Project Status</a> ·
    <a href="#️-architecture">Architecture</a> ·
    <a href="#-use-cases">Use Cases</a> ·
    <a href="#-quick-start">Quick Start</a> ·
    <a href="#-team-modes">Team Modes</a> ·
    <a href="#-coding-agent-integration">Coding Agent Integration</a> ·
    <a href="https://github.com/fishers6/clawbro/blob/main/docs/setup.md">Setup Guide</a>
  </p>
  <p>
    <img src="https://img.shields.io/badge/version-0.1.7-blue" alt="Version">
    <img src="https://img.shields.io/badge/rust-1.90%2B-orange" alt="Rust">
    <img src="https://img.shields.io/badge/license-MIT-green" alt="License">
    <img src="https://img.shields.io/badge/agents-Claude%20%7C%20Codex%20%7C%20Qwen%20%7C%20Qoder%20%7C%20Gemini-111827" alt="Agents">
    <img src="https://img.shields.io/badge/channels-Lark%20%7C%20DingTalk-4EA1FF" alt="Channels">
    <img src="https://img.shields.io/badge/runtime-Native%20%7C%20CLI%20Bridge%20%7C%20OpenClaw-8B5CF6" alt="Runtime">
    <img src="https://img.shields.io/badge/modes-Solo%20%7C%20Multi%20%7C%20Team-111827" alt="Modes">
  </p>
</div>

`clawBro` is a Rust-based system for making coding agent CLIs work together across local workflows, chat apps, and long-running team collaboration.

It stays close to the OpenClaw spirit, but pushes toward practical teamwork: Claude Code, Codex, Qwen, Qoder, Gemini, and related coding agents can be organized into solo, role-based, and lead-plus-specialist workflows, then connected to Lark, DingTalk, and WebSocket entrypoints.

## 📢 Project Status

- **[03-19]** One `clawbro` surface now brings together multiple AI coding CLIs instead of forcing one tool per workflow.
- **[03-19]** Team orchestration supports lead-driven workflows, specialist agents, milestone delivery, and named roles like `planner`, `coder`, `reviewer`, and `researcher`.
- **[03-19]** Group and direct-message usage now fit the same routing model, with Lark, DingTalk Stream Mode, DingTalk custom robot webhook, and WebSocket entrypoints.
- **[03-19]** Multi-IM connectivity is now practical for always-on chat workflows: one runtime can stay online across Lark, DingTalk, and team conversations at the same time.
- **[03-19]** Operational controls include approvals, allowlists, memory-aware sessions, `/health`, `/status`, `/doctor`, and diagnostics surfaces.

> `clawBro` is built for engineering, research, and workflow experimentation. It is meant for real agent collaboration, not just another chat wrapper.

## Key Features of clawBro:

🏛️ **Unified Control Plane**: One `clawbro` entrypoint for setup, routing, session management, diagnostics, and runtime dispatch.

🤖 **Unified Coding Agents**: Bring Claude, Codex, Qwen, Qoder, Gemini, and other coding CLIs into one product surface instead of juggling separate entrypoints.

👥 **Team Orchestration**: Support `solo`, `multi`, and `team` interaction models with lead + specialists, scope-aware routing, and milestone-style collaboration.

💬 **Group Chat Collaboration**: Connect workflows to Lark and DingTalk, route group mentions to named agents, and turn chat rooms into AI workbenches.

📡 **Always-On Multi-IM**: Keep one `clawbro` runtime online across Lark, DingTalk Stream Mode, DingTalk custom robot webhook, and WebSocket, then keep chatting without switching tools.

🧠 **Memory and Habits**: Let agents accumulate working memory, repeated preferences, review standards, and recurring project context over time.

🛡️ **Operationally Controllable**: Built-in config validation, approval flow, allowlists, doctor/status commands, and health endpoints.

## 🏗️ Architecture

```text
User / Group / WebSocket / Cron
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
- [Team Modes](#-team-modes)
- [Coding Agent Integration](#-coding-agent-integration)
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

**Install from crates.io**

```bash
cargo install clawbro
```

**Build from source**

```bash
git clone https://github.com/fishers6/clawbro.git
cd clawbro
cargo build -p clawbro --bin clawbro
```

## 🚀 Quick Start

> [!TIP]
> The recommended first path is `WebSocket + ClawBro Native`.
> Add agent rosters, bindings, channels, and Team scopes after the base path is working.

**1. Install**

```bash
cargo install clawbro
```

**2. Initialize**

```bash
clawbro setup
```

This creates the default runtime layout under `~/.clawbro/`, including:

- `config.toml`
- `.env`
- `sessions/`
- `shared/`
- `skills/`
- `personas/`
