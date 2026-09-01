#!/usr/bin/env node
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { McpClient as IsolatedMcpClient } from "./lib/native-soak-runtime.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const server = process.env.HIG_MCP_SERVER
  ? path.resolve(process.env.HIG_MCP_SERVER)
  : path.join(root, "packages", "hig-mcp-server", "bin", "hig-mcp-server.js");
const defaultBinary = path.join(root, "target", "release", process.platform === "win32" ? "hig.exe" : "hig");
const higBin = process.env.HIG_BIN || defaultBinary;
const work = fs.mkdtempSync(path.join(os.tmpdir(), "hig-mcp-integration-"));
const workspace = path.join(work, "workspace");
const outside = fs.mkdtempSync(path.join(os.tmpdir(), "hig-mcp-outside-"));
const escapeLink = path.join(work, "escape-link");
const cache = path.join(work, "cache");

fs.mkdirSync(path.join(workspace, "src"), { recursive: true });
fs.writeFileSync(path.join(workspace, "README.md"), "synthetic MCP integration fixture\n");
fs.writeFileSync(path.join(workspace, "src", "lib.rs"), "pub fn mcp_fixture() -> u8 { 1 }\n");
fs.writeFileSync(path.join(outside, "outside.txt"), "must remain outside the MCP root\n");
fs.symlinkSync(outside, escapeLink, process.platform === "win32" ? "junction" : "dir");
const expectedInputBytes = fs.statSync(path.join(workspace, "README.md")).size
  + fs.statSync(path.join(workspace, "src", "lib.rs")).size;

const child = spawn(process.execPath, [server], {
  cwd: workspace,
  env: {
    ...process.env,
    HIG_BIN: higBin,
    HIG_MCP_ALLOWED_ROOTS: work,
    HIG_MCP_WORKDIR: workspace,
    HIG_MCP_TIMEOUT_MS: "120000"
  },
  stdio: ["pipe", "pipe", "pipe"]
});

let nextId = 1;
let output = Buffer.alloc(0);
let stderr = "";
const pending = new Map();

child.stderr.on("data", (chunk) => {
  stderr += chunk.toString("utf8");
});
child.stdout.on("data", (chunk) => {
  output = Buffer.concat([output, chunk]);
  drainFrames();
});
child.on("exit", (code, signal) => {
  for (const { reject } of pending.values()) {
    reject(new Error(`MCP server exited code=${code} signal=${signal}\n${stderr}`));
  }
  pending.clear();
});

function drainFrames() {
  for (;;) {
    const headerEnd = output.indexOf("\r\n\r\n");
    if (headerEnd < 0) return;
    const header = output.subarray(0, headerEnd).toString("utf8");
    const match = /^Content-Length:\s*(\d+)/im.exec(header);
    assert(match, `MCP response is missing Content-Length: ${header}`);
    const length = Number(match[1]);
    const bodyStart = headerEnd + 4;
    if (output.length < bodyStart + length) return;
    const body = JSON.parse(output.subarray(bodyStart, bodyStart + length).toString("utf8"));
    output = output.subarray(bodyStart + length);
    const waiter = pending.get(body.id);
    if (waiter) {
      pending.delete(body.id);
      waiter.resolve(body);
    }
  }
}

function request(method, params = {}) {
  const id = nextId++;
  const message = JSON.stringify({ jsonrpc: "2.0", id, method, params });
  const body = Buffer.from(message, "utf8");
  child.stdin.write(`Content-Length: ${body.length}\r\n\r\n`);
  child.stdin.write(body);
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`MCP request timed out: ${method}`));
    }, 120000);
    pending.set(id, {
      resolve: (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      reject: (error) => {
        clearTimeout(timer);
        reject(error);
      }
    });
  });
}

async function tool(name, args = {}, expectSuccess = true) {
  const response = await request("tools/call", { name, arguments: args });
  assert.equal(response.error, undefined, `${name}: JSON-RPC error`);
  const result = response.result;
  assert.equal(typeof result?.isError, "boolean", `${name}: missing isError`);
  const payload = JSON.parse(result.content[0].text);
  assert.equal(
    result.isError,
    !expectSuccess,
    `${name}: unexpected MCP status\n${JSON.stringify(payload, null, 2)}`
  );
  if (expectSuccess) {
    assert.equal(payload.ok, true, `${name}: ${payload.stderr || payload.stdout}`);
    assert.equal(payload.code, 0, `${name}: nonzero CLI code`);
  }
  return payload;
}

const requiredTools = [
  "hig_version", "hig_help", "hig_init_project", "hig_project_status",
  "hig_project_rebuild", "hig_project_policy_show", "hig_project_policy_set",
  "hig_daemon_status", "hig_daemon_start", "hig_daemon_stop",
  "hig_session_status", "hig_session_unlock", "hig_session_clear", "hig_pack",
  "hig_unpack", "hig_inspect", "hig_migrate", "hig_cache_status", "hig_cache_gc",
  "hig_cache_compact", "hig_task_list", "hig_task_status", "hig_task_cancel",
  "hig_task_result", "hig_repo_init", "hig_repo_snapshot", "hig_repo_refs",
  "hig_repo_migrate", "hig_repo_watch_start", "hig_repo_watch_status",
  "hig_repo_watch_stop", "hig_repo_branch_list", "hig_repo_branch_create",
  "hig_repo_branch_switch", "hig_repo_branch_delete", "hig_repo_tag_list",
  "hig_repo_tag_create", "hig_repo_tag_delete", "hig_repo_log", "hig_repo_diff",
  "hig_repo_path_history", "hig_repo_restore", "hig_repo_restore_range",
  "hig_repo_storage_tree", "hig_repo_symbols", "hig_repo_symbol_history",
  "hig_repo_restore_symbol", "hig_repo_verify", "hig_repo_gc",
  "hig_recovery_init", "hig_recovery_register", "hig_recovery_capture",
  "hig_recovery_list", "hig_recovery_status", "hig_recovery_promote", "hig_recovery_audit", "hig_recovery_pin", "hig_recovery_unpin",
  "hig_recovery_tombstone", "hig_recovery_policy_show", "hig_recovery_policy_set",
  "hig_recovery_gc", "hig_recovery_scrub", "hig_recovery_repair",
  "hig_recovery_verify", "hig_recovery_restore", "hig_bench"
];

try {
  const initialized = await request("initialize", {
    protocolVersion: "2024-11-05",
    capabilities: {},
    clientInfo: { name: "hig-ci", version: "1" }
  });
  assert.equal(initialized.result.protocolVersion, "2024-11-05");
  assert.equal(initialized.result.serverInfo.name, "hig-mcp-server");
  assert.equal(initialized.result.serverInfo.version, "1.10.0");

  const unsupported = await request("initialize", {
    protocolVersion: "2099-01-01",
    capabilities: {},
    clientInfo: { name: "hig-compatibility-probe", version: "1" }
  });
  assert.equal(
    unsupported.result.protocolVersion,
    "2024-11-05",
    "server must not claim an unsupported MCP protocol version"
  );

  const listed = await request("tools/list");
  const names = listed.result.tools.map((entry) => entry.name).sort();
  assert.deepEqual(names, [...requiredTools].sort(), "MCP tool contract changed");
  for (const entry of listed.result.tools) {
    assert.equal(entry.inputSchema.additionalProperties, false, `${entry.name}: arguments must be closed`);
  }

  const version = await tool("hig_version");
  assert.match(version.stdout, /^hig 1\.10\.0$/);

  const archive = path.join(work, "mcp-roundtrip.hig");
  const unpacked = path.join(work, "unpacked");
  await tool("hig_pack", {
    inputDir: workspace,
    output: archive,
    cacheDir: cache,
    encryption: "none",
    daemon: "off",
    project: "off",
    speed: "balanced"
  });
  const inspected = await tool("hig_inspect", { archiveFile: archive });
  assert.equal(inspected.data.files.length, 2);
  assert.equal(inspected.data.input_bytes, expectedInputBytes);
  assert.equal(inspected.data.encrypted, false);
  await tool("hig_unpack", { archiveFile: archive, outputDir: unpacked });
  assert.equal(fs.readFileSync(path.join(unpacked, "src", "lib.rs"), "utf8"), fs.readFileSync(path.join(workspace, "src", "lib.rs"), "utf8"));

  const repositoryInitialized = await tool("hig_repo_init", { dir: workspace });
  const first = await tool("hig_repo_snapshot", { dir: workspace, message: "baseline", author: "mcp-ci" });
  assert.equal(first.data.created, true);
  await tool("hig_repo_branch_create", { dir: workspace, name: "feature/mcp" });
  await tool("hig_repo_tag_create", { dir: workspace, name: "mcp-baseline" });
  const refs = await tool("hig_repo_refs", { dir: workspace });
  assert.equal(refs.data.active_branch, "main");
  assert(refs.data.refs.some((entry) => entry.name === "feature/mcp"));
  assert(refs.data.refs.some((entry) => entry.name === "mcp-baseline"));
  await tool("hig_repo_verify", { dir: workspace });

  const recoveryVault = path.join(work, "recovery-vault");
  const missingVaultRoot = await tool("hig_recovery_list", {}, false);
  assert.match(missingVaultRoot.error, /vaultRoot is required/i);
  const escapedVault = await tool("hig_recovery_init", {
    vaultRoot: path.join(escapeLink, "escaped-vault")
  }, false);
  assert.match(escapedVault.error, /outside allowed roots/i);
  assert.equal(fs.existsSync(path.join(outside, "escaped-vault")), false);
  const recoveryInitialized = await tool("hig_recovery_init", { vaultRoot: recoveryVault });
  assert.equal(recoveryInitialized.data.schema, 1);

  const watchStarted = await tool("hig_repo_watch_start", {
    dir: workspace,
    debounceMs: 100,
    message: "MCP automatic snapshot",
    author: "mcp-ci",
    recoveryVault
  });
  assert.equal(watchStarted.data.active, true);
  await new Promise((resolve) => setTimeout(resolve, 500));
  const automaticContent = "pub fn mcp_fixture() -> u8 { 2 }\n";
  fs.writeFileSync(path.join(workspace, "src", "lib.rs"), automaticContent);
  const watchStatus = await waitFor(async () => {
    const status = await tool("hig_repo_watch_status", { dir: workspace });
    return status.data.last_snapshot?.created === true ? status : null;
  }, 15000, "automatic repository snapshot");
  assert.equal(watchStatus.data.last_snapshot.created, true);
  assert.equal(watchStatus.data.last_snapshot.recovery.schema, 1);
  assert.equal(watchStatus.data.last_snapshot.recovery.recovery_point.durability, "captured");
  assert.equal(watchStatus.data.recovery_durability, "captured");
  assert.equal(watchStatus.data.recovery_durability_lag, true);
  assert(Number.isInteger(watchStatus.data.recovery_rpo_lag_ms));
  const watchStopped = await tool("hig_repo_watch_stop", { dir: workspace });
  assert.equal(watchStopped.data.active, false);
  await tool("hig_repo_verify", { dir: workspace });

  const offlineContent = "pub fn mcp_fixture() -> u8 { 3 }\n";
  fs.writeFileSync(path.join(workspace, "src", "lib.rs"), offlineContent);
  const restarted = await tool("hig_repo_watch_start", {
    dir: workspace,
    debounceMs: 100,
    message: "MCP restarted snapshot",
    author: "mcp-ci",
    recoveryVault
  });
  assert.equal(restarted.data.active, true);
  const catchUp = await waitFor(async () => {
    const status = await tool("hig_repo_watch_status", { dir: workspace });
    return status.data.snapshots >= 1 ? status : null;
  }, 15000, "repository catch-up snapshot");
  assert.equal(catchUp.data.last_snapshot.created, true);
  assert.equal(catchUp.data.last_snapshot.recovery.schema, 1);
  assert(catchUp.data.last_snapshot.recovery.recovery_point.recovery_point_id);
  await tool("hig_repo_watch_stop", { dir: workspace });
  await tool("hig_repo_verify", { dir: workspace });

  const automaticRestore = path.join(work, "automatic-restore");
  await tool("hig_repo_restore", {
    dir: workspace,
    revision: "HEAD",
    outputDir: automaticRestore
  });
  assert.equal(fs.readFileSync(path.join(automaticRestore, "src", "lib.rs"), "utf8"), offlineContent);

  const range = path.join(work, "range.bin");
  await tool("hig_repo_restore_range", {
    dir: workspace,
    revision: "HEAD",
    path: "src/lib.rs",
    start: 4,
    len: 2,
    output: range
  });
  assert.equal(fs.readFileSync(range, "utf8"), "fn");

  const registered = await tool("hig_recovery_register", {
    dir: workspace,
    vaultRoot: recoveryVault
  });
  const captured = await tool("hig_recovery_capture", {
    dir: workspace,
    revision: "HEAD",
    vaultRoot: recoveryVault
  });
  assert.equal(registered.data.schema, 1);
  assert.equal(captured.data.schema, 1);
  assert.equal(captured.data.repository_id.length, 16);
  assert.equal(captured.data.recovery_point.durability, "captured");
  const repositoryId = Buffer.from(captured.data.repository_id).toString("hex");
  assert.equal(repositoryId, Buffer.from(repositoryInitialized.data.repository_id).toString("hex"));
  assert.equal(repositoryId, Buffer.from(registered.data.repository_id).toString("hex"));
  const recoveryPointId = captured.data.recovery_point.recovery_point_id;
  const recoveryList = await tool("hig_recovery_list", { vaultRoot: recoveryVault });
  assert.equal(recoveryList.data.schema, 1);
  assert.equal(recoveryList.data.repositories.length, 1);
  const recoveryStatus = await tool("hig_recovery_status", { vaultRoot: recoveryVault });
  assert.equal(recoveryStatus.data.schema, 1);
  assert.equal(recoveryStatus.data.repositories, 1);
  assert(recoveryStatus.data.recovery_points >= 1);
  assert(recoveryStatus.data.rpo_lag_millis >= 0);
  const promotedMirror = path.join(work, "promoted-mirror");
  const recoveryPromoted = await tool("hig_recovery_promote", {
    vaultRoot: recoveryVault,
    mirrors: [promotedMirror]
  });
  assert.equal(recoveryPromoted.data.schema, 1);
  assert.equal(recoveryPromoted.data.durability, "protected");
  assert.equal(recoveryPromoted.data.mirror_roots.length, 1);
  const constrainedClient = await IsolatedMcpClient.start({
    higBin,
    mcpServer: server,
    workspace,
    allowedRoot: [workspace, recoveryVault].join(path.delimiter)
  });
  try {
    const constrainedResponse = await constrainedClient.request("tools/call", {
      name: "hig_recovery_gc",
      arguments: { vaultRoot: recoveryVault }
    });
    assert.equal(constrainedResponse.result.isError, true);
    assert.match(constrainedResponse.result.content[0].text, /outside allowed roots/i);
  } finally {
    await constrainedClient.close();
  }
  const escapedPromotion = await tool("hig_recovery_promote", {
    vaultRoot: recoveryVault,
    mirrors: [path.join(escapeLink, "escaped-promotion")]
  }, false);
  assert.match(escapedPromotion.error, /outside allowed roots/i);
  assert.equal(fs.existsSync(path.join(outside, "escaped-promotion")), false);
  const protectedStatus = await tool("hig_recovery_status", { vaultRoot: recoveryVault });
  assert.equal(protectedStatus.data.protected_points, protectedStatus.data.recovery_points);
  assert.equal(protectedStatus.data.durability_lag_points, 0);
  const recoveryAudit = await tool("hig_recovery_audit", { vaultRoot: recoveryVault });
  assert.equal(recoveryAudit.data.schema, 1);
  assert.equal(recoveryAudit.data.incomplete_operation_ids.length, 0);
  assert.ok(recoveryAudit.data.events.some((event) => event.operation === "capture"));
  const policy = await tool("hig_recovery_policy_show", { vaultRoot: recoveryVault });
  assert.equal(policy.data.retention.schema, 1);
  const recoveryGc = await tool("hig_recovery_gc", { vaultRoot: recoveryVault });
  assert.equal(recoveryGc.data.schema, 1);
  assert.equal(recoveryGc.data.dry_run, true);
  assert.equal(recoveryGc.data.removed_recovery_points, 0);
  const forgedRecoveryGc = await tool("hig_recovery_gc", {
    vaultRoot: recoveryVault,
    apply: "false"
  }, false);
  assert.match(forgedRecoveryGc.error, /apply must be a boolean/i);
  const pinned = await tool("hig_recovery_pin", {
    repositoryId,
    recoveryPointId,
    vaultRoot: recoveryVault
  });
  assert.equal(pinned.data.schema, 1);
  assert.equal(pinned.data.pinned, true);
  const unpinned = await tool("hig_recovery_unpin", {
    repositoryId,
    recoveryPointId,
    vaultRoot: recoveryVault
  });
  assert.equal(unpinned.data.schema, 1);
  assert.equal(unpinned.data.pinned, false);
  const recoveryVerified = await tool("hig_recovery_verify", {
    repositoryId,
    recoveryPointId,
    vaultRoot: recoveryVault
  });
  assert.equal(recoveryVerified.data.schema, 1);
  const recoveryScrub = await tool("hig_recovery_scrub", { vaultRoot: recoveryVault });
  assert.equal(recoveryScrub.data.schema, 1);
  assert.equal(recoveryScrub.data.healthy, true);
  fs.rmSync(path.join(workspace, "README.md"));
  const tombstone = await tool("hig_recovery_tombstone", {
    repositoryId,
    kind: "file",
    sourcePath: workspace,
    path: "README.md",
    reason: "MCP integration deletion drill",
    vaultRoot: recoveryVault
  });
  assert.equal(tombstone.data.schema, 1);
  assert.equal(tombstone.data.tombstone.kind, "file");
  const traversalOutput = path.join(work, "traversal-output");
  const traversal = await tool("hig_recovery_restore", {
    repositoryId,
    recoveryPointId,
    outputDir: traversalOutput,
    path: "../../outside.txt",
    vaultRoot: recoveryVault
  }, false);
  assert.match(traversal.stderr, /path|relative|unsafe/i);
  assert.equal(fs.existsSync(traversalOutput), false);

  const existingOutput = path.join(work, "existing-recovery-output");
  fs.mkdirSync(existingOutput);
  fs.writeFileSync(path.join(existingOutput, "sentinel.txt"), "preserve\n");
  const overwriteDenied = await tool("hig_recovery_restore", {
    repositoryId,
    recoveryPointId,
    outputDir: existingOutput,
    overwrite: false,
    vaultRoot: recoveryVault
  }, false);
  assert.match(overwriteDenied.stderr, /exist|overwrite/i);
  assert.equal(fs.readFileSync(path.join(existingOutput, "sentinel.txt"), "utf8"), "preserve\n");
  const forgedOverwrite = await tool("hig_recovery_restore", {
    repositoryId,
    recoveryPointId,
    outputDir: existingOutput,
    overwrite: "false",
    vaultRoot: recoveryVault
  }, false);
  assert.match(forgedOverwrite.error, /overwrite must be a boolean/i);
  assert.equal(fs.readFileSync(path.join(existingOutput, "sentinel.txt"), "utf8"), "preserve\n");

  const escapedRestore = await tool("hig_recovery_restore", {
    repositoryId,
    recoveryPointId,
    outputDir: path.join(escapeLink, "escaped-restore"),
    vaultRoot: recoveryVault
  }, false);
  assert.match(escapedRestore.error, /outside allowed roots/i);
  assert.equal(fs.existsSync(path.join(outside, "escaped-restore")), false);
  const recoveryOutput = path.join(work, "recovery-output");
  const recoveryRestored = await tool("hig_recovery_restore", {
    repositoryId,
    recoveryPointId,
    outputDir: recoveryOutput,
    vaultRoot: recoveryVault
  });
  assert.equal(recoveryRestored.data.schema, 1);
  assert.equal(
    fs.readFileSync(path.join(recoveryOutput, "README.md"), "utf8"),
    "synthetic MCP integration fixture\n"
  );

  const denied = await tool("hig_init_project", { dir: outside }, false);
  assert.match(denied.error, /outside allowed roots/i);
  assert.equal(fs.existsSync(path.join(outside, ".hig")), false, "denied path was modified");

  console.log(`hig-mcp-integration: PASS tools=${names.length}`);
} finally {
  child.stdin.end();
  child.kill("SIGTERM");
}

async function waitFor(operation, timeoutMs, description) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = await operation();
    if (value) return value;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for ${description}`);
}
