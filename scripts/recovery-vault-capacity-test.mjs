#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  digestTree,
  fillDeterministic,
  runProcess,
} from "./lib/native-soak-runtime.mjs";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const options = parseArguments(process.argv.slice(2));
assert(options.vaultRoot, "--vault-root is required");
const vaultRoot = path.resolve(options.vaultRoot);
const executable = process.platform === "win32" ? "hig.exe" : "hig";
const higBin = path.resolve(process.env.HIG_BIN || path.join(projectRoot, "target", "release", executable));
const work = fs.mkdtempSync(path.join(os.tmpdir(), "hig-recovery-capacity-"));
const source = path.join(work, "source");
const restored = path.join(work, "restored");
const reservation = path.join(vaultRoot, ".capacity-reservation");
const report = {
  schema: 1,
  source_commit: process.env.GITHUB_SHA || await gitCommit(),
  platform: process.platform,
  architecture: process.arch,
  vault_root: vaultRoot,
  filesystem_bytes: null,
  payload_bytes: 0,
  available_before_reservation: null,
  available_at_failure: null,
  capture_exit_code: null,
  capture_error: null,
  catalog_unchanged: false,
  audit_terminal_or_incomplete: false,
  retry_succeeded: false,
  source_deleted: false,
  exact_restore: false,
  final_digest: null,
  status: "running",
};

async function main() {
  try {
    fs.mkdirSync(vaultRoot, { recursive: true });
    assert.equal(fs.readdirSync(vaultRoot).length, 0, "capacity vault root must start empty");
    const initialFs = filesystemCapacity(vaultRoot);
    report.filesystem_bytes = initialFs.total;
    report.available_before_reservation = initialFs.available;
    assert(initialFs.total <= 512 * 1024 * 1024, "capacity test refuses to fill a filesystem larger than 512 MiB");
    assert(initialFs.available >= 48 * 1024 * 1024, "capacity filesystem must provide at least 48 MiB");

    fs.mkdirSync(path.join(source, "payload"), { recursive: true });
    const block = Buffer.allocUnsafe(1024 * 1024);
    const payloadFiles = 24;
    for (let index = 0; index < payloadFiles; index += 1) {
      fillDeterministic(block, index + 1);
      fs.writeFileSync(path.join(source, "payload", `block-${String(index).padStart(3, "0")}.bin`), block);
      report.payload_bytes += block.length;
    }
    const initialized = await runHig(["repo", "init", source, "--json"]);
    const repositoryId = Buffer.from(initialized.repository_id).toString("hex");
    const snapshot = await runHig([
      "repo", "snapshot", source,
      "--message", "capacity baseline",
      "--author", "hig-capacity-test",
      "--json",
    ]);
    const expectedDigest = digestTree(source);
    await runHig(["recovery", "init", "--vault-root", vaultRoot, "--json"]);
    const catalogPath = path.join(vaultRoot, "catalog.json");
    const catalogBefore = fs.readFileSync(catalogPath);

    reserveFilesystem(vaultRoot, reservation, 2 * 1024 * 1024);
    report.available_at_failure = filesystemCapacity(vaultRoot).available;
    assert(report.available_at_failure <= 3 * 1024 * 1024, "capacity reservation did not constrain free space");
    const failed = await runProcess(higBin, [
      "recovery", "capture", source,
      "--revision", "HEAD",
      "--vault-root", vaultRoot,
      "--json",
    ], { cwd: projectRoot, timeoutMs: 180_000 });
    report.capture_exit_code = failed.code;
    report.capture_error = `${failed.stderr}\n${failed.stdout}`.trim();
    assert.notEqual(failed.code, 0, "capacity-constrained capture unexpectedly succeeded");
    assert.match(report.capture_error, /space|capacity|storage|os error 28|ENOSPC/i, "capture did not report capacity exhaustion");
    report.catalog_unchanged = catalogBefore.equals(fs.readFileSync(catalogPath));
    assert(report.catalog_unchanged, "failed capacity capture changed the recovery catalog");
    const listedAfterFailure = await runHig(["recovery", "list", "--vault-root", vaultRoot, "--json"]);
    assert.equal(listedAfterFailure.repositories.length, 0, "failed capacity capture became visible");
    const auditAfterFailure = await runHig(["recovery", "audit", "--vault-root", vaultRoot, "--json"]);
    report.audit_terminal_or_incomplete = auditAfterFailure.events.some(
      (event) => event.operation === "capture" && event.outcome === "failed",
    ) || auditAfterFailure.incomplete_operation_ids.length > 0;
    assert(report.audit_terminal_or_incomplete, "capacity failure left no audit evidence");

    fs.rmSync(reservation, { force: true });
    const retried = await runHig([
      "recovery", "capture", source,
      "--revision", "HEAD",
      "--vault-root", vaultRoot,
      "--json",
    ]);
    assert.equal(retried.recovery_point.recovery_point_id, snapshot.commit_id);
    report.retry_succeeded = true;
    await runHig([
      "recovery", "verify", repositoryId, snapshot.commit_id,
      "--vault-root", vaultRoot,
      "--json",
    ]);

    fs.rmSync(source, { recursive: true, force: true });
    report.source_deleted = true;
    await runHig([
      "recovery", "restore", repositoryId, snapshot.commit_id,
      "--output-dir", restored,
      "--vault-root", vaultRoot,
      "--json",
    ]);
    report.final_digest = digestTree(restored);
    report.exact_restore = report.final_digest === expectedDigest;
    assert(report.exact_restore, "capacity recovery restore digest mismatch");
    const scrub = await runHig(["recovery", "scrub", "--vault-root", vaultRoot, "--json"]);
    assert.equal(scrub.healthy, true, "capacity recovery vault scrub failed");
    report.status = "passed";
  } catch (error) {
    report.status = "failed";
    report.error = error instanceof Error ? error.stack || error.message : String(error);
    throw error;
  } finally {
    fs.rmSync(reservation, { force: true });
    const serialized = `${JSON.stringify(report, null, 2)}\n`;
    if (options.report) {
      const output = path.resolve(options.report);
      fs.mkdirSync(path.dirname(output), { recursive: true });
      fs.writeFileSync(output, serialized);
    }
    process.stdout.write(serialized);
    if (!process.env.HIG_RECOVERY_CAPACITY_KEEP_WORK) fs.rmSync(work, { recursive: true, force: true });
  }
}

function filesystemCapacity(root) {
  const stats = fs.statfsSync(root);
  return {
    total: Number(stats.blocks) * Number(stats.bsize),
    available: Number(stats.bavail) * Number(stats.bsize),
  };
}

function reserveFilesystem(root, output, targetAvailable) {
  const descriptor = fs.openSync(output, "wx", 0o600);
  const block = Buffer.allocUnsafe(1024 * 1024);
  let index = 0;
  try {
    while (filesystemCapacity(root).available > targetAvailable + block.length) {
      fillDeterministic(block, 0x10000 + index);
      fs.writeSync(descriptor, block);
      index += 1;
    }
    fs.fsyncSync(descriptor);
  } finally {
    fs.closeSync(descriptor);
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

async function gitCommit() {
  const result = await runProcess("git", ["rev-parse", "HEAD"], { cwd: projectRoot, timeoutMs: 10_000 });
  return result.code === 0 ? result.stdout.trim() : null;
}

function parseArguments(args) {
  const parsed = { vaultRoot: null, report: null };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--vault-root") parsed.vaultRoot = args[++index];
    else if (argument === "--report") parsed.report = args[++index];
    else throw new Error(`unknown argument: ${argument}`);
  }
  return parsed;
}

await main();
