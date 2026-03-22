# Agent Social Awareness Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the key capability gaps between clawBro and Slock.ai by giving native-runtime agents the ability to query the agent roster and send messages to other agents.

**Architecture:** Add a `CLAWBRO_GATEWAY_API_URL` env var (mirroring `CLAWBRO_TEAM_TOOL_URL` pattern) injected into native-runtime subprocesses; add two rig tools (`list_roster`, `send_to_agent`) that call the gateway's existing REST API; add a new REST endpoint `POST /api/agents/{name}/message` for external dispatch; wire the new augmentor into the `native_runtime.rs` chain.

**Tech Stack:** Rust, rig-core (Tool trait), reqwest, axum, existing `WsVirtualChannel` + `spawn_im_turn` pattern.

---

## Gap Analysis vs Slock.ai

| Capability | Slock.ai | clawBro (current) | Status |
|---|---|---|---|
| Per-agent persistent memory | ✅ MEMORY.md + notes/ | ✅ Full memory system | ✅ |
| Agent @mention routing | ✅ `agent:deliver` | ✅ MentionTrigger + roster | ✅ |
| Multi-agent team mode | ✅ chat-room style | ✅ Lead+Specialist orchestration | ✅ |
| Agent-to-agent sync delegation | ✅ send_message DM | ✅ RELAY engine | ✅ |
| Async agent-to-agent dispatch | ✅ channel fan-out | ✅ Team task orchestrator | ✅ |
| Skills/persona | ❌ | ✅ Full skills + persona | ✅ |
| Scheduler/cron | ❌ | ✅ Full cron system | ✅ |
| REST management API | ❌ | ✅ Phase 3 API | ✅ |
| WebSocket real-time events | ✅ daemon WS | ✅ Full WS protocol | ✅ |
| **`list_server` / roster query tool** | ✅ MCP tool | ❌ Missing | ❌ **This plan** |
| **Agent-initiated async messaging** | ✅ send_message | ❌ Missing | ❌ **This plan** |
| Group fan-out to ALL agents | ✅ all subscribed agents | ❌ Only front_bot | ❌ Future |
| Agent hibernation/wake | ✅ kill process, keep workspace | ❌ Always-on | ❌ Future |
| Runtime auto-detection | ✅ claude/codex/gemini | ❌ Static config | ❌ Future |

**In scope:** Tasks 1–5 (roster tool, send_to_agent tool, REST endpoint, env var injection, wire-up).
**Out of scope:** Group fan-out, hibernation/wake, runtime auto-detection.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `crates/clawbro-server/src/embedded_agent/roster_tools.rs` | **Create** | `ClawBroRosterToolAugmentor`, `ListRosterTool`, `SendToAgentTool` |
| `crates/clawbro-server/src/embedded_agent/mod.rs` | **Modify** | `pub mod roster_tools;` |
| `crates/clawbro-server/src/embedded_agent/native_runtime.rs` | **Modify** | Chain `ClawBroRosterToolAugmentor` after existing augmentors |
| `crates/clawbro-server/src/runtime/contract.rs` | **Modify** | Add `gateway_api_url: Option<String>` to `RuntimeSessionSpec` |
| `crates/clawbro-server/src/runtime/native/session_driver.rs` | **Modify** | Pass `gateway_api_url` to `spawn_command`; set env var |
| `crates/clawbro-server/src/agent_core/registry.rs` | **Modify** | Add `gateway_api_url: OnceLock<String>`; `set_gateway_api_url` |
| `crates/clawbro-server/src/gateway/api/agent_message.rs` | **Create** | `POST /api/agents/{name}/message` handler |
| `crates/clawbro-server/src/gateway/api/mod.rs` | **Modify** | `pub mod agent_message;` |
| `crates/clawbro-server/src/gateway/server.rs` | **Modify** | Register new route |
| `crates/clawbro-server/src/gateway_process.rs` | **Modify** | Call `registry.set_gateway_api_url(...)` after binding port |

---

## Chunk 1: `POST /api/agents/{name}/message` REST Endpoint

### Task 1: Create `agent_message.rs` handler

**Files:**
- Create: `crates/clawbro-server/src/gateway/api/agent_message.rs`
- Modify: `crates/clawbro-server/src/gateway/server.rs` (add route)

**Background:** This endpoint allows external systems (and later, native tools) to dispatch a turn to a specific agent by name, routing through the same `spawn_im_turn` + `WsVirtualChannel` path as `POST /api/chat`. The `InboundMsg.target_agent` field ensures the session registry routes to the named agent.

**Reference files:**
- `src/gateway/api/chat.rs` — exact same pattern (copy, adapt)
- `src/agent_core/roster.rs` — `AgentRoster::find_by_name()`
- `src/gateway/server.rs` — route registration patterns

- [ ] **Step 1: Write the failing test**

In `agent_message.rs` at the bottom, write a unit test for the scope and target_agent derivation logic (pure functions, no axum needed):

```rust
#[cfg(test)]
mod tests {
    fn resolve_scope(scope: Option<&str>) -> String {
        scope
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("main")
            .to_string()
    }

    #[test]
    fn scope_defaults_to_main() {
        assert_eq!(resolve_scope(None), "main");
    }

    #[test]
    fn explicit_scope_passes_through() {
        assert_eq!(resolve_scope(Some(" group:abc ")), "group:abc");
    }

    #[test]
    fn empty_scope_falls_back_to_main() {
        assert_eq!(resolve_scope(Some("   ")), "main");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd /Users/fishers/Desktop/repo/quickai-openclaw/clawBro
cargo test -p clawbro --lib agent_message 2>&1 | tail -20
```
Expected: FAIL with "file not found" or "module not found"

- [ ] **Step 3: Create `agent_message.rs`**

```rust
use crate::{
    channels_internal::ws_virtual::WsVirtualChannel,
    config::ProgressPresentationMode,
    gateway::api::types::ApiErrorBody,
    im_sink::spawn_im_turn,
    protocol::{InboundMsg, MsgContent, MsgSource, SessionKey},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct AgentMessageBody {
    pub message: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub sender: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentMessageResponse {
    pub ok: bool,
    pub turn_id: String,
    pub session_key: SessionKey,
}

pub async fn send_agent_message(
    Path(name): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<AgentMessageBody>,
) -> Result<Json<AgentMessageResponse>, (StatusCode, Json<ApiErrorBody>)> {
    // Validate: agent must exist in roster
    let roster = state.registry.roster.as_ref().ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody {
                error: "agent roster not configured".to_string(),
            }),
        )
    })?;
    roster.find_by_name(&name).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody {
                error: format!("agent '{}' not found", name),
            }),
        )
    })?;

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
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("main")
        .to_string();

    let session_key = SessionKey::new("ws", &scope);
    let turn_id = uuid::Uuid::new_v4().to_string();
    let sender = body
        .sender
        .as_deref()
        .unwrap_or("system")
        .to_string();

    let inbound = InboundMsg {
        id: turn_id.clone(),
        session_key: session_key.clone(),
        content: MsgContent::text(&message),
        sender,
        channel: "ws".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: Some(name),
        source: MsgSource::Human,
    };

    spawn_im_turn(
        state.registry.clone(),
        Arc::new(WsVirtualChannel),
        state.channel_registry.clone(),
        state.cfg.clone(),
        inbound,
        ProgressPresentationMode::FinalOnly,
    );

    Ok(Json(AgentMessageResponse {
        ok: true,
        turn_id,
        session_key,
    }))
}

#[cfg(test)]
mod tests {
    fn resolve_scope(scope: Option<&str>) -> String {
        scope
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("main")
            .to_string()
    }

    #[test]
    fn scope_defaults_to_main() {
        assert_eq!(resolve_scope(None), "main");
    }

    #[test]
    fn explicit_scope_passes_through() {
        assert_eq!(resolve_scope(Some(" group:abc ")), "group:abc");
    }

    #[test]
    fn empty_scope_falls_back_to_main() {
        assert_eq!(resolve_scope(Some("   ")), "main");
    }
}
```

- [ ] **Step 4: Register module in `gateway/api/mod.rs`**

Check if there's a `mod.rs` in `gateway/api/`; if not, find where modules are declared.

```bash
ls /Users/fishers/Desktop/repo/quickai-openclaw/clawBro/crates/clawbro-server/src/gateway/api/
```

Add `pub mod agent_message;` to the appropriate `mod.rs` or inline in `gateway/mod.rs`.

- [ ] **Step 5: Add route to `gateway/server.rs`**

Find the existing agents routes block and add alongside:

```rust
// In the router setup, alongside existing agent routes:
.route(
    "/api/agents/:name/message",
    post(api::agent_message::send_agent_message),
)
```

Note: check whether other agent routes use `{name}` (Axum 0.7) or `:name` (Axum 0.6). Match the existing pattern.

- [ ] **Step 6: Run tests**

```bash
cd /Users/fishers/Desktop/repo/quickai-openclaw/clawBro
cargo test -p clawbro --lib agent_message 2>&1 | tail -20
```
Expected: 3 passed

- [ ] **Step 7: Run full test suite**

```bash
cargo test -p clawbro --lib 2>&1 | tail -10
```
Expected: all pass, no failures

- [ ] **Step 8: Commit**

```bash
git add crates/clawbro-server/src/gateway/api/agent_message.rs \
        crates/clawbro-server/src/gateway/server.rs \
        crates/clawbro-server/src/gateway/api/
git commit -m "feat: add POST /api/agents/{name}/message for external agent dispatch"
```

---

## Chunk 2: Gateway API URL injection for native subprocess

### Task 2: `gateway_api_url` plumbing (RuntimeSessionSpec → env var)

**Files:**
- Modify: `crates/clawbro-server/src/runtime/contract.rs`
- Modify: `crates/clawbro-server/src/runtime/native/session_driver.rs`
- Modify: `crates/clawbro-server/src/agent_core/registry.rs`
- Modify: `crates/clawbro-server/src/gateway_process.rs`

**Background:** Exactly mirrors the `CLAWBRO_TEAM_TOOL_URL` / `team_tool_url` pattern. The registry stores the URL in a `OnceLock<String>`, includes it in `RuntimeSessionSpec`, and the session driver sets it as `CLAWBRO_GATEWAY_API_URL` env var for the subprocess. The gateway calls `registry.set_gateway_api_url()` after binding the port.

**Reference files:**
- `src/runtime/contract.rs:154` — `team_tool_url: Option<String>` (copy pattern)
- `src/agent_core/registry.rs:398,709-714,1215` — OnceLock + setter + fill into spec
- `src/runtime/native/session_driver.rs:151-183` — `spawn_command` sets env vars

- [ ] **Step 1: Write failing test in `session_driver.rs`**

At the bottom of `session_driver.rs` tests, add:

```rust
#[test]
fn spawn_command_sets_gateway_api_url_when_present() {
    // Just test that NativeCommandConfig with env includes the var.
    // We can't easily test the actual cmd.env call without spawning,
    // so test the NativeCommandConfig construction instead.
    let config = NativeCommandConfig {
        command: "echo".to_string(),
        args: vec![],
        env: vec![("CLAWBRO_GATEWAY_API_URL".to_string(), "http://localhost:7770".to_string())],
    };
    assert!(config.env.iter().any(|(k, _)| k == "CLAWBRO_GATEWAY_API_URL"));
}
```

- [ ] **Step 2: Run test to verify current state**

```bash
cargo test -p clawbro --lib session_driver 2>&1 | tail -20
```
Expected: the new test either passes trivially (since it tests NativeCommandConfig directly) or compilation fails due to missing fields.

- [ ] **Step 3: Add `gateway_api_url` to `RuntimeSessionSpec` in `contract.rs`**

Find the `RuntimeSessionSpec` struct (line ~137) and add the field after `team_tool_url`:

```rust
/// Gateway REST API base URL for native tools (e.g., list_roster, send_to_agent).
/// Injected as CLAWBRO_GATEWAY_API_URL env var in the subprocess.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub gateway_api_url: Option<String>,
```

- [ ] **Step 4: Update `to_agent_turn_request()` in `contract.rs`**

The `to_agent_turn_request()` method (line ~167) builds an `AgentTurnRequest`. `gateway_api_url` is NOT part of `AgentTurnRequest` (it's an env var, not JSON protocol). No change needed here — the env var approach means the subprocess reads it from the environment, not from the turn request.

- [ ] **Step 5: Update `spawn_command` in `session_driver.rs`**

Add `gateway_api_url: Option<&str>` parameter to `spawn_command` and `run_command_turn`:

```rust
fn spawn_command(
    config: &NativeCommandConfig,
    workspace_dir: Option<&std::path::Path>,
    team_tool_url: Option<&str>,
    gateway_api_url: Option<&str>,   // NEW
    session_ref: Option<&str>,
) -> anyhow::Result<tokio::process::Child> {
    // ... existing setup ...
    if let Some(url) = team_tool_url {
        cmd.env("CLAWBRO_TEAM_TOOL_URL", url);
    }
    if let Some(url) = gateway_api_url {           // NEW
        cmd.env("CLAWBRO_GATEWAY_API_URL", url);   // NEW
    }                                              // NEW
    // ... rest of existing setup ...
}
```

Update `run_command_turn` to pass `session.gateway_api_url.as_deref()` as the new argument:

```rust
pub async fn run_command_turn(
    config: &NativeCommandConfig,
    session: RuntimeSessionSpec,
    sink: RuntimeEventSink,
) -> anyhow::Result<TurnResult> {
    // ... existing ...
    let mut child = spawn_command(
        config,
        session.workspace_dir.as_deref(),
        session.team_tool_url.as_deref(),
        session.gateway_api_url.as_deref(),  // NEW
        Some(&render_session_key_text(&session.session_key)),
    )?;
    // ... rest unchanged ...
}
```

- [ ] **Step 6: Add `gateway_api_url` to `SessionRegistry` in `registry.rs`**

Find where `team_tool_url: OnceLock<String>` is declared (line ~398) and add alongside:

```rust
gateway_api_url: OnceLock<String>,
```

In the constructor (`SessionRegistry::new` or equivalent), add:
```rust
gateway_api_url: OnceLock::new(),
```

Add setter and accessor methods near the `team_tool_url` setters (~line 709):

```rust
pub fn set_gateway_api_url(&self, url: String) {
    let _ = self.gateway_api_url.set(url);
}

pub fn gateway_api_url(&self) -> Option<&str> {
    self.gateway_api_url.get().map(String::as_str)
}
```

Find where `RuntimeSessionSpec` is built (line ~1215, where `team_tool_url` is set) and add:

```rust
gateway_api_url: self.gateway_api_url.get().cloned(),
```

- [ ] **Step 7: Call `set_gateway_api_url` in `gateway_process.rs`**

After the server binds to its address and port (look for where `team_tool_url` is set via `registry.set_team_tool_url`), add:

```rust
// Derive base URL from the listen address (same pattern as team_tool_url)
let gateway_api_url = format!("http://{listen_addr}");
registry.set_gateway_api_url(gateway_api_url);
```

If there's no single `listen_addr` variable, use the configured port from `cfg.gateway.port` (default 7770):

```rust
let port = cfg.gateway.port.unwrap_or(7770);
registry.set_gateway_api_url(format!("http://127.0.0.1:{port}"));
```

- [ ] **Step 8: Run tests to confirm no regressions**

```bash
cargo test -p clawbro --lib 2>&1 | tail -10
```
Expected: all pass. Fix any compilation errors from the new field (add `Default` or `#[serde(default)]` as needed).

- [ ] **Step 9: Commit**

```bash
git add crates/clawbro-server/src/runtime/contract.rs \
        crates/clawbro-server/src/runtime/native/session_driver.rs \
        crates/clawbro-server/src/agent_core/registry.rs \
        crates/clawbro-server/src/gateway_process.rs
git commit -m "feat: inject CLAWBRO_GATEWAY_API_URL into native subprocess env"
```

---

## Chunk 3: `list_roster` and `send_to_agent` native rig tools

### Task 3: Create `embedded_agent/roster_tools.rs`

**Files:**
- Create: `crates/clawbro-server/src/embedded_agent/roster_tools.rs`
- Modify: `crates/clawbro-server/src/embedded_agent/mod.rs`
- Modify: `crates/clawbro-server/src/embedded_agent/native_runtime.rs`

**Background:** Follows the `ClawBroTeamToolAugmentor` pattern from `embedded_agent/team.rs` exactly. The augmentor reads `CLAWBRO_GATEWAY_API_URL` from env; if set, it adds two tools: `ListRosterTool` (calls `GET {url}/api/agents`) and `SendToAgentTool` (calls `POST {url}/api/agents/{name}/message`). These give native-runtime agents the Slock.ai `list_server` + `send_message(DM:@agent)` capabilities.

**Reference files:**
- `src/embedded_agent/team.rs` — complete pattern reference (TeamToolClient, augmentor, Tool impls)
- `src/gateway/api/agents.rs` — response shape for `GET /api/agents` (items: Vec<AgentApiView>)

- [ ] **Step 1: Write the failing test**

At the bottom of the new `roster_tools.rs` file (write it before the impl so tests drive the design):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn augmentor_from_env_is_noop_when_unset() {
        // Ensure env is not set in this test process
        std::env::remove_var("CLAWBRO_GATEWAY_API_URL");
        let aug = ClawBroRosterToolAugmentor::from_env();
        assert!(aug.base_url.is_none());
    }

    #[test]
    fn augmentor_from_env_picks_up_var() {
        std::env::set_var("CLAWBRO_GATEWAY_API_URL", "http://localhost:7770");
        let aug = ClawBroRosterToolAugmentor::from_env();
        assert_eq!(aug.base_url.as_deref(), Some("http://localhost:7770"));
        std::env::remove_var("CLAWBRO_GATEWAY_API_URL");
    }

    #[tokio::test]
    async fn list_roster_tool_definition_has_correct_name() {
        let tool = ListRosterTool {
            base_url: "http://localhost:7770".to_string(),
            client: reqwest::Client::new(),
        };
        let def = rig::tool::Tool::definition(&tool, String::new()).await;
        assert_eq!(def.name, "list_roster");
    }

    #[tokio::test]
    async fn send_to_agent_tool_definition_has_correct_name() {
        let tool = SendToAgentTool {
            base_url: "http://localhost:7770".to_string(),
            client: reqwest::Client::new(),
        };
        let def = rig::tool::Tool::definition(&tool, String::new()).await;
        assert_eq!(def.name, "send_to_agent");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p clawbro --lib roster_tools 2>&1 | tail -20
```
Expected: compile error (module does not exist)

- [ ] **Step 3: Create `roster_tools.rs`**

```rust
//! ClawBroRosterToolAugmentor — adds list_roster and send_to_agent tools to native runtime.
//!
//! Reads CLAWBRO_GATEWAY_API_URL from the subprocess environment (injected by session_driver.rs).
//! If set, augments the agent builder with two tools that call the gateway's REST API:
//!   - list_roster: GET /api/agents → returns roster summary
//!   - send_to_agent: POST /api/agents/{name}/message → dispatches a turn to another agent

use crate::agent_sdk_internal::{
    bridge::{AgentTurnRequest, ApprovalMode},
    tools::{ConfiguredAgentBuilder, EventedTool, RuntimeToolAugmentor, ToolProgressTracker},
};
use rig::{
    completion::{CompletionModel, ToolDefinition},
    tool::{Tool, ToolError},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

// ─── HTTP client ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct GatewayClient {
    base_url: String,
    client: reqwest::Client,
}

impl GatewayClient {
    fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: reqwest::Client::new(),
        }
    }

    async fn get_agents(&self) -> Result<String, ToolError> {
        let url = format!("{}/api/agents", self.base_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ToolError::ToolCallError(format!("roster request failed: {e}").into()))?;

        if !resp.status().is_success() {
            return Err(ToolError::ToolCallError(
                format!("roster request failed with status {}", resp.status()).into(),
            ));
        }

        let text = resp
            .text()
            .await
            .map_err(|e| ToolError::ToolCallError(format!("roster decode failed: {e}").into()))?;
        Ok(text)
    }

    async fn send_to_agent(
        &self,
        agent_name: &str,
        message: &str,
        scope: Option<&str>,
    ) -> Result<String, ToolError> {
        let url = format!("{}/api/agents/{}/message", self.base_url, agent_name);
        let body = json!({
            "message": message,
            "scope": scope,
            "sender": "agent",
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ToolError::ToolCallError(format!("send_to_agent request failed: {e}").into()))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ToolError::ToolCallError(format!("send_to_agent decode failed: {e}").into()))?;

        if !status.is_success() {
            return Err(ToolError::ToolCallError(
                format!("send_to_agent failed ({}): {}", status, text).into(),
            ));
        }
        Ok(text)
    }
}

// ─── ListRosterTool ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ListRosterTool {
    pub base_url: String,
    pub client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
pub struct ListRosterArgs {}

#[derive(Debug, Serialize)]
pub struct ListRosterOutput {
    pub roster_json: String,
}

impl Tool for ListRosterTool {
    const NAME: &'static str = "list_roster";
    type Error = ToolError;
    type Args = ListRosterArgs;
    type Output = ListRosterOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "List all agents currently configured in the gateway roster. Returns a JSON array of agents with their names, backend IDs, roles, and mentions.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let gc = GatewayClient {
            base_url: self.base_url.clone(),
            client: self.client.clone(),
        };
        let roster_json = gc.get_agents().await?;
        Ok(ListRosterOutput { roster_json })
    }
}

// ─── SendToAgentTool ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SendToAgentTool {
    pub base_url: String,
    pub client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
pub struct SendToAgentArgs {
    /// Name of the target agent (must match an entry in the roster).
    pub agent_name: String,
    /// Message to send to the agent.
    pub message: String,
    /// Optional session scope (defaults to "main").
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SendToAgentOutput {
    pub ok: bool,
    pub response_json: String,
}

impl Tool for SendToAgentTool {
    const NAME: &'static str = "send_to_agent";
    type Error = ToolError;
    type Args = SendToAgentArgs;
    type Output = SendToAgentOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Send a message to another agent by name. The message is dispatched asynchronously — the target agent will process it in its own session. Use list_roster first to discover available agents.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "agent_name": {
                        "type": "string",
                        "description": "Name of the target agent (e.g. 'codex', 'gemini')"
                    },
                    "message": {
                        "type": "string",
                        "description": "Message to send to the agent"
                    },
                    "scope": {
                        "type": "string",
                        "description": "Optional session scope, defaults to 'main'"
                    }
                },
                "required": ["agent_name", "message"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let gc = GatewayClient {
            base_url: self.base_url.clone(),
            client: self.client.clone(),
        };
        let response_json = gc
            .send_to_agent(
                &args.agent_name,
                &args.message,
                args.scope.as_deref(),
            )
            .await?;
        Ok(SendToAgentOutput {
            ok: true,
            response_json,
        })
    }
}

// ─── Augmentor ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ClawBroRosterToolAugmentor {
    pub base_url: Option<String>,
}

impl ClawBroRosterToolAugmentor {
    pub fn from_env() -> Self {
        Self {
            base_url: std::env::var("CLAWBRO_GATEWAY_API_URL").ok(),
        }
    }
}

impl RuntimeToolAugmentor for ClawBroRosterToolAugmentor {
    fn augment<M: CompletionModel>(
        &self,
        builder: ConfiguredAgentBuilder<M>,
        _session: &AgentTurnRequest,
        tracker: Option<ToolProgressTracker>,
        approval_mode: ApprovalMode,
    ) -> ConfiguredAgentBuilder<M> {
        let Some(base_url) = &self.base_url else {
            return builder;
        };
        let client = reqwest::Client::new();
        builder
            .tool(EventedTool::new(
                ListRosterTool {
                    base_url: base_url.clone(),
                    client: client.clone(),
                },
                tracker.clone(),
                approval_mode,
            ))
            .tool(EventedTool::new(
                SendToAgentTool {
                    base_url: base_url.clone(),
                    client,
                },
                tracker,
                approval_mode,
            ))
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn augmentor_from_env_is_noop_when_unset() {
        std::env::remove_var("CLAWBRO_GATEWAY_API_URL");
        let aug = ClawBroRosterToolAugmentor::from_env();
        assert!(aug.base_url.is_none());
    }

    #[test]
    fn augmentor_from_env_picks_up_var() {
        std::env::set_var("CLAWBRO_GATEWAY_API_URL", "http://localhost:7770");
        let aug = ClawBroRosterToolAugmentor::from_env();
        assert_eq!(aug.base_url.as_deref(), Some("http://localhost:7770"));
        std::env::remove_var("CLAWBRO_GATEWAY_API_URL");
    }

    #[tokio::test]
    async fn list_roster_tool_definition_has_correct_name() {
        let tool = ListRosterTool {
            base_url: "http://localhost:7770".to_string(),
            client: reqwest::Client::new(),
        };
        let def = rig::tool::Tool::definition(&tool, String::new()).await;
        assert_eq!(def.name, "list_roster");
    }

    #[tokio::test]
    async fn send_to_agent_tool_definition_has_correct_name() {
        let tool = SendToAgentTool {
            base_url: "http://localhost:7770".to_string(),
            client: reqwest::Client::new(),
        };
        let def = rig::tool::Tool::definition(&tool, String::new()).await;
        assert_eq!(def.name, "send_to_agent");
    }

    #[test]
    fn augmentor_has_no_base_url_by_default() {
        let aug = ClawBroRosterToolAugmentor::default();
        assert!(aug.base_url.is_none());
    }
}
```

- [ ] **Step 4: Register module in `embedded_agent/mod.rs`**

Add `pub mod roster_tools;` alongside the existing module declarations.

- [ ] **Step 5: Run tests**

```bash
cargo test -p clawbro --lib roster_tools 2>&1 | tail -20
```
Expected: 5 tests pass

- [ ] **Step 6: Wire `ClawBroRosterToolAugmentor` into `native_runtime.rs`**

Open `embedded_agent/native_runtime.rs`. The current chain is:

```rust
let team_tools = ClawBroTeamToolAugmentor::from_env();
let schedule_tools = ClawBroScheduleToolAugmentor::from_env();
let augmentor = ChainedAugmentor::new(team_tools, schedule_tools);
```

Add the roster augmentor to the chain:

```rust
use crate::embedded_agent::roster_tools::ClawBroRosterToolAugmentor;

// In run_stdio_bridge():
let team_tools = ClawBroTeamToolAugmentor::from_env();
let schedule_tools = ClawBroScheduleToolAugmentor::from_env();
let roster_tools = ClawBroRosterToolAugmentor::from_env();
let augmentor = ChainedAugmentor::new(ChainedAugmentor::new(team_tools, schedule_tools), roster_tools);
```

- [ ] **Step 7: Run full test suite**

```bash
cargo test -p clawbro --lib 2>&1 | tail -10
```
Expected: all pass

- [ ] **Step 8: Commit**

```bash
git add crates/clawbro-server/src/embedded_agent/roster_tools.rs \
        crates/clawbro-server/src/embedded_agent/mod.rs \
        crates/clawbro-server/src/embedded_agent/native_runtime.rs
git commit -m "feat: add list_roster and send_to_agent native rig tools (Slock.ai parity)"
```

---

## Chunk 4: Update API reference documentation

### Task 4: Document new endpoint in `docs/api-reference.md`

**Files:**
- Modify: `docs/api-reference.md`

**Background:** The `POST /api/agents/{name}/message` endpoint is a new public API. It must appear in the reference doc alongside the other agent management endpoints.

- [ ] **Step 1: Find the agent management section**

In `docs/api-reference.md`, find the `## Agent 管理` section (around `### POST /api/agents`).

- [ ] **Step 2: Add the new endpoint after `DELETE /api/agents/{name}`**

Add this section before the Skills section:

```markdown
### POST /api/agents/{name}/message

向指定 agent 发送消息，触发一次异步 agent turn。Turn 结果通过 WebSocket `AgentEvent` 流广播。

**路径参数:**

| 参数 | 类型 | 说明 |
|------|------|------|
| `name` | string | Agent 名称（必须存在于 roster） |

**请求体:**

```json
{
  "message": "请帮我分析 main.rs 的代码结构",
  "scope": "main",
  "sender": "orchestrator"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `message` | string | ✅ | 消息内容，不可为空 |
| `scope` | string | ❌ | Session 范围，默认 `"main"` |
| `sender` | string | ❌ | 发送方标识，默认 `"system"` |

**响应:**

```json
{
  "ok": true,
  "turn_id": "550e8400-e29b-41d4-a716-446655440000",
  "session_key": { "channel": "ws", "scope": "main" }
}
```

**错误:**

| 状态码 | 含义 |
|--------|------|
| `400` | message 为空 |
| `404` | roster 未配置或 agent 不存在 |

**用途:** 供其他 agent 通过 `send_to_agent` native tool 调用，或外部系统主动触发特定 agent 执行任务。
```

- [ ] **Step 3: Add the two native tools to the doc**

Find or create a section in the docs about native runtime tools (near the Skills section or at the bottom) and add:

```markdown
## Native Runtime 内置工具

当 gateway 注入 `CLAWBRO_GATEWAY_API_URL` 时（native backend 自动生效），native runtime agent 可使用以下额外工具：

### `list_roster`

列出 gateway 当前配置的所有 agent。

**参数:** 无

**返回:** JSON 字符串，包含 `items` 数组，每项含 `name`、`backend_id`、`role`、`mentions`、`identities`。

**用途:** 类似 Slock.ai 的 `list_server` — 让 agent 发现同伴，再通过 `send_to_agent` 委托任务。

### `send_to_agent`

向另一个 agent 发送消息，触发该 agent 的异步 turn。

**参数:**

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `agent_name` | string | ✅ | 目标 agent 名称 |
| `message` | string | ✅ | 消息内容 |
| `scope` | string | ❌ | Session 范围，默认 `"main"` |

**返回:** `{ ok: true, response_json: "..." }` — response_json 是 `/api/agents/{name}/message` 的原始 JSON 响应。

**用途:** 类似 Slock.ai 的 `send_message(channel="DM:@agent")` — 让 agent A 主动触发 agent B 执行子任务，实现异步的 agent-to-agent 协作。
```

- [ ] **Step 4: Commit**

```bash
git add docs/api-reference.md
git commit -m "docs: document POST /api/agents/{name}/message and native roster tools"
```

---

## Verification

After all tasks are committed, run the full test suite:

```bash
cd /Users/fishers/Desktop/repo/quickai-openclaw/clawBro
cargo test -p clawbro --lib 2>&1 | tail -10
```

Expected output: all existing tests pass + new tests from Tasks 1 and 3.

**End-to-end smoke test (manual):**

1. Start the gateway: `cargo run -p clawbro -- serve`
2. Check `GET /api/agents` returns the roster
3. Send: `POST /api/agents/my-agent/message {"message": "hello"}`
4. Observe WS events for the agent's reply
5. In a native agent turn, call `list_roster` tool → should return roster JSON
6. Call `send_to_agent(agent_name="codex", message="verify JWT")` → check WS for codex's turn events

---

## Out of Scope (Future Plans)

- **Group fan-out**: When a group message arrives, broadcast to ALL agents in roster (not just front_bot). Requires changes to `agent_core/registry.rs` message routing and group config.
- **Agent hibernation/wake**: Track agent idle time, kill subprocess when idle, restart on new message. Requires `BackendAdapter` lifecycle hooks and session metadata.
- **Runtime auto-detection**: Probe for `claude`, `codex`, `gemini` binaries at startup and auto-configure backends. Requires a detection service and dynamic backend registration.
- **Built-in MCP SSE server**: Expose `list_roster` and `send_to_agent` as an MCP server endpoint at `/mcp/sse` so external backends (claude-code, codex via `codex mcp`) can use these tools without native runtime.
