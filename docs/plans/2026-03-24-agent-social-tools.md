# Agent Social Tools: list_agents + send_message Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 将 `list_agents`（查询当前 roster 中所有 Agent）和 `send_message`（Agent 主动向另一个 Agent 或用户发送消息）作为两个新的 TeamTool 变体，完整集成到已有的 Team Tool 基础设施中，无需新端点、新环境变量或新 Augmentor。

**Architecture:** 在现有 `TeamTool` 枚举中新增两个变体，在 `invoke_team_http_request`（`team_contract/projection/http_rpc.rs`）处做 **短路处理**（在调用 registry/orchestrator 之前直接用 `AppState` 响应），`list_agents` 直接读 `state.registry.roster`，`send_message` 调用 `spawn_im_turn` 发起新的 Agent 对话轮次。最后在 `embedded_agent/team.rs` 用已有 `define_team_tool!` 宏注册两个 rig 工具，并修复 Solo 角色当前无任何 Team Tool 可用的问题。

**Tech Stack:** Rust · axum · rig-core · `spawn_im_turn` · `WsVirtualChannel` · `MsgSource` · `AgentRoster`

---

## 文件索引（所有需要修改或新增的文件）

| 文件 | 操作 |
|------|------|
| `crates/clawbro-server/src/team_contract/schema.rs` | Modify — 新增 `ListAgents`、`SendMessage` 变体 |
| `crates/clawbro-server/src/team_contract/visibility.rs` | Modify — 三个角色均开放新两个工具 |
| `crates/clawbro-server/src/team_contract/projection/http_rpc.rs` | Modify — 短路处理新两个调用 |
| `crates/clawbro-server/src/embedded_agent/team.rs` | Modify — 新增 rig 工具包装 + Solo 角色注入 |
| `crates/clawbro-server/src/protocol/types.rs` | Verify — 确认 `MsgSource::TeamNotify` 已存在或添加 `AgentDispatch` |

---

### Task 1: 扩展 schema — 新增两个 TeamTool 变体

**Files:**
- Modify: `crates/clawbro-server/src/team_contract/schema.rs`

**Step 1: 写失败测试（在 schema.rs 的 tests 模块）**

在 `schema.rs` 的 `#[cfg(test)] mod tests` 中添加：

```rust
#[test]
fn list_agents_tool_maps_to_correct_variant() {
    let call = TeamToolCall::ListAgents;
    assert_eq!(tool_for_call(&call), TeamTool::ListAgents);
}

#[test]
fn send_message_tool_maps_to_correct_variant() {
    let call = TeamToolCall::SendMessage {
        target: "codex".to_string(),
        message: "please review".to_string(),
        scope: None,
    };
    assert_eq!(tool_for_call(&call), TeamTool::SendMessage);
}
```

**Step 2: 运行测试确认失败**

```bash
cd clawBro && cargo test -p clawbro-server schema 2>&1 | tail -20
```
Expected: compile error — `TeamTool::ListAgents` 未定义

**Step 3: 在 `TeamTool` 枚举末尾新增两个变体**

在 `schema.rs` 的 `pub enum TeamTool` 中（`RequestHelp` 之后）追加：

```rust
    ListAgents,
    SendMessage,
```

**Step 4: 在 `TeamToolCall` 枚举末尾新增两个变体**

```rust
    ListAgents,
    SendMessage {
        /// Agent name (from roster) or "user" to reach the human operator.
        target: String,
        /// Message body to deliver.
        message: String,
        /// Optional scope override. If omitted, uses the caller's own session scope.
        #[serde(default)]
        scope: Option<String>,
    },
```

**Step 5: 更新 `tool_for_call` 的 match**

```rust
TeamToolCall::ListAgents => TeamTool::ListAgents,
TeamToolCall::SendMessage { .. } => TeamTool::SendMessage,
```

**Step 6: 更新 `canonical_progress_tools` 和 `canonical_terminal_tools`（不需要修改，仅确认不破坏）**

这两个函数返回已有工具子集，不需要包含新工具。

**Step 7: 运行测试确认通过**

```bash
cd clawBro && cargo test -p clawbro-server schema 2>&1 | tail -20
```
Expected: 所有 schema tests PASS

**Step 8: Commit**

```bash
cd clawBro && git add crates/clawbro-server/src/team_contract/schema.rs
git commit -m "feat(team-contract): add ListAgents + SendMessage TeamTool variants"
```

---

### Task 2: 更新 visibility — 三个角色均可见新工具

**Files:**
- Modify: `crates/clawbro-server/src/team_contract/visibility.rs`

**Step 1: 写失败测试**

在 `visibility.rs` 的 tests 模块末尾追加：

```rust
#[test]
fn solo_visibility_contains_social_tools() {
    let visible = visible_team_tools_for_role(RuntimeRole::Solo).visible;
    assert!(visible.contains(&TeamTool::ListAgents));
    assert!(visible.contains(&TeamTool::SendMessage));
}

#[test]
fn leader_visibility_contains_social_tools() {
    let visible = visible_team_tools_for_role(RuntimeRole::Leader).visible;
    assert!(visible.contains(&TeamTool::ListAgents));
    assert!(visible.contains(&TeamTool::SendMessage));
}

#[test]
fn specialist_visibility_contains_social_tools() {
    let visible = visible_team_tools_for_role(RuntimeRole::Specialist).visible;
    assert!(visible.contains(&TeamTool::ListAgents));
    assert!(visible.contains(&TeamTool::SendMessage));
}
```

**Step 2: 运行测试确认失败**

```bash
cd clawBro && cargo test -p clawbro-server visibility 2>&1 | tail -20
```
Expected: 3 tests FAIL (solo/leader/specialist 均缺少新工具)

**Step 3: 更新 `visible_team_tools_for_role`**

在三个 match 分支中各追加两个工具：

```rust
RuntimeRole::Solo => vec![
    TeamTool::ListAgents,
    TeamTool::SendMessage,
],
RuntimeRole::Leader => vec![
    TeamTool::CreateTask,
    TeamTool::StartExecution,
    TeamTool::RequestConfirmation,
    TeamTool::PostUpdate,
    TeamTool::GetTaskStatus,
    TeamTool::AssignTask,
    TeamTool::AcceptTask,
    TeamTool::ReopenTask,
    TeamTool::ListAgents,   // new
    TeamTool::SendMessage,  // new
],
RuntimeRole::Specialist => vec![
    TeamTool::CheckpointTask,
    TeamTool::SubmitTaskResult,
    TeamTool::BlockTask,
    TeamTool::RequestHelp,
    TeamTool::ListAgents,   // new
    TeamTool::SendMessage,  // new
],
```

**Step 4: 运行测试确认通过**

```bash
cd clawBro && cargo test -p clawbro-server visibility 2>&1 | tail -20
```
Expected: 所有 visibility tests PASS

**Step 5: Commit**

```bash
cd clawBro && git add crates/clawbro-server/src/team_contract/visibility.rs
git commit -m "feat(team-contract): expose ListAgents + SendMessage to all three roles"
```

---

### Task 3: http_rpc 短路处理 — list_agents 实现

**Files:**
- Modify: `crates/clawbro-server/src/team_contract/projection/http_rpc.rs`

**背景：** `invoke_team_http_request` 目前直接把请求交给 `state.registry.invoke_team_tool()`，而 registry 的 `invoke_team_tool` 要求必须有 `TeamOrchestrator`（Team 模式运行中才有）。Solo Agent 没有 Orchestrator，所以 `ListAgents` 和 `SendMessage` 必须在调用 registry 之前被短路拦截，直接用 `AppState` 处理。

**Step 1: 写失败测试（unit test in http_rpc.rs）**

在 `http_rpc.rs` 末尾追加 test 模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::SessionRegistry;
    use crate::agent_core::roster::{AgentEntry, AgentRoster};
    use crate::config::GatewayConfig;
    use crate::protocol::{SessionKey, TeamToolCall, TeamToolRequest};
    use crate::session::{SessionManager, SessionStorage};
    use crate::skills_internal::SkillLoader;
    use std::sync::Arc;

    fn make_state_with_roster(roster: AgentRoster) -> AppState {
        let cfg = GatewayConfig::default();
        let storage = SessionStorage::new(cfg.session.dir.clone());
        let session_manager = Arc::new(SessionManager::new(storage));
        // ⚠️ Must include global_dirs to match established test pattern
        let mut all_skill_dirs = vec![cfg.skills.dir.clone()];
        all_skill_dirs.extend(cfg.skills.global_dirs.iter().cloned());
        let skill_loader = SkillLoader::new(all_skill_dirs);
        let skills = skill_loader.load_all();
        let system_injection = skill_loader.build_system_injection(&skills);
        let skill_dirs = skill_loader.search_dirs().to_vec();
        // ⚠️ SessionRegistry::new signature: (default_backend_id, session_manager, system_injection,
        //    roster [4th], memory_system [5th], default_persona_dir [6th], default_workspace [7th], skill_dirs)
        let (registry, _rx) = SessionRegistry::new(
            None,
            session_manager,
            system_injection,
            Some(roster),   // 4th: roster
            None,           // 5th: memory_system
            None,           // 6th: default_persona_dir
            None,           // 7th: default_workspace
            skill_dirs,
        );
        AppState {
            registry: Arc::clone(&registry),
            runtime_registry: Arc::new(crate::runtime::BackendRegistry::new()),
            event_tx: registry.global_sender(),
            cfg: Arc::new(cfg),
            channel_registry: Arc::new(crate::channel_registry::ChannelRegistry::new()),
            dingtalk_webhook_channel: None,
            runtime_token: Arc::new("tok".to_string()),
            approvals: crate::runtime::ApprovalBroker::default(),
            scheduler_service: crate::scheduler_runtime::build_test_scheduler_service(),
            config_path: Arc::new(crate::config::config_file_path()),
        }
    }

    #[tokio::test]
    async fn list_agents_returns_roster_without_orchestrator() {
        let roster = AgentRoster::new(vec![
            AgentEntry {
                name: "coder".to_string(),
                mentions: vec!["@coder".to_string()],
                backend_id: "claude".to_string(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
            AgentEntry {
                name: "reviewer".to_string(),
                mentions: vec!["@reviewer".to_string()],
                backend_id: "codex".to_string(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
        ]);
        let state = make_state_with_roster(roster);
        let request = TeamToolRequest {
            session_key: SessionKey::new("ws", "main"),
            call: TeamToolCall::ListAgents,
        };
        let (status, resp) = invoke_team_http_request(&state, "tok", request).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(resp.ok);
        assert!(resp.message.contains("coder"));
        assert!(resp.message.contains("reviewer"));
        let payload = resp.payload.unwrap();
        let arr = payload.as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }
}
```

**Step 2: 运行测试确认失败**

```bash
cd clawBro && cargo test -p clawbro-server http_rpc 2>&1 | tail -20
```
Expected: FAIL — `ListAgents` 进入 registry 找不到 orchestrator

**Step 3: 在 `invoke_team_http_request` 中短路处理 ListAgents**

修改函数，在 token 校验通过之后、调用 `state.registry.invoke_team_tool()` 之前，插入匹配分支：

```rust
pub async fn invoke_team_http_request(
    state: &AppState,
    provided_token: &str,
    request: TeamToolRequest,
) -> (StatusCode, TeamToolResponse) {
    if provided_token != *state.runtime_token {
        return (
            StatusCode::UNAUTHORIZED,
            TeamToolResponse {
                ok: false,
                message: "invalid runtime token".to_string(),
                payload: None,
            },
        );
    }

    // Short-circuit: social tools handled directly via AppState (no orchestrator needed).
    // These two tools work in Solo mode where no TeamOrchestrator exists.
    match &request.call {
        TeamToolCall::ListAgents => {
            return handle_list_agents(state);
        }
        TeamToolCall::SendMessage { .. } => {
            // Destructure by value after the match (avoids borrow conflict with request.call)
        }
        _ => {
            return match state
                .registry
                .invoke_team_tool(&request.session_key, request.call)
                .await
            {
                Ok(resp) => (StatusCode::OK, resp),
                Err(err) => (
                    StatusCode::BAD_REQUEST,
                    TeamToolResponse {
                        ok: false,
                        message: err.to_string(),
                        payload: None,
                    },
                ),
            };
        }
    }

    // Reached only for SendMessage (ownership moved out of match)
    let TeamToolCall::SendMessage { target, message, scope } = request.call else {
        unreachable!()
    };
    handle_send_message(state, &request.session_key, &target, &message, scope.as_deref()).await
}
```

**Step 4: 实现 `handle_list_agents`**

在同文件新增（在 `invoke_team_http_request` 之后）：

```rust
fn handle_list_agents(state: &AppState) -> (StatusCode, TeamToolResponse) {
    let agents: Vec<serde_json::Value> = state
        .registry
        .roster
        .as_ref()
        .map(|r| {
            r.all_agents()
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "name": e.name,
                        "mentions": e.mentions,
                        "backend_id": e.backend_id,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let message = if agents.is_empty() {
        "No agents configured in roster.".to_string()
    } else {
        let names: Vec<&str> = agents
            .iter()
            .filter_map(|v| v["name"].as_str())
            .collect();
        format!("Roster has {} agent(s): {}.", agents.len(), names.join(", "))
    };

    (
        StatusCode::OK,
        TeamToolResponse {
            ok: true,
            message,
            payload: Some(serde_json::Value::Array(agents)),
        },
    )
}
```

**Step 5: 运行测试确认通过**

```bash
cd clawBro && cargo test -p clawbro-server http_rpc 2>&1 | tail -20
```
Expected: `list_agents_returns_roster_without_orchestrator` PASS

**Step 6: Commit（仅 list_agents）**

```bash
cd clawBro && git add crates/clawbro-server/src/team_contract/projection/http_rpc.rs
git commit -m "feat(http-rpc): short-circuit ListAgents, read roster from AppState"
```

---

### Task 4: http_rpc 短路处理 — send_message 实现

**Files:**
- Modify: `crates/clawbro-server/src/team_contract/projection/http_rpc.rs`

**设计说明：**
- `send_message` 的 `target` 字段是 roster 中的 agent name（如 `"coder"`）或 `"user"` 代表用户。
- 若 target 是 Agent：通过 `spawn_im_turn` + `WsVirtualChannel` 向该 Agent 的 session 发送消息（`target_agent` 字段设为 agent name），`MsgSource::TeamNotify`（防止 Lead 递归响应）。
- 若 target 是 `"user"`：用 `spawn_im_turn` 发到调用者自己的 session key（相当于 Agent 主动 PostUpdate 给用户，用 `MsgSource::TeamNotify` 避免触发回复）。
- Scope 解析：`scope` 参数若空则使用调用者 session 的 scope（`request.session_key.scope`）。

**Step 1: 写失败测试**

在 tests 模块追加：

```rust
#[tokio::test]
async fn send_message_to_unknown_agent_returns_error() {
    // Empty roster — any agent name should fail
    let state = make_state_with_roster(AgentRoster::new(vec![]));
    let request = TeamToolRequest {
        session_key: SessionKey::new("ws", "main"),
        call: TeamToolCall::SendMessage {
            target: "ghost-agent".to_string(),
            message: "hello".to_string(),
            scope: None,
        },
    };
    let (status, resp) = invoke_team_http_request(&state, "tok", request).await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert!(!resp.ok);
    assert!(resp.message.contains("ghost-agent"));
}

#[tokio::test]
async fn send_message_to_user_dispatches_without_orchestrator() {
    // "user" target bypasses roster lookup — succeeds even with empty roster
    // V1: delivers via WsVirtualChannel (WebSocket broadcast only, not IM)
    let state = make_state_with_roster(AgentRoster::new(vec![]));
    let request = TeamToolRequest {
        session_key: SessionKey::new("ws", "main"),
        call: TeamToolCall::SendMessage {
            target: "user".to_string(),
            message: "task complete".to_string(),
            scope: None,
        },
    };
    let (status, resp) = invoke_team_http_request(&state, "tok", request).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(resp.ok);
}

#[tokio::test]
async fn send_message_to_known_agent_dispatches() {
    let roster = AgentRoster::new(vec![AgentEntry {
        name: "coder".to_string(),
        mentions: vec!["@coder".to_string()],
        backend_id: "claude".to_string(),
        persona_dir: None,
        workspace_dir: None,
        extra_skills_dirs: vec![],
    }]);
    let state = make_state_with_roster(roster);
    let request = TeamToolRequest {
        session_key: SessionKey::new("ws", "main"),
        call: TeamToolCall::SendMessage {
            target: "coder".to_string(),
            message: "please review PR #42".to_string(),
            scope: None,
        },
    };
    let (status, resp) = invoke_team_http_request(&state, "tok", request).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(resp.ok);
    // Response message should include the @mention form
    assert!(resp.message.contains("@coder"));
}
```

**Step 2: 运行测试确认失败**

```bash
cd clawBro && cargo test -p clawbro-server http_rpc 2>&1 | tail -20
```
Expected: 2 new tests FAIL

**Step 3: 在文件顶部添加 imports**

```rust
use crate::channels_internal::ws_virtual::WsVirtualChannel;
use crate::config::ProgressPresentationMode;
use crate::im_sink::spawn_im_turn;
use crate::protocol::{InboundMsg, MsgContent, MsgSource};
use std::sync::Arc;
```

**Step 4: 实现 `handle_send_message`**

关键设计说明：
- `target_agent` 字段必须是 `@mention` 格式（`"@coder"`），因为 `routing.rs` 中通过 `find_by_mention()` 路由，而不是 `find_by_name()`（已在 routing.rs 测试中确认：`target_agent: Some("@claw".into())` 等）
- `"user"` 是保留目标，绕过 roster 路由，直接投递到调用者的 session（等价于 PostUpdate）
- **限制（V1）**：响应经 `WsVirtualChannel` 广播，只能通过 WebSocket 送达。DingTalk/Lark 用户的反向通知在此版本暂不支持，将在后续版本通过 `channel_registry` 查找原始 channel 来解决。

```rust
async fn handle_send_message(
    state: &AppState,
    caller_session: &crate::protocol::SessionKey,
    target: &str,
    message: &str,
    scope_override: Option<&str>,
) -> (StatusCode, TeamToolResponse) {
    // "user" is a reserved target: deliver to the caller's own session (no agent routing).
    // This is equivalent to PostUpdate but available outside team mode.
    // V1 limitation: response is broadcast via WebSocket only (WsVirtualChannel is a no-op sender).
    if target.eq_ignore_ascii_case("user") {
        let scope = scope_override
            .unwrap_or(&caller_session.scope)
            .to_string();
        let session_key = crate::protocol::SessionKey::new("ws", &scope);
        let turn_id = uuid::Uuid::new_v4().to_string();
        let inbound = InboundMsg {
            id: turn_id,
            session_key,
            content: MsgContent::text(message),
            sender: "agent".to_string(),
            channel: "ws".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: MsgSource::TeamNotify,
        };
        spawn_im_turn(
            Arc::clone(&state.registry),
            Arc::new(WsVirtualChannel),
            Arc::clone(&state.channel_registry),
            Arc::clone(&state.cfg),
            inbound,
            ProgressPresentationMode::FinalOnly,
        );
        return (
            StatusCode::OK,
            TeamToolResponse {
                ok: true,
                message: "Message delivered to user session.".to_string(),
                payload: None,
            },
        );
    }

    // Target is an agent name — verify it exists in roster before dispatching.
    let agent_exists = state
        .registry
        .roster
        .as_ref()
        .and_then(|r| r.find_by_name(target))
        .is_some();

    if !agent_exists {
        return (
            StatusCode::BAD_REQUEST,
            TeamToolResponse {
                ok: false,
                message: format!("Agent '{}' not found in roster.", target),
                payload: None,
            },
        );
    }

    let scope = scope_override
        .unwrap_or(&caller_session.scope)
        .to_string();
    let session_key = crate::protocol::SessionKey::new("ws", &scope);
    let turn_id = uuid::Uuid::new_v4().to_string();
    // ⚠️ target_agent must be @mention format — routing.rs uses find_by_mention(),
    //    not find_by_name(). Agent names from roster must be prefixed with "@".
    //    Example: name="coder" → target_agent=Some("@coder")
    let mention = format!("@{}", target);
    let inbound = InboundMsg {
        id: turn_id,
        session_key,
        content: MsgContent::text(message),
        sender: "agent".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: Some(mention),
        source: MsgSource::TeamNotify,
    };
    spawn_im_turn(
        Arc::clone(&state.registry),
        Arc::new(WsVirtualChannel),
        Arc::clone(&state.channel_registry),
        Arc::clone(&state.cfg),
        inbound,
        ProgressPresentationMode::FinalOnly,
    );

    (
        StatusCode::OK,
        TeamToolResponse {
            ok: true,
            message: format!("Message dispatched to agent '@{}'.", target),
            payload: None,
        },
    )
}
```

**Step 5: 运行测试**

```bash
cd clawBro && cargo test -p clawbro-server http_rpc 2>&1 | tail -20
```
Expected: 所有 http_rpc tests PASS

**Step 6: 运行完整编译检查**

```bash
cd clawBro && cargo check -p clawbro-server 2>&1 | tail -30
```
Expected: 0 errors

**Step 7: Commit**

```bash
cd clawBro && git add crates/clawbro-server/src/team_contract/projection/http_rpc.rs
git commit -m "feat(http-rpc): short-circuit SendMessage, dispatch via spawn_im_turn"
```

---

### Task 5: executor.rs — 防御性处理 (ListAgents/SendMessage 不应进入 executor)

**Files:**
- Modify: `crates/clawbro-server/src/team_contract/executor.rs`

**背景：** `execute_team_contract_call` 是一个非穷举 match，编译器会对未处理的变体报错（如果 `ensure_team_call_allowed` 没有先拦截）。由于 `ListAgents` 和 `SendMessage` 已被 http_rpc 短路，executor 永远不会收到这两个调用，但 Rust 的穷举 match 要求必须有 arm。

**Step 1: 写失败测试**

```rust
// In executor.rs tests — reuse the existing make_orchestrator() helper, do NOT redefine it.
#[tokio::test]
async fn executor_rejects_list_agents_as_not_handled_here() {
    let orch = make_orchestrator(); // already defined at line ~267 of executor.rs
    let result = execute_team_contract_call(
        Arc::clone(&orch),
        RuntimeRole::Solo,
        TeamToolCall::ListAgents,
    )
    .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("handled at HTTP layer"));
}

#[tokio::test]
async fn executor_rejects_send_message_as_not_handled_here() {
    let orch = make_orchestrator();
    let result = execute_team_contract_call(
        Arc::clone(&orch),
        RuntimeRole::Leader,
        TeamToolCall::SendMessage {
            target: "coder".to_string(),
            message: "hello".to_string(),
            scope: None,
        },
    )
    .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("handled at HTTP layer"));
}
```

**Step 2: 运行测试确认失败（编译错误 — executor match 不穷举）**

```bash
cd clawBro && cargo test -p clawbro-server executor 2>&1 | tail -20
```
Expected: compile error — non-exhaustive patterns

**Step 3: 在 executor.rs 的 match 末尾追加两个 arm**

在 `execute_team_contract_call` 的最终 `};` 之前：

```rust
        TeamToolCall::ListAgents | TeamToolCall::SendMessage { .. } => {
            anyhow::bail!(
                "ListAgents and SendMessage are handled at HTTP layer and must not reach executor"
            )
        }
```

**Step 4: 运行测试**

```bash
cd clawBro && cargo test -p clawbro-server executor 2>&1 | tail -20
```
Expected: 所有 executor tests PASS

**Step 5: Commit**

```bash
cd clawBro && git add crates/clawbro-server/src/team_contract/executor.rs
git commit -m "fix(executor): guard ListAgents/SendMessage arms (handled upstream)"
```

---

### Task 6: embedded_agent/team.rs — 注册两个 rig 工具 + 修复 Solo

**Files:**
- Modify: `crates/clawbro-server/src/embedded_agent/team.rs`

**背景：** 这是 Agent 实际使用工具的入口。`define_team_tool!` 宏生成实现了 rig `Tool` trait 的结构体。当前 Solo 角色 (`ExecutionRole::Solo => {}`) 完全没有工具，需要修复以注入两个新工具。

**Step 1: 修复 `ClawBroTeamToolAugmentor::augment()` — 为 Solo 绕过 `team_tools` 门控**

`augment()` 开头有一个早退门控（`bridge.rs:466` 确认 Solo 的 `tool_surface.team_tools` 默认为 `false`）：

```rust
if !session.tool_surface.team_tools {
    return builder;  // Solo agents exit here — nothing injected
}
```

后续的 `register_team_tools_with_progress` 对 Solo 的修复**永远不会被执行**，除非我们在这里为 Solo 单独处理。

修改 `augment()` 如下（只注入两个社交工具，不绕过其他工具的 team_tools 门控）：

```rust
impl RuntimeToolAugmentor for ClawBroTeamToolAugmentor {
    fn augment<M: CompletionModel>(
        &self,
        builder: ConfiguredAgentBuilder<M>,
        session: &AgentTurnRequest,
        tracker: Option<ToolProgressTracker>,
        approval_mode: ApprovalMode,
    ) -> ConfiguredAgentBuilder<M> {
        let Some(endpoint) = self.endpoint.clone() else {
            return builder;
        };
        let client = TeamToolClient::new(endpoint, session.session_ref.clone());

        // Solo agents bypass the team_tools gate — they still get social tools
        // (list_agents + send_message) as long as an endpoint is configured.
        if !session.tool_surface.team_tools {
            if session.role == ExecutionRole::Solo {
                return inject_social_tools_only(builder, &client, tracker, approval_mode);
            }
            return builder;
        }

        match tracker {
            Some(tracker) => register_team_tools_with_progress(
                builder,
                session.role,
                &session.tool_surface.allowed_team_tools,
                client,
                tracker,
                approval_mode,
            ),
            None => register_team_tools(
                builder,
                session.role,
                &session.tool_surface.allowed_team_tools,
                client,
                approval_mode,
            ),
        }
    }
}

fn inject_social_tools_only<M: CompletionModel>(
    mut builder: ConfiguredAgentBuilder<M>,
    client: &TeamToolClient,
    tracker: Option<ToolProgressTracker>,
    approval_mode: ApprovalMode,
) -> ConfiguredAgentBuilder<M> {
    let tracker = tracker.unwrap_or_else(|| ToolProgressTracker::new(std::sync::Arc::new(|_| {})));
    builder = builder.tool(EventedTool::new(
        ListAgentsTool::new(client.clone()),
        Some(tracker.clone()),
        approval_mode,
    ));
    builder = builder.tool(EventedTool::new(
        SendMessageTool::new(client.clone()),
        Some(tracker),
        approval_mode,
    ));
    builder
}
```

**Step 3: 在现有 Args 结构体区域追加新 Args**

在 `RequestHelpArgs` 之后：

```rust
#[derive(Debug, Deserialize)]
struct SendMessageArgs {
    target: String,
    message: String,
    #[serde(default)]
    scope: Option<String>,
}
```

**Step 4: 用 `define_team_tool!` 宏声明两个新工具**

在所有现有 `define_team_tool!(...)` 调用之后追加：

```rust
define_team_tool!(
    ListAgentsTool,
    "list_agents",
    serde_json::Value,
    "All roles. List all agents available in the team roster. Returns their names, mentions, and backend IDs. Use before send_message to verify the target agent name.",
    json!({"type": "object", "properties": {}}),
    |_args: serde_json::Value| TeamToolCall::ListAgents
);

define_team_tool!(
    SendMessageTool,
    "send_message",
    SendMessageArgs,
    "All roles. Send a message to another agent by name or to 'user' (the human operator). The target agent will receive this as a new conversation turn. Use list_agents first to find valid agent names.",
    json!({
        "type": "object",
        "properties": {
            "target": {
                "type": "string",
                "description": "Agent name from the roster (e.g. 'coder', 'reviewer') or 'user' to reach the human operator."
            },
            "message": {
                "type": "string",
                "description": "Message body to send."
            },
            "scope": {
                "type": "string",
                "description": "Optional session scope override. Leave empty to use the current session scope."
            }
        },
        "required": ["target", "message"]
    }),
    |args: SendMessageArgs| TeamToolCall::SendMessage {
        target: args.target,
        message: args.message,
        scope: args.scope,
    }
);
```

**Step 5: 修复 `register_team_tools_with_progress` — Solo 分支（用于 Team 模式下的 Solo 升降）**

注意：Solo 的主要注入路径已在 Step 1 的 `inject_social_tools_only` 处理。
这里的 Solo 分支是 Team 模式内部可能降级为 Solo 的情况，同样注入社交工具：

```rust
ExecutionRole::Solo => {
    for tool in visible_tools {
        builder = add_social_team_tool(builder, tool, &client, &tracker, approval_mode);
    }
}
```

**Step 6: 在 `add_leader_team_tool` 中追加两个 social tool arm（在 `_ => builder` 之前）**

```rust
TeamTool::ListAgents => builder.tool(EventedTool::new(
    ListAgentsTool::new(client.clone()),
    Some(tracker.clone()),
    approval_mode,
)),
TeamTool::SendMessage => builder.tool(EventedTool::new(
    SendMessageTool::new(client.clone()),
    Some(tracker.clone()),
    approval_mode,
)),
```

**Step 7: 在 `add_specialist_team_tool` 中追加两个 social tool arm（在 `_ => builder` 之前）**

```rust
TeamTool::ListAgents => builder.tool(EventedTool::new(
    ListAgentsTool::new(client.clone()),
    Some(tracker.clone()),
    approval_mode,
)),
TeamTool::SendMessage => builder.tool(EventedTool::new(
    SendMessageTool::new(client.clone()),
    Some(tracker.clone()),
    approval_mode,
)),
```

**Step 8: 新增 `add_social_team_tool` helper（供 Solo 分支使用）**

```rust
fn add_social_team_tool<M: CompletionModel>(
    builder: ConfiguredAgentBuilder<M>,
    tool: TeamTool,
    client: &TeamToolClient,
    tracker: &ToolProgressTracker,
    approval_mode: ApprovalMode,
) -> ConfiguredAgentBuilder<M> {
    match tool {
        TeamTool::ListAgents => builder.tool(EventedTool::new(
            ListAgentsTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        TeamTool::SendMessage => builder.tool(EventedTool::new(
            SendMessageTool::new(client.clone()),
            Some(tracker.clone()),
            approval_mode,
        )),
        _ => builder,
    }
}
```

**Step 9: 编译检查**

```bash
cd clawBro && cargo check -p clawbro-server 2>&1 | tail -20
```
Expected: 0 errors

**Step 10: 运行所有 embedded_agent 相关测试**

```bash
cd clawBro && cargo test -p clawbro-server embedded_agent 2>&1 | tail -30
```
Expected: PASS (可能没有现有 embedded_agent 测试，但 0 failures)

**Step 11: Commit**

```bash
cd clawBro && git add crates/clawbro-server/src/embedded_agent/team.rs
git commit -m "feat(embedded-agent): register list_agents + send_message rig tools; fix Solo tool injection"
```

---

### Task 7: 全量测试 + 验证

**Step 1: 运行完整 workspace 测试**

```bash
cd clawBro && cargo test --workspace 2>&1 | tail -40
```
Expected: 0 failures

**Step 2: 运行 clippy**

```bash
cd clawBro && cargo clippy --all-targets -- -D warnings 2>&1 | tail -20
```
Expected: 0 warnings elevated to errors

**Step 3: 运行 fmt 检查**

```bash
cd clawBro && cargo fmt --all -- --check 2>&1
```
Expected: no output (clean)

**Step 4: 如有 fmt 问题，先 fix 再 check**

```bash
cd clawBro && cargo fmt --all && cargo fmt --all -- --check
```

**Step 5: 完成 Commit**

```bash
cd clawBro && git add -A
git commit -m "chore: fmt + clippy cleanup for social tools"
```

---

## 架构总结

```
Agent LLM
  └─ calls list_agents / send_message tool
       └─ TeamToolClient.invoke(TeamToolCall::ListAgents | SendMessage{..})
            └─ POST /runtime/team-tools?token=...
                 └─ invoke_team_http_request
                      ├─ [SHORT-CIRCUIT] ListAgents → handle_list_agents(state)
                      │     └─ reads state.registry.roster.all_agents()
                      │          └─ returns JSON array of agent names/mentions/backend_id
                      │
                      ├─ [SHORT-CIRCUIT] SendMessage → handle_send_message(state, ...)
                      │     ├─ target == "user" → spawn_im_turn(ws, scope, TeamNotify)
                      │     └─ target is agent name → roster.find_by_name() → spawn_im_turn(target_agent=name, TeamNotify)
                      │
                      └─ [EXISTING PATH] all other tools → registry.invoke_team_tool()
```

**关键点：**
- Solo Agent 现在会通过 `tool_surface.team_tools` 标志决定是否注入工具（Solo 默认 `team_tools: false`，如需开启需要 config 层支持，见下方注意事项）
- `MsgSource::TeamNotify` 防止被发送的消息触发 Lead 的自动响应流程（现有递归防护）
- `send_message` 向 target agent 发消息时，target agent 的回复会走正常 IM 路由（发回 session scope 对应的 channel），不会回环到发送者

## Solo Tool Surface 说明（已在 Task 6 Step 1 解决）

Solo Agent 的 `tool_surface.team_tools` 默认为 `false`（`bridge.rs:466` 确认）。这一问题已在 Task 6 Step 1 通过在 `augment()` 中增加 Solo 特判路径解决：只要 `CLAWBRO_TEAM_TOOL_URL` 配置了端点，Solo Agent 就会获得 `list_agents` + `send_message` 两个工具，无需修改配置。

---

## 验证方式

1. 启动 clawbro-server（带 `CLAWBRO_TEAM_TOOL_URL` 指向本地 /runtime/team-tools）
2. 向 Solo Agent 发消息 "list all agents"
3. 观察 Agent 调用 `list_agents` 并返回 roster 列表
4. 向 Solo Agent 发消息 "send 'hello' to @coder"
5. 观察 `coder` agent session 收到新的 turn

