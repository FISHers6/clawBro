# ClawBro CLI Config Center Design

**Date:** 2026-03-22

**Status:** Approved design

**Goal:** Redesign `clawbro setup` and `clawbro config` into a complete configuration system that can fully configure providers, ACP backends, agents, channels, routing, solo mode, team mode, and WeChat login without requiring manual `config.toml` edits.

## Problem

The current CLI provides only partial configuration support:

- `clawbro setup` can generate a basic config skeleton, but it only knows about Lark and DingTalk channels and writes a simplified topology.
- `clawbro wechat-login` exists, but it is isolated from the configuration flow and does not enable the channel or create routing.
- `clawbro config` is only `show / validate / edit` and cannot perform resource-level updates.
- `status` and `doctor` do not fully reflect WeChat configuration and runtime state.

This creates a split-brain operator experience:

- login is one command
- topology is handwritten TOML
- validation is another command
- runtime diagnostics do not fully mirror configuration capabilities

## Product Outcome

After this redesign, users should be able to:

1. Run `clawbro setup` and finish with a working solo or team deployment.
2. Continue configuration in the same session if they want to add more channels, providers, or backends.
3. Re-enter configuration later through `clawbro config wizard`.
4. Automate the same changes through `clawbro config <resource> <verb> ...`.
5. Configure WeChat as a first-class channel, including login, enablement, solo routing, and DM team scopes.

## UX Principles

1. `setup` is the first-run path, not a dead-end wizard.
2. `config wizard` is the ongoing operator control center.
3. `config <verb>` is the scriptable interface for automation.
4. All three interfaces share one configuration model and one validation pipeline.
5. The CLI must support "configure one thing, then continue configuring more things" without forcing the user to start over.
6. WeChat must be a first-class channel alongside Lark and DingTalk.

## Interface Model

### Top-Level Commands

Retain these entrypoints:

- `clawbro setup`
- `clawbro config`
- `clawbro auth`
- `clawbro doctor`
- `clawbro status`
- `clawbro serve`
- `clawbro wechat-login`

`clawbro wechat-login` remains as a compatibility shortcut, but its implementation becomes the same backend used by `clawbro config channel login wechat`.

### `setup`

`setup` becomes:

- initial bootstrap
- minimal runnable configuration
- optional continuation into advanced configuration

Flow:

1. language
2. desired operating mode
3. channel choice
4. provider choice
5. provider auth and endpoint settings
6. first backend
7. first agent
8. preview
9. validate
10. apply
11. prompt for advanced continuation

If the user chooses to continue, `setup` transfers control to the same flow used by `config wizard`.

### `config wizard`

`config wizard` becomes a persistent menu system with draft state.

Main menu:

- Providers
- Backends
- Agents
- Channels
- Routing
- Modes
- Gateway and Scheduler
- Preview Diff
- Validate
- Apply
- Exit

Users can move in and out of sections without losing draft changes.

### `config <resource> <verb>`

This is the scriptable layer. It exposes resource operations such as:

- `add`
- `set`
- `remove`
- `enable`
- `disable`
- `show`
- `list`
- `link`
- `unlink`
- `validate`
- `diff`
- `apply`

## Resource Model

The CLI manages these resources:

- `gateway`
- `provider`
- `backend`
- `agent`
- `channel`
- `channel-instance`
- `binding`
- `team-scope`
- `group`
- `delivery-binding`
- `scheduler`

### Key Strategy

Use stable keys when the data model has them, otherwise use composite keys.

- provider: `id`
- backend: `id`
- agent: `name`
- channel: fixed name
- channel-instance: `channel + instance_id`
- team-scope: `channel + scope`
- group: `scope`
- binding: generated `binding_id` plus composite lookup support
- delivery-binding: generated `binding_id` plus composite lookup support

Avoid positional indexing such as "the second backend".

## Internal Architecture

Introduce a shared configuration engine used by `setup`, `config wizard`, and `config <verb>`.

Core components:

- `ConfigGraph`
  Represents a normalized in-memory configuration model.

- `ConfigDraft`
  Holds staged edits before commit.

- `ConfigPatch`
  Encodes a single CLI-originated mutation.

- `ConfigRenderer`
  Renders the normalized model back to TOML in stable order.

- `ConfigDiff`
  Produces human-readable preview output before apply.

- `ConfigValidator`
  Runs structural, topology, and runtime preflight validation.

This removes the current direct TOML string assembly used by setup.

## WeChat as a First-Class Channel

WeChat support must become complete at the CLI layer.

### Supported WeChat Operations

- enable or disable channel
- QR login
- logout
- account status inspection
- presentation configuration
- solo routing setup
- DM team setup
- outbound capability test

### Supported WeChat Deployment Shapes

- solo DM routing
- DM team lead flow

Current channel runtime is still 1:1, so group team setup for WeChat must not be exposed as a supported runtime shape. It may appear in the UI only as unavailable or future work.

### WeChat Command Examples

```bash
clawbro config channel enable wechat
clawbro config channel login wechat
clawbro config channel setup-solo wechat --agent claw
clawbro config channel setup-team wechat \
  --scope user:o9cq...@im.wechat \
  --name WeChat-DM-Team \
  --front-bot claude \
  --specialist claw \
  --public-updates minimal \
  --max-parallel 1
```

## Provider and ACP Backend Topology

The CLI must support full provider and backend topology construction.

### Provider Capabilities

- protocol selection
- API key env variable
- base URL
- default model
- vendor alias or display name

Supported protocols:

- `official_session`
- `openai_compatible`
- `anthropic_compatible`

### Backend Capabilities

- backend family
- ACP backend identity
- provider profile binding
- launch command, args, env
- approval mode
- backend-specific settings

This allows topologies such as:

- `claude-main -> deepseek anthropic-compatible`
- `codex-main -> openai official_session`
- `custom-main -> openai-compatible vendor`

### Agent Layer

Agents remain named logical identities mapped to backends.

Examples:

- `claude -> claude-main`
- `claw -> codex-main`

## Routing and Modes

Routing must be configurable from the CLI instead of handwritten.

Supported routing resources:

- default agent
- channel binding
- channel-instance binding
- scope binding
- peer binding
- delivery sender binding

Supported mode shapes:

- solo
- multi-agent non-team
- team DM
- team group

For WeChat specifically:

- solo DM is supported
- DM team is supported
- group team is not exposed as supported

## Validation Model

Validation occurs in three layers:

1. Structural validation
- missing fields
- duplicate identifiers
- unsupported enum combinations

2. Topology validation
- agent references existing backend
- backend references existing provider
- team scope references existing front bot and specialists
- required team channel is present
- unsupported channel + mode combinations are rejected

3. Runtime preflight checks
- required credentials exist
- login artifacts exist for WeChat
- ACP launch command appears runnable
- required env names are configured

Validation must run before apply, and `setup` must not produce invalid team configs.

## Draft / Preview / Apply Workflow

Every interactive path uses draft-first semantics.

Operations:

- make changes in draft
- preview diff
- validate
- apply
- reset draft

Apply behavior:

- back up previous config
- write config
- update `.env` only for owned secrets
- print summary of changed resources
- suggest restart when required

## Diagnostics and Status

`status` and `doctor` must reflect the same resource model as configuration.

### `status` should show

- configured channels including WeChat
- login state per channel
- provider and backend summary
- agent summary
- solo or team summary
- active team scopes and groups

### `doctor` should check

- channel enabled without routing
- WeChat enabled without credentials
- WeChat team scope without valid DM scope
- invalid backend-provider-agent chains
- unsupported channel-mode combinations
- missing or conflicting delivery sender bindings

## CLI Presentation

Use terminal UI polish without introducing a full-screen TUI.

Recommended stack:

- `clap` styling for help and errors
- `owo-colors` for semantic colors
- `console` and `dialoguer` for interactive menus
- `indicatif` for progress feedback

This keeps the experience colorful and readable while preserving standard CLI ergonomics.

## Migration Strategy

Existing users must be able to adopt the new CLI without losing current configs.

Rules:

- existing `config.toml` loads into the normalized model
- unknown fields are preserved when possible or explicitly surfaced
- current `wechat-login` continues to work
- `config show`, `config validate`, and `config edit` continue to exist
- `setup --reinit` becomes a high-level reset plus migration-aware bootstrap

## Non-Goals

These are out of scope for this design:

- full-screen TUI configuration center
- WeChat group team runtime support
- runtime behavior changes unrelated to configuration surfaces
- replacing manual TOML editing for expert users who still prefer it

## Success Criteria

This redesign is successful when:

1. a new user can configure WeChat solo or WeChat DM team without editing TOML
2. an operator can later add providers, backends, and channels through `config wizard`
3. the same topology can be created through scriptable `config` commands
4. `status` and `doctor` correctly report WeChat and team configuration state
5. `setup` no longer emits invalid team configuration when no channel is selected
