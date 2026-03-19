# DingTalk Custom Robot Webhook Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Rust implementation of DingTalk custom robot group webhook ingress to `clawbro` while keeping the existing DingTalk stream mode unchanged.

**Architecture:** Implement a new `dingtalk_webhook` ingress path as a separate channel module and HTTP route. Reuse the existing downstream `InboundMsg -> spawn_im_turn -> session/team/runtime` pipeline, and keep webhook-specific logic limited to request verification, payload parsing, deduplication, and `sessionWebhook` reply metadata. Phase 1 focuses on reliable group webhook delivery; Phase 2 expands toward the richer feature surface seen in `openclaw-channel-dingtalk-bot`.

**Tech Stack:** Rust, axum, reqwest, serde/serde_json, existing `clawbro` gateway/channel runtime, DingTalk custom robot webhook/sessionWebhook model

---

## Extensibility Rules

This plan should preserve future extension paths without prematurely building a generic webhook framework.

Required extension boundaries:

- keep **transport ingress** separate from **payload parsing**
- keep **payload parsing** separate from **downstream `InboundMsg` mapping**
- keep **reply signing/sending** separate from **ingress verification**
- keep **dedup logic** behind a focused helper interface instead of scattering it across handlers
- keep **allowlist** sourced from one place only
- keep **Channel::send() integration** explicit so webhook ingress still uses the existing IM sink and delivery pipeline cleanly
- keep DingTalk custom robot specifics in dedicated files so later support for:
  - DingTalk app webhook variants
  - more webhook channels
  - richer webhook verification contracts
  can reuse boundaries without forcing shared code too early

Non-goals:

- do not build a cross-channel generic webhook abstraction in phase 1
- do not introduce a second allowlist source in channel config in phase 1

The design should be extensible by **clean seams**, not by speculative framework code.

---

## File Structure

### Existing files to modify

- `crates/clawbro-server/src/config.rs`
  - Add `channels.dingtalk_webhook` config schema and validation.
- `crates/clawbro-server/src/gateway/server.rs`
  - Register the DingTalk webhook HTTP route.
- `crates/clawbro-server/src/gateway_process.rs`
  - Wire config-driven webhook ingress startup into the main runtime.
- `crates/clawbro-server/src/channels_internal/mod.rs`
  - Export the new webhook channel module.
- `docs/setup.md`
  - Add handwritten config instructions after phase 1 is complete.

### New files to create

- `crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs`
  - HTTP ingress contract, token verification, and request-level orchestration.
- `crates/clawbro-server/src/channels_internal/dingtalk_webhook_reply.rs`
  - `sessionWebhook` reply signing and sending for inbound-triggered responses.
- `crates/clawbro-server/src/channels_internal/dingtalk_webhook_types.rs`
  - Focused Rust structs for DingTalk custom robot webhook payloads.
- `crates/clawbro-server/src/channels_internal/dingtalk_webhook_dedup.rs`
  - Minimal dedup store/logic for webhook event ids if existing generic dedup is not reusable.
- `crates/clawbro-server/src/channels_internal/dingtalk_webhook_mapper.rs`
  - Translation layer from DingTalk webhook payloads into internal `InboundMsg` plus reply metadata.

### Existing reference-only files

- `openclaw-channel-dingtalk-bot/src/channel.ts`
  - Feature reference only; do not port structure directly.
- `openclaw-channel-dingtalk-bot/src/sign.ts`
  - Reference for DingTalk outbound signing logic.

### Boundary guidance

If any new file exceeds a single clear responsibility, split it instead of growing a monolith.

Preferred ownership:

- `dingtalk_webhook.rs` = HTTP ingress contract
- `dingtalk_webhook_types.rs` = DingTalk payload types only
- `dingtalk_webhook_mapper.rs` = payload -> internal model mapping
- `dingtalk_webhook_reply.rs` = outbound reply logic only
- `dingtalk_webhook_dedup.rs` = dedup policy only

Additional rules:

- webhook mode must reuse the existing `Channel`-based send path rather than inventing a parallel reply transport inside the gateway handler
- phase 1 must reuse the existing allowlist file mechanism instead of introducing a second allowlist source in channel config

---

## Chunk 1: Phase 1 Core Ingress Skeleton

### Task 1: Add Config And Channel Surface

**Files:**
- Modify: `crates/clawbro-server/src/config.rs`
- Modify: `crates/clawbro-server/src/channels_internal/mod.rs`
- Test: `crates/clawbro-server/src/config.rs`

- [ ] **Step 1: Add a failing config test for `channels.dingtalk_webhook`**

Add a focused test near existing channel config tests that parses:

```toml
[channels.dingtalk_webhook]
enabled = true
secret_key = "SEC-test"
webhook_path = "/channels/dingtalk/webhook"
presentation = "quiet"
```

Assert the config loads and values land in a dedicated webhook config struct.

- [ ] **Step 2: Run the config test and verify it fails**

Run:

```bash
cd /Users/fishers/Desktop/repo/quickai-openclaw/clawBro
cargo test -p clawbro dingtalk_webhook --lib -- --nocapture
```

Expected: FAIL because `dingtalk_webhook` config does not exist yet.

- [ ] **Step 3: Add `DingTalkWebhookConfig` to the channel config model**

Implement the minimal config shape in `config.rs`:

- `enabled: bool`
- `secret_key: String`
- `webhook_path: Option<String>` or `String`
- `access_token: Option<String>`
- `presentation: ...` aligned with existing channel presentation type

Keep it independent from the existing stream-mode DingTalk config.
Do not add `allowed_users` here; phase 1 must continue to use the existing allowlist file path via `AllowlistChecker`.

- [ ] **Step 4: Export the new module slots**

Update `channels_internal/mod.rs` with the new DingTalk webhook modules.

- [ ] **Step 5: Re-run the config test**

Run:

```bash
cd /Users/fishers/Desktop/repo/quickai-openclaw/clawBro
cargo test -p clawbro dingtalk_webhook --lib -- --nocapture
```

Expected: PASS for config parsing.

### Task 2: Create Rust Payload Types

**Files:**
- Create: `crates/clawbro-server/src/channels_internal/dingtalk_webhook_types.rs`
- Test: `crates/clawbro-server/src/channels_internal/dingtalk_webhook_types.rs`

- [ ] **Step 1: Add a failing payload parse test based on the reference plugin payload**

Use the sample payload shape from:

- `openclaw-channel-dingtalk-bot/README.zh-CN.md`

Test that Rust structs deserialize fields for:

- `conversationId`
- `conversationType`
- `senderId`
- `senderNick`
- `msgId`
- `msgtype`
- `text.content`
- `isInAtList`
- `atUsers`
- `sessionWebhook`
- `sessionWebhookExpiredTime`
- `robotCode`

- [ ] **Step 2: Run the payload test to verify it fails**

Run:

```bash
cd /Users/fishers/Desktop/repo/quickai-openclaw/clawBro
cargo test -p clawbro dingtalk_webhook_payload --lib -- --nocapture
```

Expected: FAIL because the types do not exist.

- [ ] **Step 3: Implement minimal payload structs**

Use focused structs only. Do not mirror all reference-plugin fields yet.

Add:

- root inbound payload struct
- text payload struct
- richText support placeholders for phase 2
- at-user struct

Do not embed mapping or verification helpers into these types.

- [ ] **Step 4: Re-run the payload tests**

Run:

```bash
cd /Users/fishers/Desktop/repo/quickai-openclaw/clawBro
cargo test -p clawbro dingtalk_webhook_payload --lib -- --nocapture
```

Expected: PASS.

---

## Chunk 2: HTTP Ingress, Verification, Allowlist, And Dedup

### Task 3: Build A Dedicated Webhook Handler

**Files:**
- Create: `crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs`
- Test: `crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs`

- [ ] **Step 1: Write failing verification tests**

Add tests for:

- missing token header -> reject
- token mismatch -> reject
- exact token match -> accept

Reference behavior comes from the custom robot webhook product, but implement strict equality, not the reference plugin's `startsWith`.

- [ ] **Step 2: Run the tests and verify they fail**

Run:

```bash
cd /Users/fishers/Desktop/repo/quickai-openclaw/clawBro
cargo test -p clawbro dingtalk_webhook_verification --lib -- --nocapture
```

Expected: FAIL because the handler helpers do not exist.

- [ ] **Step 3: Implement verification helpers**

Implement minimal helpers:

- token header extraction
- strict equality verification
- payload JSON parse from raw body

Keep this file ingress-focused. Do not perform runtime dispatch, richText image download, or reply sending here.

- [ ] **Step 4: Re-run verification tests**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_verification --lib -- --nocapture
```

Expected: PASS.

### Task 4: Add Scope Derivation And Mention Filtering

**Files:**
- Modify: `crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs`
- Test: `crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs`

- [ ] **Step 1: Add failing tests for scope mapping**

Add tests for:

- group webhook payload -> `group:{conversation_id}`
- direct payload -> explicitly unsupported in phase 1

- [ ] **Step 2: Add failing tests for group mention filtering**

Add tests for:

- group message with `isInAtList=true` -> accepted
- group message without mention -> ignored

- [ ] **Step 3: Run tests and verify failure**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_scope --lib -- --nocapture
cargo test -p clawbro dingtalk_webhook_mention --lib -- --nocapture
```

Expected: FAIL.

- [ ] **Step 4: Implement minimal scope and mention logic**

Implement:

- `derive_scope(...)`
- `should_process_group_message(...)`

Do not dispatch yet; just return structured parse outcomes.

- [ ] **Step 5: Re-run tests**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_scope --lib -- --nocapture
cargo test -p clawbro dingtalk_webhook_mention --lib -- --nocapture
```

Expected: PASS.

### Task 5: Reuse Existing Allowlist Source And Add Deduplication

**Files:**
- Modify: `crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs`
- Create: `crates/clawbro-server/src/channels_internal/dingtalk_webhook_dedup.rs` (if needed)
- Test: `crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs`

- [ ] **Step 1: Write a failing allowlist integration test**

Test that webhook mode uses the existing allowlist file semantics via `AllowlistChecker`, not a second config source.

- [ ] **Step 2: Write a failing duplicate event test**

Same `msgId` or webhook event id should only produce one accepted ingress event.

- [ ] **Step 3: Run the tests to verify failure**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_allowlist --lib -- --nocapture
cargo test -p clawbro dingtalk_webhook_dedup --lib -- --nocapture
```

Expected: FAIL.

- [ ] **Step 4: Implement allowlist reuse and minimal dedup**

The implementation should:

- use `AllowlistChecker::load()`
- avoid adding webhook-specific allowlist config in phase 1
- key dedup by external webhook id / `msgId`
- avoid duplicate turn creation
- remain ingress-scoped

Expose dedup behind a small helper API so future webhook channels can reuse the shape without sharing DingTalk-specific parsing code.

- [ ] **Step 5: Re-run the tests**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_allowlist --lib -- --nocapture
cargo test -p clawbro dingtalk_webhook_dedup --lib -- --nocapture
```

Expected: PASS.

---

## Chunk 3: Integrate With Gateway And Existing `Channel` Send Path

### Task 6: Add The HTTP Route

**Files:**
- Modify: `crates/clawbro-server/src/gateway/server.rs`
- Test: `crates/clawbro-server/src/gateway/server.rs`

- [ ] **Step 1: Write a failing route existence test**

Assert the router exposes:

- `POST /channels/dingtalk/webhook`

and rejects when the webhook channel is not configured or disabled.

- [ ] **Step 2: Run the route test and verify it fails**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_route --lib -- --nocapture
```

Expected: FAIL.

- [ ] **Step 3: Register the route**

Wire a dedicated handler into `build_router(...)`.

The route should:

- read raw body
- invoke webhook ingress parsing
- enqueue downstream dispatch asynchronously
- return 200 quickly

- [ ] **Step 4: Re-run route tests**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_route --lib -- --nocapture
```

Expected: PASS.

### Task 7: Define The `Channel::send()` Integration

**Files:**
- Modify: `crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs`
- Modify: `crates/clawbro-server/src/channels_internal/dingtalk_webhook_reply.rs`
- Modify: `crates/clawbro-server/src/gateway_process.rs`
- Test: `crates/clawbro-server/src/channels_internal/dingtalk_webhook_reply.rs`

- [ ] **Step 1: Write a failing test for webhook reply transport**

Assert that a webhook-mode inbound message can still flow through the existing `Channel::send()` path used by `spawn_im_turn(...)`.

The test should prove:

- `OutboundMsg.thread_ts` carries the `sessionWebhook`
- the webhook-mode channel signs and sends against that value
- no parallel ad-hoc reply path is needed in the HTTP handler

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_channel_send --lib -- --nocapture
```

Expected: FAIL.

- [ ] **Step 3: Implement the send-capable webhook channel path**

Choose one explicit model and document it in code comments:

- either add a `DingTalkWebhookChannel` implementing `Channel`
- or factor out a shared DingTalk sender used by both stream and webhook channel implementations

Phase 1 requirement:

- webhook mode must be compatible with existing `ImProgressSink` and `send_with_reply_fallback(...)`
- reply transport must continue to flow through `Channel::send()`

- [ ] **Step 4: Re-run the test**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_channel_send --lib -- --nocapture
```

Expected: PASS.

### Task 8: Convert Webhook Payloads Into `InboundMsg`

**Files:**
- Modify: `crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs`
- Modify: `crates/clawbro-server/src/channels_internal/dingtalk_webhook_mapper.rs`
- Modify: `crates/clawbro-server/src/gateway_process.rs`
- Modify: `crates/clawbro-server/src/protocol.rs` (only if extra metadata is truly required after trying `thread_ts`)
- Test: `crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs`

- [ ] **Step 1: Write a failing integration-style test**

Assert a valid webhook payload becomes an `InboundMsg` with:

- correct `SessionKey`
- correct message text
- preserved message id for dedup/debugging
- preserved reply metadata for later `sessionWebhook` reply

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_inbound_msg --lib -- --nocapture
```

Expected: FAIL.

- [ ] **Step 3: Implement the conversion path**

Implement the minimal conversion helper and wire it into the runtime path used by other channels.

Keep the downstream path identical to other ingress modes after `InboundMsg` creation.

Prefer reusing:

- `InboundMsg.thread_ts = sessionWebhook`
- `OutboundMsg.thread_ts` for replies

Only modify protocol surface if phase 1 proves that `thread_ts` reuse is insufficient.

- [ ] **Step 4: Re-run tests**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_inbound_msg --lib -- --nocapture
```

Expected: PASS.

---

## Chunk 4: Phase 1 Reply Path Using `sessionWebhook`

### Task 9: Implement Outbound Signing

**Files:**
- Create: `crates/clawbro-server/src/channels_internal/dingtalk_webhook_reply.rs`
- Test: `crates/clawbro-server/src/channels_internal/dingtalk_webhook_reply.rs`

- [ ] **Step 1: Write a failing signing test**

Using the reference plugin logic from `openclaw-channel-dingtalk-bot/src/sign.ts`, assert:

- `timestamp` is appended
- `sign` is URL-encoded base64 HMAC-SHA256 over `"{timestamp}\n{secret_key}"`

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_sign --lib -- --nocapture
```

Expected: FAIL.

- [ ] **Step 3: Implement Rust signing helper**

Implement a focused helper:

```rust
fn sign_dingtalk_secret(secret_key: &str, timestamp_ms: i64) -> SignedParams
```

Do not mix it with generic channel code.

- [ ] **Step 4: Re-run the signing tests**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_sign --lib -- --nocapture
```

Expected: PASS.

### Task 10: Implement Inbound-Triggered Reply Delivery

**Files:**
- Modify: `crates/clawbro-server/src/channels_internal/dingtalk_webhook_reply.rs`
- Modify: `crates/clawbro-server/src/gateway_process.rs`
- Test: `crates/clawbro-server/src/channels_internal/dingtalk_webhook_reply.rs`

- [ ] **Step 1: Write a failing reply delivery test**

Assert the reply path:

- uses stored `sessionWebhook`
- signs the outbound URL
- sends markdown/text payload in the DingTalk format expected by custom robot replies

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_reply --lib -- --nocapture
```

Expected: FAIL.

- [ ] **Step 3: Implement the minimal reply sender**

Implement inbound-triggered reply sending only.

Do not implement proactive outbound in phase 1.
Keep reply metadata transport-neutral enough that a later fallback sender can reuse it without rewriting ingress parsing.

- [ ] **Step 4: Re-run reply tests**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_reply --lib -- --nocapture
```

Expected: PASS.

---

## Chunk 5: Phase 1 Docs And End-To-End Regression

### Task 11: Add Handwritten Config Docs

**Files:**
- Modify: `docs/setup.md`
- Optional Modify: `README.md`

- [ ] **Step 1: Document the new config block**

Add a DingTalk webhook section with:

- purpose
- webhook path
- public HTTPS requirement
- group-only expectation
- difference from stream mode

- [ ] **Step 2: Add an example config**

Use:

```toml
[channels.dingtalk_webhook]
enabled = true
secret_key = "SEC..."
webhook_path = "/channels/dingtalk/webhook"
presentation = "quiet"
```

Also document that allowlist continues to come from:

- `~/.clawbro/allowlist.json`

- [ ] **Step 3: Review docs for misleading claims**

Ensure docs do not claim:

- 1:1 support
- proactive outbound in phase 1
- setup wizard support

### Task 12: Run Phase 1 Regression

**Files:**
- Test: existing lib tests
- Test: targeted new webhook tests

- [ ] **Step 1: Run targeted webhook tests**

Run:

```bash
cd /Users/fishers/Desktop/repo/quickai-openclaw/clawBro
cargo test -p clawbro dingtalk_webhook --lib -- --nocapture
```

Expected: PASS.

- [ ] **Step 2: Run broader channel/gateway tests**

Run:

```bash
cargo test -p clawbro gateway --lib -- --nocapture
```

Expected: PASS.

- [ ] **Step 3: Run the main build**

Run:

```bash
cargo build -p clawbro --bin clawbro
```

Expected: PASS.

- [ ] **Step 4: Commit phase 1**

```bash
git -C /Users/fishers/Desktop/repo/quickai-openclaw/clawBro add \
  crates/clawbro-server/src/config.rs \
  crates/clawbro-server/src/gateway/server.rs \
  crates/clawbro-server/src/gateway_process.rs \
  crates/clawbro-server/src/channels_internal/mod.rs \
  crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs \
  crates/clawbro-server/src/channels_internal/dingtalk_webhook_reply.rs \
  crates/clawbro-server/src/channels_internal/dingtalk_webhook_types.rs \
  crates/clawbro-server/src/channels_internal/dingtalk_webhook_dedup.rs \
  crates/clawbro-server/src/channels_internal/dingtalk_webhook_mapper.rs \
  docs/setup.md
git -C /Users/fishers/Desktop/repo/quickai-openclaw/clawBro commit -m "feat: add dingtalk custom robot webhook ingress"
```

---

## Chunk 6: Phase 2 Parity Expansion

### Task 13: Add RichText Parsing And Image Download

**Files:**
- Modify: `crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs`
- Modify: `crates/clawbro-server/src/channels_internal/dingtalk_webhook_types.rs`
- Test: `crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs`

- [ ] **Step 1: Add failing richText parse tests**

Cover:

- text nodes
- image placeholders
- unsupported nodes ignored safely

- [ ] **Step 2: Add failing image download tests**

Cover:

- access token absent -> graceful degradation
- download API failure -> placeholder fallback

- [ ] **Step 3: Implement minimal richText support**

Reference:

- `openclaw-channel-dingtalk-bot/src/channel.ts`

But keep download work out of the HTTP ACK-critical path.

- [ ] **Step 4: Re-run the tests**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_richtext --lib -- --nocapture
```

Expected: PASS.

### Task 14: Add Optional Proactive Outbound And Expiry Semantics

**Files:**
- Modify: `crates/clawbro-server/src/channels_internal/dingtalk_webhook_reply.rs`
- Modify: `crates/clawbro-server/src/config.rs`
- Test: `crates/clawbro-server/src/channels_internal/dingtalk_webhook_reply.rs`

- [ ] **Step 1: Write failing tests for `sessionWebhookExpiredTime` behavior**

Cover:

- still valid -> reply by `sessionWebhook`
- expired and no `access_token` -> structured failure / no send
- expired with `access_token` -> proactive fallback path

- [ ] **Step 2: Run the tests and verify they fail**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_expiry --lib -- --nocapture
```

Expected: FAIL.

- [ ] **Step 3: Implement expiry-aware reply policy**

Keep phase 2 logic explicit:

- immediate reply prefers `sessionWebhook`
- fallback to proactive send only when configured and allowed

Do not bake fallback policy directly into ingress parsing. Keep it inside the reply module so future outbound strategies remain swappable.

- [ ] **Step 4: Re-run the tests**

Run:

```bash
cargo test -p clawbro dingtalk_webhook_expiry --lib -- --nocapture
```

Expected: PASS.

### Task 15: Final Phase 2 Regression And Commit

**Files:**
- Test: targeted webhook tests
- Test: main build

- [ ] **Step 1: Run targeted phase 2 tests**

Run:

```bash
cargo test -p clawbro dingtalk_webhook --lib -- --nocapture
```

Expected: PASS.

- [ ] **Step 2: Run broader library tests**

Run:

```bash
cargo test -p clawbro --lib -- --nocapture
```

Expected: PASS.

- [ ] **Step 3: Run the build**

Run:

```bash
cargo build -p clawbro --bin clawbro
```

Expected: PASS.

- [ ] **Step 4: Commit phase 2**

```bash
git -C /Users/fishers/Desktop/repo/quickai-openclaw/clawBro add \
  crates/clawbro-server/src/channels_internal/dingtalk_webhook.rs \
  crates/clawbro-server/src/channels_internal/dingtalk_webhook_reply.rs \
  crates/clawbro-server/src/channels_internal/dingtalk_webhook_types.rs \
  crates/clawbro-server/src/channels_internal/dingtalk_webhook_mapper.rs \
  crates/clawbro-server/src/config.rs \
  docs/setup.md
git -C /Users/fishers/Desktop/repo/quickai-openclaw/clawBro commit -m "feat: expand dingtalk webhook toward custom robot parity"
```

---

Plan complete and saved to `clawBro/docs/plans/2026-03-19-dingtalk-custom-robot-webhook-implementation-plan.md`. Ready to execute?
