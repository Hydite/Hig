#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { npmInvocation } from "./lib/npm-command.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = parseArgs(process.argv.slice(2));
const binary = path.resolve(required(args, "binary"));
const platform = required(args, "platform");
const outputDir = path.resolve(args["output-dir"] || path.join(root, "artifacts", "npm"));
const specification = platformSpecification(platform);
const mainSource = path.join(root, "packages", "hig-mcp-server");
const templateSource = path.join(root, "packages", specification.template);
const mainManifest = readJson(path.join(mainSource, "package.json"));
const templateManifest = readJson(path.join(templateSource, "package.json"));
const version = args.version || mainManifest.version;

fs.mkdirSync(outputDir, { recursive: true });
const stageRoot = fs.mkdtempSync(path.join(outputDir, ".hig-npm-stage-"));
assertBinary(binary, specification);
assertVersion(version, mainManifest, templateManifest);

try {
  const platformStage = path.join(stageRoot, specification.template);
  const mainStage = path.join(stageRoot, "hig");
  stagePlatformPackage(platformStage);
  stageMainPackage(mainStage);
  const platformPack = npmPack(platformStage, outputDir);
  const mainPack = npmPack(mainStage, outputDir);
  process.stdout.write(`${JSON.stringify({
    version,
    platform,
    main: path.join(outputDir, mainPack.filename),
    main_integrity: mainPack.integrity,
    native: path.join(outputDir, platformPack.filename),
    native_integrity: platformPack.integrity
  })}\n`);
} finally {
  fs.rmSync(stageRoot, { recursive: true, force: true });
}

function stagePlatformPackage(destination) {
  fs.mkdirSync(path.join(destination, "bin"), { recursive: true });
  fs.copyFileSync(path.join(templateSource, "README.md"), path.join(destination, "README.md"));
  fs.copyFileSync(path.join(mainSource, "LICENSE"), path.join(destination, "LICENSE"));
  fs.copyFileSync(binary, path.join(destination, "bin", specification.executable));
  if (platform !== "windows-x86_64-msvc") {
    fs.chmodSync(path.join(destination, "bin", specification.executable), 0o755);
  }
  const manifest = { ...templateManifest, version };
  delete manifest.private;
  writeJson(path.join(destination, "package.json"), manifest);
}

function stageMainPackage(destination) {
  const files = [
    "README.md",
    "tools.md",
    "LICENSE",
    "package.json",
    "bin/hig.js",
    "bin/hig-mcp-server.js",
    "lib/resolve-hig.js",
    "examples/mcp-config.json"
  ];
  for (const relative of files) {
    const source = path.join(mainSource, relative);
    const target = path.join(destination, relative);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.copyFileSync(source, target);
  }
  fs.chmodSync(path.join(destination, "bin", "hig.js"), 0o755);
  fs.chmodSync(path.join(destination, "bin", "hig-mcp-server.js"), 0o755);
}

function assertBinary(file, specification) {
  const stat = fs.statSync(file);
  if (!stat.isFile() || stat.size === 0) throw new Error(`invalid native binary: ${file}`);
  const header = fs.readFileSync(file).subarray(0, 4);
  const hex = header.toString("hex");
  if (specification.format === "elf" && hex !== "7f454c46") {
    throw new Error(`expected ELF binary for ${platform}, got ${hex}`);
  }
  if (specification.format === "pe" && header.subarray(0, 2).toString("ascii") !== "MZ") {
    throw new Error(`expected PE binary for ${platform}, got ${hex}`);
  }
  if (specification.format === "mach-fat" && !["cafebabe", "cafebabf"].includes(hex)) {
    throw new Error(`expected universal Mach-O binary for ${platform}, got ${hex}`);
  }
  if (specification.runtime === `${process.platform}-${process.arch}` || specification.runtime === process.platform) {
    const result = spawnSync(file, ["--version"], { encoding: "utf8" });
    if (result.status !== 0 || result.stdout.trim() !== "hig 1.10.0") {
      throw new Error(`native binary version check failed: ${result.stderr || result.stdout}`);
    }
  }
}

function assertVersion(version, ...manifests) {
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error(`invalid release version: ${version}`);
  for (const manifest of manifests) {
    if (manifest.version !== version) {
      throw new Error(`package version mismatch: expected ${version}, got ${manifest.version}`);
    }
  }
}

function npmPack(directory, destination) {
  const npm = npmInvocation(["pack", "--json", "--pack-destination", destination]);
  const result = spawnSync(npm.command, npm.args, {
    cwd: directory,
    encoding: "utf8"
  });
  if (result.status !== 0) {
    throw new Error(`npm pack failed: ${result.error?.message || result.stderr || result.stdout}`);
  }
  const parsed = JSON.parse(result.stdout);
  if (!Array.isArray(parsed) || parsed.length !== 1) throw new Error("npm pack returned an unexpected report");
  return parsed[0];
}

function platformSpecification(value) {
  const specifications = {
    "macos-universal": {
      template: "hig-darwin-universal",
      executable: "hig",
      format: "mach-fat",
      runtime: "darwin"
    },
    "linux-x86_64-gnu": {
      template: "hig-linux-x64-gnu",
      executable: "hig",
      format: "elf",
      runtime: "linux-x64"
    },
    "windows-x86_64-msvc": {
      template: "hig-win32-x64-msvc",
      executable: "hig.exe",
      format: "pe",
      runtime: "win32-x64"
    }
  };
  const selected = specifications[value];
  if (!selected) throw new Error(`unsupported npm release platform: ${value}`);
  return selected;
}

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
  if (!values[key]) throw new Error(`--${key} is required`);
  return values[key];
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}
