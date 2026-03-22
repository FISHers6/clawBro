# Phase 3 Write APIs Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现三类写接口——Web Chat（不需要 IM Channel 直接网页聊天）、Config 文件读写（raw TOML get/put + 校验）、Agent CRUD（toml_edit 追加/修改/删除 `[[agent_roster]]`），并连通 Dashboard WS 事件让前端能实时收到回复流。

**Architecture:** `POST /api/chat` 构造一个带 `channel:"ws"` 的 `InboundMsg` 并调用现有 `spawn_im_turn`（使用无操作的 `WsVirtualChannel`），Agent 输出通过已有 WS `AgentEvent` 广播推送给订阅客户端。Config/Agent 写接口使用 `toml_edit` crate 格式保留地修改 `config.toml`，校验后写磁盘并返回 `restart_required:true`（当前阶段 clawBro 不自动 hot-swap Arc，UI 提示用户重启）。AppState 新增 `config_path: Arc<PathBuf>` 字段以便 write handler 定位文件。

**Tech Stack:** Rust · axum · `toml_edit` (格式保留 TOML 写) · `toml` (已有，用于 parse/validate) · `serde_json` (已有) · existing `spawn_im_turn` + `WsVirtualChannel` (new) · existing `GatewayConfig::from_toml_str` (parse) + `validate_runtime_topology` (validate)

---

## Chunk 1: Web Chat 虚拟 Channel + POST /api/chat

### Task 1: WsVirtualChannel（无操作 Channel 实现）

**Files:**
- Create: `crates/clawbro-server/src/channels_internal/ws_virtual.rs`
- Modify: `crates/clawbro-server/src/channels_internal/mod.rs` (re-export)

**背景:** `spawn_im_turn` 需要 `Arc<dyn Channel>` 来发 IM 回复。Web chat 不需要发任何 IM，回复已经通过全局 `event_tx` 广播到 WS 订阅者。所以 send/listen 均 no-op 即可。

- [ ] **Step 1: 在 `channels_internal/mod.rs` 中确认 Channel trait 签名**

读取 `crates/clawbro-server/src/channels_internal/mod.rs`，找到 `Channel` trait 定义（`name`, `send`, `listen` 三个方法）。

Run: `grep -n "trait Channel" crates/clawbro-server/src/channels_internal/mod.rs`

- [ ] **Step 2: 写 `ws_virtual.rs` 测试（先）**

```rust
// crates/clawbro-server/src/channels_internal/ws_virtual.rs
use super::Channel;
use crate::protocol::{InboundMsg, OutboundMsg};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

pub struct WsVirtualChannel;

#[async_trait]
impl Channel for WsVirtualChannel {
    fn name(&self) -> &str {
        "ws"
    }

    async fn send(&self, _msg: &OutboundMsg) -> Result<()> {
        Ok(())
    }

    async fn listen(&self, _tx: mpsc::Sender<InboundMsg>) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{MsgContent, SessionKey};

    #[tokio::test]
    async fn ws_virtual_channel_name_is_ws() {
        let ch = WsVirtualChannel;
        assert_eq!(ch.name(), "ws");
    }

    #[tokio::test]
    async fn ws_virtual_channel_send_is_noop() {
        let ch = WsVirtualChannel;
        let msg = OutboundMsg {
            session_key: SessionKey::new("ws", "main"),
            content: MsgContent::text("hello"),
            reply_to: None,
            thread_ts: None,
        };
        ch.send(&msg).await.unwrap(); // must not panic or error
    }
}
```

- [ ] **Step 3: 运行测试确认失败（模块未注册）**

Run: `cargo test -p clawbro ws_virtual 2>&1 | head -20`
Expected: 编译错误 "can't find module ws_virtual"

- [ ] **Step 4: 在 `channels_internal/mod.rs` 中注册模块**

找到现有模块声明的区域（`pub mod dingtalk;` 附近），在同级追加：

```rust
pub mod ws_virtual;
pub use ws_virtual::WsVirtualChannel;
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p clawbro ws_virtual 2>&1 | tail -10`
Expected: `2 passed`

- [ ] **Step 6: cargo check 整体**

Run: `cargo check -p clawbro 2>&1 | tail -20`
Expected: no errors

- [ ] **Step 7: 提交**

```bash
git add crates/clawbro-server/src/channels_internal/ws_virtual.rs \
        crates/clawbro-server/src/channels_internal/mod.rs
git commit -m "feat: add WsVirtualChannel (no-op Channel for web chat)"
```

---

### Task 2: AppState 增加 `config_path` 字段

**Files:**
- Modify: `crates/clawbro-server/src/state.rs`
- Modify: `crates/clawbro-server/src/gateway_process.rs`

**背景:** Config write handler 需要知道 config.toml 的路径，目前路径逻辑藏在 `GatewayConfig::load()` 内部。把它提到 AppState 让所有 handler 共享。

- [ ] **Step 1: 查看 `GatewayConfig::load()` 的路径逻辑**

路径逻辑在 `config.rs:1232`：
```rust
let path = std::env::var("CLAWBRO_CONFIG")
    .map(std::path::PathBuf::from)
    .unwrap_or_else(|_| {
        dirs::home_dir().unwrap_or_default().join(".clawbro").join("config.toml")
    });
```

- [ ] **Step 2: 在 `config.rs` 中提取路径为独立函数**

在 `GatewayConfig` impl 块之前或 `load()` 前面加：

```rust
/// 解析配置文件路径（env CLAWBRO_CONFIG 或 ~/.clawbro/config.toml）
pub fn config_file_path() -> std::path::PathBuf {
    std::env::var("CLAWBRO_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_default()
                .join(".clawbro")
                .join("config.toml")
        })
}
```

在 `load()` 中改为：
```rust
pub fn load() -> Result<Self> {
    let path = Self::config_file_path();  // <-- 改这一行
    // ... 其余不变
```

- [ ] **Step 3: 在 `state.rs` 中加 `config_path` 字段**

```rust
// state.rs
use std::path::PathBuf;

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<SessionRegistry>,
    pub runtime_registry: Arc<BackendRegistry>,
    pub event_tx: broadcast::Sender<AgentEvent>,
    pub cfg: Arc<GatewayConfig>,
    pub config_path: Arc<PathBuf>,          // <-- 新增
    pub channel_registry: Arc<ChannelRegistry>,
    pub dingtalk_webhook_channel: Option<Arc<DingTalkWebhookChannel>>,
    pub runtime_token: Arc<String>,
    pub approvals: ApprovalBroker,
    pub scheduler_service: Arc<SchedulerService>,
}
```

- [ ] **Step 4: 在 `gateway_process.rs` 中设置 `config_path`**

在 `let cfg_arc = Arc::new(cfg.clone());` 后面加：
```rust
let config_path = Arc::new(crate::config::GatewayConfig::config_file_path());
```

在 `AppState { ... }` 构造里加：
```rust
config_path: config_path.clone(),
```

- [ ] **Step 5: cargo check**

Run: `cargo check -p clawbro 2>&1 | grep "^error" | head -20`
Expected: no errors（可能有 unused import warning，忽略）

- [ ] **Step 6: 提交**

```bash
git add crates/clawbro-server/src/config.rs \
        crates/clawbro-server/src/state.rs \
        crates/clawbro-server/src/gateway_process.rs
git commit -m "feat: expose config_file_path() + AppState.config_path for write handlers"
```

---

### Task 3: POST /api/chat 处理器

**Files:**
- Create: `crates/clawbro-server/src/gateway/api/chat.rs`
- Modify: `crates/clawbro-server/src/gateway/api/mod.rs` (pub mod chat)
- Modify: `crates/clawbro-server/src/gateway/server.rs` (注册路由)

- [ ] **Step 1: 写 `chat.rs` 测试（先写，验证接口契约）**

```rust
// crates/clawbro-server/src/gateway/api/chat.rs

use crate::channels_internal::WsVirtualChannel;
use crate::config::ProgressPresentationMode;
use crate::im_sink::spawn_im_turn;
use crate::protocol::{InboundMsg, MsgContent, MsgSource, SessionKey};
use crate::state::AppState;
use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::types::ApiErrorBody;

#[derive(Debug, Deserialize)]
pub struct ChatSendBody {
    /// 用户消息内容（必填）
    pub message: String,
    /// Session scope，默认 "main"（对应 session_key.scope）
    pub scope: Option<String>,
    /// 指定 Agent（对应 InboundMsg.target_agent，如 "@claude"）
    pub agent: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ChatSendResponse {
    /// 本次 turn 的唯一 ID（同 InboundMsg.id）；WS 事件中的 run_id 与之对应
    pub turn_id: String,
    /// 订阅此 session_key 的 WS 事件可获取 Agent 回复流
    pub session_key: SessionKey,
}

pub async fn chat_send(
    State(state): State<AppState>,
    Json(body): Json<ChatSendBody>,
) -> Result<Json<ChatSendResponse>, (StatusCode, Json<ApiErrorBody>)> {
    let message = body.message.trim().to_string();
    if message.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody {
                error: "message must not be empty".to_string(),
            }),
        ));
    }

    let scope = body
        .scope
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "main".to_string());

    let session_key = SessionKey::new("ws", &scope);
    let turn_id = uuid::Uuid::new_v4().to_string();

    let inbound = InboundMsg {
        id: turn_id.clone(),
        session_key: session_key.clone(),
        content: MsgContent::text(&message),
        sender: "web".to_string(),
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: body.agent,
        source: MsgSource::Human,
    };

    // WsVirtualChannel is a no-op Channel — Agent replies go via WS AgentEvent broadcast.
    // Clients should subscribe to WS {channel:"ws", scope} before calling this endpoint.
    let channel = Arc::new(WsVirtualChannel);

    spawn_im_turn(
        state.registry.clone(),
        channel,
        state.channel_registry.clone(),
        state.cfg.clone(),
        inbound,
        ProgressPresentationMode::FinalOnly,
    );

    Ok(Json(ChatSendResponse {
        turn_id,
        session_key,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_scope_is_main() {
        let scope = None::<String>
            .map(|s: String| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "main".to_string());
        assert_eq!(scope, "main");
    }

    #[test]
    fn custom_scope_passes_through() {
        let scope = Some("  group:abc  ".to_string())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "main".to_string());
        assert_eq!(scope, "group:abc");
    }

    #[test]
    fn empty_scope_string_falls_back_to_main() {
        let scope = Some("   ".to_string())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "main".to_string());
        assert_eq!(scope, "main");
    }
}
```

- [ ] **Step 2: 运行单元测试（应该通过——纯逻辑，无 AppState 依赖）**

Run: `cargo test -p clawbro gateway::api::chat 2>&1 | tail -10`
Expected: 3 passed（或编译错误，先修 mod 注册）

- [ ] **Step 3: 注册模块**

在 `gateway/api/mod.rs` 中加：
```rust
pub mod chat;
```

- [ ] **Step 4: 在 `server.rs` 中注册路由**

在 `build_router` 的路由链中加：
```rust
.route("/api/chat", post(api::chat::chat_send))
```

注意 `post` 已在 `use axum::routing::{get, post}` 中导入。

- [ ] **Step 5: cargo check**

Run: `cargo check -p clawbro 2>&1 | grep "^error" | head -20`
Expected: no errors

- [ ] **Step 6: 集成冒烟测试**

用 curl 测（需要 gateway 在跑）：
```bash
# 先订阅 WS，再发 chat（手动验证时需要 wscat 或 websocat）
curl -X POST http://localhost:7770/api/chat \
  -H "Content-Type: application/json" \
  -d '{"message":"你好，介绍一下你自己", "scope":"main"}' | jq
```
Expected response:
```json
{"turn_id": "uuid-xxx", "session_key": {"channel":"ws","scope":"main"}}
```

- [ ] **Step 7: 提交**

```bash
git add crates/clawbro-server/src/gateway/api/chat.rs \
        crates/clawbro-server/src/gateway/api/mod.rs \
        crates/clawbro-server/src/gateway/server.rs
git commit -m "feat: POST /api/chat — web chat without IM channel"
```

---

## Chunk 2: Config 读写 API

### Task 4: 添加 toml_edit 依赖

**Files:**
- Modify: `crates/clawbro-server/Cargo.toml`

- [ ] **Step 1: 在 `[dependencies]` 中加 toml_edit**

在 `Cargo.toml` 的 `[dependencies]` 块末尾追加：
```toml
toml_edit = "0.22"
```

- [ ] **Step 2: cargo check 确认依赖解析**

Run: `cargo check -p clawbro 2>&1 | tail -5`
Expected: 无 "no matching package" 错误

- [ ] **Step 3: 提交**

```bash
git add crates/clawbro-server/Cargo.toml
git commit -m "deps: add toml_edit 0.22 for format-preserving config writes"
```

---

### Task 5: GET /api/config/raw + PUT /api/config/raw + POST /api/config/validate

**Files:**
- Create: `crates/clawbro-server/src/gateway/api/config_write.rs`
- Modify: `crates/clawbro-server/src/gateway/api/mod.rs`
- Modify: `crates/clawbro-server/src/gateway/server.rs`

**功能说明:**
- `GET /api/config/raw` — 返回 config.toml 原始 TOML 字符串（若文件不存在返回空字符串）
- `PUT /api/config/raw` — 接受完整 TOML 字符串，parse + validate，写磁盘，返回 `{ok, restart_required:true}`
- `POST /api/config/validate` — 仅 parse + validate，不写磁盘，返回 `{ok, issues:[]}`

- [ ] **Step 1: 写 `config_write.rs` 单元测试（先）**

```rust
// crates/clawbro-server/src/gateway/api/config_write.rs

use crate::config::GatewayConfig;
use crate::state::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::types::ApiErrorBody;

#[derive(Debug, Serialize)]
pub struct RawConfigResponse {
    pub content: String,
    /// config.toml 的绝对路径（前端展示用）
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct PutRawConfigBody {
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct WriteConfigResponse {
    pub ok: bool,
    pub path: String,
    /// 当前 clawBro 版本不支持运行时热重载 — 需要重启 gateway 生效
    pub restart_required: bool,
}

#[derive(Debug, Serialize)]
pub struct ValidateConfigResponse {
    pub ok: bool,
    pub error: Option<String>,
}

pub async fn get_raw_config(
    State(state): State<AppState>,
) -> Json<RawConfigResponse> {
    let path = state.config_path.as_ref().clone();
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    Json(RawConfigResponse {
        content,
        path: path.display().to_string(),
    })
}

pub async fn put_raw_config(
    State(state): State<AppState>,
    Json(body): Json<PutRawConfigBody>,
) -> Result<Json<WriteConfigResponse>, (StatusCode, Json<ApiErrorBody>)> {
    // 1. Parse
    let cfg = GatewayConfig::from_toml_str(&body.content).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody {
                error: format!("config parse error: {e}"),
            }),
        )
    })?;

    // 2. Validate topology
    cfg.validate_runtime_topology().map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody {
                error: format!("config validation error: {e}"),
            }),
        )
    })?;

    // 3. Write to disk
    let path: PathBuf = state.config_path.as_ref().clone();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiErrorBody {
                    error: format!("cannot create config dir: {e}"),
                }),
            )
        })?;
    }
    std::fs::write(&path, &body.content).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorBody {
                error: format!("write config failed: {e}"),
            }),
        )
    })?;

    tracing::info!(path = %path.display(), "config.toml updated via API");

    Ok(Json(WriteConfigResponse {
        ok: true,
        path: path.display().to_string(),
        restart_required: true,
    }))
}

pub async fn validate_config(
    Json(body): Json<PutRawConfigBody>,
) -> Json<ValidateConfigResponse> {
    let result = GatewayConfig::from_toml_str(&body.content)
        .and_then(|cfg| cfg.validate_runtime_topology().map(|_| ()));
    match result {
        Ok(()) => Json(ValidateConfigResponse { ok: true, error: None }),
        Err(e) => Json(ValidateConfigResponse {
            ok: false,
            error: Some(e.to_string()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_returns_ok_for_minimal_config() {
        let minimal = r#"
[gateway]
host = "0.0.0.0"
port = 7770
"#;
        let result = GatewayConfig::from_toml_str(minimal)
            .and_then(|cfg| cfg.validate_runtime_topology().map(|_| ()));
        assert!(result.is_ok(), "minimal config should validate: {:?}", result);
    }

    #[test]
    fn validate_returns_error_for_invalid_toml() {
        let bad = "this is [not valid toml {{";
        let result = GatewayConfig::from_toml_str(bad);
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: 运行单元测试**

Run: `cargo test -p clawbro config_write 2>&1 | tail -10`
Expected: 2 passed（在注册模块后）

- [ ] **Step 3: 注册模块 + 路由**

`gateway/api/mod.rs` 加：
```rust
pub mod config_write;
```

`server.rs` `build_router` 中加：
```rust
.route("/api/config/raw", get(api::config_write::get_raw_config))
.route("/api/config/raw", put(api::config_write::put_raw_config))
.route("/api/config/validate", post(api::config_write::validate_config))
```

注意在 `use axum::routing::{get, post}` 中加 `put`：
```rust
use axum::routing::{get, post, put};
```

- [ ] **Step 4: cargo check**

Run: `cargo check -p clawbro 2>&1 | grep "^error" | head -20`
Expected: no errors

- [ ] **Step 5: 提交**

```bash
git add crates/clawbro-server/src/gateway/api/config_write.rs \
        crates/clawbro-server/src/gateway/api/mod.rs \
        crates/clawbro-server/src/gateway/server.rs
git commit -m "feat: GET/PUT /api/config/raw + POST /api/config/validate"
```

---

## Chunk 3: Agent CRUD（toml_edit）

### Task 6: Agent 写操作处理器

**Files:**
- Create: `crates/clawbro-server/src/gateway/api/agents_write.rs`
- Modify: `crates/clawbro-server/src/gateway/api/mod.rs`
- Modify: `crates/clawbro-server/src/gateway/server.rs`

**设计说明:**
- `POST /api/agents` — 追加一个 `[[agent_roster]]` 段到 config.toml
- `PATCH /api/agents/{name}` — 修改已有 `[[agent_roster]]` 中 name 匹配的条目
- `DELETE /api/agents/{name}` — 删除匹配条目

所有操作都用 `toml_edit` 格式保留写入，然后重新 parse+validate，最后写磁盘。

- [ ] **Step 1: 写单元测试（先）**

```rust
// crates/clawbro-server/src/gateway/api/agents_write.rs

use crate::config::GatewayConfig;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use toml_edit::{DocumentMut, Item, Table, value};

use super::types::ApiErrorBody;

#[derive(Debug, Serialize)]
pub struct AgentWriteResponse {
    pub ok: bool,
    /// 受影响的 agent name
    pub name: String,
    pub restart_required: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentBody {
    pub name: String,
    pub backend_id: String,
    #[serde(default)]
    pub mentions: Vec<String>,
    pub persona_dir: Option<String>,
    pub workspace_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PatchAgentBody {
    pub backend_id: Option<String>,
    pub mentions: Option<Vec<String>>,
    pub persona_dir: Option<String>,
    pub workspace_dir: Option<String>,
}

/// 从 config.toml 路径读取 toml_edit Document，若文件不存在返回空 Document
fn read_document(path: &PathBuf) -> Result<DocumentMut, String> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    content.parse::<DocumentMut>().map_err(|e| e.to_string())
}

/// 写 Document 到磁盘，校验后再写
fn write_document(path: &PathBuf, doc: &DocumentMut) -> Result<(), String> {
    let content = doc.to_string();
    // Validate by re-parsing
    GatewayConfig::from_toml_str(&content)
        .and_then(|cfg| cfg.validate_runtime_topology())
        .map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, content).map_err(|e| e.to_string())
}

/// `[[agent_roster]]` 数组中找到 name 匹配的 index，返回 None 若不存在
fn find_agent_roster_index(doc: &DocumentMut, name: &str) -> Option<usize> {
    doc.get("agent_roster")
        .and_then(|v| v.as_array_of_tables())
        .and_then(|arr| {
            arr.iter().position(|tbl| {
                tbl.get("name")
                    .and_then(|v| v.as_str())
                    .map(|n| n == name)
                    .unwrap_or(false)
            })
        })
}

pub async fn create_agent(
    State(state): State<AppState>,
    Json(body): Json<CreateAgentBody>,
) -> Result<Json<AgentWriteResponse>, (StatusCode, Json<ApiErrorBody>)> {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody { error: "name is required".to_string() }),
        ));
    }

    let path: PathBuf = state.config_path.as_ref().clone();
    let mut doc = read_document(&path).map_err(|e| (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiErrorBody { error: format!("read config failed: {e}") }),
    ))?;

    // 重复检查
    if find_agent_roster_index(&doc, &name).is_some() {
        return Err((
            StatusCode::CONFLICT,
            Json(ApiErrorBody { error: format!("agent '{name}' already exists") }),
        ));
    }

    // 构建新的 [[agent_roster]] 表
    let mut tbl = Table::new();
    tbl["name"] = value(name.as_str());
    tbl["backend_id"] = value(body.backend_id.trim());
    if !body.mentions.is_empty() {
        let mut arr = toml_edit::Array::new();
        for m in &body.mentions {
            arr.push(m.as_str());
        }
        tbl["mentions"] = Item::Value(toml_edit::Value::Array(arr));
    }
    if let Some(dir) = &body.persona_dir {
        tbl["persona_dir"] = value(dir.trim());
    }
    if let Some(dir) = &body.workspace_dir {
        tbl["workspace_dir"] = value(dir.trim());
    }

    // 追加到 agent_roster 数组
    if doc.get("agent_roster").is_none() {
        doc["agent_roster"] = Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }
    if let Some(arr) = doc["agent_roster"].as_array_of_tables_mut() {
        arr.push(tbl);
    }

    write_document(&path, &doc).map_err(|e| (
        StatusCode::BAD_REQUEST,
        Json(ApiErrorBody { error: format!("config invalid after edit: {e}") }),
    ))?;

    tracing::info!(agent = %name, "agent created via API");

    Ok(Json(AgentWriteResponse { ok: true, name, restart_required: true }))
}

pub async fn patch_agent(
    Path(agent_name): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<PatchAgentBody>,
) -> Result<Json<AgentWriteResponse>, (StatusCode, Json<ApiErrorBody>)> {
    let path: PathBuf = state.config_path.as_ref().clone();
    let mut doc = read_document(&path).map_err(|e| (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiErrorBody { error: format!("read config failed: {e}") }),
    ))?;

    let idx = find_agent_roster_index(&doc, &agent_name).ok_or_else(|| (
        StatusCode::NOT_FOUND,
        Json(ApiErrorBody { error: format!("agent '{agent_name}' not found") }),
    ))?;

    if let Some(arr) = doc["agent_roster"].as_array_of_tables_mut() {
        let tbl = &mut arr[idx];
        if let Some(bid) = &body.backend_id {
            tbl["backend_id"] = value(bid.trim());
        }
        if let Some(mentions) = &body.mentions {
            let mut arr_val = toml_edit::Array::new();
            for m in mentions {
                arr_val.push(m.as_str());
            }
            tbl["mentions"] = Item::Value(toml_edit::Value::Array(arr_val));
        }
        if let Some(dir) = &body.persona_dir {
            tbl["persona_dir"] = value(dir.trim());
        }
        if let Some(dir) = &body.workspace_dir {
            tbl["workspace_dir"] = value(dir.trim());
        }
    }

    write_document(&path, &doc).map_err(|e| (
        StatusCode::BAD_REQUEST,
        Json(ApiErrorBody { error: format!("config invalid after edit: {e}") }),
    ))?;

    tracing::info!(agent = %agent_name, "agent updated via API");

    Ok(Json(AgentWriteResponse { ok: true, name: agent_name, restart_required: true }))
}

pub async fn delete_agent(
    Path(agent_name): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<AgentWriteResponse>, (StatusCode, Json<ApiErrorBody>)> {
    let path: PathBuf = state.config_path.as_ref().clone();
    let mut doc = read_document(&path).map_err(|e| (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiErrorBody { error: format!("read config failed: {e}") }),
    ))?;

    let idx = find_agent_roster_index(&doc, &agent_name).ok_or_else(|| (
        StatusCode::NOT_FOUND,
        Json(ApiErrorBody { error: format!("agent '{agent_name}' not found") }),
    ))?;

    if let Some(arr) = doc["agent_roster"].as_array_of_tables_mut() {
        arr.remove(idx);
    }

    write_document(&path, &doc).map_err(|e| (
        StatusCode::BAD_REQUEST,
        Json(ApiErrorBody { error: format!("config invalid after edit: {e}") }),
    ))?;

    tracing::info!(agent = %agent_name, "agent deleted via API");

    Ok(Json(AgentWriteResponse { ok: true, name: agent_name, restart_required: true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_test_config(dir: &std::path::Path, content: &str) -> PathBuf {
        let path = dir.join("config.toml");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn find_agent_roster_index_returns_none_when_absent() {
        let content = r#"
[gateway]
host = "0.0.0.0"
port = 7770
"#;
        let doc: DocumentMut = content.parse().unwrap();
        assert!(find_agent_roster_index(&doc, "rex").is_none());
    }

    #[test]
    fn find_agent_roster_index_returns_correct_index() {
        let content = r#"
[[agent_roster]]
name = "alpha"
backend_id = "backend-a"

[[agent_roster]]
name = "beta"
backend_id = "backend-b"
"#;
        let doc: DocumentMut = content.parse().unwrap();
        assert_eq!(find_agent_roster_index(&doc, "alpha"), Some(0));
        assert_eq!(find_agent_roster_index(&doc, "beta"), Some(1));
        assert!(find_agent_roster_index(&doc, "gamma").is_none());
    }

    #[test]
    fn write_document_round_trips_without_corruption() {
        let tmp = tempdir().unwrap();
        let content = r#"
[gateway]
host = "0.0.0.0"
port = 7770

[[agent_roster]]
name = "rex"
backend_id = "claude-acp"
mentions = ["@rex"]
"#;
        // Write a file that GatewayConfig can parse (we need a backend for validation to pass,
        // but agent_roster without backend section fails validate_runtime_topology).
        // So only test the file-write path here, not the validation path.
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, content).unwrap();
        let doc: DocumentMut = content.parse().unwrap();
        let output = doc.to_string();
        // The round-trip should preserve key-value content
        assert!(output.contains("name = \"rex\""));
        assert!(output.contains("backend_id = \"claude-acp\""));
    }
}
```

- [ ] **Step 2: 运行单元测试**

Run: `cargo test -p clawbro agents_write 2>&1 | tail -15`
Expected: 3 passed（在注册模块后）

- [ ] **Step 3: 注册模块 + 路由**

`gateway/api/mod.rs` 加：
```rust
pub mod agents_write;
```

`server.rs` 中加路由（`build_router` 的链里，紧接在已有 `/api/agents` 路由后面）：
```rust
.route("/api/agents", post(api::agents_write::create_agent))
.route("/api/agents/{name}", axum::routing::patch(api::agents_write::patch_agent))
.route("/api/agents/{name}", axum::routing::delete(api::agents_write::delete_agent))
```

在 `use axum::routing::{get, post, put};` 中不需要额外 import，`axum::routing::patch`/`delete` 用全路径引用即可。或者统一改 use 为：
```rust
use axum::routing::{delete, get, patch, post, put};
```
然后路由写 `patch(...)`, `delete(...)`。

- [ ] **Step 4: cargo check**

Run: `cargo check -p clawbro 2>&1 | grep "^error" | head -20`
Expected: no errors

- [ ] **Step 5: 冒烟测试（需要 gateway 跑且有 backend 配置）**

```bash
# 创建 agent
curl -X POST http://localhost:7770/api/agents \
  -H "Content-Type: application/json" \
  -d '{"name":"testbot","backend_id":"claude-acp","mentions":["@testbot"]}' | jq

# 查询（用现有 GET /api/config/raw 或 GET /api/agents）
curl http://localhost:7770/api/config/raw | jq -r '.content' | grep -A5 testbot

# 删除
curl -X DELETE http://localhost:7770/api/agents/testbot | jq
```

- [ ] **Step 6: 提交**

```bash
git add crates/clawbro-server/src/gateway/api/agents_write.rs \
        crates/clawbro-server/src/gateway/api/mod.rs \
        crates/clawbro-server/src/gateway/server.rs
git commit -m "feat: POST/PATCH/DELETE /api/agents — agent CRUD via toml_edit"
```

---

## Chunk 4: Session 写操作

### Task 7: DELETE /api/sessions（清空 session 历史）

**Files:**
- Create: `crates/clawbro-server/src/gateway/api/sessions_write.rs`
- Modify: `crates/clawbro-server/src/gateway/api/mod.rs`
- Modify: `crates/clawbro-server/src/gateway/server.rs`

**功能:** 删除指定 session 的历史记录（清空 JSONL 事件，保留 session 元数据允许继续使用）。query params 与 `GET /api/sessions` 相同：`channel` + `scope`。

- [ ] **Step 1: 了解 `SessionManager` 删除/清空 API**

先搜索现有 SessionManager 方法：
```bash
grep -n "pub.*fn.*delete\|pub.*fn.*clear\|pub.*fn.*reset\|pub.*fn.*purge" \
  crates/clawbro-server/src/session.rs | head -20
```
根据找到的方法决定如何实现。若没有清空方法，用 `delete_session` 或直接删除 JSONL 文件。

- [ ] **Step 2: 写 `sessions_write.rs`**

```rust
// crates/clawbro-server/src/gateway/api/sessions_write.rs

use crate::protocol::SessionKey;
use crate::state::AppState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

use super::types::ApiErrorBody;

#[derive(Debug, Deserialize)]
pub struct SessionLocator {
    pub channel: String,
    pub scope: String,
    pub channel_instance: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionDeleteResponse {
    pub ok: bool,
    pub session_key: SessionKey,
}

pub async fn delete_session_history(
    Query(q): Query<SessionLocator>,
    State(state): State<AppState>,
) -> Result<Json<SessionDeleteResponse>, (StatusCode, Json<ApiErrorBody>)> {
    let session_key = if let Some(instance) = q.channel_instance {
        SessionKey::with_instance(&q.channel, instance, &q.scope)
    } else {
        SessionKey::new(&q.channel, &q.scope)
    };

    // Attempt to delete via session manager
    let deleted = state
        .registry
        .session_manager_ref()
        .delete_session(&session_key)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiErrorBody {
                    error: format!("delete session failed: {e}"),
                }),
            )
        })?;

    if !deleted {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody {
                error: format!(
                    "session '{}:{}' not found",
                    session_key.channel, session_key.scope
                ),
            }),
        ));
    }

    tracing::info!(
        channel = %session_key.channel,
        scope = %session_key.scope,
        "session history deleted via API"
    );

    Ok(Json(SessionDeleteResponse { ok: true, session_key }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_locator_with_instance_builds_correct_key() {
        let q = SessionLocator {
            channel: "lark".to_string(),
            scope: "group:abc".to_string(),
            channel_instance: Some("default".to_string()),
        };
        let key = SessionKey::with_instance(&q.channel, q.channel_instance.unwrap(), &q.scope);
        assert_eq!(key.channel, "lark");
        assert_eq!(key.scope, "group:abc");
        assert_eq!(key.channel_instance.as_deref(), Some("default"));
    }

    #[test]
    fn session_locator_without_instance_builds_simple_key() {
        let q = SessionLocator {
            channel: "ws".to_string(),
            scope: "main".to_string(),
            channel_instance: None,
        };
        let key = SessionKey::new(&q.channel, &q.scope);
        assert!(key.channel_instance.is_none());
    }
}
```

**注意:** 若 `SessionManager` 没有 `delete_session()` 方法，先在 `session.rs` 中实现（删除 JSONL 文件 + 从 store 中移除记录）。这个 Task 需要先 grep `session.rs` 确认。

- [ ] **Step 3: 若 `SessionManager::delete_session` 不存在，先实现它**

```bash
grep -n "pub.*fn.*delete\|pub.*fn.*remove" \
  crates/clawbro-server/src/session.rs | head -20
```

若不存在，在 `SessionManager` 中添加：
```rust
/// 删除 session（清空历史，从 store 中移除）。
/// 返回 `true` 若 session 存在并已删除，`false` 若不存在。
pub async fn delete_session(&self, session_key: &SessionKey) -> Result<bool> {
    // 实现取决于 SessionStorage 的内部结构；先读 session.rs 再具体实现
    todo!()
}
```

然后根据 `SessionStorage` 的 API 完成实现。

- [ ] **Step 4: 注册模块 + 路由**

`gateway/api/mod.rs`:
```rust
pub mod sessions_write;
```

`server.rs`:
```rust
.route("/api/sessions", delete(api::sessions_write::delete_session_history))
```

- [ ] **Step 5: cargo check + 运行测试**

Run: `cargo test -p clawbro sessions_write 2>&1 | tail -10`
Expected: 2 passed

Run: `cargo check -p clawbro 2>&1 | grep "^error" | head -10`
Expected: no errors

- [ ] **Step 6: 提交**

```bash
git add crates/clawbro-server/src/gateway/api/sessions_write.rs \
        crates/clawbro-server/src/gateway/api/mod.rs \
        crates/clawbro-server/src/gateway/server.rs
git commit -m "feat: DELETE /api/sessions — clear session history"
```

---

## 最终整体验证

- [ ] **全量 cargo test**

Run: `cargo test -p clawbro 2>&1 | tail -20`
Expected: all existing tests pass + new tests pass, 0 failures

- [ ] **API 快速回归（需要 gateway 运行）**

```bash
# 1. Web chat
curl -s -X POST http://localhost:7770/api/chat \
  -H "Content-Type: application/json" \
  -d '{"message":"ping","scope":"test-phase3"}' | jq '.turn_id'

# 2. Get raw config
curl -s http://localhost:7770/api/config/raw | jq '.path'

# 3. Validate config
curl -s -X POST http://localhost:7770/api/config/validate \
  -H "Content-Type: application/json" \
  -d '{"content":"[gateway]\nhost=\"0.0.0.0\"\nport=7770"}' | jq '.ok'

# 4. Create agent (需要有 backend-id 存在于 config)
# 先用 GET /api/backends 获取可用 backend_id
BACKEND_ID=$(curl -s http://localhost:7770/api/backends | jq -r '.items[0].id')
curl -s -X POST http://localhost:7770/api/agents \
  -H "Content-Type: application/json" \
  -d "{\"name\":\"testbot\",\"backend_id\":\"$BACKEND_ID\",\"mentions\":[\"@testbot\"]}" | jq

# 5. Delete the test agent
curl -s -X DELETE http://localhost:7770/api/agents/testbot | jq
```

- [ ] **最终提交（若有未提交内容）**

```bash
git status
git add -A
git commit -m "chore: phase3 write apis complete"
```

---

## 新增 API 总览

| Method | Path | 说明 |
|--------|------|------|
| POST | `/api/chat` | Web chat，无需 IM channel |
| GET | `/api/config/raw` | 读取 config.toml 原始内容 |
| PUT | `/api/config/raw` | 写入并验证 config.toml（需重启生效）|
| POST | `/api/config/validate` | 仅验证 TOML，不写磁盘 |
| POST | `/api/agents` | 追加 `[[agent_roster]]` |
| PATCH | `/api/agents/{name}` | 修改指定 agent 配置 |
| DELETE | `/api/agents/{name}` | 删除指定 agent |
| DELETE | `/api/sessions` | 清空 session 历史 |

**重要提示（前端集成）:**
- 调用 `POST /api/chat` 前，先通过 WS 订阅 `{type:"Subscribe", session_key:{channel:"ws",scope:"main"}}`，再发 POST，然后监听 `AgentEvent` 获取流式回复
- Config/Agent 写操作返回 `restart_required: true` — 前端需引导用户重启 gateway（Tauri 壳可以 `invoke("restart_gateway")`）
