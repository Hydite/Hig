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
const executable = process.platform === "win32" ? "hig.exe" : "hig";
const currentBin = path.resolve(options.currentBin || process.env.HIG_BIN || path.join(projectRoot, "target", "release", executable));
const policyPath = path.resolve(options.policy || path.join(projectRoot, "fixtures", "performance", "cold-path-ci-policy.json"));
const policy = JSON.parse(fs.readFileSync(policyPath, "utf8"));
validatePolicy(policy);

if (options.selfTest) {
  policySelfTest(policy);
  process.stdout.write("cold-path-policy-self-test: PASS\n");
  process.exit(0);
}

const workRoot = path.resolve(options.workDir || fs.mkdtempSync(path.join(os.tmpdir(), "hig-cold-path-")));
const ownsWorkRoot = !options.workDir;
const corpus = options.corpus ? path.resolve(options.corpus) : path.join(workRoot, "corpus");
const report = {
  schema: 1,
  mode: options.mode,
  source_commit: process.env.GITHUB_SHA || await gitCommit(),
  platform: process.platform,
  architecture: process.arch,
  started_at: new Date().toISOString(),
  work_dir: workRoot,
  corpus: corpus,
  environment: null,
  binaries: {},
  corpus_summary: null,
  samples: [],
  summaries: {},
  gates: {},
  release_gate_status: "RUNNING"
};

try {
  fs.mkdirSync(workRoot, { recursive: true });
  if (options.mode === "ci") createSyntheticCorpus(corpus, policy.corpus);
  assert(fs.statSync(corpus).isDirectory(), `corpus is not a directory: ${corpus}`);

  const corpusDigest = digestTree(corpus);
  report.corpus_summary = {
    files: corpusDigest.files,
    bytes: corpusDigest.bytes,
    sha256: corpusDigest.digest
  };
  report.environment = await qualifyVolume(workRoot);
  report.binaries.current = await inspectBinary(currentBin);

  if (options.mode === "ci") {
    for (let index = 0; index < policy.samples; index += 1) {
      report.samples.push(await runSample("current", currentBin, index + 1, corpusDigest));
    }
    report.summaries.current = summarize(report.samples);
    report.gates = evaluateCi(report.summaries.current, report.corpus_summary, policy.limits, true);
  } else {
    assert(options.v196Bin, "release mode requires --v196-bin");
    assert(options.v197Bin, "release mode requires --v197-bin");
    const v196Bin = path.resolve(options.v196Bin);
    const v197Bin = path.resolve(options.v197Bin);
    report.binaries.v196 = await inspectBinary(v196Bin, "1.9.6", options.v196Sha256);
    report.binaries.v197 = await inspectBinary(v197Bin, "1.9.7", options.v197Sha256);
    assert.equal(report.binaries.current.version, "1.10.0", "current binary must report HIG 1.10.0");

    const order = [
      ["v196", v196Bin], ["current", currentBin], ["current", currentBin], ["v196", v196Bin],
      ["v197", v197Bin], ["current", currentBin], ["current", currentBin], ["v197", v197Bin]
    ];
    const counters = { v196: 0, v197: 0, current: 0 };
    for (const [variant, binary] of order) {
      counters[variant] += 1;
      report.samples.push(await runSample(variant, binary, counters[variant], corpusDigest));
    }
    for (const variant of ["v196", "v197", "current"]) {
      report.summaries[variant] = summarize(report.samples.filter((sample) => sample.variant === variant));
    }
    report.gates = evaluateRelease(report.summaries, report.corpus_summary);
  }

  report.release_gate_status = Object.values(report.gates).every((gate) => gate.passed)
    ? (report.environment.qualified ? "PASS" : "ENVIRONMENT_NOT_QUALIFIED")
    : "FAIL";
  if (options.requireQualified) {
    assert.equal(report.environment.qualified, true, `benchmark volume is not qualified: ${report.environment.reasons.join(", ")}`);
  }
  assert.notEqual(report.release_gate_status, "FAIL", gateFailureMessage(report.gates));
} catch (error) {
  report.release_gate_status = "FAIL";
  report.error = error instanceof Error ? error.stack || error.message : String(error);
  throw error;
} finally {
  report.finished_at = new Date().toISOString();
  const serialized = `${JSON.stringify(report, null, 2)}\n`;
  if (options.report) {
    const output = path.resolve(options.report);
    fs.mkdirSync(path.dirname(output), { recursive: true });
    fs.writeFileSync(output, serialized);
  }
  process.stdout.write(serialized);
  if (ownsWorkRoot && !process.env.HIG_COLD_PATH_KEEP_WORK) fs.rmSync(workRoot, { recursive: true, force: true });
}

async function runSample(variant, binary, sampleNumber, expectedCorpus) {
  const sampleRoot = path.join(workRoot, "sample");
  fs.rmSync(sampleRoot, { recursive: true, force: true });
  fs.mkdirSync(sampleRoot, { recursive: true });
  const archive = path.join(sampleRoot, "output.hig");
  const cache = path.join(sampleRoot, "cache");
  const restored = path.join(sampleRoot, "restored");
  const started = process.hrtime.bigint();
  const result = await runProcess(binary, [
    "pack", corpus, "--output", archive, "--cache-dir", cache,
    "--daemon", "off", "--project", "off", "--speed", "fastest",
    "--encryption", "none", "--json"
  ], 20 * 60_000);
  const wallUs = Number((process.hrtime.bigint() - started) / 1000n);
  assert.equal(result.code, 0, `${variant} pack failed\n${result.stderr}\n${result.stdout}`);
  const payload = parseJsonOutput(result.stdout, `${variant} pack`);
  const value = payload.report || payload;
  const unpack = await runProcess(currentBin, ["unpack", archive, "--output-dir", restored], 20 * 60_000);
  assert.equal(unpack.code, 0, `${variant} archive restore failed\n${unpack.stderr}\n${unpack.stdout}`);
  const restoredDigest = digestTree(restored);
  assert.deepEqual(restoredDigest, expectedCorpus, `${variant} restored corpus differs from source`);

  const sample = {
    variant,
    sample: sampleNumber,
    wall_us: wallUs,
    total_us: numberAt(value, ["timings_us", "total_us"], durationToUs(value.duration)),
    scan_us: numberAt(value, ["scan", "scan_wall_us"], numberAt(value, ["critical", "scan_kdf_wall_ms"], 0) * 1000),
    scan_read_us: numberAt(value, ["scan", "read_us"], null),
    scan_content_hash_us: numberAt(value, ["scan", "content_hash_us"], null),
    block_prepare_us: numberAt(value, ["critical", "block_prepare_ms"], 0) * 1000,
    output_write_us: numberAt(value, ["timings_us", "output_write_us"], numberAt(value, ["critical", "output_write_ms"], 0) * 1000),
    archive_bytes: Number(value.archive_bytes),
    input_files: Number(value.input_files),
    input_bytes: Number(value.input_bytes),
    peak_pipeline_memory_bytes: Number(value.peak_pipeline_memory_bytes ?? value.pipeline?.pipeline_peak_memory_bytes ?? 0),
    source_read_bytes: Number(value.blocks?.source_read_bytes ?? 0),
    source_hot_raw_bytes: Number(value.blocks?.source_hot_raw_bytes ?? 0),
    payload_memory_bytes: Number(value.blocks?.payload_source_memory_bytes ?? 0),
    payload_spool_bytes: Number(value.blocks?.payload_source_spool_bytes ?? 0),
    restored_sha256: restoredDigest.digest
  };
  assert.equal(sample.input_files, expectedCorpus.files, `${variant}: input file count changed`);
  assert.equal(sample.input_bytes, expectedCorpus.bytes, `${variant}: input byte count changed`);
  for (const field of ["total_us", "scan_us", "block_prepare_us", "output_write_us", "archive_bytes", "peak_pipeline_memory_bytes"]) {
    assert(Number.isFinite(sample[field]) && sample[field] >= 0, `${variant}: invalid ${field}`);
  }
  fs.rmSync(sampleRoot, { recursive: true, force: true });
  return sample;
}

function summarize(samples) {
  assert(samples.length > 0, "cannot summarize an empty sample set");
  const metrics = [
    "wall_us", "total_us", "scan_us", "scan_read_us", "scan_content_hash_us",
    "block_prepare_us", "output_write_us", "archive_bytes", "peak_pipeline_memory_bytes",
    "source_read_bytes", "source_hot_raw_bytes", "payload_memory_bytes", "payload_spool_bytes"
  ];
  const summary = { samples: samples.length };
  for (const metric of metrics) {
    const values = samples.map((sample) => sample[metric]).filter(Number.isFinite);
    summary[`${metric}_median`] = values.length ? median(values) : null;
    summary[`${metric}_max`] = values.length ? Math.max(...values) : null;
  }
  return summary;
}

function evaluateCi(current, corpusSummary, limits, restoreQuality) {
  return {
    total: gate(current.total_us_median <= limits.total_median_us, current.total_us_median, limits.total_median_us, "max"),
    scan: gate(current.scan_us_median <= limits.scan_median_us, current.scan_us_median, limits.scan_median_us, "max"),
    block_prepare: gate(current.block_prepare_us_median <= limits.block_prepare_median_us, current.block_prepare_us_median, limits.block_prepare_median_us, "max"),
    output_write: gate(current.output_write_us_median <= limits.output_write_median_us, current.output_write_us_median, limits.output_write_median_us, "max"),
    peak_memory: gate(current.peak_pipeline_memory_bytes_max <= limits.peak_pipeline_memory_bytes, current.peak_pipeline_memory_bytes_max, limits.peak_pipeline_memory_bytes, "max"),
    archive_size: gate(current.archive_bytes_max <= corpusSummary.bytes * limits.archive_to_input_ratio, current.archive_bytes_max, corpusSummary.bytes * limits.archive_to_input_ratio, "max"),
    restore_quality: gate(restoreQuality, restoreQuality ? "all sample digests matched" : "digest mismatch", "exact", "exact")
  };
}

function evaluateRelease(summaries, corpusSummary) {
  const current = summaries.current;
  const v196 = summaries.v196;
  const v197 = summaries.v197;
  const maxBaselineArchive = Math.max(v196.archive_bytes_median, v197.archive_bytes_median);
  return {
    v196_total: ratioGate(current.total_us_median, v196.total_us_median, 1.1),
    v197_total: ratioGate(current.total_us_median, v197.total_us_median, 1.1),
    v196_scan: ratioGate(current.scan_us_median, v196.scan_us_median, 1.1),
    v197_scan_improvement: ratioGate(current.scan_us_median, v197.scan_us_median, 0.95),
    v197_block_prepare: ratioGate(current.block_prepare_us_median, v197.block_prepare_us_median, 1.1),
    v197_output_write: ratioGate(current.output_write_us_median, v197.output_write_us_median, 1.1),
    peak_memory_budget: gate(current.peak_pipeline_memory_bytes_max <= 1024 * 1024 * 1024, current.peak_pipeline_memory_bytes_max, 1024 * 1024 * 1024, "max"),
    archive_size: gate(current.archive_bytes_max <= maxBaselineArchive * 1.01, current.archive_bytes_max, maxBaselineArchive * 1.01, "max"),
    input_identity: gate(corpusSummary.files > 0 && corpusSummary.bytes > 0, `${corpusSummary.files}/${corpusSummary.bytes}`, "> 0", "min"),
    restore_quality: gate(true, "all sample digests matched", "exact", "exact")
  };
}

function gate(passed, actual, limit, direction) {
  return { passed: Boolean(passed), actual, limit, direction };
}

function ratioGate(candidate, baseline, maximumRatio) {
  return gate(candidate <= baseline * maximumRatio, candidate, baseline * maximumRatio, `max ${maximumRatio}x baseline`);
}

function gateFailureMessage(gates) {
  return Object.entries(gates).filter(([, value]) => !value.passed).map(([name, value]) => `${name}: actual=${value.actual} limit=${value.limit}`).join("; ");
}

function policySelfTest(selectedPolicy) {
  const input = { files: 10, bytes: 1000 };
  const passing = {
    total_us_median: 1, scan_us_median: 1, block_prepare_us_median: 1,
    output_write_us_median: 1, peak_pipeline_memory_bytes_max: 1, archive_bytes_max: 1
  };
  assert(Object.values(evaluateCi(passing, input, selectedPolicy.limits, true)).every((value) => value.passed));
  const fields = [
    ["total_us_median", "total_median_us"], ["scan_us_median", "scan_median_us"],
    ["block_prepare_us_median", "block_prepare_median_us"], ["output_write_us_median", "output_write_median_us"],
    ["peak_pipeline_memory_bytes_max", "peak_pipeline_memory_bytes"]
  ];
  for (const [field, limit] of fields) {
    const candidate = { ...passing, [field]: selectedPolicy.limits[limit] + 1 };
    assert(Object.values(evaluateCi(candidate, input, selectedPolicy.limits, true)).some((value) => !value.passed), `${field} regression was accepted`);
  }
  const oversized = { ...passing, archive_bytes_max: input.bytes * selectedPolicy.limits.archive_to_input_ratio + 1 };
  assert.equal(evaluateCi(oversized, input, selectedPolicy.limits, true).archive_size.passed, false, "archive-size regression was accepted");
  assert.equal(evaluateCi(passing, input, selectedPolicy.limits, false).restore_quality.passed, false, "restore-quality regression was accepted");
}

async function qualifyVolume(root) {
  const bytes = 256 * 1024 * 1024;
  const source = path.join(root, ".hig-copy-source.bin");
  const chunk = Buffer.alloc(1024 * 1024, 0xa5);
  const file = fs.openSync(source, "w");
  try {
    for (let remaining = bytes; remaining > 0; remaining -= chunk.length) fs.writeSync(file, chunk);
  } finally {
    fs.closeSync(file);
  }
  const speeds = [];
  for (let index = 0; index < 3; index += 1) {
    const target = path.join(root, `.hig-copy-target-${index}.bin`);
    const started = process.hrtime.bigint();
    copyBuffered(source, target);
    const seconds = Number(process.hrtime.bigint() - started) / 1e9;
    speeds.push(256 / Math.max(seconds, 0.000001));
    fs.rmSync(target, { force: true });
  }
  fs.rmSync(source, { force: true });
  const stats = fs.statfsSync(root);
  const freeBytes = Number(stats.bavail) * Number(stats.bsize);
  const medianMiBs = median(speeds);
  const p95MiBs = percentile(speeds, 0.95);
  const reasons = [];
  if (medianMiBs < 650) reasons.push("256 MiB copy median below 650 MiB/s");
  if (p95MiBs < 500) reasons.push("256 MiB copy p95 below 500 MiB/s");
  if (freeBytes < 20 * 1024 * 1024 * 1024) reasons.push("free space below 20 GiB");
  return { path: root, copy_256_mib_samples_mib_s: speeds, copy_256_mib_median_mib_s: medianMiBs, copy_256_mib_p95_mib_s: p95MiBs, free_bytes: freeBytes, qualified: reasons.length === 0, reasons };
}

function copyBuffered(source, target) {
  const input = fs.openSync(source, "r");
  const output = fs.openSync(target, "w");
  const buffer = Buffer.allocUnsafe(8 * 1024 * 1024);
  try {
    for (;;) {
      const read = fs.readSync(input, buffer, 0, buffer.length, null);
      if (read === 0) break;
      fs.writeSync(output, buffer, 0, read);
    }
    fs.fsyncSync(output);
  } finally {
    fs.closeSync(input);
    fs.closeSync(output);
  }
}

function createSyntheticCorpus(root, specification) {
  fs.rmSync(root, { recursive: true, force: true });
  fs.mkdirSync(path.join(root, "small"), { recursive: true });
  fs.mkdirSync(path.join(root, "large"), { recursive: true });
  for (let index = 0; index < specification.small_files; index += 1) {
    const prefix = `export const fixture_${index} = ${index};\n`;
    const content = (prefix + "cold-path deterministic source line\n".repeat(256)).slice(0, specification.small_file_bytes).padEnd(specification.small_file_bytes, "x");
    fs.writeFileSync(path.join(root, "small", `${String(index).padStart(5, "0")}.js`), content);
  }
  const block = deterministicBytes(1024 * 1024, specification.seed);
  for (let fileIndex = 0; fileIndex < specification.large_files; fileIndex += 1) {
    const output = fs.openSync(path.join(root, "large", `payload-${fileIndex}.bin`), "w");
    try {
      for (let offset = 0; offset < specification.large_file_bytes; offset += block.length) {
        block.writeUInt32LE((specification.seed + fileIndex + offset) >>> 0, 0);
        fs.writeSync(output, block, 0, Math.min(block.length, specification.large_file_bytes - offset));
      }
    } finally {
      fs.closeSync(output);
    }
  }
}

function deterministicBytes(length, seed) {
  const bytes = Buffer.allocUnsafe(length);
  let value = seed >>> 0;
  for (let offset = 0; offset < length; offset += 4) {
    value ^= value << 13;
    value ^= value >>> 17;
    value ^= value << 5;
    bytes.writeUInt32LE(value >>> 0, offset);
  }
  return bytes;
}

function digestTree(root) {
  const hash = crypto.createHash("sha256");
  let files = 0;
  let bytes = 0;
  const visit = (directory, relative = "") => {
    const entries = fs.readdirSync(directory, { withFileTypes: true }).sort((left, right) => left.name.localeCompare(right.name, "en"));
    for (const entry of entries) {
      const item = path.join(directory, entry.name);
      const itemRelative = relative ? `${relative}/${entry.name}` : entry.name;
      if (entry.isDirectory()) visit(item, itemRelative);
      else if (entry.isFile()) {
        const content = fs.readFileSync(item);
        files += 1;
        bytes += content.length;
        hash.update(`f\0${itemRelative}\0${content.length}\0`);
        hash.update(content);
      } else if (entry.isSymbolicLink()) {
        const target = fs.readlinkSync(item);
        files += 1;
        bytes += Buffer.byteLength(target);
        hash.update(`l\0${itemRelative}\0${target}\0`);
      }
    }
  };
  visit(root);
  return { files, bytes, digest: hash.digest("hex") };
}

async function inspectBinary(binary, expectedVersion = null, expectedSha256 = null) {
  assert(fs.statSync(binary).isFile(), `binary does not exist: ${binary}`);
  const versionResult = await runProcess(binary, ["--version"], 30_000);
  assert.equal(versionResult.code, 0, `cannot execute ${binary}: ${versionResult.stderr}`);
  const match = /^hig (\d+\.\d+\.\d+)$/m.exec(versionResult.stdout.trim());
  assert(match, `unexpected HIG version output: ${versionResult.stdout}`);
  const sha256 = hashFile(binary);
  if (expectedVersion) assert.equal(match[1], expectedVersion, `unexpected version for ${binary}`);
  if (expectedSha256) assert.equal(sha256, expectedSha256.toLowerCase(), `SHA-256 mismatch for ${binary}`);
  return { path: binary, version: match[1], sha256 };
}

function hashFile(file) {
  const hash = crypto.createHash("sha256");
  hash.update(fs.readFileSync(file));
  return hash.digest("hex");
}

function durationToUs(duration) {
  if (!duration) return 0;
  return Number(duration.secs || 0) * 1_000_000 + Math.floor(Number(duration.nanos || 0) / 1000);
}

function numberAt(value, keys, fallback) {
  let current = value;
  for (const key of keys) current = current?.[key];
  return Number.isFinite(Number(current)) ? Number(current) : fallback;
}

function median(values) {
  return percentile(values, 0.5);
}

function percentile(values, fraction) {
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.ceil((sorted.length - 1) * fraction)];
}

function validatePolicy(value) {
  assert.equal(value.schema, 1, "unsupported cold-path policy schema");
  assert(Number.isInteger(value.samples) && value.samples >= 3, "policy requires at least three samples");
  for (const number of Object.values(value.corpus)) assert(Number.isInteger(number) && number > 0, "invalid corpus policy");
  for (const number of Object.values(value.limits)) assert(Number.isFinite(number) && number > 0, "invalid regression limit");
}

function parseJsonOutput(output, label) {
  try {
    return JSON.parse(output.trim());
  } catch (error) {
    throw new Error(`${label} returned invalid JSON: ${error.message}\n${output}`);
  }
}

async function runProcess(command, args, timeoutMs) {
  const child = spawn(command, args, { cwd: projectRoot, stdio: ["ignore", "pipe", "pipe"] });
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

async function gitCommit() {
  const result = await runProcess("git", ["rev-parse", "HEAD"], 10_000);
  return result.code === 0 ? result.stdout.trim() : null;
}

function parseArguments(args) {
  const parsed = {
    mode: "ci", currentBin: null, v196Bin: null, v197Bin: null,
    v196Sha256: null, v197Sha256: null, corpus: null, workDir: null,
    report: null, policy: null, requireQualified: false, selfTest: false
  };
  const valueOptions = new Map([
    ["--mode", "mode"], ["--current-bin", "currentBin"], ["--v196-bin", "v196Bin"],
    ["--v197-bin", "v197Bin"], ["--v196-sha256", "v196Sha256"], ["--v197-sha256", "v197Sha256"],
    ["--corpus", "corpus"], ["--work-dir", "workDir"], ["--report", "report"], ["--policy", "policy"]
  ]);
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (valueOptions.has(argument)) parsed[valueOptions.get(argument)] = args[++index];
    else if (argument === "--require-qualified") parsed.requireQualified = true;
    else if (argument === "--self-test") parsed.selfTest = true;
    else throw new Error(`unknown argument: ${argument}`);
  }
  assert(["ci", "release"].includes(parsed.mode), "mode must be ci or release");
  return parsed;
}
