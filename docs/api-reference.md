# clawBro Gateway API 接入文档

> **版本:** Phase 3 · 2026-03-22
> **适用场景:** Dashboard 前端、外部集成
> **Base URL:** `http://<host>:<port>`（默认 `http://localhost:7770`）
> **WebSocket:** `ws://<host>:<port>/ws`

---

## 目录

- [通用约定](#通用约定)
- [WebSocket 协议](#websocket-协议)
  - [连接与认证](#连接与认证)
  - [客户端 → 服务端消息](#客户端--服务端消息)
  - [服务端 → 客户端事件（AgentEvent）](#服务端--客户端事件agentevent)
  - [Dashboard 主题事件](#dashboard-主题事件)
- [聊天](#聊天)
  - [POST /api/chat](#post-apichat)
- [Agent 管理](#agent-管理)
  - [GET /api/agents](#get-apiagents)
  - [GET /api/agents/{name}](#get-apiagentsname)
  - [POST /api/agents](#post-apiagents)
  - [PATCH /api/agents/{name}](#patch-apiagentsname)
  - [DELETE /api/agents/{name}](#delete-apiagentsname)
  - [GET /api/agents/{name}/skills](#get-apiagentsnamesklills)
- [配置管理](#配置管理)
  - [GET /api/config/effective](#get-apiconfigeffective)
  - [GET /api/config/spec](#get-apiconfigspec)
  - [GET /api/config/raw](#get-apiconfigraw)
  - [PUT /api/config/raw](#put-apiconfigraw)
  - [POST /api/config/validate](#post-apiconfigvalidate)
- [Session 管理](#session-管理)
  - [GET /api/sessions](#get-apisessions)
  - [GET /api/sessions/detail](#get-apisessionsdetail)
  - [GET /api/sessions/messages](#get-apisessionsmessages)
  - [GET /api/sessions/events](#get-apisessionsevents)
  - [DELETE /api/sessions](#delete-apisessions)
- [Backend 管理](#backend-管理)
  - [GET /api/backends](#get-apibackends)
  - [GET /api/backends/{backend_id}](#get-apibackendsbackend_id)
- [Channel 管理](#channel-管理)
  - [GET /api/channels](#get-apichannels)
  - [GET /api/channels/{channel_id}](#get-apichannelschannel_id)
- [Skills](#skills)
  - [GET /api/skills](#get-apiskills)
- [Approvals（工具调用审批）](#approvals工具调用审批)
  - [GET /api/approvals](#get-apiapprovals)
  - [GET /api/approvals/{approval_id}](#get-apiapprovalsapproval_id)
  - [POST /api/approvals/{approval_id}/approve](#post-apiapprovalsapproval_idapprove)
  - [POST /api/approvals/{approval_id}/deny](#post-apiapprovalsapproval_iddeny)
- [Scheduler（定时任务）](#scheduler定时任务)
  - [GET /api/scheduler/jobs](#get-apischedulerjobs)
  - [GET /api/scheduler/jobs/{job_id}](#get-apischedulerjobsjob_id)
  - [GET /api/scheduler/jobs/{job_id}/runs](#get-apischedulerjobsjob_idruns)
  - [POST /api/scheduler/jobs/{job_id}/run-now](#post-apischedulerjobsjob_idrun-now)
- [Teams（多 Agent 协作）](#teams多-agent-协作)
  - [GET /api/teams](#get-apiteams)
  - [GET /api/teams/{team_id}](#get-apiteamsteam_id)
  - [GET /api/teams/{team_id}/artifacts](#get-apiteamsteam_idartifacts)
  - [GET /api/teams/{team_id}/tasks/{task_id}](#get-apiteamsteam_idtaskstask_id)
- [Tasks（任务）](#tasks任务)
  - [GET /api/tasks](#get-apitasks)
  - [GET /api/tasks/{task_id}](#get-apitaskstask_id)
- [系统端点](#系统端点)
  - [GET /health](#get-health)
  - [GET /status](#get-status)
  - [GET /doctor](#get-doctor)

---

## 通用约定

### 错误格式

所有错误响应使用统一格式：

```json
{
  "error": "具体错误描述"
}
```

| HTTP 状态码 | 含义 |
|-------------|------|
| `200` | 成功 |
| `400` | 请求参数错误（缺失字段、TOML 解析失败、校验失败等）|
| `404` | 资源不存在 |
| `409` | 冲突（如创建已存在的 agent）|
| `500` | 服务端内部错误 |

### 列表响应格式

所有列表接口统一包裹在 `items` 字段中：

```json
{
  "items": [...]
}
```

### SessionKey 结构

多处接口使用 `SessionKey` 标识一个会话：

```json
{
  "channel": "ws",
  "scope": "main",
  "channel_instance": null
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `channel` | `string` | 频道类型：`"ws"` / `"lark"` / `"dingtalk"` 等 |
| `scope` | `string` | 会话范围：`"main"` / `"user:<id>"` / `"group:<id>"` |
| `channel_instance` | `string?` | 多实例频道的实例 ID（如 Lark 多应用场景），可省略 |

### 重启信号

Config / Agent 写操作均返回 `restart_required: true`，表示当前运行时不支持热重载，需要重启 gateway 后配置才生效。前端应展示提示引导用户重启。

---

## WebSocket 协议

### 连接与认证

```
ws://<host>:<port>/ws
```

若 config.toml 中配置了 `auth.ws_token`，需在连接时携带 Bearer Token：

```
Authorization: Bearer <token>
```

未配置 `ws_token` 时无需认证，所有连接直接通过。

---

### 客户端 → 服务端消息

所有客户端消息为 JSON 文本帧，通过 `type` 字段区分类型（聊天消息除外，见下文）。

#### 1. Subscribe — 订阅 Session 事件

订阅后，该 session 的所有 `AgentEvent` 都会推送到当前 WS 连接。

```json
{
  "type": "Subscribe",
  "session_key": {
    "channel": "ws",
    "scope": "main"
  }
}
```

#### 2. Unsubscribe — 取消订阅

```json
{
  "type": "Unsubscribe",
  "session_key": {
    "channel": "ws",
    "scope": "main"
  }
}
```

#### 3. SubscribeTopic — 订阅 Dashboard 主题事件

用于监听全局状态变化（backend 上线/下线、approval 请求等），详见 [Dashboard 主题事件](#dashboard-主题事件)。

```json
{
  "type": "SubscribeTopic",
  "topic": { "kind": "approvals" }
}
```

#### 4. UnsubscribeTopic — 取消 Dashboard 主题订阅

```json
{
  "type": "UnsubscribeTopic",
  "topic": { "kind": "approvals" }
}
```

#### 5. ResolveApproval — 解决工具调用审批

```json
{
  "type": "ResolveApproval",
  "approval_id": "approval-uuid",
  "decision": "approve"
}
```

| `decision` 取值 | 含义 |
|-----------------|------|
| `"approve"` | 批准 |
| `"deny"` | 拒绝 |

#### 6. 发送聊天消息（InboundMsg，无 type 字段）

**直接发送原始 `InboundMsg` JSON，不带 `type` 字段**（`#[serde(untagged)]`）。发送后自动订阅该 session 的事件，无需提前发 `Subscribe`。

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "session_key": {
    "channel": "ws",
    "scope": "main"
  },
  "content": {
    "type": "Text",
    "text": "你好，介绍一下你自己"
  },
  "sender": "web",
  "channel": "ws",
  "timestamp": "2026-03-22T10:00:00Z",
  "source": "Human"
}
```

**InboundMsg 字段说明：**

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `id` | `string` | ✅ | 唯一标识，建议 UUID v4 |
| `session_key` | `SessionKey` | ✅ | 会话标识，web chat 用 `channel:"ws"` |
| `content` | `MsgContent` | ✅ | 消息内容，见下方 |
| `sender` | `string` | ✅ | 发送者标识，web chat 固定填 `"web"` |
| `channel` | `string` | ✅ | 同 `session_key.channel`，填 `"ws"` |
| `timestamp` | `string` | ✅ | ISO 8601 UTC 时间 |
| `thread_ts` | `string?` | ❌ | 平台线程 ID，web chat 不需要 |
| `target_agent` | `string?` | ❌ | 指定 Agent，如 `"@claude"`；不填则用默认 Agent |
| `source` | `string` | ❌ | 来源类型，默认 `"Human"` |

**MsgContent 格式：**

```json
// 文本消息
{ "type": "Text", "text": "消息内容" }

// 图片消息
{ "type": "Image", "url": "https://...", "caption": "可选说明" }

// 文件消息
{ "type": "File", "url": "https://...", "name": "filename.pdf" }
```

**WS vs REST 对比：**

| | WS 直接发消息 | REST POST /api/chat |
|---|---|---|
| 适用场景 | Dashboard（已建立 WS 连接）| HTTP-only 客户端 |
| 需提前订阅 | **不需要**（自动订阅）| 需要（先 Subscribe 再发）|
| 返回 turn_id | 无（客户端自己生成 id）| ✅ 同步返回 |
| 推荐 | **Dashboard 首选** | 外部集成 |

---

### 服务端 → 客户端事件（AgentEvent）

通过 `type` 字段区分，所有事件均包含 `session_id`（UUID 格式）用于关联请求。

#### TextDelta — 流式文字片段

```json
{
  "type": "TextDelta",
  "session_id": "uuid",
  "delta": "今天天气"
}
```

#### TurnComplete — 本轮对话完成

```json
{
  "type": "TurnComplete",
  "session_id": "uuid",
  "full_text": "今天天气不错，适合出行。",
  "sender": "claude"
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `full_text` | `string` | 本轮完整回复文本 |
| `sender` | `string?` | Agent 名称，可能为 null |

#### Thinking — Agent 思考中

```json
{
  "type": "Thinking",
  "session_id": "uuid"
}
```

#### ToolCallStart — 工具调用开始

```json
{
  "type": "ToolCallStart",
  "session_id": "uuid",
  "tool_name": "bash",
  "call_id": "call-uuid"
}
```

#### ToolCallResult — 工具调用返回结果

```json
{
  "type": "ToolCallResult",
  "session_id": "uuid",
  "call_id": "call-uuid",
  "result": "工具执行输出内容"
}
```

#### ToolCallFailed — 工具调用失败

```json
{
  "type": "ToolCallFailed",
  "session_id": "uuid",
  "tool_name": "bash",
  "call_id": "call-uuid",
  "error": "Permission denied"
}
```

#### ApprovalRequest — 需要用户审批工具调用

```json
{
  "type": "ApprovalRequest",
  "session_id": "uuid",
  "session_key": { "channel": "ws", "scope": "main" },
  "approval_id": "approval-uuid",
  "prompt": "Agent 请求执行以下命令，是否允许？",
  "command": "rm -rf /tmp/test",
  "cwd": "/home/user",
  "host": "localhost",
  "agent_id": "claude",
  "expires_at_ms": 1742640000000
}
```

收到后，前端应展示审批弹窗，用户操作后通过 `ResolveApproval` 消息或 REST `POST /api/approvals/{id}/approve` 响应。

#### Error — Agent 执行出错

```json
{
  "type": "Error",
  "session_id": "uuid",
  "message": "Backend connection timeout"
}
```

---

### Dashboard 主题事件

通过 `SubscribeTopic` 订阅后，服务端推送全局状态变化事件。使用 `kind` 字段区分 Topic，与 AgentEvent 使用同一 WS 连接。

**可用 Topic 及对应事件：**

| Topic JSON | 触发事件类型 |
|------------|-------------|
| `{"kind":"approvals"}` | `ApprovalPending`、`ApprovalResolved` |
| `{"kind":"backends"}` | `BackendUpdated`（所有 backend）|
| `{"kind":"backend","backend_id":"xxx"}` | `BackendUpdated`（指定 backend）|
| `{"kind":"channels"}` | `ChannelUpdated`（所有 channel）|
| `{"kind":"channel","channel":"lark"}` | `ChannelUpdated`（指定 channel）|
| `{"kind":"session","session_key":{...}}` | `SessionUpdated`（指定 session）|
| `{"kind":"scheduler"}` | `SchedulerJobUpdated`、`SchedulerJobDeleted`、`SchedulerRunUpdated` |
| `{"kind":"scheduler_job","job_id":"xxx"}` | 指定 job 的 Scheduler 事件 |
| `{"kind":"team","team_id":"xxx"}` | `TeamLeaderUpdate`、`TeamChannelSend`、`TeamRoutingEvent`、`TaskUpdated` |
| `{"kind":"task","team_id":"xxx","task_id":"T001"}` | 指定任务的 Team 事件 |

---

## 聊天

### POST /api/chat

启动一次 web chat 对话轮次。Agent 的回复通过 WebSocket `AgentEvent` 流式推送，调用方需提前订阅对应 session 的 WS 事件（或改用 WS 直接发消息，会自动订阅）。

**Request**

```
POST /api/chat
Content-Type: application/json
```

```json
{
  "message": "你好，介绍一下你自己",
  "scope": "main",
  "agent": "@claude"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `message` | `string` | ✅ | 用户消息内容，不能为空 |
| `scope` | `string?` | ❌ | Session scope，默认 `"main"`；空字符串也会退回 `"main"` |
| `agent` | `string?` | ❌ | 指定目标 Agent，如 `"@claude"`；不填则使用默认 Agent |

**Response 200**

```json
{
  "turn_id": "550e8400-e29b-41d4-a716-446655440000",
  "session_key": {
    "channel": "ws",
    "scope": "main"
  }
}
```

| 字段 | 说明 |
|------|------|
| `turn_id` | 本轮唯一 ID，可用于前端标记消息；WS `AgentEvent` 的 `session_id` 是内部 UUID，与 `turn_id` 不同 |
| `session_key` | 本次会话 key，用于后续 WS Subscribe 或 GET /api/sessions |

**Errors**

| 状态码 | 原因 |
|--------|------|
| `400` | `message` 为空或仅含空白字符 |

**推荐接入流程（REST 方式）：**

```
1. 先 WS Subscribe: {"type":"Subscribe","session_key":{"channel":"ws","scope":"main"}}
2. 发 POST /api/chat {"message":"...","scope":"main"}
3. 监听 WS TextDelta（流式片段）+ TurnComplete（完整回复）
```

---

## Agent 管理

### GET /api/agents

获取所有已配置 Agent 列表（来自 `agent_roster`），按名称排序。

**Response 200**

```json
{
  "items": [
    {
      "name": "claude",
      "mentions": ["@claude", "@ai"],
      "backend_id": "claude-acp",
      "role": "solo",
      "identities": [],
      "persona_dir_configured": false,
      "workspace_dir_configured": true,
      "extra_skills_dir_count": 0,
      "effective_mcp": ["filesystem", "git"]
    }
  ]
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `name` | `string` | Agent 名称（唯一标识）|
| `mentions` | `string[]` | 触发该 Agent 的 @ 关键词列表 |
| `backend_id` | `string` | 关联的 backend ID |
| `role` | `string` | `"solo"` / `"lead"` / `"specialist"` |
| `identities` | `string[]` | Agent 在 team 中的身份标签，如 `"front_bot"`、`"roster_member"` |
| `persona_dir_configured` | `bool` | 是否配置了人格目录 |
| `workspace_dir_configured` | `bool` | 是否配置了工作区目录 |
| `extra_skills_dir_count` | `number` | 额外 skill 目录数量 |
| `effective_mcp` | `string[]` | 实际生效的 MCP server 名称列表 |

---

### GET /api/agents/{name}

获取单个 Agent 详情。

**Response 200** — 与列表中单个 item 结构相同

**Errors**

| 状态码 | 原因 |
|--------|------|
| `404` | Agent 名称不存在 |

---

### POST /api/agents

向 `config.toml` 追加一条 `[[agent_roster]]` 记录。写入后需重启生效。

**Request**

```json
{
  "name": "rex",
  "backend_id": "claude-acp",
  "mentions": ["@rex", "@r"],
  "persona_dir": "/home/user/.clawbro/personas/rex",
  "workspace_dir": "/home/user/workspace"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | `string` | ✅ | Agent 唯一名称，不能为空 |
| `backend_id` | `string` | ✅ | 关联 backend ID（必须在 config 中已存在）|
| `mentions` | `string[]` | ❌ | 触发关键词，默认空数组 |
| `persona_dir` | `string?` | ❌ | 人格目录路径 |
| `workspace_dir` | `string?` | ❌ | 工作区目录路径 |

**Response 200**

```json
{
  "ok": true,
  "name": "rex",
  "restart_required": true
}
```

**Errors**

| 状态码 | 原因 |
|--------|------|
| `400` | `name` 为空，或写入后配置校验失败 |
| `409` | 同名 Agent 已存在 |
| `500` | 读写 config 文件失败 |

---

### PATCH /api/agents/{name}

修改已有 Agent 的配置字段。只更新请求中不为 `null` 的字段，其余保持不变。

**Request**

```json
{
  "backend_id": "claude-acp-v2",
  "mentions": ["@rex"],
  "persona_dir": "/new/persona",
  "workspace_dir": null
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `backend_id` | `string?` | 新的 backend ID |
| `mentions` | `string[]?` | 覆盖全部 mentions（注意是覆盖而非追加）|
| `persona_dir` | `string?` | 新的人格目录路径 |
| `workspace_dir` | `string?` | 新的工作区目录路径 |

**Response 200**

```json
{
  "ok": true,
  "name": "rex",
  "restart_required": true
}
```

**Errors**

| 状态码 | 原因 |
|--------|------|
| `404` | Agent 名称不存在 |
| `400` | 修改后配置校验失败 |
| `500` | 读写 config 文件失败 |

---

### DELETE /api/agents/{name}

从 `config.toml` 删除指定 Agent 记录。

**Response 200**

```json
{
  "ok": true,
  "name": "rex",
  "restart_required": true
}
```

**Errors**

| 状态码 | 原因 |
|--------|------|
| `404` | Agent 名称不存在 |
| `400` | 删除后配置校验失败 |
| `500` | 读写 config 文件失败 |

---

### GET /api/agents/{name}/skills

获取指定 Agent 的 Skill 视图，包含 host skills（本机已安装）和 effective skills（实际生效）。

**Response 200**

```json
{
  "agent_id": "rex",
  "role": "solo",
  "backend_id": "claude-acp",
  "supports_native_local_skills": true,
  "host_skills": [
    {
      "name": "git-helper",
      "version": "1.0.0",
      "source_label": "global",
      "path": "/home/user/.clawbro/skills/git-helper"
    }
  ],
  "effective_skills": [...],
  "roots": [
    {
      "label": "agent",
      "path": "/home/user/.clawbro/skills",
      "exists": true
    }
  ]
}
```

**Errors**

| 状态码 | 原因 |
|--------|------|
| `404` | Agent 不存在或未配置 roster |

---

## 配置管理

### GET /api/config/effective

获取当前运行时的有效配置摘要（不含敏感信息）。

**Response 200**

```json
{
  "default_backend_id": "claude-acp",
  "roster_agents": ["claude", "rex"],
  "team_scopes": [
    {
      "scope": "group:oc_abc123",
      "name": "研发团队",
      "channel": "lark",
      "front_bot": "rex",
      "roster": ["rex", "claude", "codex"]
    }
  ],
  "delivery_sender_bindings": [],
  "channels": ["lark", "dingtalk"]
}
```

---

### GET /api/config/spec

获取当前配置的完整规格视图，包含所有配置项（敏感字段以 `*_configured: bool` 替代实际值，不泄露 secret）。

**Response 200**（部分示例）

```json
{
  "gateway": {
    "host": "0.0.0.0",
    "port": 7770,
    "require_mention_in_groups": true,
    "default_workspace_configured": true
  },
  "auth": {
    "ws_token_configured": true
  },
  "channels": {
    "lark": {
      "enabled": true,
      "presentation": "final_only",
      "default_instance": "prod",
      "instances": [
        {
          "id": "prod",
          "app_id": "cli_xxxx",
          "bot_name": "ClawBro",
          "app_secret_configured": true
        }
      ]
    }
  },
  "memory": {
    "distill_every_n": 10,
    "distiller_binary": "clawbro-distiller",
    "shared_dir_configured": true,
    "shared_memory_max_words": 2000,
    "agent_memory_max_words": 1000
  },
  "scheduler": {
    "enabled": true,
    "poll_secs": 5,
    "max_concurrent": 3,
    "max_fetch_per_tick": 10,
    "default_timezone": "Asia/Shanghai",
    "db_path_configured": true,
    "lease_secs": 60
  },
  "agent_roster": [...],
  "backends": [...],
  "groups": [...],
  "team_scopes": [...],
  "bindings": [...]
}
```

---

### GET /api/config/raw

读取 `config.toml` 文件原始内容（格式保留，含注释）。若文件不存在返回空字符串。

**Response 200**

```json
{
  "content": "[gateway]\nhost = \"0.0.0.0\"\nport = 7770\n\n# ...",
  "path": "/home/user/.clawbro/config.toml"
}
```

| 字段 | 说明 |
|------|------|
| `content` | config.toml 原始 TOML 文本 |
| `path` | 配置文件在服务端的绝对路径（展示用）|

---

### PUT /api/config/raw

写入完整 config.toml 内容。先解析 + 校验，通过后写入磁盘，失败则不写。

**Request**

```json
{
  "content": "[gateway]\nhost = \"0.0.0.0\"\nport = 7770\n\n[agent]\nbackend_id = \"claude-acp\"\n"
}
```

**Response 200**

```json
{
  "ok": true,
  "path": "/home/user/.clawbro/config.toml",
  "restart_required": true
}
```

**Errors**

| 状态码 | 原因 |
|--------|------|
| `400` | TOML 解析失败（语法错误）|
| `400` | 配置校验失败（如缺少必要字段）|
| `500` | 写磁盘失败 |

---

### POST /api/config/validate

仅校验 TOML 内容，不写磁盘。可用于编辑器实时提示。

**Request**

```json
{
  "content": "[gateway]\nhost = \"0.0.0.0\"\nport = 7770\n"
}
```

**Response 200**（始终返回 200，校验结果在 body 中）

```json
// 校验通过
{
  "ok": true,
  "error": null
}

// 校验失败
{
  "ok": false,
  "error": "TOML parse error: invalid key at line 3"
}
```

---

## Session 管理

### GET /api/sessions

列出所有 Session（或按条件筛选）。

**Query 参数**

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `channel` | `string` | 条件必填 | 频道类型，如 `ws` / `lark`；有 `scope` 时必须同时提供 |
| `scope` | `string` | 条件必填 | 会话 scope；有 `channel` 时必须同时提供 |
| `channel_instance` | `string` | ❌ | 多实例频道的实例 ID |

> 不传任何参数 = 返回全部 session；只传 `channel` 或只传 `scope` = 400 错误。

**Response 200**

```json
{
  "items": [
    {
      "session_id": "a1b2c3d4",
      "channel": "ws",
      "scope": "main",
      "channel_instance": null,
      "created_at": "2026-03-22T09:00:00Z",
      "updated_at": "2026-03-22T10:30:00Z",
      "message_count": 12,
      "status": "idle",
      "backend_id": "claude-acp",
      "running_since": null
    }
  ]
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `session_id` | `string` | Session 内部 ID |
| `status` | `string` | `"idle"` / `"running"` |
| `running_since` | `datetime?` | 仅 `status=="running"` 时存在 |
| `backend_id` | `string?` | 当前绑定的 backend，可能为 null |

**Errors**

| 状态码 | 原因 |
|--------|------|
| `400` | 只传了 `channel` 或只传了 `scope`（必须成对出现）|

---

### GET /api/sessions/detail

获取单个 Session 详情（与列表 item 相同结构）。

**Query 参数**（必填）

| 参数 | 类型 | 必填 |
|------|------|------|
| `channel` | `string` | ✅ |
| `scope` | `string` | ✅ |
| `channel_instance` | `string` | ❌ |

**Response 200** — 与列表 item 结构相同

**Errors**

| 状态码 | 原因 |
|--------|------|
| `404` | Session 不存在 |

---

### GET /api/sessions/messages

获取 Session 的历史消息列表（存储的对话记录）。

**Query 参数** — 同 `/api/sessions/detail`

**Response 200**

```json
{
  "items": [
    {
      "id": "msg-uuid",
      "role": "user",
      "content": "你好",
      "timestamp": "2026-03-22T10:00:00Z",
      "sender": "web"
    },
    {
      "id": "msg-uuid-2",
      "role": "assistant",
      "content": "你好！我是 claude...",
      "timestamp": "2026-03-22T10:00:05Z",
      "sender": "claude"
    }
  ]
}
```

| 字段 | 说明 |
|------|------|
| `role` | `"user"` / `"assistant"` |
| `sender` | 发送者标识，可能为 null |

---

### GET /api/sessions/events

获取 Session 的原始事件日志（JSONL 形式，每条事件含 type + payload）。

**Query 参数** — 同 `/api/sessions/detail`

**Response 200**

```json
{
  "items": [
    {
      "timestamp": "2026-03-22T10:00:05Z",
      "event_type": "TurnComplete",
      "payload": { "full_text": "...", "sender": "claude" }
    }
  ]
}
```

---

### DELETE /api/sessions

清空指定 Session 的历史记录（重置对话，下次聊天从头开始）。

**Query 参数**

| 参数 | 类型 | 必填 |
|------|------|------|
| `channel` | `string` | ✅ |
| `scope` | `string` | ✅ |
| `channel_instance` | `string` | ❌ |

**Response 200**

```json
{
  "ok": true,
  "session_key": {
    "channel": "ws",
    "scope": "main"
  }
}
```

**Errors**

| 状态码 | 原因 |
|--------|------|
| `404` | Session 不存在 |
| `500` | 重置失败 |

---

## Backend 管理

### GET /api/backends

列出所有已配置的 backend 及其运行状态。

**Response 200**

```json
{
  "items": [
    {
      "backend_id": "claude-acp",
      "family": "acp",
      "adapter_key": "acp",
      "registered": true,
      "adapter_registered": true,
      "probed": true,
      "healthy": true,
      "error": null,
      "approval_mode": "auto",
      "supports_native_local_skills": true,
      "launch": {
        "type": "external_command",
        "command": "claude",
        "args": ["--acp"],
        "env_keys": ["ANTHROPIC_API_KEY"]
      },
      "notes": []
    }
  ]
}
```

**BackendLaunchView 类型：**

```json
// 外部命令
{ "type": "external_command", "command": "claude", "args": [...], "env_keys": [...] }

// Gateway WS 远程 backend
{
  "type": "gateway_ws",
  "endpoint": "ws://remote:7770/ws",
  "token_configured": true,
  "password_configured": false,
  "role": "specialist",
  "scopes": ["group:team-1"],
  "agent_id": null,
  "lead_helper_mode": false
}

// 内置命令（bundled binary）
{ "type": "bundled_command" }
```

---

### GET /api/backends/{backend_id}

获取单个 Backend 详情，结构与列表 item 相同。

**Errors**

| 状态码 | 原因 |
|--------|------|
| `404` | Backend ID 不存在 |

---

## Channel 管理

### GET /api/channels

列出所有已配置的 Channel（Lark、DingTalk 等）及其状态。

**Response 200**

```json
{
  "items": [
    {
      "channel": "lark",
      "configured": true,
      "enabled": true,
      "routing_present": true,
      "credential_state": "configured",
      "presentation": "final_only",
      "default_instance": "prod",
      "trigger_policy": null,
      "notes": []
    }
  ]
}
```

| `credential_state` 取值 | 说明 |
|------------------------|------|
| `"configured"` | 凭据完整 |
| `"partial"` | 部分缺失 |
| `"missing"` | 未配置 |

---

### GET /api/channels/{channel_id}

获取单个 Channel 详情，`channel_id` 为 `"lark"` / `"dingtalk"` / `"dingtalk_webhook"` 等。

**Errors**

| 状态码 | 原因 |
|--------|------|
| `404` | Channel 未配置 |

---

## Skills

### GET /api/skills

获取全局（host-level）Skill 概览，含安装的 skill 列表和根目录信息。

**Response 200**

```json
{
  "host_skills": [
    {
      "name": "git-helper",
      "version": "1.0.0",
      "source_label": "global",
      "path": "/home/user/.clawbro/skills/git-helper"
    }
  ],
  "effective_skills": [...],
  "roots": [
    {
      "label": "global",
      "path": "/home/user/.clawbro/skills",
      "exists": true
    }
  ],
  "default_skills": { ... }
}
```

---

## Approvals（工具调用审批）

当 backend 的 `approval_mode` 为 `"manual"` 时，Agent 的危险工具调用（如 bash）会产生 Approval 请求，需要前端用户确认后才能继续执行。

### GET /api/approvals

列出当前所有待处理的 Approval 请求。

**Response 200**

```json
{
  "items": [
    {
      "approval_id": "approval-uuid",
      "prompt": "Agent 请求执行 bash 命令，是否允许？",
      "command": "ls -la /etc",
      "cwd": "/home/user",
      "host": "localhost",
      "agent_id": "claude",
      "expires_at_ms": 1742640000000
    }
  ]
}
```

---

### GET /api/approvals/{approval_id}

获取单个 Approval 详情。

**Errors**

| 状态码 | 原因 |
|--------|------|
| `404` | Approval 不存在或已过期 |

---

### POST /api/approvals/{approval_id}/approve

批准工具调用。Agent 将继续执行。

**Response 200**

```json
{
  "approval_id": "approval-uuid",
  "decision": "approve",
  "resolved": true
}
```

---

### POST /api/approvals/{approval_id}/deny

拒绝工具调用。Agent 将收到拒绝信号并停止该工具调用。

**Response 200**

```json
{
  "approval_id": "approval-uuid",
  "decision": "deny",
  "resolved": true
}
```

> 也可通过 WebSocket `ResolveApproval` 消息完成审批，效果相同。

---

## Scheduler（定时任务）

### GET /api/scheduler/jobs

列出所有定时任务。

**Response 200**

```json
{
  "items": [
    {
      "id": "job-uuid",
      "name": "daily-report",
      "enabled": true,
      "schedule": { "cron": "0 9 * * *" },
      "timezone": "Asia/Shanghai",
      "target": { ... },
      "next_run_at": "2026-03-23T09:00:00Z",
      "last_run_at": "2026-03-22T09:00:00Z",
      "last_success_at": "2026-03-22T09:01:30Z",
      "running_since": null,
      "max_retries": 3,
      "source_kind": "config",
      "source_actor": "system",
      "created_at": "2026-03-01T00:00:00Z",
      "updated_at": "2026-03-22T09:01:30Z"
    }
  ]
}
```

---

### GET /api/scheduler/jobs/{job_id}

获取单个定时任务详情。

**Errors**

| 状态码 | 原因 |
|--------|------|
| `404` | Job 不存在 |

---

### GET /api/scheduler/jobs/{job_id}/runs

获取指定 Job 的历史执行记录。

**Response 200**

```json
{
  "items": [
    {
      "id": "run-uuid",
      "job_id": "job-uuid",
      "scheduled_at": "2026-03-22T09:00:00Z",
      "started_at": "2026-03-22T09:00:01Z",
      "finished_at": "2026-03-22T09:01:30Z",
      "status": "succeeded",
      "attempt": 1,
      "error": null,
      "result_summary": "Report generated successfully",
      "trigger_reason": "due",
      "executor_session_key": "lark/group:abc",
      "executor_agent": "claude"
    }
  ]
}
```

| `status` 取值 | 说明 |
|--------------|------|
| `"running"` | 执行中 |
| `"succeeded"` | 成功 |
| `"failed"` | 失败 |
| `"skipped"` | 跳过 |

| `trigger_reason` 取值 | 说明 |
|----------------------|------|
| `"due"` | 按时触发 |
| `"run_now"` | 手动触发 |
| `"misfire_recovery"` | 补偿触发 |

---

### POST /api/scheduler/jobs/{job_id}/run-now

立即触发一次定时任务执行（不等待下次计划时间）。

**Response 204** — 无响应体（成功）

**Errors**

| 状态码 | 原因 |
|--------|------|
| `404` | Job 不存在 |

---

## Teams（多 Agent 协作）

Team 模式下，一个 Lead Agent 协调多个 Specialist Agent 并行完成复杂任务。

### GET /api/teams

列出所有活跃 Team。

**Response 200**

```json
{
  "items": [
    {
      "team_id": "team-uuid",
      "state": "running",
      "scope": "group:oc_abc123",
      "channel": "lark",
      "channel_instance": "prod",
      "lead_agent_name": "rex",
      "specialists": ["claude", "codex"],
      "tool_surface_ready": true,
      "task_counts": {
        "total": 5,
        "pending": 1,
        "in_progress": 2,
        "completed": 2,
        "failed": 0
      },
      "healthy": true,
      "notes": []
    }
  ]
}
```

---

### GET /api/teams/{team_id}

获取单个 Team 详情（含 Leader 最新更新、Channel 发送记录、路由统计等）。

**Errors**

| 状态码 | 原因 |
|--------|------|
| `404` | Team 不存在 |

---

### GET /api/teams/{team_id}/artifacts

获取 Team 工作目录中的上下文文件列表（TEAM.md、AGENTS.md、TASKS.md 等）。

**Response 200**

```json
{
  "items": [
    {
      "name": "team",
      "file_name": "TEAM.md",
      "path": "/home/user/.clawbro/teams/team-uuid/TEAM.md",
      "present": true,
      "size_bytes": 1024
    }
  ]
}
```

---

### GET /api/teams/{team_id}/tasks/{task_id}

获取 Team 中某个 Task 的详情及其产出文件。

**Response 200**

```json
{
  "team_id": "team-uuid",
  "id": "T001",
  "title": "实现用户认证模块",
  "status_raw": "completed",
  "assignee_hint": "claude",
  "retry_count": 0,
  "timeout_secs": 1800,
  "spec": "实现 JWT 认证...",
  "success_criteria": "所有测试通过",
  "completion_note": "已完成，PR 已提交",
  "artifacts": [
    {
      "name": "result",
      "file_name": "result.md",
      "path": "/tasks/T001/result.md",
      "present": true,
      "size_bytes": 512
    }
  ]
}
```

---

## Tasks（任务）

### GET /api/tasks

列出所有 Team 的所有 Task（跨 Team 汇总视图）。

**Response 200**

```json
{
  "items": [
    {
      "team_id": "team-uuid",
      "id": "T001",
      "title": "实现用户认证",
      "status_raw": "completed",
      "assignee_hint": "claude",
      "retry_count": 0,
      "timeout_secs": 1800,
      "spec": null,
      "success_criteria": null,
      "completion_note": "Done"
    }
  ]
}
```

---

### GET /api/tasks/{task_id}

获取全局唯一的 Task 详情（`task_id` 在所有 Team 中唯一时返回；若有歧义返回 400）。

**Errors**

| 状态码 | 原因 |
|--------|------|
| `400` | 多个 Team 中存在相同 task_id，产生歧义 |
| `404` | Task 不存在 |

---

## 系统端点

### GET /health

健康检查，返回 gateway 运行状态摘要。

**Response 200**（JSON 对象，含运行时信息）

---

### GET /status

运行时状态概览，含 backend 状态、team 列表、活跃 session 数等。

**Response 200**（JSON 对象）

---

### GET /doctor

诊断报告，检查配置完整性、backend 连通性、channel 状态等，给出问题列表和建议。

**Response 200**（JSON 对象，含 findings 数组）

---

## 快速参考：完整路由表

| 方法 | 路径 | 说明 |
|------|------|------|
| `WS` | `/ws` | WebSocket 双向通信 |
| `GET` | `/health` | 健康检查 |
| `GET` | `/status` | 运行状态 |
| `GET` | `/doctor` | 诊断报告 |
| `POST` | `/api/chat` | Web Chat 发消息 |
| `GET` | `/api/agents` | 列出 Agents |
| `POST` | `/api/agents` | 创建 Agent |
| `GET` | `/api/agents/{name}` | Agent 详情 |
| `PATCH` | `/api/agents/{name}` | 更新 Agent |
| `DELETE` | `/api/agents/{name}` | 删除 Agent |
| `GET` | `/api/agents/{name}/skills` | Agent Skills |
| `GET` | `/api/config/effective` | 有效配置摘要 |
| `GET` | `/api/config/spec` | 完整配置规格（脱敏）|
| `GET` | `/api/config/raw` | 读取 config.toml |
| `PUT` | `/api/config/raw` | 写入 config.toml |
| `POST` | `/api/config/validate` | 校验 TOML 内容 |
| `GET` | `/api/sessions` | 列出 Sessions |
| `DELETE` | `/api/sessions` | 清空 Session 历史 |
| `GET` | `/api/sessions/detail` | Session 详情 |
| `GET` | `/api/sessions/messages` | Session 历史消息 |
| `GET` | `/api/sessions/events` | Session 事件日志 |
| `GET` | `/api/backends` | 列出 Backends |
| `GET` | `/api/backends/{backend_id}` | Backend 详情 |
| `GET` | `/api/channels` | 列出 Channels |
| `GET` | `/api/channels/{channel_id}` | Channel 详情 |
| `GET` | `/api/skills` | 全局 Skills 概览 |
| `GET` | `/api/approvals` | 待审批列表 |
| `GET` | `/api/approvals/{approval_id}` | Approval 详情 |
| `POST` | `/api/approvals/{approval_id}/approve` | 批准 |
| `POST` | `/api/approvals/{approval_id}/deny` | 拒绝 |
| `GET` | `/api/scheduler/jobs` | 定时任务列表 |
| `GET` | `/api/scheduler/jobs/{job_id}` | Job 详情 |
| `GET` | `/api/scheduler/jobs/{job_id}/runs` | Job 执行历史 |
| `POST` | `/api/scheduler/jobs/{job_id}/run-now` | 立即执行 Job |
| `GET` | `/api/teams` | 列出 Teams |
| `GET` | `/api/teams/{team_id}` | Team 详情 |
| `GET` | `/api/teams/{team_id}/artifacts` | Team 上下文文件 |
| `GET` | `/api/teams/{team_id}/tasks/{task_id}` | Team Task 详情 |
| `GET` | `/api/tasks` | 全局 Task 列表 |
| `GET` | `/api/tasks/{task_id}` | Task 详情 |
