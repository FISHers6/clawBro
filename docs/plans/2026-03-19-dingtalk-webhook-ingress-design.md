# DingTalk Custom Robot Webhook Ingress Design

Date: 2026-03-19

## Goal

Add DingTalk **custom robot group webhook** support to `clawbro` without replacing or destabilizing the existing DingTalk stream-mode channel.

Target outcome:

- `clawbro` supports two DingTalk inbound modes:
  - existing `stream` mode
  - new `custom robot webhook` mode
- both modes feed the same downstream message/session/team pipeline
- webhook correctness is enforced by strict verification, deduplication, and fast ACK semantics
- long-term functionality approaches the useful feature surface of `openclaw-channel-dingtalk-bot`, but with cleaner runtime boundaries

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

## What The Reference Plugin Does

`openclaw-channel-dingtalk-bot` implements a DingTalk **custom robot** webhook channel.

Relevant references:

- `openclaw-channel-dingtalk-bot/README.zh-CN.md`
- `openclaw-channel-dingtalk-bot/src/channel.ts`
- `openclaw-channel-dingtalk-bot/src/sign.ts`

Useful reference behaviors:

- group-only custom robot webhook ingress
- `sessionWebhook` reply path
- DingTalk HMAC-SHA256 outbound signing
- group @ mention filtering
- `text` and `richText` inbound parsing
- optional active outbound through `robot/send`
- optional image download through `robot/messageFiles/download`

But it should **not** be copied directly into `clawbro` as-is.

Observed weaknesses in the reference plugin:

- inbound verification uses `secretKey.startsWith(token)` instead of strict equality
- webhook handler does heavy work before ACK
- no durable deduplication layer
- no explicit replay/freshness guard
- ingress parsing, runtime dispatch, outbound reply, and media download are tightly coupled in one module

`clawbro` should aim for functional parity, but with stricter correctness and clearer boundaries.

## Design Decision

Use a **separate DingTalk custom robot webhook channel implementation**.

Do **not** fold webhook handling into the existing `channels_internal/dingtalk.rs` stream module.

Reason:

- stream and webhook are different transport protocols
- the custom robot product is group-centric and semantically different from app/stream mode
- mixing them in one module will blur boundaries and raise regression risk
- the downstream pipeline is already shared, so only ingress needs to differ

## Functional Target

`clawbro` should ultimately support most of the practical feature surface of `openclaw-channel-dingtalk-bot`, but with stricter reliability boundaries.

Target parity areas:

- custom robot webhook ingress
- group-only @ mention filtering
- text inbound parsing
- richText inbound parsing
- `sessionWebhook` reply delivery with DingTalk HMAC signing
- optional active outbound via `robot/send + access_token`
- optional image download via `robot/messageFiles/download`

Areas that must be implemented more strictly than the reference plugin:

- no `startsWith` token verification
- no heavy inline processing before ACK
- durable deduplication
- explicit session webhook expiry handling
- clearer separation between ingress, parsing, dispatch, and outbound reply code

## Recommended Architecture

### 1. Separate Config Surface

Add a new optional config block instead of overloading the existing DingTalk stream config.

Proposed shape:

```toml
[channels.dingtalk_webhook]
enabled = true
secret_key = "SEC..."
webhook_path = "/channels/dingtalk/webhook"
access_token = "..."
allowed_users = ["*"]
presentation = "quiet"
```

Notes:

- `secret_key` is the custom robot secret and is used for:
  - inbound request verification
  - outbound DingTalk HMAC signing
- `access_token` is optional in phase 1 and reserved for:
  - richText image download
  - proactive robot/send delivery
- `webhook_path` must be explicit and route-scoped
- this does not replace `[channels.dingtalk]`

### 2. Separate Ingress Module

Add a dedicated module:

- `crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs`

Responsibilities:

- verify DingTalk custom robot webhook request
- parse raw payload
- extract sender, conversation, event id, message text, bot mention state, and `sessionWebhook`
- map to `InboundMsg`
- attach webhook-specific reply metadata needed for later outbound reply

Non-responsibilities:

- session orchestration
- team routing
- LLM dispatch
- outbound delivery policy beyond storing necessary metadata
- richText media download in phase 1

Those remain in existing shared runtime paths.

### 3. HTTP Route Integration

Extend gateway router with a dedicated webhook endpoint.

Likely location:

- `crates/clawbro-server/src/gateway/server.rs`

Proposed route:

- `POST /channels/dingtalk/webhook`

Handler flow:

1. read raw body
2. verify required headers/token
3. handle DingTalk verification handshake if the webhook product requires it
4. parse payload
5. deduplicate by external event/message id
6. build `InboundMsg`
7. dispatch asynchronously into existing session pipeline
8. return success quickly

Important boundary:

- webhook handler must never run full model execution inline before acknowledging

### 4. Shared Downstream Message Model

Webhook mode should derive session scope according to the custom robot payload semantics.

Proposed mapping:

- group message -> `group:{conversation_id}`
- direct message -> not supported in phase 1 unless the custom robot product proves it is available and stable

This preserves compatibility with:

- session storage
- allowlists
- team scopes
- delivery routing

No downstream code should need to know whether a DingTalk message arrived via stream or custom robot webhook.

### 5. Reply Model

For webhook mode, inbound-triggered replies should prefer `sessionWebhook`.

Phase 1 reply contract:

- use the inbound payload's `sessionWebhook`
- sign outbound requests with DingTalk HMAC-SHA256
- reply only while the session webhook is still valid

Future extension:

- if `sessionWebhookExpiredTime` is expired, optionally fall back to proactive outbound when `access_token` is configured

This mirrors the reference plugin's useful behavior but makes expiry semantics explicit.

### 6. Reliability Contract

Webhook mode must be state-safe, not best-effort.

Required controls:

- strict inbound token equality verification
- optional future replay/freshness controls if DingTalk headers support them
- deduplication using webhook event id / message id
- malformed payload rejection
- allowlist enforcement before dispatch
- structured logs for:
  - invalid token
  - malformed payload
  - unsupported event type
  - duplicate event

`zeroclaw` already follows these principles in its webhook-oriented channels and gateway handlers. That is the pattern to reuse.

### 7. Group @ Mention Filtering

Phase 1 should explicitly match the custom robot group workflow:

- process group messages only when the bot is mentioned
- ignore non-mention group traffic
- do not pretend webhook mode is a general passive listener for all group content

This should be enforced before dispatch, not left to downstream prompt behavior.

## Phase Plan

### Phase 1: Reliable Core

Phase 1 is the recommended initial implementation.

Scope:

- custom robot group webhook ingress
- strict token equality verification
- fast ACK
- deduplication
- group @ filtering
- text inbound parsing
- `sessionWebhook` reply delivery with signing
- shared downstream `InboundMsg` pipeline reuse

Explicit non-goals in phase 1:

- 1:1 direct message support
- proactive outbound by default
- richText image download
- setup wizard integration

### Phase 2: Feature Parity Expansion

After phase 1 is stable in production, expand toward reference-plugin parity.

Scope:

- richText image download via `robot/messageFiles/download`
- optional `access_token` powered proactive outbound
- `sessionWebhookExpiredTime` aware reply fallback behavior
- broader payload compatibility handling
- optional `setup` / `doctor` support

## Testing Requirements

Minimum phase 1 test coverage:

1. valid webhook request is accepted and converted into `InboundMsg`
2. invalid token is rejected
3. duplicate webhook event is ignored
4. challenge verification is answered correctly when applicable
5. group payloads derive correct `SessionKey.scope`
6. non-@ group messages are ignored
7. allowlist rejection works before dispatch
8. `sessionWebhook` replies are signed correctly

Phase 2 adds:

9. richText payload parsing
10. image download fallback behavior
11. `sessionWebhookExpiredTime` handling
12. proactive outbound signing and delivery

## Documentation Plan

Document DingTalk as two independent receive modes:

- DingTalk Stream Mode
- DingTalk Custom Robot Webhook Mode

The docs should make these differences explicit:

- stream mode requires no public inbound HTTP port
- custom robot webhook mode requires public HTTPS exposure
- stream mode keeps a long-lived connection
- webhook mode relies on inbound callbacks plus `sessionWebhook` replies
- custom robot webhook mode is group-oriented and should be documented as such

Do not expose webhook fields in `clawbro setup` in phase 1.

Reason:

- the protocol and required fields may still change during implementation
- handwritten config is safer for the first supported version

## Recommended Implementation Order

1. add config schema for `channels.dingtalk_webhook`
2. add `dingtalk_webhook.rs`
3. add gateway route and handler wiring
4. map webhook payload into `InboundMsg`
5. add strict token verification and deduplication
6. add `sessionWebhook` signed reply support
7. add tests
8. update docs
9. only then consider phase 2 parity features

## Non-Goals

This design does not include:

- replacing DingTalk stream mode
- refactoring all webhook channels into a generic webhook framework
- changing downstream team/session semantics
- promising 1:1 custom robot support in phase 1

## Final Recommendation

Implement DingTalk custom robot webhook as a **new ingress channel**, not as a transport branch inside the existing DingTalk stream implementation.

This keeps the architecture clean:

- `dingtalk` = stream ingress
- `dingtalk_webhook` = custom robot webhook ingress
- both share the same downstream runtime and team pipeline

That is the cleanest way to add webhook support without destabilizing the current production path, while still allowing `clawbro` to grow toward the more complete feature surface of `openclaw-channel-dingtalk-bot`.
