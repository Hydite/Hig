#!/usr/bin/env node
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const mainManifest = readJson("packages/hig-mcp-server/package.json");
const expected = mainManifest.version;
assert.match(expected, /^\d+\.\d+\.\d+$/, "main npm package has an invalid version");

const cargo = spawnSync(
  "cargo",
  ["metadata", "--no-deps", "--format-version", "1"],
  { cwd: root, encoding: "utf8" }
);
assert.equal(cargo.status, 0, cargo.stderr || "cargo metadata failed");
const workspacePackages = new Map(
  JSON.parse(cargo.stdout).packages.map((entry) => [entry.name, entry.version])
);
for (const name of ["hig-core", "hig-cli", "hig-ffi", "hig-desktop"]) {
  assert.equal(workspacePackages.get(name), expected, `${name} version is not ${expected}`);
}

for (const file of [
  "apps/hig-desktop/package.json",
  "apps/hig-desktop/src-tauri/tauri.conf.json",
  "packages/hig-darwin-universal/package.json",
  "packages/hig-linux-x64-gnu/package.json",
  "packages/hig-win32-x64-msvc/package.json"
]) {
  assert.equal(readJson(file).version, expected, `${file} version is not ${expected}`);
}

for (const dependency of [
  "@zorker/hig-darwin-universal",
  "@zorker/hig-linux-x64-gnu",
  "@zorker/hig-win32-x64-msvc"
]) {
  assert.equal(
    mainManifest.optionalDependencies[dependency],
    expected,
    `${dependency} optional dependency is not ${expected}`
  );
}

const cli = spawnSync(
  "cargo",
  ["run", "--quiet", "-p", "hig-cli", "--", "--version"],
  { cwd: root, encoding: "utf8" }
);
assert.equal(cli.status, 0, cli.stderr || "CLI version probe failed");
assert.equal(cli.stdout.trim(), `hig ${expected}`, "CLI runtime version drifted");

const binary = path.join(
  root,
  "target",
  "debug",
  process.platform === "win32" ? "hig.exe" : "hig"
);
const smoke = spawnSync(
  process.execPath,
  [path.join(root, "packages/hig-mcp-server/bin/hig-mcp-server.js"), "--smoke"],
  { cwd: root, encoding: "utf8", env: { ...process.env, HIG_BIN: binary } }
);
assert.equal(smoke.status, 0, smoke.stderr || "MCP smoke failed");
assert.equal(smoke.stdout.trim(), `hig ${expected}`, "MCP runtime version drifted");

process.stdout.write(`hig-release-version: PASS ${expected}\n`);

function readJson(relative) {
  return JSON.parse(fs.readFileSync(path.join(root, relative), "utf8"));
}
