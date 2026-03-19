# DingTalk Webhook Ingress Design

Date: 2026-03-19

## Goal

Add DingTalk webhook receive support to `clawbro` without replacing or destabilizing the existing DingTalk stream-mode channel.

Target outcome:

- `clawbro` supports two DingTalk inbound modes:
  - existing `stream` mode
  - new `webhook` mode
- both modes feed the same downstream message/session/team pipeline
- webhook correctness is enforced by signature verification, deduplication, and fast ACK semantics

## Current State

`clawbro` currently supports DingTalk only through stream mode.

Relevant code:

- `crates/clawbro-server/src/channels_internal/dingtalk.rs`
- `crates/clawbro-server/src/gateway_process.rs`
- `crates/clawbro-server/src/gateway/server.rs`

Current DingTalk path:

1. obtain DingTalk access token
2. open DingTalk stream connection
3. receive WebSocket events
4. map event to `InboundMsg`
5. hand off to `spawn_im_turn(...)`

This path is stable and should remain unchanged.

## What ZeroClaw Actually Does

`zeroclaw` does **not** implement a DingTalk webhook channel.

Observed behavior:

- Lark/Feishu support `websocket` and `webhook`
- DingTalk support is `stream mode`

Relevant references:

- `zeroclaw/src/onboard/wizard.rs`
- `zeroclaw/src/channels/lark.rs`
- `zeroclaw/docs/reference/api/channels-reference.md`

So there is no DingTalk webhook implementation to copy directly from `zeroclaw`.

What is reusable from `zeroclaw` is the webhook architecture pattern:

- dedicated HTTP ingress route
- verify request before dispatch
- deduplicate by external event id
- acknowledge quickly
- translate into internal channel/session message model

## Design Decision

Use a **separate DingTalk webhook channel implementation**.

Do **not** fold webhook handling into the existing `channels_internal/dingtalk.rs` stream module.

Reason:

- stream and webhook are different transport protocols
- mixing them in one module will blur boundaries and raise regression risk
- the downstream pipeline is already shared, so only ingress needs to differ

## Recommended Architecture

### 1. Separate Config Surface

Add a new optional config block instead of overloading the existing DingTalk config.

Proposed shape:

```toml
[channels.dingtalk_webhook]
enabled = true
client_id = "..."
client_secret = "..."
signing_secret = "..."
listen_path = "/channels/dingtalk/webhook"
allowed_users = ["*"]
presentation = "quiet"
```

Notes:

- `client_id` / `client_secret` stay available for proactive replies or follow-up API fetches if required
- `signing_secret` is only for webhook verification
- `listen_path` must be explicit and route-scoped
- this does not replace `[channels.dingtalk]`

### 2. Separate Ingress Module

Add a dedicated module:

- `crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs`

Responsibilities:

- verify DingTalk webhook request
- parse raw payload
- extract sender, conversation, event id, message text, optional bot mention state
- map to `InboundMsg`

Non-responsibilities:

- session orchestration
- team routing
- LLM dispatch
- outbound delivery policy

Those remain in existing shared runtime paths.

### 3. HTTP Route Integration

Extend gateway router with a dedicated webhook endpoint.

Likely location:

- `crates/clawbro-server/src/gateway/server.rs`

Proposed route:

- `POST /channels/dingtalk/webhook`

Handler flow:

1. read raw body
2. verify required headers/signature/timestamp
3. handle challenge handshake if DingTalk webhook protocol requires it
4. parse payload
5. deduplicate by external event/message id
6. build `InboundMsg`
7. dispatch asynchronously into existing session pipeline
8. return success quickly

Important boundary:

- webhook handler must never run full model execution inline before acknowledging

### 4. Shared Downstream Message Model

Webhook mode should derive session scope exactly like stream mode.

Proposed mapping:

- group message -> `group:{conversation_id}`
- direct message -> `user:{sender_id}`

This preserves compatibility with:

- session storage
- allowlists
- team scopes
- delivery routing

No downstream code should need to know whether a DingTalk message arrived via stream or webhook.

### 5. Reliability Contract

Webhook mode must be state-safe, not best-effort.

Required controls:

- signature verification against the raw body
- optional timestamp freshness check
- deduplication using webhook event id / message id
- malformed payload rejection
- allowlist enforcement before dispatch
- structured logs for:
  - invalid signature
  - stale request
  - unsupported event type
  - duplicate event

`zeroclaw` already follows these principles in its webhook-oriented channels and gateway handlers. That is the pattern to reuse.

### 6. Challenge / Verification Handling

If DingTalk webhook setup requires a URL verification handshake, support it as a first-class branch in the handler.

That logic must:

- run before normal message parsing
- produce the exact response shape required by DingTalk
- not create `InboundMsg`

This is similar in spirit to `zeroclaw`'s Feishu/Lark webhook verification flow, though the DingTalk payload format will differ.

## Testing Requirements

Minimum test coverage:

1. valid webhook request is accepted and converted into `InboundMsg`
2. invalid signature is rejected
3. stale timestamp is rejected
4. duplicate webhook event is ignored
5. challenge verification is answered correctly
6. group/private payloads derive correct `SessionKey.scope`
7. allowlist rejection works before dispatch

## Documentation Plan

Document DingTalk as two independent receive modes:

- DingTalk Stream Mode
- DingTalk Webhook Mode

The docs should make these differences explicit:

- stream mode requires no public inbound HTTP port
- webhook mode requires public HTTPS exposure
- stream mode keeps a long-lived connection
- webhook mode relies on signed inbound callbacks

Do not expose webhook fields in `clawbro setup` in phase 1.

Reason:

- the protocol and required fields may still change during implementation
- handwritten config is safer for the first supported version

## Recommended Implementation Order

1. add config schema for `channels.dingtalk_webhook`
2. add `dingtalk_webhook.rs`
3. add gateway route and handler wiring
4. map webhook payload into `InboundMsg`
5. add signature verification and deduplication
6. add tests
7. update docs
8. only then consider `setup` / `doctor` support

## Non-Goals

This design does not include:

- replacing DingTalk stream mode
- refactoring all webhook channels into a generic webhook framework
- changing downstream team/session semantics

## Final Recommendation

Implement DingTalk webhook as a **new ingress channel**, not as a transport branch inside the existing DingTalk stream implementation.

This keeps the architecture clean:

- `dingtalk` = stream ingress
- `dingtalk_webhook` = webhook ingress
- both share the same downstream runtime and team pipeline

That is the cleanest way to add webhook support without destabilizing the current production path.
