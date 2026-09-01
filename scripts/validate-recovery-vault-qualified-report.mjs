#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

const options = parseArguments(process.argv.slice(2));
const reportPath = path.resolve(options.report);
const report = JSON.parse(fs.readFileSync(reportPath, "utf8"));

assert.equal(report.schema, 1);
assert.equal(report.status, "passed", "Recovery Vault performance qualification failed");
assert.equal(report.mode, options.expectedMode);
assert.equal(report.source_commit, options.expectedCommit);
assert(report.fixture?.bytes >= options.minimumFixtureBytes, "qualified fixture is too small");
assert.equal(report.fixture?.mutation_bytes, 1);
assert.match(report.final_digest, /^[0-9a-f]{64}$/);
assert.equal(report.exact_restore?.direct_repository, true);
assert.equal(report.exact_restore?.primary_vault, true);
assert.equal(report.exact_restore?.mirror_after_source_and_primary_loss, true);

for (const operation of [
  "repository_snapshot_initial",
  "recovery_capture_initial",
  "repository_snapshot_incremental",
  "recovery_capture_incremental",
  "repository_restore_direct",
  "recovery_restore_primary",
  "recovery_gc",
  "recovery_verify_primary",
  "recovery_scrub_primary",
  "recovery_restore_mirror_after_source_and_primary_loss",
  "recovery_scrub_survivor",
]) {
  assert(report.operations?.[operation]?.duration_ms > 0, `${operation} duration is missing`);
}

assert(report.storage?.primary_vault_bytes >= report.fixture.bytes * 0.95, "primary capacity is not credible");
assert(report.storage?.mirror_vault_bytes >= report.fixture.bytes * 0.95, "mirror capacity is not credible");
assert(
  report.storage?.combined_vault_to_logical_ratio >= 1.9
    && report.storage.combined_vault_to_logical_ratio <= 2.2,
  "combined Vault capacity ratio is outside the qualified range",
);
assert(report.comparisons?.incremental_object_reuse_ratio >= 0.9);
assert(report.comparisons?.incremental_bytes_written_ratio <= 0.05);
assert(report.comparisons?.primary_restore_throughput_mib_s > 0);
assert(report.comparisons?.mirror_restore_throughput_mib_s > 0);

if (options.expectedMode === "qualified") {
  assert.equal(report.platform, "darwin");
  assert.equal(report.architecture, "arm64");
  assert(report.operations.recovery_restore_primary.duration_ms < 300_000, "primary restore exceeded RTO");
  assert(
    report.operations.recovery_restore_mirror_after_source_and_primary_loss.duration_ms < 300_000,
    "mirror restore exceeded RTO",
  );
  assert(report.peak_cli_rss_bytes > 0 && report.peak_cli_rss_bytes <= 1024 ** 3);
}

process.stdout.write(`${JSON.stringify({
  status: "qualified",
  report: reportPath,
  source_commit: report.source_commit,
  platform: report.platform,
  architecture: report.architecture,
  fixture: report.fixture,
  storage: report.storage,
  comparisons: report.comparisons,
  peak_cli_rss_bytes: report.peak_cli_rss_bytes,
  final_digest: report.final_digest,
}, null, 2)}\n`);

function parseArguments(args) {
  const parsed = {
    report: null,
    expectedCommit: process.env.GITHUB_SHA || null,
    expectedMode: "qualified",
    minimumFixtureBytes: 1024 ** 3,
  };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--report") parsed.report = args[++index];
    else if (argument === "--expected-commit") parsed.expectedCommit = args[++index];
    else if (argument === "--expected-mode") parsed.expectedMode = args[++index];
    else if (argument === "--minimum-fixture-bytes") parsed.minimumFixtureBytes = Number(args[++index]);
    else throw new Error(`unknown argument: ${argument}`);
  }
  assert(parsed.report, "--report is required");
  assert(parsed.expectedCommit, "--expected-commit or GITHUB_SHA is required");
  assert(["ci", "qualified"].includes(parsed.expectedMode));
  assert(Number.isInteger(parsed.minimumFixtureBytes) && parsed.minimumFixtureBytes > 0);
  return parsed;
}
