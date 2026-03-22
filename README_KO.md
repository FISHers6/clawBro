<div align="center">
  <img src="./assets/logo.png" alt="clawBro Logo" width="200">
  <h1>🦀 clawBro: CLI Coding Agent가 OpenClaw처럼 채팅과 팀 협업 안에서 실제로 일하게 만들기</h1>
  <p>
    <strong>OpenClaw의 방향을 바탕으로 Claude Code, Codex, Qwen, Qoder, Gemini 같은 CLI Coding Agent가 함께 협업하고, WeChat, Lark, DingTalk, 팀 워크플로에 연결되도록 합니다.</strong>
  </p>
  <p>
    <a href="./README.md"><strong>English</strong></a> ·
    <a href="./README_ZH.md"><strong>中文</strong></a> ·
    <a href="./README_JA.md"><strong>日本語</strong></a> ·
  </p>
  <p>
    <a href="#-프로젝트-상태">프로젝트 상태</a> ·
    <a href="#️-아키텍처">아키텍처</a> ·
    <a href="#-활용-시나리오">활용 시나리오</a> ·
    <a href="#-빠른-시작">빠른 시작</a> ·
    <a href="#-팀-모드">팀 모드</a> ·
    <a href="#-cli-coding-agent-통합">CLI Coding Agent 통합</a> ·
    <a href="./docs/setup.md">설정 가이드</a>
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

`clawBro` 는 Rust로 작성된 AI 협업 시스템입니다. 단순히 챗봇을 한 겹 감싸는 것이 아니라, 여러 CLI Coding Agent가 실제로 함께 일하도록 만드는 것이 목적입니다.

OpenClaw의 방향을 이어가되, 실전 협업에 더 초점을 맞춥니다. Claude Code, Codex, Qwen, Qoder, Gemini 등을 하나의 워크플로에 넣고, WeChat, DM, 그룹 채팅, Lark, DingTalk, WebSocket, Team 모드에서 함께 활용할 수 있습니다.

공식 WeChat 랍스터 경로도 강조합니다. WeChat front bot이 앞에서 대화를 유지하고, 뒤에서는 shrimp soldiers와 crab generals에게 일을 분배하는 구성이 가능합니다.

## 📢 프로젝트 상태

- **[03-20]** 내장 스케줄러가 들어왔습니다. 일회성 알림, 정확한 시각 실행, 고정 간격 폴링, cron 작업, 채팅에서 바로 만드는 리마인더, 현재 대화 기준 일괄 정리까지 같은 런타임에서 처리합니다.
- **[03-19]** 여러 AI CLI Coding Agent를 하나의 협업 흐름 안에 넣을 수 있어, 도구마다 따로 운영할 필요가 줄었습니다.
- **[03-19]** 현재 가장 안정적인 협업 형태는 Lead가 대외 응답을 맡고 specialist가 뒤에서 실행한 뒤, Lead가 milestone을 정리하는 방식입니다.
- **[03-19]** WeChat, Lark, DingTalk Stream Mode, DingTalk Custom Robot Webhook, WebSocket에 연결할 수 있어 로컬 사용에서 그룹 채팅까지 단계적으로 확장할 수 있습니다.
- **[03-19]** 이제 하나의 `clawbro` 런타임을 여러 IM에 상시 연결해 두고 WeChat, Lark, DingTalk, 팀 대화를 동시에 운영할 수 있습니다.
- **[03-19]** approvals, allowlist, memory-aware sessions, `/health`, `/status`, `/doctor`, diagnostics 기능을 제공합니다.

> `clawBro` 는 엔지니어링 협업, 리서치 워크플로, 그룹 채팅 AI 도우미, 멀티 Agent 실험에 적합합니다.

## clawBro의 핵심 특징

🤖 **통합 CLI Coding Agent**: Claude Code, Codex, Qwen, Qoder, Gemini 등을 같은 시스템 안에서 함께 사용할 수 있습니다.

👥 **팀 협업 모드**: `solo`, `multi`, `team` 을 지원하며, 개인 사용에서 Lead + Specialists 협업까지 확장할 수 있습니다.

💬 **채팅 연결**: Lark / DingTalk에 연결할 수 있고, WebSocket부터 시작하는 것도 가능합니다.

📡 **상시 Multi-IM 연결**: 하나의 `clawbro` 런타임을 Lark, DingTalk Stream, DingTalk Webhook, WebSocket에 동시에 연결해 두고 계속 대화할 수 있습니다.

🧠 **기억과 습관**: 공유 메모리와 역할 메모리를 통해 프로젝트 맥락과 선호를 누적할 수 있습니다.

⏰ **스케줄링과 폴링**: `delay`, `at`, `every`, `cron` 네 가지 방식으로 알림도 만들고, 나중에 agent가 다시 일하게 할 수도 있습니다.

🛡️ **운영 가능성**: config validate, doctor/status, 승인 흐름, 헬스 체크를 제공합니다.

## 🏗️ 아키텍처

```text
사용자 / 그룹 / WebSocket / Scheduled Jobs
              |
              v
           clawbro
              |
              +--> 라우팅 / 세션 / 메모리 / Bindings / Team
              |
              +--> ClawBro Native ------> runtime-bridge ------> clawbro-agent-sdk
              |
              +--> Coding CLI Bridge ---> Claude / Codex / Qwen / Qoder / Gemini / custom coding CLIs
              |
              +--> OpenClaw Gateway ----> remote agent runtime
              |
              +--> Channels ------------> Lark / DingTalk / WebSocket delivery
```

## 목차

- [프로젝트 상태](#-프로젝트-상태)
- [핵심 특징](#clawbro의-핵심-특징)
- [아키텍처](#️-아키텍처)
- [기능 개요](#-기능-개요)
- [활용 시나리오](#-활용-시나리오)
- [설치](#-설치)
- [빠른 시작](#-빠른-시작)
- [정시 작업](#-정시-작업)
- [팀 모드](#-팀-모드)
- [CLI Coding Agent 통합](#-cli-coding-agent-통합)
- [채팅 채널](#-채팅-채널)
- [설정과 운영](#️-설정과-운영)
- [프로젝트 구조](#️-프로젝트-구조)
- [문서](#-문서)
- [포지셔닝](#-포지셔닝)

## ✨ 기능 개요

<table align="center">
  <tr align="center">
    <th><p align="center">🤖 Agent Hub</p></th>
    <th><p align="center">👥 팀 협업</p></th>
    <th><p align="center">🧠 장기 기억</p></th>
  </tr>
  <tr>
    <td align="center">Claude Code, Codex, Qwen, Qoder, Gemini 등을 하나의 환경에서 다룰 수 있습니다.</td>
    <td align="center">Lead + Specialists, 그룹 역할 라우팅, milestone 중심 협업을 지원합니다.</td>
    <td align="center">공유 메모리와 역할 메모리로 습관, 맥락, 선호를 지속적으로 축적합니다.</td>
  </tr>
</table>

## 🌟 활용 시나리오

### 🚀 풀스택 앱 개발

- `@planner` 가 요구사항을 분해
- `@coder` 가 API, UI, 데이터 모델 구현
- `@reviewer` 가 품질과 리스크 점검
- `@tester` 가 경계 케이스와 빠진 테스트 보완

Team 모드에서는 Lead가 대외 대화를 정리하고 specialist가 뒤에서 작업합니다. 그룹 채팅에서는 AI 프로젝트 룸처럼 동작할 수 있습니다.

### 📚 심층 리서치와 보고서 작성

- `@researcher` 가 자료 수집
- `@critic` 이 약점과 반례 탐색
- `@writer` 가 보고서 작성
- Lead가 진행 상황과 최종 결론 요약

기술 조사, 아키텍처 비교, 논문 리뷰, 업계 분석에 적합합니다.

### 🧑‍💻 PR 리뷰와 설계 리뷰

- `@coder` 가 구현 확인
- `@reviewer` 가 유지보수성과 리스크 검토
- `@researcher` 가 의존성이나 대안을 조사
- Lead가 최종 판단을 정리

단발성 bot 응답보다 실제 팀 리뷰에 더 가까운 흐름을 제공합니다.

### 💬 그룹 채팅 속 다중 Agent 워크벤치

- `@planner`
- `@coder`
- `@reviewer`
- `@researcher`

같은 역할명을 사용해 개발팀, 스터디 그룹, 제품 토론, 기술 지원 방에서 활용할 수 있습니다.

### 🧠 기억 기반 Coding 습관

지속적으로 사용할수록 다음과 같은 정보를 점점 기억합니다.

- 아키텍처 선호
- 리뷰 기준
- 네이밍 스타일
- 프로젝트 고유 흐름
- 반복적으로 기억시키는 항목

### 🎭 놀이 활용: 마피아 / TRPG / 역할 채팅

- Lead가 마피아 진행자 역할 수행
- specialist가 심판, 해설, 복기 담당, 캐릭터 역할 수행
- 그룹에서 스토리 진행이나 역할 대화를 운영

## 📦 설치

**권장**

```bash
cargo install clawbro
```

**GitHub Release** (Rust 불필요)

1. GitHub Releases 에서 내 플랫폼용 아카이브를 다운로드
2. 압축 해제
3. `./clawbro --version` 실행

전역 명령처럼 쓰고 싶다면 `PATH` 에 넣으면 됩니다:

```bash
chmod +x clawbro
mv clawbro ~/.local/bin/clawbro
```

**npm** (바이너리 설치기)

```bash
npm install -g clawbro
clawbro --version
```

**소스에서 빌드** (개발자 경로)

```bash
cd clawBro
cargo build -p clawbro --bin clawbro
```

## 🚀 빠른 시작

> [!TIP]
> 여기서는 대표 사례만 남깁니다. 더 자세한 구성은 [Setup Guide](./docs/setup.md) 또는 `clawbro config wizard` 로 이어가면 됩니다.

**사례 1: 로컬 최소 시작**

```bash
cargo install clawbro
clawbro setup
clawbro config validate
source ~/.clawbro/.env
clawbro serve
```

**사례 2: 공식 WeChat 랍스터, solo**

```bash
clawbro setup --preset wechat-solo
clawbro config channel login wechat
clawbro config channel setup-solo wechat --agent claw
clawbro config validate
clawbro serve
```

**사례 3: WeChat DM Team, front bot이 부대를 이끔**

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

## ⏰ 정시 작업

`clawBro` 는 불렀을 때만 답하는 도구가 아니라, 시간이 되면 스스로 다시 움직일 수 있습니다.

- **4가지 스케줄 방식**:
  - `delay`: “1분 후에 휴대폰 충전하라고 알려줘”
  - `at`: “내일 오전 9시에 회의 알려줘”
  - `every`: “30분마다 서비스 상태를 확인해줘”
  - `cron`: “평일 18시에 issue 진행 상황을 요약해줘”
- **2가지 실행 스타일**:
  - 고정 알림은 채팅으로 바로 다시 보냅니다
  - 동적인 작업은 시간이 되면 agent를 다시 깨워서 실행합니다
- **채팅에서는 자연어로 바로 말하면 됩니다**:
  - “1분 후에 휴대폰 충전하라고 알려줘”
  - “매분 베이징 시간을 알려줘”
  - “휴대폰 충전 알림 삭제해줘”
  - “이 대화의 알림을 전부 지워줘”
- **운영자는 CLI로도 다룰 수 있습니다**:

```bash
clawbro schedule add-delay --name phone-charge --delay 1m --prompt "Reminder: charge your phone"
clawbro schedule add-every --name service-check --every 30m --target-kind agent-turn --prompt "Check service status and report anomalies."
clawbro schedule list
clawbro schedule delete --name phone-charge
clawbro schedule delete-all --current-session-key 'lark@alpha:user:ou_xxx'
```

한마디로, 리마인더는 정확히 튀어나오고, agent 작업은 시간이 되면 다시 출근합니다.

## 📦 배포 방식

지금 `clawBro` 는 세 가지 설치 경로를 제공합니다.

- **Cargo**:
  - `cargo install clawbro`
- **GitHub Release 바이너리**:
  - 플랫폼에 맞는 아카이브 다운로드
  - 압축을 풀고 `./clawbro` 실행
  - 필요하면 나중에 `PATH` 로 이동
- **npm 바이너리 설치기**:
  - `npm install -g clawbro`
  - 설치 중 현재 머신에 맞는 GitHub Release 바이너리를 내려받음

1단계 지원 플랫폼:

- macOS Apple Silicon
- macOS Intel
- Linux x86_64

macOS 에서는 아직 notarization 이 없어서 첫 실행 시 일반적인 경고가 뜰 수 있습니다.

## 👥 팀 모드

| 모드 | 역할 | 적합한 용도 |
| --- | --- | --- |
| **Solo** | 단일 Agent | 개인 비서, 로컬 보조 |
| **Multi** | 이름 있는 Agent 구성 시작점 | `@planner`, `@reviewer` 같은 역할 기반 룸 |
| **Team** | Lead가 specialist를 조율 | 개발 협업, 심층 조사, 리뷰 작업 |

> 현재 가장 안정적인 형태는 Lead 주도 Team 모드이며, specialist는 뒤에서 실행을 담당합니다.

## 🔌 CLI Coding Agent 통합

| 통합 경로 | 현재 역할 | 설명 |
| --- | --- | --- |
| **ClawBro Native** | 기본 실행 경로 | 내부 runtime bridge 사용 |
| **Coding CLI bridge** | 외부 coding CLI 호환 계층 | 여러 CLI Coding Agent를 하나의 사용 방식으로 통합 |
| **OpenClaw Gateway** | 원격 실행 연결 | OpenClaw WS 실행 경로 |

지원 예시:

- Claude
- Codex
- Qwen
- Qoder
- Gemini
- custom coding CLIs

## 💬 채팅 채널

| 채널 | 상태 | 설명 |
| --- | --- | --- |
| **WeChat (official lobster)** | Structured | 공식 WeChat 로그인, WeChat solo, WeChat DM Team 지원 |
| **Lark / Feishu** | Complete | `final_only`, `progress_compact` 지원 |
| **DingTalk** | Structured | 같은 방향의 기능 제공 |
| **WebSocket** | Structured | 최소 시작 경로로 권장 |

## ⚙️ 설정과 운영

주요 명령:

- `clawbro setup`
- `clawbro config wizard`
- `clawbro config validate`
- `clawbro config channel login wechat`
- `clawbro config channel setup-solo wechat --agent claw`
- `clawbro config channel setup-team wechat --scope user:o9cqxxxx@im.wechat --front-bot claude --specialist claw`
- `clawbro serve`
- `clawbro status`
- `clawbro doctor`

## 🗂️ 프로젝트 구조

```text
clawBro/
├── crates/clawbro-server/
├── crates/clawbro-agent/
├── crates/clawbro-runtime/
├── crates/clawbro-channels/
├── crates/clawbro-agent-sdk/
├── crates/clawbro-session/
├── crates/clawbro-skills/
├── crates/clawbro-server/src/scheduler/
└── docs/
```

## 📚 문서

- [Setup Guide](./docs/setup.md)
- [Getting Started From Zero](./docs/getting-started-from-zero.md)
- [Runtime Backends](./docs/runtime-backends.md)
- [Backend Support Matrix](./docs/backend-support-matrix.md)
- [Routing Contract](./docs/routing-contract.md)
- [Doctor and Status Operations](./docs/operations/doctor-and-status.md)
- [Context Filesystem Contract](./docs/context-filesystem-contract.md)

## 🎯 포지셔닝

- 여러 coding agent를 채팅과 워크플로에 연결하고 싶은 팀
- Lead + Specialists 방식으로 복잡한 작업을 처리하고 싶은 개인 개발자
- OpenClaw와 CLI Coding Agent 활용 방식을 하나로 묶고 싶은 설계자
