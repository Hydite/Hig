# HIG Linux CLI and MCP Production Package Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Produce and verify a Linux x86_64 GNU CLI and IDE MCP package for HIG v1.10.0 without changing archive or repository semantics.

**Architecture:** Build `hig-cli` natively on the supplied Ubuntu 24.04 x86_64 host from a source snapshot of the current workspace. Bundle the resulting binary with the existing Node stdio MCP adapter in a platform-labelled tarball. The package embeds no credentials and retains the adapter's allowed-root boundary.

**Tech Stack:** Rust stable, Cargo, Node.js 18+, MCP stdio, GNU tar, SHA-256.

---

### Task 1: Establish Linux build baseline

**Files:**
- Create: `artifacts/hig-v1.10.0-linux-x86_64-gnu-release-qa.md`

**Step 1:** Inspect the build host architecture, OS, libc, disk capacity, Node, Rust, compiler, and linker.

**Step 2:** Install Rust stable into the unprivileged build user's home only if Cargo is absent.

**Step 3:** Record the supported runtime baseline as Ubuntu 24.04 x86_64 / glibc 2.39 and Node 18+.

### Task 2: Build and verify the CLI

**Files:**
- Verify: `crates/hig-cli/src/main.rs`
- Verify: `crates/hig-core/src/repository.rs`

**Step 1:** Transfer a source snapshot that excludes local build outputs, artifacts, private paper inputs, and VCS metadata.

**Step 2:** Run format check, `hig-core` tests, CLI tests, and Clippy with warnings denied.

**Step 3:** Build `cargo build --release -p hig-cli`, then run CLI version, archive round-trip, repository verify, byte-range restore, and symbol restore smoke checks.

### Task 3: Package the IDE adapter

**Files:**
- Verify: `packages/hig-mcp-server/bin/hig-mcp-server.js`
- Modify: `packages/hig-mcp-server/README.md`
- Create: `artifacts/hig-v1.10.0-ide-mcp-linux-x86_64-gnu.tar.gz`
- Create: `artifacts/hig-v1.10.0-ide-mcp-linux-x86_64-gnu.tar.gz.sha256`

**Step 1:** Copy the Linux release binary into the MCP package as `bin/hig`.

**Step 2:** Add Linux runtime/build baseline documentation and a Linux IDE configuration example.

**Step 3:** Create a clean tarball excluding macOS metadata and calculate SHA-256.

### Task 4: Validate the distributable package

**Files:**
- Create: `artifacts/hig-v1.10.0-linux-x86_64-gnu-release-qa.md`

**Step 1:** Extract the tarball into an empty directory on the Linux host.

**Step 2:** Run `bin/hig --version`, MCP `--smoke`, MCP `initialize`, and constrained `tools/call` repository checks from the extracted package.

**Step 3:** Verify the SHA-256 checksum, record package contents and test results, and retain the source workspace unchanged except for packaging documentation and artifacts.
