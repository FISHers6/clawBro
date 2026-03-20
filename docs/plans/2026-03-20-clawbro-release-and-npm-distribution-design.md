# ClawBro GitHub Release and npm Distribution Design

**Date:** 2026-03-20

**Goal:** Make `clawbro` easy to install for non-Rust users by adding GitHub Release binary publishing and an npm-based binary installer, while keeping Rust as the single implementation source of truth.

## Summary

`clawbro` now has a clean single-package Cargo boundary and can be published as one crate. That is good for Rust users, but it is not enough for broad adoption. Most users who want to "just try it" should not need to install a Rust toolchain first.

The recommended distribution model is:

- crates.io remains supported for Rust-native installation
- GitHub Releases become the canonical binary distribution channel
- npm becomes a thin installer and launcher for those release binaries

This keeps the product architecture clean:

- Rust remains the only product implementation
- GitHub Release is the binary asset registry
- npm is only a transport and installation surface

## Current State

The current repo state already supports this direction:

- `clawbro` is the only public Cargo package
- CLI entrypoint is unified in `crates/clawbro-server/src/bin/clawbro_cli.rs`
- Native backend can resolve the current executable path through `current_exe()`
- installation docs already present `cargo install clawbro`

The repo does not yet have:

- GitHub Actions workflows
- GitHub Release automation
- npm packaging files
- binary artifact naming conventions
- platform packaging scripts

## Design Principles

1. Rust is the only implementation source of truth.
2. Do not create a second implementation in Node.js.
3. GitHub Release artifacts must be directly usable without npm.
4. npm must only download, cache, and dispatch the correct prebuilt binary.
5. Users should have three supported install paths:
   - `cargo install clawbro`
   - direct GitHub Release download
   - `npm i -g clawbro` or `npx clawbro`
6. Release automation must be deterministic and tag-driven.
7. First phase should prefer operational simplicity over maximum platform coverage.

## Recommended Release Model

### 1. GitHub Release as the canonical binary channel

Each tagged version, for example `v0.1.7`, should produce release assets such as:

- `clawbro-darwin-aarch64.tar.gz`
- `clawbro-darwin-x86_64.tar.gz`
- `clawbro-linux-x86_64.tar.gz`
- `clawbro-linux-aarch64.tar.gz`
- optional later:
  - `clawbro-windows-x86_64.zip`

Each asset should contain:

- `clawbro` or `clawbro.exe`
- short install instructions
- checksum metadata

An additional `SHA256SUMS` asset should be uploaded for verification.

GitHub Release is the right canonical source because:

- it matches the git tag and changelog story
- it works for users who do not want npm
- it gives npm a stable, versioned source for binary downloads

### 2. npm as a binary installer, not as a runtime implementation

The npm package should contain:

- a tiny JS launcher in `bin/clawbro.js`
- a `postinstall` script that downloads the correct release artifact
- a small platform resolution layer

The npm package should not:

- compile Rust on user machines
- reimplement `clawbro` behavior in JS
- become a second product runtime

User experience should be:

```bash
npm i -g clawbro
clawbro setup
clawbro serve
```

and also:

```bash
npx clawbro --version
```

## Why This Is Better Than Alternatives

### Alternative A: Cargo only

Pros:

- minimal maintenance
- already works

Cons:

- requires Rust toolchain
- too much friction for many users

This should remain supported, but not be the only path.

### Alternative B: npm installs prebuilt binaries

Pros:

- easy to install
- keeps Rust as source of truth
- works well with GitHub Releases
- suitable for developer audiences already using Node-based tooling

Cons:

- requires release automation
- adds packaging scripts

This is the recommended path.

### Alternative C: npm compiles Rust locally

Pros:

- avoids binary upload matrix

Cons:

- still requires Rust
- slower installs
- more user-side failure modes

This is not recommended.

## Phase 1 Scope

Recommended first-phase platform matrix:

- macOS Apple Silicon
- macOS x86_64
- Linux x86_64

Optional in phase 1 if straightforward:

- Linux aarch64

Deferred to phase 2:

- Windows x86_64
- macOS signing and notarization
- Homebrew formula

This keeps the first implementation small and realistic.

## Release Workflow

### Cargo release

The Rust crate release remains straightforward:

1. bump version in `crates/clawbro-server/Cargo.toml`
2. update README badges and install references
3. publish to crates.io
4. tag release in git, for example `v0.1.7`

### GitHub Release

On tag push:

1. build release binaries in CI
2. package platform artifacts
3. generate checksums
4. create or update GitHub Release
5. upload artifacts

### npm release

After GitHub Release assets exist:

1. bump npm package version to match
2. publish npm package
3. npm installer downloads the matching GitHub Release artifact for the current OS/arch

## Proposed Repository Structure

Add:

- `.github/workflows/test.yml`
- `.github/workflows/release.yml`
- `npm/package.json`
- `npm/bin/clawbro.js`
- `npm/scripts/postinstall.js`
- `npm/scripts/platform.js`

Optional:

- `scripts/release/checksums.sh`
- `scripts/release/package-artifact.sh`

The Rust codebase itself should not be restructured for this.

## Binary Resolution Contract

The npm installer should map:

- `darwin + arm64` -> `clawbro-darwin-aarch64.tar.gz`
- `darwin + x64` -> `clawbro-darwin-x86_64.tar.gz`
- `linux + x64` -> `clawbro-linux-x86_64.tar.gz`
- `linux + arm64` -> `clawbro-linux-aarch64.tar.gz`
- later `win32 + x64` -> `clawbro-windows-x86_64.zip`

It should:

- download from the tagged GitHub Release URL
- verify expected archive shape
- unpack into the npm package directory
- mark the binary executable on Unix
- fail with a clear message if the platform is unsupported

## User-Facing Install Paths

README and docs should converge on three supported install stories:

### Rust users

```bash
cargo install clawbro
```

### Binary users

- download from GitHub Releases
- unpack
- run `clawbro setup`

### Node/npm users

```bash
npm i -g clawbro
clawbro setup
```

## Operational Risks

### 1. macOS Gatekeeper

Unsigned binaries may warn on first launch. This is acceptable in phase 1, but should be documented.

### 2. Release asset drift

If npm package versions and GitHub Release assets diverge, installs will fail. The release process must make GitHub Release the dependency of npm publish, not the other way around.

### 3. Platform naming drift

Artifact names must be stable and machine-resolvable. Once published, changing naming conventions becomes expensive.

### 4. Partial publish states

If crates.io succeeds but GitHub Release or npm publish fails, the product can be in a partially shipped state. Documentation should define the release order clearly.

## Final Recommendation

Adopt:

- crates.io for Rust-native users
- GitHub Release as the canonical binary artifact channel
- npm as a thin binary installer and command shim

Do not adopt:

- npm-based local Rust compilation
- a second JS implementation
- backend-specific installers

This is the cleanest path that keeps the architecture elegant and makes `clawbro` much easier to try.
