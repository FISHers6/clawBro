# Agent Team Completion Routing Architecture

**Date**: 2026-03-13
**Status**: Implemented (completion_routing.rs + orchestrator.rs + team_runtime.rs)
**Reference**: OpenClaw subagent-announce.ts, subagent-announce-dispatch.ts

---

## 1. 背景与问题

### 1.1 原始问题

早期 ClawBro team runtime 的完成通知有三个根本缺陷：

1. `lead_session_key` 在启动时静态注入，所有 completion 都发往同一固定目标，无法追踪"是谁发起了这次 dispatch"
2. 所有 milestone 事件通过 `render_for_im()` 直接发到 IM group，没有"内部通知父 session"和"外发给用户"的区分
3. 没有 pending completion 持久化，gateway 重启时进行中的 team 任务通知会丢失

### 1.2 OpenClaw 参考实现

OpenClaw 在 `subagent-announce.ts` + `subagent-announce-dispatch.ts` 实现了完整的父子回流模型：

- 子任务完成后产出结构化 `task_completion` 内部事件（不是裸文本）
- 回流目标是 `requester_session_key`，不是 channel，不是 bot
- 按父 session 活跃状态选择 steered / queued / direct / fallback 四种分发路径
- `replyInstruction` 控制父 agent 收到后是静默处理还是转成用户可见回复
- 无 bot 的子 agent 仍可回流父 session（`deliver=false` 路径）

---

## 2. 五层概念模型

这是整个设计的基础，必须严格分层，不可混淆。

```
┌─────────────────────────────────────────────────────┐
│  Agent                                              │
│  • runtime (native / acp_claude / acp_codex)        │
│  • tools + persona + memory                         │
│  • session ownership                                │
│  • 可有 0 个或多个 Bot 绑定                          │
└──────────────────┬──────────────────────────────────┘
                   │ optional
┌──────────────────▼──────────────────────────────────┐
│  Bot / Channel Account                              │
│  • 外部收发入口（Lark bot token / DingTalk appKey） │
│  • 负责 ingress/egress 身份                          │
│  • 不等于 agent                                      │
└──────────────────┬──────────────────────────────────┘
                   │
┌──────────────────▼──────────────────────────────────┐
│  Binding                                            │
│  • scope → agent 路由                               │
│  • channel + account_id + peer 匹配规则              │
└─────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────┐
│  Session                                            │
│  • 一条对话上下文（顶层用户会话 or 内部专家会话）    │
│  • 有 session_key = (channel, scope)                 │
│  • 与 Bot 无关：specialist session 没有 bot          │
└─────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────┐
│  Run                                                │
│  • 一次具体执行                                      │
│  • run_id / parent_run_id                           │
│  • requester_session_key（谁发起了这次 dispatch）    │
│  • child_session_key（specialist 的 session）        │
└─────────────────────────────────────────────────────┘
```

**关键推论**：
- specialist agent 不需要 bot
- 子任务回流父 session 是内部路由，不需要外部 delivery target
- 只有"最终直接通知用户"才需要可投递的 channel target
- bot/account 是 delivery layer，不污染 agent model

---

## 3. 统一 Completion Contract

无论 child runtime 是 native、ACP Claude、ACP Codex，完成后统一产出 `TeamRoutingEnvelope`：

```rust
pub struct TeamRoutingEnvelope {
    pub run_id: String,
    pub parent_run_id: Option<String>,
    pub team_id: String,
    pub requester_session_key: Option<SessionKey>,   // 谁发起了这次 dispatch
    pub fallback_session_keys: Vec<SessionKey>,       // 次级目标链
    pub delivery_status: RoutingDeliveryStatus,
    pub event: TeamRoutingEvent,
}

pub struct TeamRoutingEvent {
    pub task_id: String,
    pub kind: TeamRoutingEventKind,      // TaskCompleted / TaskBlocked / ...
    pub agent: String,
    pub detail: Option<String>,
    pub reply_policy: CompletionReplyPolicy,
}

pub struct CompletionReplyPolicy {
    pub audience: CompletionAudience,    // ParentOnly / UserVisible / ParentThenUser
    pub silence_ok: bool,
    pub dedupe_key: Option<String>,
}
```

对应 OpenClaw 的 `task_completion` 内部事件 + `replyInstruction` 字段。

**关键设计**：
- `result` 是数据，不是直接给用户的话术
- `CompletionAudience` 决定父 agent 收到后是静默处理还是对用户可见
- 运行时状态（token、cost）放结构化字段，不混进自然语言

---

## 4. 分发状态机

### 4.1 RoutingDeliveryStatus

```rust
pub enum RoutingDeliveryStatus {
    NotRouted,           // 初始状态
    DirectDelivered,     // 直接发到 requester session（session 空闲）
    QueuedDelivered,     // 发到 requester session（session 繁忙，标记排队）
    FallbackRedirected,  // 用了 fallback_session_keys 中的次级目标
    PersistedPending,    // tx 满或 session 不存在，已持久化等待重发
    FailedTerminal,      // 永久失败
}
```

### 4.2 分发优先级算法

```
dispatch_team_routing_event(envelope):
  1. 持 pending_store_lock（整个序列原子化）
  2. flush_pending_routing_events_locked()  // 先重放积压事件，保序
  3. 按 routing_attempt_targets() 顺序尝试：
     - 主目标：requester_session_key
     - 次级目标：fallback_session_keys[0], [1], ...
  4. 对每个目标：
     - is_session_busy() → 标记 QueuedDelivered / DirectDelivered
     - tx.try_send() → 成功则记录 delivery_status，返回
     - 失败（channel 满）→ append_pending_completion()，返回
  5. 所有目标都失败 → PersistedPending 持久化
```

对比 OpenClaw 的 steered/queued/direct/fallback：

| OpenClaw 模式 | ClawBro 对应 | 说明 |
|--------------|-------------|------|
| Steered（注入当前轮次）| 暂未实现 | 需要 embedded message queue |
| Queued（当前轮结束后处理）| QueuedDelivered（隐式）| 靠 session semaphore 串行保证 |
| Direct（直接触发父 session）| DirectDelivered | 主路径 |
| Fallback（向上回退 requester chain）| FallbackRedirected | fallback_session_keys |
| Pending 持久化 | PersistedPending | JSONL 文件 |

### 4.3 Pending 持久化与恢复

```
pending-completions.jsonl  ← append-only，每行一个 TeamRoutingEnvelope
routing-events.jsonl       ← 审计日志，记录所有已路由事件
```

恢复触发点：
- `set_team_notify_tx()` 时自动 flush（gateway 重启后第一次可用时）
- 每次 `dispatch_team_routing_event()` 前先 flush 积压

`pending_store_lock` (Mutex) 覆盖所有 load/replace/append 操作，防止并发写坏文件。

---

## 5. CompletionAudience 语义

| 值 | IM 发送 | 适用场景 |
|----|---------|---------|
| `ParentOnly` | ❌ 不发 | 内部调度事件（TaskDispatched、Checkpoint）|
| `UserVisible` | ✅ 发 | 用户直接关心的通知 |
| `ParentThenUser` | ✅ 发 | 先通知父 agent，同时也推送给用户（TaskDone、AllTasksDone）|

对应 OpenClaw 的 `replyInstruction` 三种 mode：
- `ParentOnly` ↔ `requesterIsSubagent=true` → "内部编排更新，重复则 SILENT"
- `UserVisible` ↔ `expectsCompletionMessage=true` → "立即转成用户语言发出"
- `ParentThenUser` ↔ 默认 → "发给用户，如果已发过则 NO_REPLY"

---

## 6. ACP Agent 的通用接口要求

ACP 只影响 child run 怎么执行，不影响 completion contract。

ACP child 完成后，宿主统一：
1. 从 `TurnComplete` 事件或 transcript 捕获 `result`
2. 用 `build_routing_envelope(task_id, agent, event)` 组装 `TeamRoutingEnvelope`
3. 交给 `dispatch_team_routing_event()` 路由

```
ACP Runtime（claude-agent-acp / codex-acp）
    ↓ 执行完成
capture_result(child_session_key) → String
    ↓
build_routing_envelope(task_id, agent, TeamRoutingEvent::completed(...))
    ↓
dispatch_team_routing_event(envelope)
    ↓
团队通知路由系统（与 runtime 无关）
```

**原则**：
- ACP child 不需要自己懂 channel
- ACP child 不需要自己有 bot
- completion 回流由宿主 control-plane 完成，不绑在 claude-agent-acp 私有行为上

---

## 7. 配置分层建议

```toml
# agent 层：只描述执行单元
[[agent]]
id = "claude"
runtime = "acp_claude"

[[agent]]
id = "codex"
runtime = "acp_codex"

[[agent]]
id = "native"
runtime = "clawbro_native"

# channel 层：只描述收发入口
[channels.lark.account.main]
# bot token / app credentials

# binding 层：路由规则
[[binding]]
channel = "lark"
scope = "group:lark:xxx"
agent = "claude"

# group 层：团队模式配置
[[group]]
channel = "lark"
scope = "group:lark:xxx"

[group.interaction]
mode = "team"
front_bot = "claude"          # 只负责顶层 ingress/egress

[group.team]
roster = ["codex", "native"]  # specialist 不要求有单独 channel account
```

---

## 8. 当前实现状态（2026-03-13）

### 已实现 ✅

| 能力 | 文件 | 说明 |
|------|------|------|
| `requester_session_key` per dispatch | `orchestrator.rs:274-293` | DispatchContextRecord per (task_id, agent) |
| `parent_run_id` 追踪 | `orchestrator.rs:166-169` | 完整父子审计链 |
| `deliver=false` 内部路径 | `completion_routing.rs:6-11` | ParentOnly 不发 IM |
| Pending 持久化 + 恢复 | `session.rs:244-282` | JSONL + atomic rename |
| 统一 CompletionRoutingEvent | `completion_routing.rs:48-67` | 单一 enum-based 设计 |
| `FallbackRedirected` 路径 | `team_runtime.rs:351-359` | fallback_session_keys 生效 |
| 并发安全 flush | `orchestrator.rs:1159-1240` | pending_store_lock 覆盖完整序列 |
| ParentThenUser 正确发 IM | `team_runtime.rs:375-382` | `!matches!(ParentOnly)` |
| 90 team tests, 0 failures | workspace | 回归覆盖 |

### 暂未实现（低优先级）

| 能力 | OpenClaw 对应 | 原因 |
|------|--------------|------|
| Steered 模式（注入当前轮次）| `queueEmbeddedPiMessage()` | 需要 embedded message queue，当前场景够用 |
| 自动 requester-chain 向上溯源 | `subagent-announce.ts:1374` | fallback 目前需手动配置 |
| `/reset` 清除 team 任务状态 | — | `/reset` 只清消息，需配合 `/team stop` |
| SILENT/NO_REPLY 去重 token | `SILENT_REPLY_TOKEN` | Lead 可用 silence_ok flag 替代 |

---

## 9. 与 OpenClaw 对比总结

| 维度 | OpenClaw | ClawBro Gateway |
|------|---------|-----------------|
| 完成通知载体 | 结构化 `task_completion` 内部事件 | 结构化 `TeamRoutingEnvelope` |
| 状态机持久化 | JS 内存（进程级） | SQLite + 乐观锁（崩溃安全）|
| 分发路径 | Steered > Queued > Direct > Fallback | Queued(隐式) > Direct > FallbackRedirected > Pending |
| Pending 持久化 | 无 | JSONL + 启动时恢复 |
| replyInstruction | 自然语言指令 | `CompletionAudience` 枚举（更类型安全）|
| requester 追踪 | per-dispatch session key | per-(task_id, agent) HashMap |
| SILENT 去重 | `NO_REPLY` token | `silence_ok` flag + `dedupe_key` |
| Steered 模式 | ✅ 实现 | ❌ 未实现 |
| 父子链向上溯源 | 自动沿 requester chain | 手动配置 fallback_session_keys |

**核心优势**：SQLite 状态机 + 持久化 pending completions 使 ClawBro 在崩溃恢复方面比 OpenClaw 更健壮。
**核心差距**：Steered 模式（mid-turn injection）尚未实现；自动 requester chain 溯源需要后续补充。
