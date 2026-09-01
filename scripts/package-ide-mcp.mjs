#!/usr/bin/env node
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
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
fs.mkdirSync(outputDir, { recursive: true });
const stageRoot = fs.mkdtempSync(path.join(outputDir, ".hig-package-stage-"));
const stage = path.join(stageRoot, "hig-mcp-server");

assertFile(binary);
fs.mkdirSync(path.join(stage, "bin"), { recursive: true });
fs.mkdirSync(path.join(stage, "lib"), { recursive: true });
fs.mkdirSync(path.join(stage, "examples"), { recursive: true });
for (const relative of ["README.md", "LICENSE", "package.json", "tools.md", "bin/hig.js", "bin/hig-mcp-server.js", "lib/resolve-hig.js", "examples/mcp-config.json"]) {
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

const tar = spawnSync("tar", ["-czf", path.join("..", archiveName), "hig-mcp-server"], {
  cwd: stageRoot,
  stdio: "inherit"
});
if (tar.status !== 0) process.exit(tar.status || 1);
const digest = createHash("sha256").update(fs.readFileSync(archive)).digest("hex");
fs.writeFileSync(`${archive}.sha256`, `${digest}  ${archiveName}\n`);
console.log(JSON.stringify({ archive, sha256: digest, platform, version }));

function parseArgs(values) {
  const allowed = new Set(["binary", "platform", "output-dir", "version"]);
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
  const value = values[key];
  if (!value) throw new Error(`--${key} is required`);
  return value;
}

function assertFile(file) {
  const stat = fs.statSync(file);
  if (!stat.isFile() || stat.size === 0) throw new Error(`invalid CLI binary: ${file}`);
}
