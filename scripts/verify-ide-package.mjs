#!/usr/bin/env node
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const archive = path.resolve(process.argv[2] || "");
const archiveBytes = readRegularFile(archive);
const checksum = `${archive}.sha256`;
const expected = fs.readFileSync(checksum, "utf8").trim().split(/\s+/)[0];
const actual = createHash("sha256").update(archiveBytes).digest("hex");
if (actual !== expected) throw new Error(`checksum mismatch: expected ${expected}, got ${actual}`);

const archiveDir = path.dirname(archive);
const extractRoot = fs.mkdtempSync(path.join(archiveDir, ".hig-package-verify-"));
run("tar", ["-xzf", path.basename(archive), "-C", path.basename(extractRoot)], { cwd: archiveDir });
const packageRoot = path.join(extractRoot, "hig-mcp-server");
const executableName = process.platform === "win32" ? "hig.exe" : "hig";
const binary = path.join(packageRoot, "bin", executableName);
const server = path.join(packageRoot, "bin", "hig-mcp-server.js");
const manifestPath = path.join(packageRoot, "package.json");
for (const file of [binary, server, manifestPath, path.join(packageRoot, "tools.md")]) {
  if (!fs.statSync(file).isFile()) throw new Error(`package file missing: ${file}`);
}

const packageVersion = JSON.parse(fs.readFileSync(manifestPath, "utf8")).version;
const version = run(binary, ["--version"], { capture: true }).stdout.trim();
if (version !== `hig ${packageVersion}`) throw new Error(`unexpected CLI version: ${version}`);
run(process.execPath, [server, "--smoke"], { env: { ...process.env, HIG_BIN: binary } });
run(process.execPath, [path.join(root, "scripts", "mcp-integration-test.mjs")], {
  env: { ...process.env, HIG_BIN: binary, HIG_MCP_SERVER: server }
});
console.log(`hig-ide-package: PASS ${path.basename(archive)} sha256=${actual}`);

function readRegularFile(file) {
  const descriptor = fs.openSync(file, "r");
  try {
    const stat = fs.fstatSync(descriptor);
    if (!stat.isFile() || stat.size === 0) throw new Error("archive path is required");
    return fs.readFileSync(descriptor);
  } finally {
    fs.closeSync(descriptor);
  }
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd,
    stdio: options.capture ? ["ignore", "pipe", "pipe"] : "inherit",
    encoding: options.capture ? "utf8" : undefined,
    env: options.env || process.env
  });
  if (result.status !== 0) {
    throw new Error(`${command} failed with status ${result.status}: ${result.stderr || ""}`);
  }
  return result;
}
