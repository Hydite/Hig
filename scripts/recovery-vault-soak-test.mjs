#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  McpClient,
  countEntries,
  countFiles,
  delay,
  digestTree,
  fillDeterministic,
  runProcess,
  spawnObserved,
  terminateProcess,
  waitFor,
} from "./lib/native-soak-runtime.mjs";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const options = parseArguments(process.argv.slice(2));
const durationMinutes = options.durationMinutes
  ?? Number(process.env.HIG_RECOVERY_SOAK_MINUTES || (options.mode === "release" ? 120 : 0.25));
assert(Number.isFinite(durationMinutes) && durationMinutes > 0, "duration must be greater than zero");
if (options.mode === "release") {
  assert(durationMinutes >= 120, "release soak evidence requires at least 120 minutes");
}

const executable = process.platform === "win32" ? "hig.exe" : "hig";
const higBin = path.resolve(process.env.HIG_BIN || path.join(projectRoot, "target", "release", executable));
const mcpServer = path.resolve(
  process.env.HIG_MCP_SERVER
    || path.join(projectRoot, "packages", "hig-mcp-server", "bin", "hig-mcp-server.js"),
);
const work = fs.mkdtempSync(path.join(os.tmpdir(), "hig-recovery-vault-soak-"));
process.env.HIG_RECOVERY_AUTH_DIR = path.join(work, "recovery-auth");
const workspace = path.join(work, "workspace");
const primaryVault = path.join(work, "primary-vault");
const mirrorVault = path.join(work, "mirror-vault");
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
  recovery_captures: 0,
  checkpoints: 0,
  mcp_restart_recoveries: 0,
  commit_ids: [],
  recovery_point_ids: [],
  interruption: {},
  source_loss: null,
  primary_vault_loss: null,
  primary_audit_before_loss: null,
  final_gc: null,
  final_scrub: null,
  final_audit: null,
  final_digest: null,
  status: "running",
};

fs.mkdirSync(path.join(workspace, "src"), { recursive: true });
fs.mkdirSync(path.join(workspace, "mutable"), { recursive: true });
fs.mkdirSync(restoreRoot, { recursive: true });
fs.writeFileSync(path.join(workspace, "README.md"), "HIG Recovery Vault native soak fixture\n");
fs.writeFileSync(path.join(workspace, "src", "state.rs"), "pub const STATE: u64 = 0;\n");
fs.writeFileSync(path.join(workspace, "mutable", "rename-a.txt"), "rename generation 0\n");

async function main() {
  let mcp = null;
  try {
    const initialized = await runHig(["repo", "init", workspace, "--json"]);
    const repositoryId = Buffer.from(initialized.repository_id).toString("hex");
    const baseline = await runHig([
      "repo", "snapshot", workspace,
      "--message", "recovery soak baseline",
      "--author", "hig-recovery-soak",
      "--json",
    ]);
    recordCommit(baseline.commit_id);
    await runHig([
      "recovery", "init",
      "--vault-root", primaryVault,
      "--mirror", mirrorVault,
      "--json",
    ]);

    mcp = await McpClient.start({ higBin, mcpServer, workspace, allowedRoot: work });
    await startRecoveryWatcher(mcp, "native Recovery Vault soak");
    recordWatcherCapture(await waitForWatcherSnapshot(mcp), repositoryId);

    const deadline = startedAt.getTime() + durationMinutes * 60_000;
    const restartThresholds = [
      startedAt.getTime() + durationMinutes * 20_000,
      startedAt.getTime() + durationMinutes * 40_000,
    ];
    let iteration = 0;
    while (Date.now() < deadline || iteration < 4) {
      iteration += 1;
      mutateWorkspace(iteration);
      recordWatcherCapture(await waitForWatcherSnapshot(mcp), repositoryId);

      const checkpointEvery = options.mode === "release" ? 10 : 2;
      if (iteration % checkpointEvery === 0) {
        await verifyRecoveryCheckpoint(`iteration-${iteration}`, repositoryId);
      }

      const ciRestartFallback = options.mode === "ci" && iteration >= report.mcp_restart_recoveries + 2;
      if (restartThresholds.length > 0 && (Date.now() >= restartThresholds[0] || ciRestartFallback)) {
        restartThresholds.shift();
        mcp = await exerciseMcpRestart(mcp, iteration, repositoryId);
      }
      if (Date.now() < deadline) await delay(options.mode === "release" ? 30_000 : 100);
    }

    while (report.mcp_restart_recoveries < 2) {
      mcp = await exerciseMcpRestart(mcp, iteration + report.mcp_restart_recoveries + 1, repositoryId);
    }
    await mcp.tool("hig_repo_watch_stop", { dir: workspace });
    await mcp.close();
    mcp = null;

    await verifyRecoveryCheckpoint("pre-interruption", repositoryId);
    report.interruption.capture = await exerciseCaptureInterruption(repositoryId);
    report.interruption.restore = await exerciseRestoreInterruption(
      repositoryId,
      report.interruption.capture.recovery_point_id,
      report.interruption.capture.expected_digest,
    );
    const gcResult = await exerciseGcInterruption(repositoryId);
    report.interruption.gc = gcResult.interruption;
    report.final_gc = gcResult.finalGc;

    const finalPointId = gcResult.recoveryPointId;
    const expectedDigest = digestTree(workspace);
    await verifyPointAt(primaryVault, repositoryId, finalPointId);
    await verifyPointAt(mirrorVault, repositoryId, finalPointId);
    const primaryScrub = await runHig(["recovery", "scrub", "--vault-root", primaryVault, "--json"]);
    assert.equal(primaryScrub.healthy, true, "primary vault scrub failed before loss drill");
    const primaryAudit = await runHig(["recovery", "audit", "--vault-root", primaryVault, "--json"]);
    assert(primaryAudit.incomplete_operation_ids.length >= 3, "process kills were not retained in the audit journal");
    report.primary_audit_before_loss = summarizeAudit(primaryAudit);

    fs.rmSync(workspace, { recursive: true, force: true });
    const sourceLossOutput = path.join(restoreRoot, "source-loss");
    await restorePoint(primaryVault, repositoryId, finalPointId, sourceLossOutput, false);
    assert.equal(digestTree(sourceLossOutput), expectedDigest, "source-loss restore digest mismatch");
    report.source_loss = { workspace_deleted: true, exact_restore: true };

    fs.rmSync(sourceLossOutput, { recursive: true, force: true });
    fs.rmSync(primaryVault, { recursive: true, force: true });
    const primaryLossOutput = path.join(restoreRoot, "primary-loss");
    await restorePoint(mirrorVault, repositoryId, finalPointId, primaryLossOutput, false);
    assert.equal(digestTree(primaryLossOutput), expectedDigest, "primary-vault-loss restore digest mismatch");
    report.primary_vault_loss = { primary_vault_deleted: true, mirror_exact_restore: true };

    report.final_scrub = await runHig(["recovery", "scrub", "--vault-root", mirrorVault, "--json"]);
    assert.equal(report.final_scrub.healthy, true, "surviving mirror scrub failed");
    report.final_audit = summarizeAudit(
      await runHig(["recovery", "audit", "--vault-root", mirrorVault, "--json"]),
    );
    report.final_digest = digestTree(primaryLossOutput);
    assert.equal(report.final_digest, expectedDigest, "final Recovery Vault digest mismatch");
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
    if (!process.env.HIG_RECOVERY_SOAK_KEEP_WORK) fs.rmSync(work, { recursive: true, force: true });
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

async function startRecoveryWatcher(client, message) {
  return client.tool("hig_repo_watch_start", {
    dir: workspace,
    debounceMs: options.mode === "release" ? 500 : 100,
    message,
    author: "hig-recovery-soak",
    recoveryVault: primaryVault,
  });
}

async function waitForWatcherSnapshot(client) {
  let lastStatus;
  let status;
  try {
    status = await waitFor(async () => {
      const current = await client.tool("hig_repo_watch_status", { dir: workspace });
      lastStatus = current;
      if (!current.data.active) {
        throw new Error(`Recovery Vault watcher exited: ${watcherDiagnostics(current)}`);
      }
      return current.data.snapshots > client.watcherSnapshots ? current : null;
    }, options.mode === "release" ? 5 * 60_000 : 60_000, "Recovery Vault watcher snapshot");
  } catch (error) {
    if (lastStatus && !String(error.message).includes("Recovery Vault watcher exited")) {
      error.message = `${error.message}; last watcher status: ${watcherDiagnostics(lastStatus)}`;
    }
    throw error;
  }
  client.watcherSnapshots = status.data.snapshots;
  return status;
}

function watcherDiagnostics(status) {
  const data = status.data;
  return JSON.stringify({
    active: data.active,
    snapshots: data.snapshots,
    exit_code: data.exit_code,
    signal: data.signal,
    recovery_last_success_at: data.recovery_last_success_at,
    recovery_rpo_lag_ms: data.recovery_rpo_lag_ms,
    stderr: typeof data.stderr === "string" ? data.stderr.slice(-8192) : data.stderr,
  });
}

function recordWatcherCapture(status, repositoryId) {
  const snapshot = status.data.last_snapshot;
  assert(snapshot, "watcher did not report its last snapshot");
  assert(snapshot.recovery, "watcher snapshot omitted Recovery Vault capture");
  assert.equal(snapshot.recovery.recovery_point.durability, "protected", "watcher capture was not mirrored");
  assert.equal(Buffer.from(snapshot.recovery.repository_id).toString("hex"), repositoryId);
  report.snapshots += 1;
  report.recovery_captures += 1;
  recordCommit(snapshot.commit_id);
  recordRecoveryPoint(snapshot.recovery.recovery_point.recovery_point_id);
}

async function exerciseMcpRestart(client, iteration, repositoryId) {
  const headBefore = readHead();
  await client.killHard();
  await delay(750);
  fs.writeFileSync(path.join(workspace, "src", "offline.rs"), `pub const OFFLINE: u64 = ${iteration};\n`);
  report.operations.modify += 1;
  await delay(options.mode === "release" ? 1500 : 500);
  assert.equal(readHead(), headBefore, "orphan watcher published after MCP termination");

  const restarted = await McpClient.start({ higBin, mcpServer, workspace, allowedRoot: work });
  await startRecoveryWatcher(restarted, "Recovery Vault restart catch-up");
  const status = await waitForWatcherSnapshot(restarted);
  assert.equal(status.data.last_snapshot.created, true, "MCP restart did not publish catch-up state");
  assert.notEqual(status.data.last_snapshot.commit_id, headBefore, "MCP catch-up did not advance HEAD");
  recordWatcherCapture(status, repositoryId);
  report.mcp_restart_recoveries += 1;
  await runHig(["repo", "verify", workspace, "--json"]);
  return restarted;
}

async function verifyRecoveryCheckpoint(label, repositoryId) {
  await runHig(["repo", "verify", workspace, "--json"]);
  const listed = await listVault(primaryVault);
  const registration = listed.repositories.find(
    (entry) => Buffer.from(entry.repository_id).toString("hex") === repositoryId,
  );
  assert(registration, `${label}: recovery repository is missing`);
  const point = registration.recovery_points[report.recovery_point_ids.at(-1)];
  assert(point, `${label}: no available recovery point`);
  await verifyPointAt(primaryVault, repositoryId, point.recovery_point_id);
  await verifyPointAt(mirrorVault, repositoryId, point.recovery_point_id);
  const output = path.join(restoreRoot, `${label}-${report.checkpoints}`);
  await restorePoint(primaryVault, repositoryId, point.recovery_point_id, output, false);
  assert.equal(digestTree(output), digestTree(workspace), `${label}: exact restore digest mismatch`);
  report.checkpoints += 1;
  fs.rmSync(output, { recursive: true, force: true });
}

async function exerciseCaptureInterruption(repositoryId) {
  const interruptRoot = path.join(workspace, "capture-interruption");
  fs.mkdirSync(interruptRoot, { recursive: true });
  const fileCount = options.mode === "release" ? 2048 : 512;
  const block = Buffer.allocUnsafe(64 * 1024);
  for (let index = 0; index < fileCount; index += 1) {
    fillDeterministic(block, index + 1);
    fs.writeFileSync(path.join(interruptRoot, `payload-${String(index).padStart(5, "0")}.bin`), block);
  }
  const snapshot = await runHig([
    "repo", "snapshot", workspace,
    "--message", "capture interruption payload",
    "--author", "hig-recovery-soak",
    "--json",
  ]);
  assert.equal(snapshot.created, true, "capture interruption fixture did not create a revision");
  recordCommit(snapshot.commit_id);
  const expectedDigest = digestTree(workspace);
  const preparedBefore = preparedOperationIds(primaryVault);
  const objectsBefore = countVaultObjects(primaryVault, repositoryId);
  const child = spawnObserved(higBin, [
    "recovery", "capture", workspace,
    "--revision", "HEAD",
    "--vault-root", primaryVault,
    "--json",
  ], projectRoot);
  const observation = await waitFor(() => {
    const operationId = findNewPreparedOperation(primaryVault, preparedBefore, "capture");
    const objectCount = countVaultObjects(primaryVault, repositoryId);
    if (child.exitCode !== null) throw new Error("capture completed before interruption could be injected");
    return operationId && objectCount > objectsBefore ? { operationId, objectCount } : null;
  }, 120_000, "capture audit and immutable object publication");
  const terminated = await terminateProcess(child);
  assert.notEqual(terminated.code, 0, "interrupted recovery capture unexpectedly completed");
  const audit = await runHig(["recovery", "audit", "--vault-root", primaryVault, "--json"]);
  assert(audit.incomplete_operation_ids.includes(observation.operationId), "capture interruption is absent from audit");

  const recovered = await captureHead();
  assert.equal(recovered.recovery_point.recovery_point_id, snapshot.commit_id);
  assert.equal(recovered.recovery_point.durability, "protected");
  await verifyPointAt(primaryVault, repositoryId, snapshot.commit_id);
  await verifyPointAt(mirrorVault, repositoryId, snapshot.commit_id);
  recordRecoveryPoint(snapshot.commit_id);
  return {
    operation_id: observation.operationId,
    killed_after_prepared: true,
    killed_after_object_publication: true,
    objects_before: objectsBefore,
    objects_observed: observation.objectCount,
    retry_protected: true,
    recovery_point_id: snapshot.commit_id,
    expected_digest: expectedDigest,
  };
}

async function exerciseRestoreInterruption(repositoryId, recoveryPointId, expectedDigest) {
  const output = path.join(restoreRoot, "interrupted-restore");
  const preparedBefore = preparedOperationIds(primaryVault);
  const child = spawnObserved(higBin, [
    "recovery", "restore", repositoryId, recoveryPointId,
    "--output-dir", output,
    "--vault-root", primaryVault,
    "--json",
  ], projectRoot);
  const stagePrefix = `.${path.basename(output)}.hig-restore-stage.`;
  const observation = await waitFor(() => {
    const operationId = findNewPreparedOperation(primaryVault, preparedBefore, "restore");
    const stage = fs.readdirSync(path.dirname(output), { withFileTypes: true })
      .find((entry) => entry.isDirectory() && entry.name.startsWith(stagePrefix));
    if (child.exitCode !== null) throw new Error("restore completed before interruption could be injected");
    if (!operationId || !stage) return null;
    const stagePath = path.join(path.dirname(output), stage.name);
    return countEntries(stagePath) > 10 ? { operationId, stagePath } : null;
  }, 120_000, "recovery restore staging publication");
  const terminated = await terminateProcess(child);
  assert.notEqual(terminated.code, 0, "interrupted recovery restore unexpectedly completed");
  assert.equal(fs.existsSync(output), false, "interrupted recovery restore published a partial destination");
  const audit = await runHig(["recovery", "audit", "--vault-root", primaryVault, "--json"]);
  assert(audit.incomplete_operation_ids.includes(observation.operationId), "restore interruption is absent from audit");
  fs.rmSync(observation.stagePath, { recursive: true, force: true });

  await restorePoint(primaryVault, repositoryId, recoveryPointId, output, false);
  assert.equal(digestTree(output), expectedDigest, "restore interruption retry digest mismatch");
  fs.rmSync(output, { recursive: true, force: true });
  return {
    operation_id: observation.operationId,
    killed_after_prepared: true,
    killed_after_staging: true,
    destination_unpublished: true,
    exact_retry: true,
  };
}

async function exerciseGcInterruption(repositoryId) {
  fs.rmSync(path.join(workspace, "capture-interruption"), { recursive: true, force: true });
  report.operations.delete += 1;
  await snapshotAndCapture("post-interruption payload cleanup");
  for (let index = 0; index < 2; index += 1) {
    fs.writeFileSync(path.join(workspace, "src", "gc-marker.rs"), `pub const GC_MARKER: u8 = ${index};\n`);
    report.operations.modify += 1;
    await snapshotAndCapture(`GC recovery revision ${index}`);
  }
  const latest = report.recovery_point_ids.at(-1);
  const before = await listVault(primaryVault);
  const totalBefore = Object.keys(before.repositories[0].recovery_points).length;
  assert(totalBefore >= 4, "GC interruption requires multiple recovery points");
  await runHig([
    "recovery", "policy", "set",
    "--vault-root", primaryVault,
    "--minimum-points", "1",
    "--minimum-retention-days", "0",
    "--maximum-points", "1",
    "--json",
  ]);

  const temporaryCount = options.mode === "release" ? 30_000 : 5_000;
  createTemporaryObjectLoad(primaryVault, repositoryId, temporaryCount);
  createTemporaryObjectLoad(mirrorVault, repositoryId, temporaryCount);
  const preparedBefore = preparedOperationIds(primaryVault);
  const child = spawnObserved(higBin, [
    "recovery", "gc",
    "--vault-root", primaryVault,
    "--apply",
    "--json",
  ], projectRoot);
  const observation = await waitFor(() => {
    const operationId = findNewPreparedOperation(primaryVault, preparedBefore, "garbage_collection");
    const pending = pendingRecoveryPointCount(primaryVault, repositoryId);
    if (child.exitCode !== null) throw new Error("Recovery Vault GC completed before interruption could be injected");
    return operationId && pending > 0 ? { operationId, pending } : null;
  }, 180_000, "Recovery Vault GC pending-deletion catalog publication");
  const terminated = await terminateProcess(child);
  assert.notEqual(terminated.code, 0, "interrupted Recovery Vault GC unexpectedly completed");
  const audit = await runHig(["recovery", "audit", "--vault-root", primaryVault, "--json"]);
  assert(audit.incomplete_operation_ids.includes(observation.operationId), "GC interruption is absent from audit");

  const recovered = await runHig(["recovery", "gc", "--vault-root", primaryVault, "--apply", "--json"]);
  const repeated = await runHig(["recovery", "gc", "--vault-root", primaryVault, "--apply", "--json"]);
  assert.equal(repeated.candidate_recovery_points, 0, "repeated Recovery Vault GC found candidates");
  assert.equal(repeated.removed_recovery_points, 0, "repeated Recovery Vault GC removed state");
  const finalList = await listVault(primaryVault);
  assert.equal(Object.keys(finalList.repositories[0].recovery_points).length, 1);
  assert(finalList.repositories[0].recovery_points[latest], "GC removed the newest recovery point");
  assert.equal(countTemporaryObjectLoad(primaryVault, repositoryId), 0, "primary temporary objects survived GC retry");
  assert.equal(countTemporaryObjectLoad(mirrorVault, repositoryId), 0, "mirror temporary objects survived GC retry");
  await verifyPointAt(primaryVault, repositoryId, latest);
  await verifyPointAt(mirrorVault, repositoryId, latest);
  return {
    interruption: {
      operation_id: observation.operationId,
      killed_after_prepared: true,
      killed_after_pending_catalog: true,
      pending_recovery_points: observation.pending,
      temporary_objects_per_vault: temporaryCount,
      exact_retry: true,
      idempotent: true,
    },
    finalGc: { recovered: summarizeGc(recovered), repeated: summarizeGc(repeated) },
    recoveryPointId: latest,
  };
}

async function snapshotAndCapture(message) {
  const snapshot = await runHig([
    "repo", "snapshot", workspace,
    "--message", message,
    "--author", "hig-recovery-soak",
    "--json",
  ]);
  assert.equal(snapshot.created, true, `${message}: snapshot was unchanged`);
  recordCommit(snapshot.commit_id);
  const capture = await captureHead();
  assert.equal(capture.recovery_point.recovery_point_id, snapshot.commit_id);
  assert.equal(capture.recovery_point.durability, "protected");
  recordRecoveryPoint(snapshot.commit_id);
  return capture;
}

async function captureHead() {
  report.recovery_captures += 1;
  return runHig([
    "recovery", "capture", workspace,
    "--revision", "HEAD",
    "--vault-root", primaryVault,
    "--json",
  ]);
}

async function verifyPointAt(vault, repositoryId, recoveryPointId) {
  return runHig([
    "recovery", "verify", repositoryId, recoveryPointId,
    "--vault-root", vault,
    "--json",
  ]);
}

async function restorePoint(vault, repositoryId, recoveryPointId, output, overwrite) {
  return runHig([
    "recovery", "restore", repositoryId, recoveryPointId,
    "--output-dir", output,
    ...(overwrite ? ["--overwrite"] : []),
    "--vault-root", vault,
    "--json",
  ]);
}

async function listVault(vault) {
  return runHig(["recovery", "list", "--vault-root", vault, "--json"]);
}

function readHead() {
  return fs.readFileSync(path.join(workspace, ".hig", "repository", "refs", "heads", "main"), "utf8").trim();
}

function preparedOperationIds(vault) {
  const events = path.join(vault, "events");
  if (!fs.existsSync(events)) return new Set();
  return new Set(
    fs.readdirSync(events)
      .filter((name) => name.endsWith(".prepared.json"))
      .map((name) => name.split(".")[0]),
  );
}

function findNewPreparedOperation(vault, before, operation) {
  const events = path.join(vault, "events");
  if (!fs.existsSync(events)) return null;
  for (const name of fs.readdirSync(events)) {
    if (!name.endsWith(".prepared.json")) continue;
    const operationId = name.split(".")[0];
    if (before.has(operationId)) continue;
    const document = JSON.parse(fs.readFileSync(path.join(events, name), "utf8"));
    if (document.payload.operation === operation) return operationId;
  }
  return null;
}

function readCatalog(vault) {
  return JSON.parse(fs.readFileSync(path.join(vault, "catalog.json"), "utf8")).payload;
}

function pendingRecoveryPointCount(vault, repositoryId) {
  const registration = readCatalog(vault).repositories[repositoryId];
  if (!registration) return 0;
  return Object.values(registration.recovery_points)
    .filter((point) => point.state === "pending_deletion").length;
}

function vaultRepository(vault, repositoryId) {
  return path.join(vault, "repositories", repositoryId, ".hig", "repository");
}

function countVaultObjects(vault, repositoryId) {
  return countFiles(path.join(vaultRepository(vault, repositoryId), "objects"), (name) => /^[0-9a-fA-F]{62}$/.test(name));
}

function temporaryObjectRoot(vault, repositoryId) {
  return path.join(vaultRepository(vault, repositoryId), "objects", ".recovery-soak-gc-interruption");
}

function createTemporaryObjectLoad(vault, repositoryId, count) {
  const root = temporaryObjectRoot(vault, repositoryId);
  fs.mkdirSync(root, { recursive: true });
  for (let index = 0; index < count; index += 1) {
    fs.writeFileSync(path.join(root, `.recovery-soak.tmp.${String(index).padStart(6, "0")}`), "GC interruption fixture\n");
  }
}

function countTemporaryObjectLoad(vault, repositoryId) {
  return countFiles(temporaryObjectRoot(vault, repositoryId));
}

function recordCommit(commitId) {
  if (commitId && report.commit_ids.at(-1) !== commitId) report.commit_ids.push(commitId);
}

function recordRecoveryPoint(pointId) {
  if (pointId && report.recovery_point_ids.at(-1) !== pointId) report.recovery_point_ids.push(pointId);
}

function summarizeAudit(audit) {
  const operations = {};
  for (const event of audit.events) {
    const key = `${event.operation}:${event.outcome}`;
    operations[key] = (operations[key] || 0) + 1;
  }
  return {
    vault_root: audit.vault_root,
    event_count: audit.events.length,
    incomplete_operation_ids: audit.incomplete_operation_ids,
    operations,
  };
}

function summarizeGc(gc) {
  return {
    dry_run: gc.dry_run,
    total_recovery_points: gc.total_recovery_points,
    retained_recovery_points: gc.retained_recovery_points,
    candidate_recovery_points: gc.candidate_recovery_points,
    removed_recovery_points: gc.removed_recovery_points,
    stored_bytes_before: gc.stored_bytes_before,
    projected_stored_bytes: gc.projected_stored_bytes,
    policy_satisfied: gc.policy_satisfied,
    repositories: gc.repositories,
  };
}

async function runHig(args) {
  const result = await runProcess(higBin, args, { cwd: projectRoot, timeoutMs: 300_000 });
  assert.equal(result.code, 0, `hig ${args.join(" ")} failed\n${result.stderr}\n${result.stdout}`);
  try {
    return JSON.parse(result.stdout.trim());
  } catch (error) {
    throw new Error(`hig ${args.join(" ")} returned invalid JSON: ${error.message}\n${result.stdout}`);
  }
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

await main();
