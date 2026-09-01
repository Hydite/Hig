#!/usr/bin/env node
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { npmInvocation } from "./lib/npm-command.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = parseArgs(process.argv.slice(2));
const mainTarball = path.resolve(required(args, "main"));
const nativeTarball = path.resolve(required(args, "native"));
const platformPackage = required(args, "platform-package");
const work = fs.mkdtempSync(path.join(os.tmpdir(), "hig-npm-verify-"));
const installRoot = path.join(work, "install");
const mainRoot = path.join(installRoot, "node_modules", "@zorker", "hig");
const nativeRoot = path.join(installRoot, "node_modules", "@zorker", platformPackage);
const nativeBinary = path.join(nativeRoot, "bin", process.platform === "win32" ? "hig.exe" : "hig");
const launcher = path.join(mainRoot, "bin", "hig.js");
const server = path.join(mainRoot, "bin", "hig-mcp-server.js");
const cleanEnv = { ...process.env };
delete cleanEnv.HIG_BIN;

fs.mkdirSync(installRoot, { recursive: true });
fs.writeFileSync(path.join(installRoot, "package.json"), "{\"private\":true}\n");
runNpm([
  "install", "--ignore-scripts", "--no-audit", "--no-fund", "--package-lock=false",
  nativeTarball, mainTarball
], { cwd: installRoot });

for (const file of [nativeBinary, launcher, server, path.join(mainRoot, "lib", "resolve-hig.js")]) {
  assert(fs.statSync(file).isFile(), `installed file is missing: ${file}`);
}
assert.equal(fs.existsSync(path.join(mainRoot, "bin", process.platform === "win32" ? "hig.exe" : "hig")), false,
  "main package must not bundle a native binary");

const version = run(process.execPath, [launcher, "--version"], { capture: true, env: cleanEnv });
assert.equal(version.stdout.trim(), "hig 1.10.0", `unexpected launcher version: ${version.stdout}`);
const smoke = run(process.execPath, [server, "--smoke"], { capture: true, env: cleanEnv });
assert.equal(smoke.stdout.trim(), "hig 1.10.0", `unexpected MCP smoke output: ${smoke.stdout}`);

const fixture = path.join(work, "fixture");
const archive = path.join(work, "fixture.hig");
const restored = path.join(work, "restored");
fs.mkdirSync(fixture, { recursive: true });
fs.writeFileSync(path.join(fixture, "hello.txt"), "HIG npm package verification\n");
run(process.execPath, [launcher, "pack", fixture, "--output", archive, "--encryption", "none", "--daemon", "off", "--project", "off", "--json"], { capture: true, env: cleanEnv });
run(process.execPath, [launcher, "unpack", archive, "--output-dir", restored], { capture: true, env: cleanEnv });
assert.equal(fs.readFileSync(path.join(restored, "hello.txt"), "utf8"), "HIG npm package verification\n");

run(process.execPath, [path.join(root, "scripts", "mcp-integration-test.mjs")], {
  env: { ...cleanEnv, HIG_BIN: nativeBinary, HIG_MCP_SERVER: server }
});

process.stdout.write(`hig-npm-package: PASS ${process.platform}/${process.arch} ${path.basename(mainTarball)} ${path.basename(nativeTarball)}\n`);

function run(command, commandArgs, options = {}) {
  const result = spawnSync(command, commandArgs, {
    cwd: options.cwd || root,
    env: options.env || process.env,
    stdio: options.capture ? ["ignore", "pipe", "pipe"] : "inherit",
    encoding: options.capture ? "utf8" : undefined
  });
  if (result.status !== 0) {
    throw new Error(`${command} failed with status ${result.status}: ${result.stderr || ""}`);
  }
  return result;
}

function runNpm(args, options = {}) {
  const invocation = npmInvocation(args);
  return run(invocation.command, invocation.args, options);
}

function parseArgs(values) {
  const allowed = new Set(["main", "native", "platform-package"]);
  const parsed = Object.create(null);
  for (let index = 0; index < values.length; index += 2) {
    const flag = values[index];
    if (!flag?.startsWith("--") || values[index + 1] === undefined) {
      throw new Error(`invalid argument sequence near ${flag || "end"}`);
    }
    const key = flag.slice(2);
    if (!allowed.has(key)) throw new Error(`unsupported argument: --${key}`);
    parsed[key] = values[index + 1];
  }
  return parsed;
}

function required(values, key) {
  if (!values[key]) throw new Error(`--${key} is required`);
  return values[key];
}
