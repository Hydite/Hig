#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

const options = parseArguments(process.argv.slice(2));
const reportPath = path.resolve(options.report);
const report = JSON.parse(fs.readFileSync(reportPath, "utf8"));

assert.equal(report.schema, 2, "Recovery Vault soak report schema must be 2");
assert.equal(report.status, "passed", "Recovery Vault soak did not pass");
assert.equal(report.mode, options.expectedMode, "Recovery Vault soak mode does not match the expected mode");
assert.equal(report.source_commit, options.expectedCommit, "soak report commit does not match the qualified commit");
assert(
  report.requested_duration_seconds >= options.minimumDurationSeconds,
  "requested soak duration is below the qualification minimum",
);
assert(
  report.duration_seconds >= options.minimumDurationSeconds,
  "observed soak duration is below the qualification minimum",
);
assert(report.platform && report.architecture, "native platform identity is missing");
for (const operation of ["create", "modify", "rename", "delete"]) {
  assert(report.operations?.[operation] > 0, `soak did not exercise ${operation}`);
}
assert(report.snapshots >= 4, "soak produced too few automatic snapshots");
assert(report.recovery_captures >= report.snapshots, "Recovery Vault capture count is inconsistent");
assert(report.checkpoints >= 2, "soak produced too few exact-restore checkpoints");
assert(report.mcp_restart_recoveries >= 2, "soak did not prove two MCP restart recoveries");

assert.equal(report.interruption?.capture?.killed_after_prepared, true);
assert.equal(report.interruption?.capture?.killed_after_object_publication, true);
assert.equal(report.interruption?.capture?.retry_protected, true);
assert.equal(report.interruption?.restore?.killed_after_prepared, true);
assert.equal(report.interruption?.restore?.killed_after_staging, true);
assert.equal(report.interruption?.restore?.destination_unpublished, true);
assert.equal(report.interruption?.restore?.exact_retry, true);
assert.equal(report.interruption?.gc?.killed_after_prepared, true);
assert.equal(report.interruption?.gc?.killed_after_pending_catalog, true);
assert.equal(report.interruption?.gc?.exact_retry, true);
assert.equal(report.interruption?.gc?.idempotent, true);

assert.equal(report.source_loss?.workspace_deleted, true);
assert.equal(report.source_loss?.exact_restore, true);
assert.equal(report.primary_vault_loss?.primary_vault_deleted, true);
assert.equal(report.primary_vault_loss?.mirror_exact_restore, true);
assert.match(report.final_digest, /^[0-9a-f]{64}$/);
assert.equal(report.final_scrub?.healthy, true, "final scrub is unhealthy");
assert(
  report.final_scrub.locations.every((location) => location.healthy && location.errors.length === 0),
  "a final scrub location is unhealthy",
);
assert.deepEqual(report.final_audit?.incomplete_operation_ids, [], "final audit contains incomplete operations");
assert.equal(report.final_gc?.repeated?.candidate_recovery_points, 0);
assert.equal(report.final_gc?.repeated?.removed_recovery_points, 0);
for (const repository of Object.values(report.final_gc?.repeated?.repositories || {})) {
  assert.equal(repository.unreachable_objects, 0, "final GC retained unreachable objects");
  assert.equal(repository.temporary_files, 0, "final GC retained temporary files");
}

validateMetric(report.metrics?.watcher_rpo_ms, 4, "watcher RPO");
validateMetric(report.metrics?.capture_latency_ms, 1, "capture latency");
validateMetric(report.metrics?.verify_latency_ms, 1, "verification latency");
validateMetric(report.metrics?.restore_latency_ms, 1, "restore latency");
validateMetric(report.metrics?.restore_throughput_mib_s, 1, "large-restore throughput");
validateMetric(report.metrics?.gc_latency_ms, 2, "GC latency");
assert(report.metrics.watcher_rpo_ms.maximum < 300_000, "watcher RPO exceeded the release timeout");
assert(report.metrics.logical_snapshot_bytes_total > 0);
assert(report.metrics.capture_reachable_objects_total > 0);
assert(report.metrics.capture_stored_objects_written_total >= 0);
assert(report.metrics.capture_stored_bytes_written_total >= 0);
assert(report.metrics.object_dedup_reuse_ratio >= 0 && report.metrics.object_dedup_reuse_ratio <= 1);
assert(report.metrics.storage_write_ratio >= 0);
assert(report.metrics.restore_bytes_total > 0);
assert(report.metrics.aggregate_restore_throughput_mib_s > 0);
assert(report.metrics.harness_peak_rss_bytes > 0);

process.stdout.write(`${JSON.stringify({
  status: "qualified",
  report: reportPath,
  source_commit: report.source_commit,
  platform: report.platform,
  architecture: report.architecture,
  duration_seconds: report.duration_seconds,
  snapshots: report.snapshots,
  recovery_captures: report.recovery_captures,
  checkpoints: report.checkpoints,
  mcp_restart_recoveries: report.mcp_restart_recoveries,
  metrics: report.metrics,
  final_digest: report.final_digest,
}, null, 2)}\n`);

function validateMetric(metric, minimumSamples, label) {
  assert(metric?.samples >= minimumSamples, `${label} has too few samples`);
  for (const field of ["minimum", "p05", "median", "p95", "maximum"]) {
    assert(Number.isFinite(metric[field]) && metric[field] >= 0, `${label} ${field} is invalid`);
  }
  assert(metric.minimum <= metric.p05);
  assert(metric.p05 <= metric.median);
  assert(metric.median <= metric.p95);
  assert(metric.p95 <= metric.maximum);
}

function parseArguments(args) {
  const parsed = {
    report: null,
    expectedCommit: process.env.GITHUB_SHA || null,
    expectedMode: "release",
    minimumDurationSeconds: 7200,
  };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--report") parsed.report = args[++index];
    else if (argument === "--expected-commit") parsed.expectedCommit = args[++index];
    else if (argument === "--expected-mode") parsed.expectedMode = args[++index];
    else if (argument === "--minimum-duration-seconds") {
      parsed.minimumDurationSeconds = Number(args[++index]);
    } else throw new Error(`unknown argument: ${argument}`);
  }
  assert(parsed.report, "--report is required");
  assert(parsed.expectedCommit, "--expected-commit or GITHUB_SHA is required");
  assert(["ci", "release"].includes(parsed.expectedMode), "expected mode must be ci or release");
  assert(
    Number.isFinite(parsed.minimumDurationSeconds) && parsed.minimumDurationSeconds > 0,
    "minimum duration must be greater than zero",
  );
  return parsed;
}
