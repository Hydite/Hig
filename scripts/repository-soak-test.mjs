#!/usr/bin/env node
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const options = parseArguments(process.argv.slice(2));
const durationMinutes = options.durationMinutes ?? Number(process.env.HIG_REPOSITORY_SOAK_MINUTES || (options.mode === "release" ? 120 : 0.25));
assert(Number.isFinite(durationMinutes) && durationMinutes > 0, "duration must be greater than zero");
if (options.mode === "release") {
  assert(durationMinutes >= 120, "release soak evidence requires at least 120 minutes");
}

const executable = process.platform === "win32" ? "hig.exe" : "hig";
const higBin = path.resolve(process.env.HIG_BIN || path.join(projectRoot, "target", "release", executable));
const mcpServer = path.resolve(process.env.HIG_MCP_SERVER || path.join(projectRoot, "packages", "hig-mcp-server", "bin", "hig-mcp-server.js"));
const work = fs.mkdtempSync(path.join(os.tmpdir(), "hig-repository-soak-"));
const workspace = path.join(work, "workspace");
const repositoryState = path.join(workspace, ".hig", "repository");
const restoreRoot = path.join(work, "restores");
const startedAt = new Date();
const report = {
  schema: 1,
  mode: options.mode,
  source_commit: process.env.GITHUB_SHA || await gitCommit(),
  platform: process.platform,
  architecture: process.arch,
  hig_binary: higBin,
  started_at: startedAt.toISOString(),
  requested_duration_seconds: Math.round(durationMinutes * 60),
  operations: { create: 0, modify: 0, rename: 0, delete: 0 },
  snapshots: 0,
  checkpoints: 0,
  mcp_restart_recoveries: 0,
  commit_ids: [],
  interruption: {},
  final_gc: null,
  final_verification: null,
  final_digest: null,
  status: "running"
};

fs.mkdirSync(path.join(workspace, "src"), { recursive: true });
fs.mkdirSync(path.join(workspace, "mutable"), { recursive: true });
fs.mkdirSync(restoreRoot, { recursive: true });
fs.writeFileSync(path.join(workspace, "README.md"), "HIG native repository soak fixture\n");
fs.writeFileSync(path.join(workspace, "src", "state.rs"), "pub const STATE: u64 = 0;\n");
fs.writeFileSync(path.join(workspace, "mutable", "rename-a.txt"), "rename generation 0\n");

async function main() {
  let mcp = null;
  try {
  await runHig(["repo", "init", workspace, "--json"]);
  const baseline = await runHig(["repo", "snapshot", workspace, "--message", "soak baseline", "--author", "hig-soak", "--json"]);
  recordCommit(baseline.commit_id);

  mcp = await McpClient.start({ higBin, mcpServer, workspace, allowedRoot: work });
  await mcp.tool("hig_repo_watch_start", {
    dir: workspace,
    debounceMs: options.mode === "release" ? 500 : 100,
    message: "native soak automatic snapshot",
    author: "hig-soak"
  });

  const deadline = Date.now() + durationMinutes * 60_000;
  const restartThresholds = [startedAt.getTime() + durationMinutes * 20_000, startedAt.getTime() + durationMinutes * 40_000];
  let iteration = 0;
  while (Date.now() < deadline || iteration < 4) {
    iteration += 1;
    mutateWorkspace(iteration);
    const status = await waitForWatcherSnapshot(mcp);
    report.snapshots += 1;
    recordCommit(status.data.last_snapshot.commit_id);

    const checkpointEvery = options.mode === "release" ? 1 : 2;
    if (iteration % checkpointEvery === 0) await verifyCheckpoint(`iteration-${iteration}`);

    const ciRestartFallback = options.mode === "ci" && iteration >= report.mcp_restart_recoveries + 2;
    if (restartThresholds.length > 0 && (Date.now() >= restartThresholds[0] || ciRestartFallback)) {
      restartThresholds.shift();
      mcp = await exerciseMcpRestart(mcp, iteration);
    }
    if (Date.now() < deadline) await delay(options.mode === "release" ? 30_000 : 100);
  }

  while (report.mcp_restart_recoveries < 2) {
    mcp = await exerciseMcpRestart(mcp, iteration + report.mcp_restart_recoveries + 1);
  }
  await mcp.tool("hig_repo_watch_stop", { dir: workspace });
  await mcp.close();
  mcp = null;

  await verifyCheckpoint("pre-interruption");
  report.interruption.snapshot = await exerciseSnapshotInterruption();
  report.interruption.gc = await exerciseGcInterruption();
  const completed = await runHig(["repo", "snapshot", workspace, "--message", "post-interruption publication", "--author", "hig-soak", "--json"]);
  assert.equal(completed.created, true, "post-interruption snapshot must publish pending state");
  recordCommit(completed.commit_id);
  report.interruption.restore = await exerciseRestoreInterruption();
  await verifyCheckpoint("final");

  const finalGc = await runHig(["repo", "gc", workspace, "--apply", "--json"]);
  assert.equal(finalGc.unreachable_objects, 0, "final GC found unreachable objects");
  assert.equal(finalGc.temporary_files, 0, "final GC found temporary objects");
  report.final_gc = finalGc;
  report.final_verification = await runHig(["repo", "verify", workspace, "--json"]);
  report.final_digest = digestTree(workspace);
  report.status = "passed";
  } catch (error) {
    report.status = "failed";
    report.error = error instanceof Error ? error.stack || error.message : String(error);
    throw error;
  } finally {
    if (mcp) await mcp.close().catch(() => {});
    report.finished_at = new Date().toISOString();
    report.duration_seconds = Number(((Date.now() - startedAt.getTime()) / 1000).toFixed(3));
    const serialized = `${JSON.stringify(report, null, 2)}\n`;
    if (options.report) {
      const output = path.resolve(options.report);
      fs.mkdirSync(path.dirname(output), { recursive: true });
      fs.writeFileSync(output, serialized);
    }
    process.stdout.write(serialized);
    if (!process.env.HIG_REPOSITORY_SOAK_KEEP_WORK) fs.rmSync(work, { recursive: true, force: true });
  }
}

function mutateWorkspace(iteration) {
  fs.writeFileSync(path.join(workspace, "src", "state.rs"), `pub const STATE: u64 = ${iteration};\n`);
  report.operations.modify += 1;

  const created = path.join(workspace, "mutable", `entry-${String(iteration).padStart(6, "0")}.txt`);
  fs.writeFileSync(created, `created iteration ${iteration}\n${"x".repeat(iteration % 127)}\n`);
  report.operations.create += 1;

  const from = path.join(workspace, "mutable", iteration % 2 === 0 ? "rename-b.txt" : "rename-a.txt");
  const to = path.join(workspace, "mutable", iteration % 2 === 0 ? "rename-a.txt" : "rename-b.txt");
  fs.renameSync(from, to);
  fs.appendFileSync(to, `rename generation ${iteration}\n`);
  report.operations.rename += 1;

  if (iteration > 8) {
    const expired = path.join(workspace, "mutable", `entry-${String(iteration - 8).padStart(6, "0")}.txt`);
    if (fs.existsSync(expired)) {
      fs.rmSync(expired);
      report.operations.delete += 1;
    }
  }
}

async function exerciseMcpRestart(client, iteration) {
  const headBefore = readHead();
  await client.killHard();
  await delay(750);
  fs.writeFileSync(path.join(workspace, "src", "offline.rs"), `pub const OFFLINE: u64 = ${iteration};\n`);
  report.operations.modify += 1;
  await delay(options.mode === "release" ? 1500 : 500);
  assert.equal(readHead(), headBefore, "orphan watcher published after MCP was terminated");

  const restarted = await McpClient.start({ higBin, mcpServer, workspace, allowedRoot: work });
  await restarted.tool("hig_repo_watch_start", {
    dir: workspace,
    debounceMs: options.mode === "release" ? 500 : 100,
    message: "native soak restart catch-up",
    author: "hig-soak"
  });
  const status = await waitForWatcherSnapshot(restarted);
  assert.equal(status.data.last_snapshot.created, true, "MCP restart did not publish catch-up state");
  assert.notEqual(status.data.last_snapshot.commit_id, headBefore, "catch-up did not advance HEAD");
  report.snapshots += 1;
  report.mcp_restart_recoveries += 1;
  recordCommit(status.data.last_snapshot.commit_id);
  await runHig(["repo", "verify", workspace, "--json"]);
  return restarted;
}

async function verifyCheckpoint(label) {
  const verify = await runHig(["repo", "verify", workspace, "--json"]);
  assert(verify.checked_objects > 0, `${label}: verify did not inspect objects`);
  const output = path.join(restoreRoot, `${label}-${report.checkpoints}`);
  const restored = await runHig(["repo", "restore", workspace, "--revision", "HEAD", "--output-dir", output, "--json"]);
  assert.equal(restored.commit_id, readHead(), `${label}: restore resolved the wrong commit`);
  const expected = digestTree(workspace);
  const actual = digestTree(output);
  assert.equal(actual, expected, `${label}: restored tree digest differs from workspace`);
  report.checkpoints += 1;
  fs.rmSync(output, { recursive: true, force: true });
}

async function exerciseSnapshotInterruption() {
  const interruptRoot = path.join(workspace, "interrupt-snapshot");
  fs.mkdirSync(interruptRoot, { recursive: true });
  const block = Buffer.allocUnsafe(1024 * 1024);
  for (let file = 0; file < 48; file += 1) {
    fillDeterministic(block, file + 1);
    fs.writeFileSync(path.join(interruptRoot, `payload-${String(file).padStart(3, "0")}.bin`), block);
  }
  const headBefore = readHead();
  const objectsBefore = countRepositoryObjects();
  const child = spawn(higBin, ["repo", "snapshot", workspace, "--message", "interrupted snapshot", "--author", "hig-soak", "--json"], {
    cwd: projectRoot,
    stdio: ["ignore", "pipe", "pipe"]
  });
  const observed = await waitFor(() => countRepositoryObjects() > objectsBefore, 60_000, "snapshot immutable object publication");
  assert(observed, "snapshot interruption did not observe a new immutable object");
  const objectsObserved = countRepositoryObjects();
  const result = await terminateProcess(child);
  assert.notEqual(result.code, 0, "interrupted snapshot unexpectedly completed");
  assert.equal(readHead(), headBefore, "interrupted snapshot advanced HEAD");
  await runHig(["repo", "verify", workspace, "--json"]);
  const dry = await runHig(["repo", "gc", workspace, "--json"]);
  assert(dry.unreachable_objects > 0 || dry.temporary_files > 0, "interrupted snapshot left no collectable state");
  return {
    killed_after_object_publication: true,
    head_unchanged: true,
    objects_before: objectsBefore,
    objects_observed: objectsObserved,
    unreachable_objects: dry.unreachable_objects,
    temporary_files: dry.temporary_files
  };
}

async function exerciseGcInterruption() {
  const tempRoot = path.join(repositoryState, "objects", ".soak-gc-interruption");
  fs.mkdirSync(tempRoot, { recursive: true });
  const temporaryCount = options.mode === "release" ? 50_000 : 20_000;
  for (let index = 0; index < temporaryCount; index += 1) {
    fs.writeFileSync(path.join(tempRoot, `.soak.tmp.${String(index).padStart(6, "0")}`), "interrupted gc fixture\n");
  }
  const headBefore = readHead();
  const deletionStarted = new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      watcher.close();
      reject(new Error("timed out waiting for GC deletion to begin"));
    }, 60_000);
    const watcher = fs.watch(tempRoot, (eventType, name) => {
      if (eventType !== "rename" || !name) return;
      const item = path.join(tempRoot, String(name));
      if (fs.existsSync(item)) return;
      clearTimeout(timeout);
      watcher.close();
      resolve();
    });
  });
  const child = spawn(higBin, ["repo", "gc", workspace, "--apply", "--json"], {
    cwd: projectRoot,
    stdio: ["ignore", "pipe", "pipe"]
  });
  await deletionStarted;
  assert.equal(child.exitCode, null, "GC completed before it could be interrupted");
  const result = await terminateProcess(child);
  assert.notEqual(result.code, 0, "interrupted GC unexpectedly completed");
  assert.equal(readHead(), headBefore, "interrupted GC changed HEAD");
  await runHig(["repo", "verify", workspace, "--json"]);
  const remainingBeforeRecovery = countFiles(tempRoot);
  assert(remainingBeforeRecovery > 0, "interrupted GC left no recovery work");

  const recovered = await runHig(["repo", "gc", workspace, "--apply", "--json"]);
  assert(recovered.removed_objects > 0 || recovered.removed_temporary_files > 0, "recovery GC removed no interrupted state");
  const repeated = await runHig(["repo", "gc", workspace, "--apply", "--json"]);
  assert.equal(repeated.unreachable_objects, 0, "repeated GC found unreachable objects");
  assert.equal(repeated.temporary_files, 0, "repeated GC found temporary objects");
  await runHig(["repo", "verify", workspace, "--json"]);
  return {
    killed_after_deletion_started: true,
    head_unchanged: true,
    temporary_files_created: temporaryCount,
    temporary_files_remaining_before_recovery: remainingBeforeRecovery,
    recovery_removed_objects: recovered.removed_objects,
    recovery_removed_temporary_files: recovered.removed_temporary_files,
    repository_verified: true,
    idempotent: true
  };
}

async function exerciseRestoreInterruption() {
  const output = path.join(work, "interrupted-restore");
  const child = spawn(higBin, ["repo", "restore", workspace, "--revision", "HEAD", "--output-dir", output, "--json"], {
    cwd: projectRoot,
    stdio: ["ignore", "pipe", "pipe"]
  });
  const stagePrefix = `.${path.basename(output)}.hig-restore-stage.`;
  const observedStage = await waitFor(() => {
    const stage = fs.readdirSync(path.dirname(output), { withFileTypes: true })
      .find((entry) => entry.isDirectory() && entry.name.startsWith(stagePrefix));
    if (!stage) return null;
    const stagePath = path.join(path.dirname(output), stage.name);
    return countEntries(stagePath) > 1 ? stagePath : null;
  }, 60_000, "restore staging publication");
  assert(observedStage, "restore interruption did not observe staged files");
  const result = await terminateProcess(child);
  assert.notEqual(result.code, 0, "interrupted restore unexpectedly completed");
  assert.equal(fs.existsSync(output), false, "interrupted restore partially published its destination");
  await runHig(["repo", "verify", workspace, "--json"]);
  fs.rmSync(observedStage, { recursive: true, force: true });
  return { killed_after_staging: true, destination_unpublished: true, repository_verified: true };
}

async function waitForWatcherSnapshot(client) {
  const status = await waitFor(async () => {
    const status = await client.tool("hig_repo_watch_status", { dir: workspace });
    return status.data.snapshots > client.watcherSnapshots ? status : null;
  }, options.mode === "release" ? 5 * 60_000 : 30_000, "repository watcher snapshot");
  client.watcherSnapshots = status.data.snapshots;
  return status;
}

function readHead() {
  return fs.readFileSync(path.join(repositoryState, "refs", "heads", "main"), "utf8").trim();
}

function recordCommit(commitId) {
  if (commitId && report.commit_ids.at(-1) !== commitId) report.commit_ids.push(commitId);
}

function digestTree(root) {
  const hash = crypto.createHash("sha256");
  const visit = (directory, relative = "") => {
    const entries = fs.readdirSync(directory, { withFileTypes: true })
      .filter((entry) => !(relative === "" && entry.name === ".hig"))
      .sort((left, right) => left.name.localeCompare(right.name, "en"));
    for (const entry of entries) {
      const itemRelative = relative ? `${relative}/${entry.name}` : entry.name;
      const item = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        hash.update(`d\0${itemRelative}\0`);
        visit(item, itemRelative);
      } else if (entry.isFile()) {
        const bytes = fs.readFileSync(item);
        hash.update(`f\0${itemRelative}\0${bytes.length}\0`);
        hash.update(bytes);
      } else if (entry.isSymbolicLink()) {
        hash.update(`l\0${itemRelative}\0${fs.readlinkSync(item)}\0`);
      }
    }
  };
  visit(root);
  return hash.digest("hex");
}

function countRepositoryObjects() {
  return countFiles(path.join(repositoryState, "objects"), (name) => /^[0-9a-fA-F]{62}$/.test(name));
}

function countEntries(root) {
  let count = 0;
  const visit = (directory) => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      count += 1;
      if (entry.isDirectory()) visit(path.join(directory, entry.name));
    }
  };
  visit(root);
  return count;
}

function countFiles(root, predicate = () => true) {
  if (!fs.existsSync(root)) return 0;
  let count = 0;
  const visit = (directory) => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      const item = path.join(directory, entry.name);
      if (entry.isDirectory()) visit(item);
      else if (entry.isFile() && predicate(entry.name)) count += 1;
    }
  };
  visit(root);
  return count;
}

function fillDeterministic(buffer, seed) {
  let value = seed >>> 0;
  for (let offset = 0; offset < buffer.length; offset += 4) {
    value ^= value << 13;
    value ^= value >>> 17;
    value ^= value << 5;
    buffer.writeUInt32LE(value >>> 0, offset);
  }
}

async function runHig(args) {
  const result = await runProcess(higBin, args, { cwd: projectRoot, timeoutMs: 180_000 });
  assert.equal(result.code, 0, `hig ${args.join(" ")} failed\n${result.stderr}\n${result.stdout}`);
  try {
    return JSON.parse(result.stdout.trim());
  } catch (error) {
    throw new Error(`hig ${args.join(" ")} returned invalid JSON: ${error.message}\n${result.stdout}`);
  }
}

async function runProcess(command, args, { cwd, timeoutMs }) {
  const child = spawn(command, args, { cwd, stdio: ["ignore", "pipe", "pipe"] });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk) => { stdout += chunk; });
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  const timer = setTimeout(() => child.kill("SIGKILL"), timeoutMs);
  return new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      resolve({ code, signal, stdout, stderr });
    });
  });
}

async function terminateProcess(child) {
  const closed = new Promise((resolve) => child.once("close", (code, signal) => resolve({ code, signal })));
  child.kill("SIGKILL");
  return Promise.race([
    closed,
    delay(10_000).then(() => { throw new Error("terminated child did not exit"); })
  ]);
}

async function waitFor(operation, timeoutMs, description) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = await operation();
    if (value) return value;
    await delay(20);
  }
  throw new Error(`timed out waiting for ${description}`);
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function gitCommit() {
  const result = await runProcess("git", ["rev-parse", "HEAD"], { cwd: projectRoot, timeoutMs: 10_000 });
  return result.code === 0 ? result.stdout.trim() : null;
}

function parseArguments(args) {
  const parsed = { mode: "ci", durationMinutes: null, report: null };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--mode") parsed.mode = args[++index];
    else if (argument === "--duration-minutes") parsed.durationMinutes = Number(args[++index]);
    else if (argument === "--report") parsed.report = args[++index];
    else throw new Error(`unknown argument: ${argument}`);
  }
  assert(["ci", "release"].includes(parsed.mode), "mode must be ci or release");
  return parsed;
}

class McpClient {
  static async start({ higBin: binary, mcpServer: server, workspace: root, allowedRoot }) {
    const client = new McpClient(binary, server, root, allowedRoot);
    await client.initialize();
    return client;
  }

  constructor(binary, server, root, allowedRoot) {
    this.nextId = 1;
    this.watcherSnapshots = 0;
    this.pending = new Map();
    this.output = Buffer.alloc(0);
    this.stderr = "";
    this.child = spawn(process.execPath, [server], {
      cwd: root,
      env: { ...process.env, HIG_BIN: binary, HIG_MCP_ALLOWED_ROOTS: allowedRoot, HIG_MCP_WORKDIR: root, HIG_MCP_TIMEOUT_MS: "180000" },
      stdio: ["pipe", "pipe", "pipe"]
    });
    this.child.stdout.on("data", (chunk) => {
      this.output = Buffer.concat([this.output, chunk]);
      this.drain();
    });
    this.child.stderr.on("data", (chunk) => { this.stderr += chunk.toString("utf8"); });
    this.child.on("close", (code, signal) => {
      for (const waiter of this.pending.values()) waiter.reject(new Error(`MCP exited code=${code} signal=${signal}\n${this.stderr}`));
      this.pending.clear();
    });
  }

  async initialize() {
    const response = await this.request("initialize", { protocolVersion: "2024-11-05", capabilities: {}, clientInfo: { name: "hig-repository-soak", version: "1" } });
    assert.equal(response.result?.protocolVersion, "2024-11-05", "MCP initialization failed");
  }

  async tool(name, args = {}) {
    const response = await this.request("tools/call", { name, arguments: args });
    assert.equal(response.error, undefined, `${name}: JSON-RPC error`);
    const payload = JSON.parse(response.result.content[0].text);
    assert.equal(response.result.isError, false, `${name}: ${payload.stderr || payload.error}`);
    assert.equal(payload.ok, true, `${name}: operation failed`);
    return payload;
  }

  request(method, params) {
    const id = this.nextId++;
    const bytes = Buffer.from(JSON.stringify({ jsonrpc: "2.0", id, method, params }), "utf8");
    this.child.stdin.write(`Content-Length: ${bytes.length}\r\n\r\n`);
    this.child.stdin.write(bytes);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`MCP request timed out: ${method}`));
      }, 180_000);
      this.pending.set(id, {
        resolve: (value) => { clearTimeout(timer); resolve(value); },
        reject: (error) => { clearTimeout(timer); reject(error); }
      });
    });
  }

  drain() {
    for (;;) {
      const headerEnd = this.output.indexOf("\r\n\r\n");
      if (headerEnd < 0) return;
      const header = this.output.subarray(0, headerEnd).toString("utf8");
      const match = /^Content-Length:\s*(\d+)/im.exec(header);
      assert(match, `MCP response is missing Content-Length: ${header}`);
      const length = Number(match[1]);
      const bodyStart = headerEnd + 4;
      if (this.output.length < bodyStart + length) return;
      const body = JSON.parse(this.output.subarray(bodyStart, bodyStart + length).toString("utf8"));
      this.output = this.output.subarray(bodyStart + length);
      const waiter = this.pending.get(body.id);
      if (waiter) {
        this.pending.delete(body.id);
        waiter.resolve(body);
      }
    }
  }

  async killHard() {
    if (this.child.exitCode !== null) return;
    await terminateProcess(this.child);
  }

  async close() {
    if (this.child.exitCode !== null) return;
    const closed = new Promise((resolve) => this.child.once("close", resolve));
    this.child.stdin.end();
    const graceful = await Promise.race([closed.then(() => true), delay(5000).then(() => false)]);
    if (!graceful && this.child.exitCode === null) await this.killHard();
  }
}

await main();
