# ClawBro Single Package Publish Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure ClawBro so `cargo publish --dry-run -p clawbro` passes with a single publicly-installable top-level package and `cargo install clawbro` becomes the only user-facing Cargo path.

**Architecture:** Keep `clawbro` as the only public package and binary, and fold the current internal `clawbro-*` workspace crates into the top-level package as internal modules. Preserve behavior first; this is a packaging refactor, not a feature rewrite. Historical plans/research docs are explicitly out of scope until the package and publish path are stable.

**Tech Stack:** Rust workspace, Cargo packaging/publishing, Tokio, Axum, reqwest, RMCP, internal ClawBro runtime/session/channel subsystems

---

## Scope

This plan covers only the publishability refactor needed to make `clawbro` a single publishable package.

In scope:
- Removing `clawbro` package dependencies on internal `clawbro-*` crates
- Moving required code into the top-level `clawbro` package as modules
- Preserving the `clawbro` binary and runtime behavior
- Making `cargo package -p clawbro` and `cargo publish -p clawbro --dry-run` pass

Out of scope:
- Publishing `clawbro-agent-sdk`
- Cleaning historical `docs/plans/` and `docs/research/`
- Feature changes to team mode, channels, or runtime semantics
- Rebranding internal crate directories unless required to unblock packaging

---

## Current Problem Summary

Today [clawbro/Cargo.toml](/Users/fishers/Desktop/repo/quickai-openclaw/clawBro/crates/clawbro-server/Cargo.toml) is a top-level package, but it depends on unpublished internal crates:

- `clawbro-protocol`
- `clawbro-session`
- `clawbro-agent`
- `clawbro-runtime`
- `clawbro-channels`
- `clawbro-skills`
- `clawbro-cron`

This causes packaging failure:

```bash
cargo package -p clawbro --allow-dirty --no-verify
```

Current expected failure:

```text
no matching package named `clawbro-agent` found
```

That failure is fundamental Cargo behavior. A publicly published crate cannot depend on private unpublished path crates.

---

## Target End State

After this refactor:

- `clawbro` is the only public Cargo package
- `clawbro` is the only public binary users install
- `clawbro` no longer depends on any internal `clawbro-*` crates
- internal subsystems exist as modules within the `clawbro` package
- `cargo install clawbro` is the supported Cargo installation path
- internal workspace crates may remain temporarily during migration, but must not be required by the published package

Target package surface:

- Package name: `clawbro`
- Library crate name: `clawbro`
- Binary name: `clawbro`

---

## File Structure Plan

### Top-level package to modify

- Modify: `clawBro/crates/clawbro-server/Cargo.toml`
- Modify: `clawBro/crates/clawbro-server/src/lib.rs`
- Modify: `clawBro/crates/clawbro-server/src/bin/clawbro_cli.rs`
- Modify: `clawBro/crates/clawbro-server/src/gateway_process.rs`

### New internal module trees to create inside `clawbro`

- Create: `clawBro/crates/clawbro-server/src/protocol/`
- Create: `clawBro/crates/clawbro-server/src/session/`
- Create: `clawBro/crates/clawbro-server/src/runtime/`
- Create: `clawBro/crates/clawbro-server/src/agent_core/`
- Create: `clawBro/crates/clawbro-server/src/channels_internal/`
- Create: `clawBro/crates/clawbro-server/src/skills_internal/`
- Create: `clawBro/crates/clawbro-server/src/cron_internal/`

### Existing source crates that act as migration inputs

- Read from: `clawBro/crates/clawbro-protocol/src/**`
- Read from: `clawBro/crates/clawbro-session/src/**`
- Read from: `clawBro/crates/clawbro-runtime/src/**`
- Read from: `clawBro/crates/clawbro-agent/src/**`
- Read from: `clawBro/crates/clawbro-channels/src/**`
- Read from: `clawBro/crates/clawbro-skills/src/**`
- Read from: `clawBro/crates/clawbro-cron/src/**`

### Product/docs to update once publish path is stable

- Modify: `clawBro/README.md`
- Modify: `clawBro/docs/setup.md`
- Modify: `clawBro/docs/getting-started-from-zero.md`

### Tests to preserve

- Test: `clawBro/crates/clawbro-server/tests/e2e_gateway.rs`
- Test: `clawBro/crates/clawbro-server/tests/e2e_lark.rs`
- Test: `clawBro/crates/clawbro-agent/tests/mixed_backend_team.rs`
- Test: `clawBro/crates/clawbro-server/src/**` unit tests

---

## Migration Strategy

The migration must be done bottom-up. Do not start by moving CLI or gateway entrypoints. Remove external crate boundaries from the lowest-level shared types first, then lift runtime and orchestration layers.

Recommended order:

1. Protocol
2. Session
3. Runtime
4. Agent core
5. Channels
6. Skills
7. Cron
8. Top-level package manifest cleanup
9. Packaging and dry-run publish validation

---

## Chunk 1: Publish Boundary and Package Metadata

### Task 1: Make `clawbro` the explicit public package target

**Files:**
- Modify: `clawBro/crates/clawbro-server/Cargo.toml`
- Modify: `clawBro/crates/clawbro-server/src/lib.rs`

- [ ] **Step 1: Add package metadata required for public publishing**

Add or update in `clawBro/crates/clawbro-server/Cargo.toml`:

```toml
[package]
name = "clawbro"
version = "0.1.0"
edition = "2021"
rust-version = "1.90"
description = "ClawBro AI runtime and CLI"
license = "MIT"
repository = "https://github.com/fishers/clawbro"
readme = "README.md"
keywords = ["ai", "agent", "cli", "runtime"]
categories = ["command-line-utilities"]
```

- [ ] **Step 2: Rename the library crate from `clawbro_server` to `clawbro`**

Update:

```toml
[lib]
name = "clawbro"
path = "src/lib.rs"
```

Then fix internal imports from:

```rust
use clawbro_server::...
```

to:

```rust
use clawbro::...
```

- [ ] **Step 3: Run a compile check for the package rename**

Run:

```bash
cargo build -p clawbro --bin clawbro
```

Expected:
- PASS
- Any failures should be import-path related only

- [ ] **Step 4: Commit package surface rename**

```bash
git add crates/clawbro-server/Cargo.toml crates/clawbro-server/src/lib.rs crates/clawbro-server/src/bin/clawbro_cli.rs crates/clawbro-server/src/gateway_process.rs
git commit -m "refactor: prepare clawbro package publish surface"
```

---

## Chunk 2: Fold Protocol and Session into the Top-Level Package

### Task 2: Internalize protocol and session code as package modules

**Files:**
- Create: `clawBro/crates/clawbro-server/src/protocol/mod.rs`
- Create: `clawBro/crates/clawbro-server/src/session/mod.rs`
- Modify: `clawBro/crates/clawbro-server/src/lib.rs`
- Modify: `clawBro/crates/clawbro-server/Cargo.toml`
- Read from: `clawBro/crates/clawbro-protocol/src/**`
- Read from: `clawBro/crates/clawbro-session/src/**`

- [ ] **Step 1: Copy protocol sources into `src/protocol/`**

Move the implementation, not the dependency:
- `clawbro-protocol/src/lib.rs`
- supporting files under that crate

Preserve module boundaries where possible.

- [ ] **Step 2: Copy session sources into `src/session/`**

Move:
- `clawbro-session/src/lib.rs`
- `manager.rs`
- storage-related files

- [ ] **Step 3: Expose the modules from top-level `lib.rs`**

Add:

```rust
pub mod protocol;
pub mod session;
```

- [ ] **Step 4: Replace external imports**

Rewrite imports from:

```rust
use clawbro_protocol::...
use clawbro_session::...
```

to:

```rust
use crate::protocol::...
use crate::session::...
```

- [ ] **Step 5: Remove protocol/session path dependencies from Cargo.toml**

Delete:

```toml
clawbro-protocol = { path = "../clawbro-protocol", version = "0.1.0" }
clawbro-session = { path = "../clawbro-session", version = "0.1.0" }
```

- [ ] **Step 6: Run targeted tests**

Run:

```bash
cargo test -p clawbro --lib protocol
cargo test -p clawbro --lib session
```

Expected:
- PASS or only import-path fixes required

- [ ] **Step 7: Commit protocol/session fold-in**

```bash
git add crates/clawbro-server/src/protocol crates/clawbro-server/src/session crates/clawbro-server/src/lib.rs crates/clawbro-server/Cargo.toml
git commit -m "refactor: internalize protocol and session into clawbro"
```

---

## Chunk 3: Fold Runtime and Agent Core into the Top-Level Package

### Task 3: Internalize runtime and agent orchestration

**Files:**
- Create: `clawBro/crates/clawbro-server/src/runtime/`
- Create: `clawBro/crates/clawbro-server/src/agent_core/`
- Modify: `clawBro/crates/clawbro-server/src/lib.rs`
- Modify: `clawBro/crates/clawbro-server/Cargo.toml`
- Read from: `clawBro/crates/clawbro-runtime/src/**`
- Read from: `clawBro/crates/clawbro-agent/src/**`

- [ ] **Step 1: Copy runtime sources into `src/runtime/`**

Priority files:
- backend definitions
- conductor/runtime dispatch
- contract/launch spec
- native adapter
- backend registry

- [ ] **Step 2: Copy agent sources into `src/agent_core/`**

Priority files:
- registry
- routing
- context assembly
- persona
- prompt builder
- team registry and routing glue

- [ ] **Step 3: Expose runtime and agent modules from top-level `lib.rs`**

```rust
pub mod runtime;
pub mod agent_core;
```

- [ ] **Step 4: Replace external imports**

Rewrite:

```rust
use clawbro_runtime::...
use clawbro_agent::...
```

to internal equivalents.

- [ ] **Step 5: Remove runtime/agent path dependencies**

Delete:

```toml
clawbro-agent = { path = "../clawbro-agent", version = "0.1.0" }
clawbro-runtime = { path = "../clawbro-runtime", version = "0.1.0" }
```

- [ ] **Step 6: Run targeted compile and tests**

Run:

```bash
cargo build -p clawbro --bin clawbro
cargo test -p clawbro --lib --no-run
```

Expected:
- PASS

- [ ] **Step 7: Commit runtime/agent fold-in**

```bash
git add crates/clawbro-server/src/runtime crates/clawbro-server/src/agent_core crates/clawbro-server/src/lib.rs crates/clawbro-server/Cargo.toml
git commit -m "refactor: internalize runtime and agent core into clawbro"
```

---

## Chunk 4: Fold Channels, Skills, and Cron into the Top-Level Package

### Task 4: Internalize operational modules

**Files:**
- Create: `clawBro/crates/clawbro-server/src/channels_internal/`
- Create: `clawBro/crates/clawbro-server/src/skills_internal/`
- Create: `clawBro/crates/clawbro-server/src/cron_internal/`
- Modify: `clawBro/crates/clawbro-server/src/lib.rs`
- Modify: `clawBro/crates/clawbro-server/Cargo.toml`
- Read from: `clawBro/crates/clawbro-channels/src/**`
- Read from: `clawBro/crates/clawbro-skills/src/**`
- Read from: `clawBro/crates/clawbro-cron/src/**`

- [ ] **Step 1: Copy channels sources into `src/channels_internal/`**

- [ ] **Step 2: Copy skills sources into `src/skills_internal/`**

- [ ] **Step 3: Copy cron sources into `src/cron_internal/`**

- [ ] **Step 4: Expose modules from top-level `lib.rs`**

```rust
pub mod channels_internal;
pub mod skills_internal;
pub mod cron_internal;
```

- [ ] **Step 5: Replace external imports**

Rewrite:

```rust
use clawbro_channels::...
use clawbro_skills::...
use clawbro_cron::...
```

to internal module imports.

- [ ] **Step 6: Remove remaining internal path dependencies**

Delete:

```toml
clawbro-channels = { path = "../clawbro-channels", version = "0.1.0" }
clawbro-skills = { path = "../clawbro-skills", version = "0.1.0" }
clawbro-cron = { path = "../clawbro-cron", version = "0.1.0" }
```

- [ ] **Step 7: Run integration-oriented compile**

Run:

```bash
cargo build -p clawbro --bin clawbro
cargo test -p clawbro --test e2e_gateway --no-run
cargo test -p clawbro --test e2e_lark --no-run
```

Expected:
- PASS

- [ ] **Step 8: Commit operational module fold-in**

```bash
git add crates/clawbro-server/src/channels_internal crates/clawbro-server/src/skills_internal crates/clawbro-server/src/cron_internal crates/clawbro-server/src/lib.rs crates/clawbro-server/Cargo.toml
git commit -m "refactor: internalize channels skills and cron into clawbro"
```

---

## Chunk 5: Packaging, Publish Dry-Run, and Documentation

### Task 5: Make `clawbro` packageable and publishable

**Files:**
- Modify: `clawBro/crates/clawbro-server/Cargo.toml`
- Modify: `clawBro/README.md`
- Modify: `clawBro/docs/setup.md`
- Modify: `clawBro/docs/getting-started-from-zero.md`

- [ ] **Step 1: Ensure no internal `clawbro-*` dependencies remain in published package**

Run:

```bash
rg 'clawbro-(protocol|session|agent|runtime|channels|skills|cron)' crates/clawbro-server/Cargo.toml crates/clawbro-server/src
```

Expected:
- No Cargo dependency references remain
- Only historical comments/tests may remain if harmless

- [ ] **Step 2: Verify package contents**

Run:

```bash
cargo package -p clawbro --allow-dirty --no-verify
```

Expected:
- PASS

- [ ] **Step 3: Verify publish dry-run**

Run:

```bash
cargo publish -p clawbro --dry-run --allow-dirty
```

Expected:
- PASS

- [ ] **Step 4: Verify install path**

Run:

```bash
cargo install --path crates/clawbro-server --bin clawbro --locked --force
clawbro --help
```

Expected:
- install succeeds
- `clawbro --help` prints normal command list

- [ ] **Step 5: Update user docs to the final install path**

Required docs changes:
- `cargo install clawbro`
- `cargo install --git ... --bin clawbro` as source-install fallback
- remove any claim that users need `clawbro-gateway`

- [ ] **Step 6: Commit publishability changes**

```bash
git add crates/clawbro-server/Cargo.toml crates/clawbro-server/src README.md docs/setup.md docs/getting-started-from-zero.md
git commit -m "feat: make clawbro a single publishable package"
```

---

## Final Validation Checklist

- [ ] `cargo build -p clawbro --bin clawbro`
- [ ] `cargo test -p clawbro --lib --no-run`
- [ ] `cargo test -p clawbro --test e2e_gateway --no-run`
- [ ] `cargo test -p clawbro --test e2e_lark --no-run`
- [ ] `cargo package -p clawbro --allow-dirty --no-verify`
- [ ] `cargo publish -p clawbro --dry-run --allow-dirty`
- [ ] `cargo install --path crates/clawbro-server --bin clawbro --locked --force`
- [ ] `clawbro setup --help`
- [ ] `clawbro serve --help`

---

## Risks and Guardrails

1. **Import churn risk**
- Mitigation: migrate bottom-up and compile after each chunk.

2. **Module visibility drift**
- Mitigation: preserve existing API shapes first; rename only after publish dry-run passes.

3. **Behavior regressions hidden inside refactor**
- Mitigation: do not mix behavioral changes with packaging refactor.

4. **Disk pressure during rebuild**
- Mitigation: use `cargo clean` between large chunks if `/clawBro/target` grows too large.

5. **Over-refactoring historical docs**
- Mitigation: keep `docs/plans/` and `docs/research/` out of scope until package publish path is green.

---

## Recommended Execution Order

1. Chunk 1
2. Chunk 2
3. Chunk 3
4. Chunk 4
5. Chunk 5

Do not skip the chunk boundaries. The package only becomes publishable after internal Cargo dependencies are fully eliminated.
