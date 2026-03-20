# ClawBro Release and npm Distribution Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add GitHub Release binary publishing and npm-based binary installation for `clawbro`, while keeping Rust as the only implementation.

**Architecture:** GitHub Releases become the canonical distribution channel for prebuilt binaries. A small npm package downloads the matching release artifact and forwards execution to the installed binary. Cargo publishing remains supported for Rust-native users.

**Tech Stack:** GitHub Actions, Rust/Cargo, shell packaging scripts, Node.js/npm, prebuilt binary archives

---

## File Map

**CI / Release**
- Create: `clawBro/.github/workflows/test.yml`
- Create: `clawBro/.github/workflows/release.yml`
- Create: `clawBro/scripts/release/package-artifact.sh`
- Create: `clawBro/scripts/release/checksums.sh`

**npm package**
- Create: `clawBro/npm/package.json`
- Create: `clawBro/npm/bin/clawbro.js`
- Create: `clawBro/npm/scripts/postinstall.js`
- Create: `clawBro/npm/scripts/platform.js`
- Create: `clawBro/npm/README.md`

**Docs**
- Modify: `clawBro/README.md`
- Modify: `clawBro/README_ZH.md`
- Modify: `clawBro/README_JA.md`
- Modify: `clawBro/README_KO.md`
- Modify: `clawBro/docs/setup.md`

## Chunk 1: GitHub Actions Foundation

### Task 1: Add test workflow

**Files:**
- Create: `clawBro/.github/workflows/test.yml`

- [ ] **Step 1: Write a workflow that runs on pull requests and pushes**
- [ ] **Step 2: Include `cargo check -p clawbro --lib`**
- [ ] **Step 3: Include focused scheduler/runtime tests that already matter for release confidence**
- [ ] **Step 4: Validate YAML syntax locally if possible**
- [ ] **Step 5: Commit**

### Task 2: Add release workflow

**Files:**
- Create: `clawBro/.github/workflows/release.yml`
- Create: `clawBro/scripts/release/package-artifact.sh`
- Create: `clawBro/scripts/release/checksums.sh`

- [ ] **Step 1: Build a tag-triggered workflow for `v*` tags**
- [ ] **Step 2: Add matrix builds for `darwin-aarch64`, `darwin-x86_64`, and `linux-x86_64`**
- [ ] **Step 3: Package each build into a stable archive name**
- [ ] **Step 4: Generate `SHA256SUMS`**
- [ ] **Step 5: Upload artifacts to GitHub Release**
- [ ] **Step 6: Commit**

## Chunk 2: npm Binary Installer

### Task 3: Create npm package shell

**Files:**
- Create: `clawBro/npm/package.json`
- Create: `clawBro/npm/bin/clawbro.js`
- Create: `clawBro/npm/scripts/platform.js`
- Create: `clawBro/npm/README.md`

- [ ] **Step 1: Write package metadata for an installable CLI wrapper**
- [ ] **Step 2: Add `bin` mapping to `clawbro.js`**
- [ ] **Step 3: Implement platform-to-artifact resolution**
- [ ] **Step 4: Make the JS entrypoint exec the downloaded binary**
- [ ] **Step 5: Commit**

### Task 4: Add postinstall downloader

**Files:**
- Create: `clawBro/npm/scripts/postinstall.js`
- Modify: `clawBro/npm/package.json`

- [ ] **Step 1: Download the correct GitHub Release artifact for current OS/arch**
- [ ] **Step 2: Unpack into the npm package directory**
- [ ] **Step 3: Mark Unix binaries executable**
- [ ] **Step 4: Fail clearly for unsupported platforms**
- [ ] **Step 5: Add local smoke verification instructions**
- [ ] **Step 6: Commit**

## Chunk 3: Documentation and Release Contract

### Task 5: Document the three install paths

**Files:**
- Modify: `clawBro/README.md`
- Modify: `clawBro/README_ZH.md`
- Modify: `clawBro/README_JA.md`
- Modify: `clawBro/README_KO.md`
- Modify: `clawBro/docs/setup.md`

- [ ] **Step 1: Add GitHub Release install instructions**
- [ ] **Step 2: Add npm install instructions**
- [ ] **Step 3: Keep `cargo install clawbro` as the Rust-native path**
- [ ] **Step 4: Document first-phase supported platforms**
- [ ] **Step 5: Mention unsigned macOS caveat if phase 1 ships unsigned binaries**
- [ ] **Step 6: Commit**

## Chunk 4: Release Dry Run

### Task 6: Validate end-to-end release assumptions

**Files:**
- Modify as needed based on dry-run findings

- [ ] **Step 1: Run local `cargo publish -p clawbro --dry-run`**
- [ ] **Step 2: Validate archive naming conventions**
- [ ] **Step 3: Validate npm package local layout with `npm pack`**
- [ ] **Step 4: Verify the JS shim resolves the installed binary path**
- [ ] **Step 5: Verify docs match actual commands**
- [ ] **Step 6: Commit**

## Success Criteria

- GitHub Actions can build and package tagged release binaries
- GitHub Release artifacts have stable OS/arch names
- npm package installs a prebuilt binary instead of compiling Rust locally
- `clawbro` remains implemented only in Rust
- docs clearly describe Cargo, GitHub Release, and npm install paths
- first-phase release process is simple enough to operate reliably

Plan complete and saved to `clawBro/docs/plans/2026-03-20-clawbro-release-and-npm-distribution-plan.md`. Ready to execute?
