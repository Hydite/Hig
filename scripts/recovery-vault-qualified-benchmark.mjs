#!/usr/bin/env node
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { digestTree, runProcess } from "./lib/native-soak-runtime.mjs";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const options = parseArguments(process.argv.slice(2));
const fixtureMib = options.fixtureMib ?? (options.mode === "qualified" ? 1024 : 32);
assert(Number.isInteger(fixtureMib) && fixtureMib >= 16 && fixtureMib % 4 === 0);
if (options.mode === "qualified") assert(fixtureMib >= 1024, "qualified fixture must be at least 1 GiB");

const executable = process.platform === "win32" ? "hig.exe" : "hig";
const higBin = path.resolve(process.env.HIG_BIN || path.join(projectRoot, "target", "release", executable));
const work = fs.mkdtempSync(path.join(os.tmpdir(), "hig-recovery-qualified-"));
process.env.HIG_RECOVERY_AUTH_DIR = path.join(work, "recovery-auth");
const workspace = path.join(work, "workspace");
const primaryVault = path.join(work, "primary-vault");
const mirrorVault = path.join(work, "mirror-vault");
const directOutput = path.join(work, "direct-restore");
const primaryOutput = path.join(work, "primary-restore");
const mirrorOutput = path.join(work, "mirror-restore");
const fileBytes = 4 * 1024 * 1024;
const fileCount = fixtureMib / 4;
const fixtureBytes = fileCount * fileBytes;
const startedAt = new Date();
const report = {
  schema: 1,
  mode: options.mode,
  source_commit: process.env.GITHUB_SHA || await gitCommit(),
  platform: process.platform,
  architecture: process.arch,
  fixture: { files: fileCount, bytes: fixtureBytes, mutation_bytes: 1 },
  started_at: startedAt.toISOString(),
  operations: {},
  storage: {},
  comparisons: {},
  exact_restore: {},
  final_digest: null,
  peak_cli_rss_bytes: null,
  status: "running",
};

try {
  createFixture();
  const init = await runHig(["repo", "init", workspace, "--json"], "repository_init");
  const repositoryId = Buffer.from(init.value.repository_id).toString("hex");
  const initialSnapshot = await runHig([
    "repo", "snapshot", workspace,
    "--message", "qualified initial snapshot",
    "--author", "hig-recovery-benchmark",
    "--json",
  ], "repository_snapshot_initial");
  assert.equal(initialSnapshot.value.input_bytes, fixtureBytes);

  await runHig([
    "recovery", "init",
    "--vault-root", primaryVault,
    "--mirror", mirrorVault,
    "--json",
  ], "recovery_init");
  const initialCapture = await runHig([
    "recovery", "capture", workspace,
    "--revision", "HEAD",
    "--vault-root", primaryVault,
    "--json",
  ], "recovery_capture_initial");
  assert.equal(initialCapture.value.recovery_point.durability, "protected");

  mutateOneByte();
  const incrementalSnapshot = await runHig([
    "repo", "snapshot", workspace,
    "--message", "qualified one-byte mutation",
    "--author", "hig-recovery-benchmark",
    "--json",
  ], "repository_snapshot_incremental");
  assert.equal(incrementalSnapshot.value.created, true);
  assert.equal(incrementalSnapshot.value.input_bytes, fixtureBytes);
  const incrementalCapture = await runHig([
    "recovery", "capture", workspace,
    "--revision", "HEAD",
    "--vault-root", primaryVault,
    "--json",
  ], "recovery_capture_incremental");
  assert.equal(incrementalCapture.value.recovery_point.durability, "protected");
  assert.equal(incrementalCapture.value.recovery_point.recovery_point_id, incrementalSnapshot.value.commit_id);

  const expectedDigest = digestTree(workspace);
  const directRestore = await runHig([
    "repo", "restore", workspace,
    "--revision", "HEAD",
    "--output-dir", directOutput,
    "--json",
  ], "repository_restore_direct");
  assert.equal(directRestore.value.bytes, fixtureBytes);
  assert.equal(digestTree(directOutput), expectedDigest);
  report.exact_restore.direct_repository = true;
  fs.rmSync(directOutput, { recursive: true, force: true });

  const pointId = incrementalCapture.value.recovery_point.recovery_point_id;
  const primaryRestore = await runHig([
    "recovery", "restore", repositoryId, pointId,
    "--output-dir", primaryOutput,
    "--vault-root", primaryVault,
    "--json",
  ], "recovery_restore_primary");
  assert.equal(primaryRestore.value.restore.bytes, fixtureBytes);
  assert.equal(digestTree(primaryOutput), expectedDigest);
  report.exact_restore.primary_vault = true;
  fs.rmSync(primaryOutput, { recursive: true, force: true });

  await runHig([
    "recovery", "policy", "set",
    "--vault-root", primaryVault,
    "--minimum-points", "1",
    "--minimum-retention-days", "0",
    "--maximum-points", "1",
    "--json",
  ], "recovery_policy");
  const gc = await runHig([
    "recovery", "gc", "--vault-root", primaryVault, "--apply", "--json",
  ], "recovery_gc");
  assert.equal(gc.value.policy_satisfied, true);
  assert.equal(gc.value.retained_recovery_points, 1);
  await runHig([
    "recovery", "verify", repositoryId, pointId,
    "--vault-root", primaryVault,
    "--json",
  ], "recovery_verify_primary");
  const scrub = await runHig([
    "recovery", "scrub", "--vault-root", primaryVault, "--json",
  ], "recovery_scrub_primary");
  assert.equal(scrub.value.healthy, true);

  report.storage.primary_vault_bytes = directoryBytes(primaryVault);
  report.storage.mirror_vault_bytes = directoryBytes(mirrorVault);
  report.storage.combined_vault_to_logical_ratio = Number(
    ((report.storage.primary_vault_bytes + report.storage.mirror_vault_bytes) / fixtureBytes).toFixed(6),
  );

  fs.rmSync(workspace, { recursive: true, force: true });
  fs.rmSync(primaryVault, { recursive: true, force: true });
  const mirrorRestore = await runHig([
    "recovery", "restore", repositoryId, pointId,
    "--output-dir", mirrorOutput,
    "--vault-root", mirrorVault,
    "--json",
  ], "recovery_restore_mirror_after_source_and_primary_loss");
  assert.equal(mirrorRestore.value.restore.bytes, fixtureBytes);
  assert.equal(digestTree(mirrorOutput), expectedDigest);
  report.exact_restore.mirror_after_source_and_primary_loss = true;
  const finalScrub = await runHig([
    "recovery", "scrub", "--vault-root", mirrorVault, "--json",
  ], "recovery_scrub_survivor");
  assert.equal(finalScrub.value.healthy, true);

  report.comparisons.incremental_object_reuse_ratio = reuseRatio(
    incrementalCapture.value.recovery_point.reachable_objects,
    incrementalCapture.value.recovery_point.stored_objects_written,
  );
  report.comparisons.incremental_bytes_written_ratio = Number(
    (incrementalCapture.value.recovery_point.stored_bytes_written / fixtureBytes).toFixed(6),
  );
  report.comparisons.primary_restore_to_direct_ratio = latencyRatio(
    report.operations.recovery_restore_primary.duration_ms,
    report.operations.repository_restore_direct.duration_ms,
  );
  report.comparisons.mirror_restore_to_direct_ratio = latencyRatio(
    report.operations.recovery_restore_mirror_after_source_and_primary_loss.duration_ms,
    report.operations.repository_restore_direct.duration_ms,
  );
  report.comparisons.primary_restore_throughput_mib_s = throughput(
    fixtureBytes,
    report.operations.recovery_restore_primary.duration_ms,
  );
  report.comparisons.mirror_restore_throughput_mib_s = throughput(
    fixtureBytes,
    report.operations.recovery_restore_mirror_after_source_and_primary_loss.duration_ms,
  );
  report.final_digest = digestTree(mirrorOutput);
  assert.equal(report.final_digest, expectedDigest);
  validateQualification();
  report.status = "passed";
} catch (error) {
  report.status = "failed";
  report.error = error instanceof Error ? error.stack || error.message : String(error);
  throw error;
} finally {
  report.finished_at = new Date().toISOString();
  report.duration_seconds = Number(((Date.now() - startedAt.getTime()) / 1000).toFixed(3));
  const serialized = `${JSON.stringify(report, null, 2)}\n`;
  if (options.report) {
    const output = path.resolve(options.report);
    fs.mkdirSync(path.dirname(output), { recursive: true });
    fs.writeFileSync(output, serialized);
  }
  process.stdout.write(serialized);
  if (!process.env.HIG_RECOVERY_BENCH_KEEP_WORK) fs.rmSync(work, { recursive: true, force: true });
}

function createFixture() {
  const root = path.join(workspace, "payload");
  fs.mkdirSync(root, { recursive: true });
  const zeros = Buffer.alloc(fileBytes);
  const key = crypto.createHash("sha256").update("HIG Recovery Vault qualified fixture v1").digest();
  const blocksPerFile = BigInt(fileBytes / 16);
  for (let index = 0; index < fileCount; index += 1) {
    const iv = Buffer.alloc(16);
    iv.writeBigUInt64BE(BigInt(index) * blocksPerFile, 8);
    const bytes = crypto.createCipheriv("aes-256-ctr", key, iv).update(zeros);
    fs.writeFileSync(path.join(root, `payload-${String(index).padStart(4, "0")}.bin`), bytes);
  }
}

function mutateOneByte() {
  const target = path.join(
    workspace,
    "payload",
    `payload-${String(Math.floor(fileCount / 2)).padStart(4, "0")}.bin`,
  );
  const handle = fs.openSync(target, "r+");
  try {
    fs.writeSync(handle, Buffer.from([0xa5]), 0, 1, Math.floor(fileBytes / 2));
    fs.fsyncSync(handle);
  } finally {
    fs.closeSync(handle);
  }
}

async function runHig(args, label) {
  const started = performance.now();
  const timed = process.platform === "darwin" && fs.existsSync("/usr/bin/time");
  const command = timed ? "/usr/bin/time" : higBin;
  const commandArgs = timed ? ["-l", higBin, ...args] : args;
  const result = await runProcess(command, commandArgs, { cwd: projectRoot, timeoutMs: 30 * 60_000 });
  const durationMs = Number((performance.now() - started).toFixed(3));
  assert.equal(result.code, 0, `hig ${args.join(" ")} failed\n${result.stderr}\n${result.stdout}`);
  let value;
  try {
    value = JSON.parse(result.stdout.trim());
  } catch (error) {
    throw new Error(`hig ${args.join(" ")} returned invalid JSON: ${error.message}\n${result.stdout}`);
  }
  const peakRss = timed ? parseDarwinPeakRss(result.stderr) : null;
  if (peakRss !== null) {
    report.peak_cli_rss_bytes = Math.max(report.peak_cli_rss_bytes || 0, peakRss);
  }
  report.operations[label] = { duration_ms: durationMs, peak_rss_bytes: peakRss };
  return { value, durationMs, peakRss };
}

function parseDarwinPeakRss(stderr) {
  const match = stderr.match(/\n\s*(\d+)\s+maximum resident set size/);
  assert(match, "macOS resource report omitted maximum resident set size");
  return Number(match[1]);
}

function directoryBytes(root) {
  let total = 0;
  const visit = (directory) => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      const item = path.join(directory, entry.name);
      if (entry.isDirectory()) visit(item);
      else if (entry.isFile()) total += fs.statSync(item).size;
    }
  };
  visit(root);
  return total;
}

function reuseRatio(reachable, written) {
  assert(reachable > 0 && written <= reachable);
  return Number((1 - written / reachable).toFixed(6));
}

function latencyRatio(value, baseline) {
  assert(value > 0 && baseline > 0);
  return Number((value / baseline).toFixed(6));
}

function throughput(bytes, durationMs) {
  assert(bytes > 0 && durationMs > 0);
  return Number(((bytes / (1024 * 1024)) / (durationMs / 1000)).toFixed(3));
}

function validateQualification() {
  assert.equal(report.exact_restore.direct_repository, true);
  assert.equal(report.exact_restore.primary_vault, true);
  assert.equal(report.exact_restore.mirror_after_source_and_primary_loss, true);
  assert(report.comparisons.incremental_object_reuse_ratio >= 0.9, "incremental object reuse fell below 90%");
  assert(report.comparisons.incremental_bytes_written_ratio <= 0.05, "one-byte capture wrote over 5% of the fixture");
  if (options.mode === "qualified") {
    const rtoMs = 5 * 60_000;
    assert(report.operations.recovery_restore_primary.duration_ms < rtoMs, "primary 1 GiB restore exceeded RTO");
    assert(
      report.operations.recovery_restore_mirror_after_source_and_primary_loss.duration_ms < rtoMs,
      "mirror 1 GiB restore exceeded RTO",
    );
    assert(report.peak_cli_rss_bytes > 0, "qualified macOS peak-memory evidence is missing");
    assert(report.peak_cli_rss_bytes <= 1024 ** 3, "qualified CLI peak memory exceeded 1 GiB");
  }
}

async function gitCommit() {
  const result = await runProcess("git", ["rev-parse", "HEAD"], { cwd: projectRoot, timeoutMs: 10_000 });
  return result.code === 0 ? result.stdout.trim() : null;
}

function parseArguments(args) {
  const parsed = { mode: "ci", fixtureMib: null, report: null };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--mode") parsed.mode = args[++index];
    else if (argument === "--fixture-mib") parsed.fixtureMib = Number(args[++index]);
    else if (argument === "--report") parsed.report = args[++index];
    else throw new Error(`unknown argument: ${argument}`);
  }
  assert(["ci", "qualified"].includes(parsed.mode), "mode must be ci or qualified");
  return parsed;
}
