# ClawBro CLI Config Center Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a complete `setup + config` configuration system that can fully configure providers, ACP backends, agents, WeChat/Lark/DingTalk channels, routing, solo mode, and team mode without handwritten TOML.

**Architecture:** Replace ad-hoc CLI config generation with a shared configuration draft engine. `setup`, `config wizard`, and scriptable `config` subcommands will all mutate the same normalized config model, then preview, validate, and apply changes through a single renderer and validator pipeline.

**Tech Stack:** Rust, clap, console, dialoguer, owo-colors, indicatif, existing `GatewayConfig` model, existing WeChat login flow.

---

## Chunk 1: Configuration Engine Foundation

### Task 1: Introduce normalized CLI configuration model

**Files:**
- Create: `crates/clawbro-server/src/cli/config_model.rs`
- Create: `crates/clawbro-server/src/cli/config_keys.rs`
- Modify: `crates/clawbro-server/src/cli/mod.rs`
- Test: `crates/clawbro-server/src/cli/config_model.rs`

- [ ] **Step 1: Define normalized resource structs**

Create structs for:

- provider resources
- backend resources
- agent resources
- channel resources
- team scope resources
- binding resources

- [ ] **Step 2: Define stable key helpers**

Implement helper types for:

- provider id
- backend id
- agent name
- team scope composite key
- binding synthetic ids

- [ ] **Step 3: Add conversion from `GatewayConfig`**

Implement read-side conversion from loaded config into normalized CLI model.

- [ ] **Step 4: Add unit tests**

Cover:

- parsing current real-world config shapes
- team scope composite key behavior
- resource identity stability

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p clawbro --lib cli::config_model
```

### Task 2: Add draft, patch, diff, and apply engine

**Files:**
- Create: `crates/clawbro-server/src/cli/config_draft.rs`
- Create: `crates/clawbro-server/src/cli/config_patch.rs`
- Create: `crates/clawbro-server/src/cli/config_diff.rs`
- Create: `crates/clawbro-server/src/cli/config_apply.rs`
- Modify: `crates/clawbro-server/src/cli/mod.rs`
- Test: `crates/clawbro-server/src/cli/config_draft.rs`

- [ ] **Step 1: Implement `ConfigDraft`**

Support staged resource edits without immediate disk writes.

- [ ] **Step 2: Implement patch operations**

Support:

- add
- set
- remove
- enable
- disable
- link
- unlink

- [ ] **Step 3: Implement diff output**

Produce human-readable before/after summaries suitable for `config diff` and wizard preview.

- [ ] **Step 4: Implement apply path**

Write:

- config backup
- config render
- optional `.env` updates

- [ ] **Step 5: Add tests**

Cover:

- stable diffs
- backup creation
- no-op apply
- patch sequencing

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test -p clawbro --lib cli::config_draft
```

## Chunk 2: Validation and Rendering

### Task 3: Replace setup TOML string generation with shared renderer

**Files:**
- Create: `crates/clawbro-server/src/cli/config_render.rs`
- Modify: `crates/clawbro-server/src/cli/setup/writer.rs`
- Test: `crates/clawbro-server/src/cli/config_render.rs`

- [ ] **Step 1: Implement canonical renderer**

Render normalized model to stable TOML ordering.

- [ ] **Step 2: Preserve current required config sections**

Ensure renderer covers:

- gateway
- provider profiles
- backends
- agent roster
- bindings
- team scopes
- groups
- channels
- scheduler

- [ ] **Step 3: Make setup writer delegate to renderer**

Remove string-concatenation ownership from `writer.rs`.

- [ ] **Step 4: Update tests**

Port setup writer tests to renderer-backed expectations.

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p clawbro --lib cli::setup::writer
```

### Task 4: Add CLI-specific validation layer

**Files:**
- Create: `crates/clawbro-server/src/cli/config_validate.rs`
- Modify: `crates/clawbro-server/src/cli/config_cmd.rs`
- Modify: `crates/clawbro-server/src/cli/setup/mod.rs`
- Test: `crates/clawbro-server/src/cli/config_validate.rs`

- [ ] **Step 1: Implement structural validation**

Check:

- missing ids
- duplicate keys
- malformed team scope references

- [ ] **Step 2: Implement topology validation**

Check:

- provider/backend/agent linkage
- front bot and specialist existence
- unsupported channel-mode combinations

- [ ] **Step 3: Implement runtime preflight validation**

Check:

- WeChat credentials
- required env vars
- ACP launch basics

- [ ] **Step 4: Wire validation into `config validate` and `setup`**

Prevent `setup` from writing invalid team configs.

- [ ] **Step 5: Add tests**

Include the current invalid case where team mode with no channel would previously pass setup generation.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test -p clawbro --lib cli::config_validate
```

## Chunk 3: WeChat Configuration Surface

### Task 5: Add WeChat to setup channel selection

**Files:**
- Modify: `crates/clawbro-server/src/cli/setup/channel.rs`
- Modify: `crates/clawbro-server/src/cli/setup/mode.rs`
- Modify: `crates/clawbro-server/src/cli/i18n.rs`
- Test: `crates/clawbro-server/src/cli/setup/channel.rs`

- [ ] **Step 1: Extend `ChannelConfig` with WeChat**

Add WeChat variant with presentation and login-state placeholders.

- [ ] **Step 2: Add setup prompts for WeChat**

Prompt user for:

- enable WeChat
- login now or later
- presentation mode

- [ ] **Step 3: Update team-scope defaults for WeChat DM**

Make `user:...@im.wechat` the correct suggested scope family.

- [ ] **Step 4: Add unit tests**

Verify setup flow can choose WeChat and derive valid DM defaults.

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p clawbro --lib cli::setup::channel
```

### Task 6: Add WeChat config commands

**Files:**
- Modify: `crates/clawbro-server/src/cli/args.rs`
- Create: `crates/clawbro-server/src/cli/config_channel.rs`
- Modify: `crates/clawbro-server/src/cli/config_cmd.rs`
- Test: `crates/clawbro-server/src/cli/config_channel.rs`

- [ ] **Step 1: Add channel subcommands**

Support:

- enable
- disable
- show
- login
- logout
- test
- setup-solo
- setup-team

- [ ] **Step 2: Reuse existing WeChat QR login flow**

Route `config channel login wechat` through the same backend as `clawbro wechat-login`.

- [ ] **Step 3: Add WeChat solo and DM team helpers**

These should generate or update:

- `[channels.wechat]`
- wechat bindings
- wechat team scopes

- [ ] **Step 4: Keep `WechatLogin` command as alias**

Preserve compatibility while documenting the new path.

- [ ] **Step 5: Add tests**

Cover:

- login dispatch
- enabling channel
- setup-solo mutation
- setup-team mutation

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test -p clawbro --lib cli::config_channel
```

## Chunk 4: Provider, Backend, Agent, and Routing Commands

### Task 7: Add provider resource commands

**Files:**
- Modify: `crates/clawbro-server/src/cli/args.rs`
- Create: `crates/clawbro-server/src/cli/config_provider.rs`
- Modify: `crates/clawbro-server/src/cli/config_cmd.rs`
- Test: `crates/clawbro-server/src/cli/config_provider.rs`

- [ ] **Step 1: Add provider CRUD subcommands**

Support:

- add
- set
- remove
- list
- show

- [ ] **Step 2: Model supported provider protocols**

Support:

- official_session
- openai_compatible
- anthropic_compatible

- [ ] **Step 3: Add tests**

Cover DeepSeek anthropic-compatible and OpenAI official-session examples.

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test -p clawbro --lib cli::config_provider
```

### Task 8: Add backend and agent resource commands

**Files:**
- Create: `crates/clawbro-server/src/cli/config_backend.rs`
- Create: `crates/clawbro-server/src/cli/config_agent.rs`
- Modify: `crates/clawbro-server/src/cli/args.rs`
- Modify: `crates/clawbro-server/src/cli/config_cmd.rs`
- Test: `crates/clawbro-server/src/cli/config_backend.rs`
- Test: `crates/clawbro-server/src/cli/config_agent.rs`

- [ ] **Step 1: Add backend CRUD subcommands**

Support ACP backend creation with:

- id
- acp backend identity
- provider profile
- launch command and args
- env

- [ ] **Step 2: Add agent CRUD subcommands**

Support:

- name
- mentions
- backend mapping

- [ ] **Step 3: Add topology-aware validation hooks**

Reject invalid backend-provider and agent-backend references at command time.

- [ ] **Step 4: Add tests**

Cover:

- `claude-main -> deepseek-anthropic`
- `codex-main -> openai-official`
- `claude -> claude-main`

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p clawbro --lib cli::config_backend
cargo test -p clawbro --lib cli::config_agent
```

### Task 9: Add binding and team-scope commands

**Files:**
- Create: `crates/clawbro-server/src/cli/config_binding.rs`
- Create: `crates/clawbro-server/src/cli/config_team_scope.rs`
- Modify: `crates/clawbro-server/src/cli/args.rs`
- Modify: `crates/clawbro-server/src/cli/config_cmd.rs`
- Test: `crates/clawbro-server/src/cli/config_binding.rs`
- Test: `crates/clawbro-server/src/cli/config_team_scope.rs`

- [ ] **Step 1: Add binding commands**

Support:

- channel binding
- channel-instance binding
- scope binding
- peer binding
- default binding

- [ ] **Step 2: Add team-scope commands**

Support:

- add
- set
- remove
- list

Including:

- front bot
- specialists
- public updates
- max parallel

- [ ] **Step 3: Add WeChat-specific DM constraints**

Reject unsupported WeChat group team setup through CLI validation.

- [ ] **Step 4: Add tests**

Cover:

- WeChat solo binding
- WeChat DM team scope
- Lark group team scope

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p clawbro --lib cli::config_binding
cargo test -p clawbro --lib cli::config_team_scope
```

## Chunk 5: Interactive Config Wizard

### Task 10: Build config wizard shell and menus

**Files:**
- Create: `crates/clawbro-server/src/cli/config_wizard/mod.rs`
- Create: `crates/clawbro-server/src/cli/config_wizard/menu.rs`
- Create: `crates/clawbro-server/src/cli/config_wizard/summary.rs`
- Modify: `crates/clawbro-server/src/cli/config_cmd.rs`
- Test: `crates/clawbro-server/src/cli/config_wizard/menu.rs`

- [ ] **Step 1: Add `config wizard` entrypoint**

Launch draft-backed interactive configuration session.

- [ ] **Step 2: Add main menu sections**

Include:

- Providers
- Backends
- Agents
- Channels
- Routing
- Modes
- Preview
- Validate
- Apply

- [ ] **Step 3: Show current summary on each page**

Provide visible current topology while navigating.

- [ ] **Step 4: Add tests for menu state transitions**

Test menu routing and draft persistence without requiring terminal snapshots.

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p clawbro --lib cli::config_wizard
```

### Task 11: Build resource-specific wizard screens

**Files:**
- Create: `crates/clawbro-server/src/cli/config_wizard/providers.rs`
- Create: `crates/clawbro-server/src/cli/config_wizard/backends.rs`
- Create: `crates/clawbro-server/src/cli/config_wizard/agents.rs`
- Create: `crates/clawbro-server/src/cli/config_wizard/channels.rs`
- Create: `crates/clawbro-server/src/cli/config_wizard/routing.rs`
- Create: `crates/clawbro-server/src/cli/config_wizard/modes.rs`
- Test: `crates/clawbro-server/src/cli/config_wizard/channels.rs`

- [ ] **Step 1: Add provider screens**

Allow create and edit for provider protocol, key env, base URL, and model.

- [ ] **Step 2: Add backend screens**

Allow ACP topology creation and editing.

- [ ] **Step 3: Add WeChat channel screens**

Support:

- enable or disable
- login
- solo setup
- DM team setup

- [ ] **Step 4: Add routing and mode screens**

Allow switching between solo and team paths without starting over.

- [ ] **Step 5: Add tests**

Verify generated draft mutations for common flows.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test -p clawbro --lib cli::config_wizard::channels
```

## Chunk 6: Setup Rework

### Task 12: Rebuild setup on top of the shared wizard engine

**Files:**
- Modify: `crates/clawbro-server/src/cli/setup/mod.rs`
- Modify: `crates/clawbro-server/src/cli/setup/provider.rs`
- Modify: `crates/clawbro-server/src/cli/setup/mode.rs`
- Modify: `crates/clawbro-server/src/cli/setup/auth_cfg.rs`
- Test: `crates/clawbro-server/src/cli/setup/mod.rs`

- [ ] **Step 1: Change setup from bespoke writer to draft bootstrap**

Create a first-pass draft rather than writing final TOML directly.

- [ ] **Step 2: Implement minimal runnable phase**

Set up:

- language
- channel
- provider
- backend
- first agent

- [ ] **Step 3: Add "continue with advanced configuration" handoff**

Transition into `config wizard` from the same draft session.

- [ ] **Step 4: Add non-interactive support**

Ensure non-interactive setup can still emit valid configs, including team configs with explicit channels.

- [ ] **Step 5: Add tests**

Cover:

- solo bootstrap
- WeChat solo bootstrap
- WeChat team bootstrap
- advanced continuation prompt path

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test -p clawbro --lib cli::setup
```

## Chunk 7: Status, Doctor, and Documentation

### Task 13: Make status and doctor reflect WeChat and the new config model

**Files:**
- Modify: `crates/clawbro-server/src/cli/status.rs`
- Modify: `crates/clawbro-server/src/diagnostics.rs`
- Test: `crates/clawbro-server/src/diagnostics.rs`
- Test: `crates/clawbro-server/src/cli/status.rs`

- [ ] **Step 1: Update status channel summary**

Show WeChat and multi-channel combinations correctly.

- [ ] **Step 2: Update diagnostics channel collection**

Include WeChat in configured and enabled checks.

- [ ] **Step 3: Add WeChat-specific doctor findings**

Check:

- credentials present
- enabled without routing
- DM team scope validity

- [ ] **Step 4: Add tests**

Cover WeChat-enabled config in status and diagnostics.

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p clawbro --lib diagnostics
cargo test -p clawbro --lib cli::status
```

### Task 14: Update CLI docs and migration guidance

**Files:**
- Modify: `README_ZH.md`
- Modify: `crates/clawbro-server/README.md`
- Create: `docs/operations/cli-config-center.md`

- [ ] **Step 1: Document new setup and config flows**

Include:

- setup path
- config wizard path
- scriptable resource commands

- [ ] **Step 2: Add WeChat examples**

Include:

- login
- solo setup
- DM team setup

- [ ] **Step 3: Add migration notes**

Explain how existing users move from handwritten TOML to CLI-driven changes.

- [ ] **Step 4: Review docs for consistency**

Check command names, flags, and examples against implementation.

## Final Validation

### Task 15: Run integration validation across major flows

**Files:**
- Modify: `crates/clawbro-server/tests/e2e_gateway.rs`
- Create: `crates/clawbro-server/tests/cli_config_center.rs`

- [ ] **Step 1: Add e2e tests for setup-generated config**

Cover:

- WeChat solo
- WeChat DM team
- Lark team

- [ ] **Step 2: Add e2e tests for scriptable config commands**

Cover:

- provider add
- backend add
- agent add
- channel enable
- team-scope add

- [ ] **Step 3: Run focused test suites**

Run:

```bash
cargo test -p clawbro --lib
cargo test -p clawbro --test cli_config_center
cargo test -p clawbro --test e2e_gateway
```

- [ ] **Step 4: Run formatting**

Run:

```bash
cargo fmt --all
```

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/specs/2026-03-22-clawbro-cli-config-center-design.md \
        docs/superpowers/plans/2026-03-22-clawbro-cli-config-center.md
git commit -m "docs: add clawbro cli config center design and plan"
```
