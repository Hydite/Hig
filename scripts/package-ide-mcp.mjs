#!/usr/bin/env node
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = parseArgs(process.argv.slice(2));
const binary = path.resolve(required(args, "binary"));
const platform = required(args, "platform");
const outputDir = path.resolve(args["output-dir"] || path.join(root, "artifacts", "ci"));
const packageJson = JSON.parse(fs.readFileSync(path.join(root, "packages", "hig-mcp-server", "package.json"), "utf8"));
const version = args.version || packageJson.version;
const executableName = platform.startsWith("windows-") ? "hig.exe" : "hig";
const archiveName = `hig-v${version}-ide-mcp-${platform}.tar.gz`;
const archive = path.join(outputDir, archiveName);
const stageRoot = fs.mkdtempSync(path.join(os.tmpdir(), "hig-package-"));
const stage = path.join(stageRoot, "hig-mcp-server");

assertFile(binary);
fs.mkdirSync(path.join(stage, "bin"), { recursive: true });
fs.mkdirSync(path.join(stage, "examples"), { recursive: true });
for (const relative of ["README.md", "package.json", "tools.md", "bin/hig-mcp-server.js", "examples/mcp-config.json"]) {
  const source = path.join(root, "packages", "hig-mcp-server", relative);
  const destination = path.join(stage, relative);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.copyFileSync(source, destination);
}
fs.copyFileSync(binary, path.join(stage, "bin", executableName));
if (!platform.startsWith("windows-")) {
  fs.chmodSync(path.join(stage, "bin", executableName), 0o755);
  fs.chmodSync(path.join(stage, "bin", "hig-mcp-server.js"), 0o755);
}

fs.mkdirSync(outputDir, { recursive: true });
const tar = spawnSync("tar", ["-czf", archive, "-C", stageRoot, "hig-mcp-server"], { stdio: "inherit" });
if (tar.status !== 0) process.exit(tar.status || 1);
const digest = createHash("sha256").update(fs.readFileSync(archive)).digest("hex");
fs.writeFileSync(`${archive}.sha256`, `${digest}  ${archiveName}\n`);
console.log(JSON.stringify({ archive, sha256: digest, platform, version }));

function parseArgs(values) {
  const parsed = {};
  for (let index = 0; index < values.length; index += 2) {
    const flag = values[index];
    if (!flag?.startsWith("--") || values[index + 1] === undefined) {
      throw new Error(`invalid argument sequence near ${flag || "end"}`);
    }
    parsed[flag.slice(2)] = values[index + 1];
  }
  return parsed;
}

function required(values, key) {
  const value = values[key];
  if (!value) throw new Error(`--${key} is required`);
  return value;
}

function assertFile(file) {
  const stat = fs.statSync(file);
  if (!stat.isFile() || stat.size === 0) throw new Error(`invalid CLI binary: ${file}`);
}
