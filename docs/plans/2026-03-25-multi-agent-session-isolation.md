# Multi-Agent Session Isolation + @all Broadcast Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 在同一个群里配置多个 Solo Agent 时，每个 Agent 拥有独立 Session（独立历史、独立 semaphore），并支持 `@all` / 无 mention 时自动向所有 Roster Agent 广播消息。

**Architecture:** 在 `im_sink.rs::spawn_im_turn()` 入口增加 `expand_for_multi_agent()` 展开逻辑——当 Roster 有多个 Agent 且不在 Team 模式时，将单条 InboundMsg 展开为每个 Agent 各一条，scope 格式由 `"group:abc"` 变为 `"group:abc:claude"` / `"group:abc:codex"`。所有调用方不变（公开 API 签名不变），Team 模式和单 Agent 配置完全不受影响。

**Tech Stack:** Rust · `im_sink.rs` · `agent_core/roster.rs` · `agent_core/registry.rs` · `channels_internal/mention_parsing.rs`

---

## 文件索引

| 文件 | 操作 |
|------|------|
| `crates/clawbro-server/src/agent_core/roster.rs` | Modify — 新增 `conversation_scope()` / `agent_scoped_scope()` |
| `crates/clawbro-server/src/agent_core/registry.rs` | Modify — 新增 `pub fn has_active_team_for_key()` |
| `crates/clawbro-server/src/im_sink.rs` | Modify — 新增 `expand_for_multi_agent()`，重构 `spawn_im_turn()` |

**不需要修改的文件（关键）：**
- `gateway_process.rs` — 调用 `spawn_im_turn()` 的签名不变
- `gateway/ws_handler.rs` — V1 限制：WS 客户端需显式 @mention（已知限制，文档记录）
- `channels_internal/mention_trigger.rs` — dispatch 后走 `spawn_im_turn`，自动受益
- `team_contract/projection/http_rpc.rs` — `send_message` dispatch 走 `spawn_im_turn`，自动受益

---

### Task 1: roster.rs — 新增 scope 工具函数

**Files:**
- Modify: `crates/clawbro-server/src/agent_core/roster.rs`

#### 背景

- `conversation_scope()`: 从已带 agent 后缀的 scope 还原基础 scope。
  - `"group:abc:claude"` → `"group:abc"`（若 "claude" 在 roster 中）
  - `"group:abc"` → `"group:abc"`（无后缀，原样返回）
- `agent_scoped_scope()`: 给 scope 追加 agent 名称后缀，确保隔离。
  - `("group:abc", "claude")` → `"group:abc:claude"`

#### Step 1: 写失败测试

在 `roster.rs` 末尾的 `#[cfg(test)] mod tests` 中添加（若无则新建）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_roster() -> AgentRoster {
        AgentRoster::new(vec![
            AgentEntry {
                name: "claude".to_string(),
                mentions: vec!["@claude".to_string()],
                backend_id: "claude".to_string(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
            AgentEntry {
                name: "codex".to_string(),
                mentions: vec!["@codex".to_string()],
                backend_id: "codex".to_string(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
        ])
    }

    #[test]
    fn agent_scoped_scope_appends_lowercase_name() {
        assert_eq!(agent_scoped_scope("group:abc", "Claude"), "group:abc:claude");
        assert_eq!(agent_scoped_scope("user:123", "codex"), "user:123:codex");
    }

    #[test]
    fn conversation_scope_strips_known_agent_suffix() {
        let roster = make_roster();
        assert_eq!(roster.conversation_scope("group:abc:claude"), "group:abc");
        assert_eq!(roster.conversation_scope("group:abc:codex"), "group:abc");
    }

    #[test]
    fn conversation_scope_passthrough_when_no_agent_suffix() {
        let roster = make_roster();
        assert_eq!(roster.conversation_scope("group:abc"), "group:abc");
        assert_eq!(roster.conversation_scope("user:123"), "user:123");
    }

    #[test]
    fn conversation_scope_does_not_strip_unknown_suffix() {
        let roster = make_roster();
        // "ghost" is not in roster — must not be stripped
        assert_eq!(roster.conversation_scope("group:abc:ghost"), "group:abc:ghost");
    }
}
```

#### Step 2: 运行确认失败

```bash
cd /Users/fishers/Desktop/repo/quickai-openclaw/clawBro
cargo test -p clawbro agent_core::roster::tests 2>&1 | tail -20
```

Expected: compile error — `agent_scoped_scope` / `conversation_scope` 未定义。

#### Step 3: 实现工具函数

在 `roster.rs` 的 `impl AgentRoster` 块外添加自由函数，在 `impl AgentRoster` 内添加方法：

```rust
/// Returns `"{scope}:{agent_name_lowercase}"` — used for per-agent session isolation
/// in multi-agent Solo deployments.
pub fn agent_scoped_scope(scope: &str, agent_name: &str) -> String {
    format!("{}:{}", scope, agent_name.to_lowercase())
}
```

在 `impl AgentRoster` 内（`default_agent()` 之后）添加：

```rust
/// Strips the per-agent suffix added by `agent_scoped_scope`, restoring the
/// original conversation scope.
///
/// `"group:abc:claude"` → `"group:abc"` (when "claude" is in roster)
/// `"group:abc"` → `"group:abc"` (no suffix, returned as-is)
pub fn conversation_scope<'a>(&self, scope: &'a str) -> &'a str {
    for agent in &self.agents {
        let suffix = format!(":{}", agent.name.to_lowercase());
        if scope.len() > suffix.len() && scope.ends_with(&suffix) {
            return &scope[..scope.len() - suffix.len()];
        }
    }
    scope
}
```

#### Step 4: 运行确认通过

```bash
cargo test -p clawbro agent_core::roster::tests 2>&1 | tail -20
```

Expected: 全部 PASS。

#### Step 5: Commit

```bash
git add crates/clawbro-server/src/agent_core/roster.rs
git commit -m "feat(roster): add agent_scoped_scope + conversation_scope utilities"
```

---

### Task 2: registry.rs — 新增 `has_active_team_for_key()`

**Files:**
- Modify: `crates/clawbro-server/src/agent_core/registry.rs`

#### 背景

`expand_for_multi_agent()` 需要判断"当前 scope 是否已有 Team Orchestrator"。Team Orchestrators 用 `team_id: String` 存储，`get_orchestrator_for_session()` 是私有方法。需要暴露一个公开的检查方法。

#### Step 1: 写失败测试

在 `registry.rs` 的 `#[cfg(test)] mod tests` 中添加：

```rust
#[test]
fn has_active_team_returns_false_when_no_orchestrator() {
    let (registry, _rx) = make_test_registry();
    let key = SessionKey::new("lark", "group:oc_test");
    assert!(!registry.has_active_team_for_key(&key));
}
```

（使用文件中已有的 `make_test_registry()` 测试辅助函数，若不存在则参考其他测试中的 registry 构造方式。）

#### Step 2: 运行确认失败

```bash
cargo test -p clawbro agent_core::registry::tests::has_active_team 2>&1 | tail -15
```

Expected: compile error — `has_active_team_for_key` 未定义。

#### Step 3: 实现

在 `registry.rs` 中 `get_team_orchestrator()` 之后添加：

```rust
/// Returns true if a TeamOrchestrator is registered for the session key's scope.
/// Used by multi-agent broadcast expansion to skip team-mode sessions.
pub fn has_active_team_for_key(&self, session_key: &SessionKey) -> bool {
    self.get_orchestrator_for_session(session_key).is_some()
}
```

#### Step 4: 运行确认通过

```bash
cargo test -p clawbro agent_core::registry::tests::has_active_team 2>&1 | tail -15
```

Expected: PASS。

#### Step 5: Commit

```bash
git add crates/clawbro-server/src/agent_core/registry.rs
git commit -m "feat(registry): expose has_active_team_for_key for multi-agent expansion"
```

---

### Task 3: im_sink.rs — 实现展开逻辑并重构 `spawn_im_turn`

**Files:**
- Modify: `crates/clawbro-server/src/im_sink.rs`

#### 背景

核心逻辑全在这里：

1. `expand_for_multi_agent(inbound, registry)` → `Vec<InboundMsg>`
   - Roster ≤ 1 个 Agent → 原样返回（不影响单 Agent 配置）
   - 有 Team Orchestrator → 原样返回（不影响 Team 模式）
   - Source 不是 Human（BotMention / Heartbeat / TeamNotify）：
     - 有特定 target → 仅修正 scope 后缀
     - 无 target 或 @all → 不展开（非 Human 来源不广播）
   - Human source + 无 target 或 `@all` → 展开为每个 Agent 各一条
   - Human source + 特定 `@mention` → 修正 scope 后缀（确保隔离）

2. `spawn_im_turn()` 公开签名不变，内部调用 `expand_for_multi_agent()` 后为每条展开消息 spawn 一个 turn。

#### Step 1: 写失败测试

在 `im_sink.rs` 末尾的 `#[cfg(test)]` 模块中添加（若无则新建）：

```rust
#[cfg(test)]
mod multi_agent_expand_tests {
    use super::*;
    use crate::agent_core::roster::{AgentEntry, AgentRoster};
    use crate::agent_core::SessionRegistry;
    use crate::config::GatewayConfig;
    use crate::protocol::{InboundMsg, MsgContent, MsgSource, SessionKey};
    use crate::session::{SessionManager, SessionStorage};
    use crate::skills_internal::SkillLoader;
    use std::sync::Arc;

    fn make_two_agent_registry() -> Arc<SessionRegistry> {
        let cfg = GatewayConfig::default();
        let storage = SessionStorage::new(cfg.session.dir.clone());
        let session_manager = Arc::new(SessionManager::new(storage));
        let skill_loader = SkillLoader::new(vec![cfg.skills.dir.clone()]);
        let skills = skill_loader.load_all();
        let system_injection = skill_loader.build_system_injection(&skills);
        let skill_dirs = skill_loader.search_dirs().to_vec();
        let roster = AgentRoster::new(vec![
            AgentEntry {
                name: "claude".to_string(),
                mentions: vec!["@claude".to_string()],
                backend_id: "claude".to_string(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
            AgentEntry {
                name: "codex".to_string(),
                mentions: vec!["@codex".to_string()],
                backend_id: "codex".to_string(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
        ]);
        let (registry, _rx) = SessionRegistry::new(
            None,
            session_manager,
            system_injection,
            Some(roster),
            None,
            None,
            None,
            skill_dirs,
        );
        registry
    }

    fn make_inbound(scope: &str, target: Option<&str>, source: MsgSource) -> InboundMsg {
        InboundMsg {
            id: "msg-001".to_string(),
            session_key: SessionKey::new("lark", scope),
            content: MsgContent::text("hello"),
            sender: "user-001".to_string(),
            channel: "lark".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: target.map(str::to_string),
            source,
        }
    }

    #[test]
    fn no_target_human_expands_to_all_agents() {
        let registry = make_two_agent_registry();
        let inbound = make_inbound("group:abc", None, MsgSource::Human);
        let msgs = expand_for_multi_agent(&inbound, &registry);
        assert_eq!(msgs.len(), 2);
        let scopes: Vec<&str> = msgs.iter().map(|m| m.session_key.scope.as_str()).collect();
        assert!(scopes.contains(&"group:abc:claude"));
        assert!(scopes.contains(&"group:abc:codex"));
    }

    #[test]
    fn at_all_human_expands_to_all_agents() {
        let registry = make_two_agent_registry();
        let inbound = make_inbound("group:abc", Some("@all"), MsgSource::Human);
        let msgs = expand_for_multi_agent(&inbound, &registry);
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn specific_mention_human_scopes_single_agent() {
        let registry = make_two_agent_registry();
        let inbound = make_inbound("group:abc", Some("@claude"), MsgSource::Human);
        let msgs = expand_for_multi_agent(&inbound, &registry);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].session_key.scope, "group:abc:claude");
        assert_eq!(msgs[0].target_agent.as_deref(), Some("@claude"));
    }

    #[test]
    fn bot_mention_source_scopes_but_does_not_broadcast() {
        let registry = make_two_agent_registry();
        // BotMention with no target — must NOT broadcast (anti-recursion)
        let inbound = make_inbound("group:abc", Some("@codex"), MsgSource::BotMention);
        let msgs = expand_for_multi_agent(&inbound, &registry);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].session_key.scope, "group:abc:codex");
    }

    #[test]
    fn single_agent_roster_passes_through_unchanged() {
        let cfg = GatewayConfig::default();
        let storage = SessionStorage::new(cfg.session.dir.clone());
        let session_manager = Arc::new(SessionManager::new(storage));
        let skill_loader = SkillLoader::new(vec![cfg.skills.dir.clone()]);
        let skills = skill_loader.load_all();
        let system_injection = skill_loader.build_system_injection(&skills);
        let skill_dirs = skill_loader.search_dirs().to_vec();
        let roster = AgentRoster::new(vec![AgentEntry {
            name: "claude".to_string(),
            mentions: vec!["@claude".to_string()],
            backend_id: "claude".to_string(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        }]);
        let (registry, _rx) = SessionRegistry::new(
            None, session_manager, system_injection,
            Some(roster), None, None, None, skill_dirs,
        );
        let inbound = make_inbound("group:abc", None, MsgSource::Human);
        let msgs = expand_for_multi_agent(&inbound, &registry);
        // Single agent: no expansion, scope unchanged
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].session_key.scope, "group:abc");
    }

    #[test]
    fn already_agent_scoped_specific_mention_strips_and_rescopes() {
        // Caller's scope is "group:abc:claude" (already suffixed).
        // MentionTrigger dispatches "@codex" — scope must become "group:abc:codex".
        let registry = make_two_agent_registry();
        let inbound = make_inbound("group:abc:claude", Some("@codex"), MsgSource::BotMention);
        let msgs = expand_for_multi_agent(&inbound, &registry);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].session_key.scope, "group:abc:codex");
    }

    #[test]
    fn fanout_messages_have_unique_ids() {
        let registry = make_two_agent_registry();
        let inbound = make_inbound("group:abc", None, MsgSource::Human);
        let msgs = expand_for_multi_agent(&inbound, &registry);
        assert_eq!(msgs.len(), 2);
        assert_ne!(msgs[0].id, msgs[1].id);
    }
}
```

#### Step 2: 运行确认失败

```bash
cargo test -p clawbro im_sink::multi_agent_expand_tests 2>&1 | tail -20
```

Expected: compile error — `expand_for_multi_agent` 未定义。

#### Step 3: 实现 `expand_for_multi_agent`

在 `im_sink.rs` 顶部 use 列表添加（已有的略过）：

```rust
use crate::agent_core::roster::agent_scoped_scope;
use crate::channels_internal::mention_parsing::derive_fanout_message_id;
```

在 `spawn_im_turn` 函数**之前**添加：

```rust
/// Expands a single InboundMsg into one message per roster agent for multi-agent
/// Solo deployments. Returns the original message unchanged when:
/// - Roster has ≤ 1 agent (single-agent config)
/// - A TeamOrchestrator is active for this scope (team mode takes over)
///
/// Expansion conditions (must ALL be true):
/// - Roster has ≥ 2 agents
/// - No active TeamOrchestrator
/// - Human source AND (no target_agent OR target is "@all")
///
/// For Human messages with a specific @mention, the scope is suffixed with the
/// agent name but no additional messages are created.
///
/// For non-Human sources (BotMention, Heartbeat, TeamNotify) with a specific
/// @mention, only the scope suffix is applied (prevents broadcast loops).
pub(crate) fn expand_for_multi_agent(
    inbound: &InboundMsg,
    registry: &crate::agent_core::SessionRegistry,
) -> Vec<InboundMsg> {
    use crate::protocol::MsgSource;

    let roster = match registry.roster.as_ref() {
        Some(r) if r.all_agents().len() > 1 => r,
        _ => return vec![inbound.clone()], // Single-agent or no roster: pass through
    };

    // Strip any existing agent suffix to get the conversation-level scope.
    let base_scope = roster.conversation_scope(&inbound.session_key.scope);

    // Build a normalised key at the base scope to check for a TeamOrchestrator.
    let base_key = crate::protocol::SessionKey {
        channel: inbound.session_key.channel.clone(),
        scope: base_scope.to_string(),
        channel_instance: inbound.session_key.channel_instance.clone(),
    };
    if registry.has_active_team_for_key(&base_key) {
        return vec![inbound.clone()]; // Team mode: orchestrator handles dispatch
    }

    let target = inbound.target_agent.as_deref();
    let is_broadcast_target = target.is_none()
        || target.map(|t| t.eq_ignore_ascii_case("@all")).unwrap_or(false);

    // Broadcast: Human sends with no mention or @all → expand to every roster agent.
    if is_broadcast_target && matches!(inbound.source, MsgSource::Human) {
        return roster
            .all_agents()
            .iter()
            .map(|agent| {
                let scoped_scope = agent_scoped_scope(base_scope, &agent.name);
                let mention = agent
                    .mentions
                    .first()
                    .cloned()
                    .unwrap_or_else(|| format!("@{}", agent.name));
                let mut msg = inbound.clone();
                msg.session_key.scope = scoped_scope;
                msg.target_agent = Some(mention.clone());
                msg.id = derive_fanout_message_id(&inbound.id, Some(&mention));
                msg
            })
            .collect();
    }

    // Specific mention (any source): suffix scope for session isolation.
    if let Some(mention) = target {
        if let Some(agent) = roster.find_by_mention(mention) {
            let mut msg = inbound.clone();
            msg.session_key.scope = agent_scoped_scope(base_scope, &agent.name);
            return vec![msg];
        }
    }

    // Fallback: unknown mention or non-Human broadcast — pass through unchanged.
    vec![inbound.clone()]
}
```

#### Step 4: 重构 `spawn_im_turn`

将现有 `spawn_im_turn` 函数签名保持不变，在函数体**最开始**插入展开逻辑，并将原有 spawn 逻辑提取为内部调用：

找到 `pub fn spawn_im_turn(` 函数定义，在其函数体**第一行**（`tokio::spawn(` 之前）插入：

```rust
    // Multi-agent expansion: may produce >1 message for broadcast scenarios.
    let expanded = expand_for_multi_agent(&inbound, &registry);
    if expanded.len() > 1 {
        for msg in expanded {
            spawn_im_turn(
                Arc::clone(&registry),
                Arc::clone(&channel),
                Arc::clone(&channel_registry),
                Arc::clone(&cfg),
                msg,
                presentation,
            );
        }
        return;
    }
    // Single message (expanded.len() == 1): use the (possibly scope-updated) message.
    let inbound = expanded.into_iter().next().unwrap_or(inbound);
```

#### Step 5: 运行测试

```bash
cargo test -p clawbro im_sink::multi_agent_expand_tests 2>&1 | tail -30
```

Expected: 7 tests PASS。

#### Step 6: 全量测试

```bash
cargo test -p clawbro 2>&1 | tail -20
```

Expected: 全部 PASS（允许已知的 `cli::skills` / `runtime::openclaw::adapter` 失败）。

#### Step 7: Commit

```bash
git add crates/clawbro-server/src/im_sink.rs
git commit -m "feat(im-sink): multi-agent session isolation + @all broadcast expansion

- expand_for_multi_agent(): scope group:abc → group:abc:agent_name
- Human @all or no-mention → fan-out to every roster agent (independent sessions)
- Human specific @mention → scope-suffix only (no extra messages)
- BotMention/Heartbeat/TeamNotify + specific target → scope-suffix only (anti-recursion)
- Single-agent roster or active TeamOrchestrator → pass through unchanged
- spawn_im_turn() public API unchanged; all call sites unaffected"
```

---

### Task 4: 验证集成行为

**Step 1: 运行全量测试确认无回归**

```bash
cd /Users/fishers/Desktop/repo/quickai-openclaw/clawBro
cargo test -p clawbro 2>&1 | grep -E "^(test result|FAILED|error)" | head -20
```

Expected: `test result: ok` 行，无新 FAILED。

**Step 2: Clippy 检查**

```bash
cargo clippy -p clawbro -- -D warnings 2>&1 | grep "^error" | head -20
```

Expected: 无新 error。

**Step 3: 验证关键场景（代码推演）**

| 场景 | Roster | target_agent | source | 展开结果 |
|------|--------|-------------|--------|---------|
| 用户在群里发消息，不 @任何人 | claude + codex | None | Human | 2 条：`group:abc:claude` + `group:abc:codex` |
| 用户 `@all 帮我分析` | claude + codex | `@all` | Human | 2 条：同上 |
| 用户 `@claude 帮我写代码` | claude + codex | `@claude` | Human | 1 条：`group:abc:claude` |
| claude 的 MentionTrigger 发给 codex | claude + codex | `@codex` | BotMention | 1 条：`group:abc:codex` |
| send_message target=codex（从 claude session） | claude + codex | `@codex` | BotMention | 1 条：`group:abc:codex` |
| Team 模式群消息 | 任意 | 任意 | Human | 原样（orchestrator 存在） |
| 单 Agent 配置 | claude only | None | Human | 1 条：`group:abc`（无变化） |

---

## 已知 V1 限制（文档记录，不阻塞实现）

1. **WS 客户端不支持广播**：`ws_handler.rs` 直接调用 `registry.handle_with_context()`，绕过 `spawn_im_turn`。WS 前端（AionUi）需显式 @mention 特定 Agent，不支持 @all 广播。
2. **Auto-promote + 多 Agent**：若 `auto_promote=true` 在多 Agent 场景下触发，Team Orchestrator 会用 agent-scoped scope（如 `group:abc:claude`）作为 team_id，与直接配置 Team 模式的 team_id 不同。建议：多 Agent 场景不与 auto_promote 同时使用。
3. **DM scope 不受影响**：`user:123` scope 只有一个 Agent（用户直接与该 Agent 对话），无需 broadcast，pass-through 逻辑正确。
4. **Cron 任务不走 expand_for_multi_agent**：`scheduler_runtime.rs` 直接调用 `registry.handle_with_context()`，绕过 `spawn_im_turn`。Cron 任务在多 Agent 场景下若未配置 `target_agent`，会使用未带 agent 后缀的原始 scope（如 `"group:abc"`），该 session 与任何 agent-scoped session 相互独立，历史不共享。建议：在多 Agent 配置下，Cron 任务应显式指定 `agent` 字段。
