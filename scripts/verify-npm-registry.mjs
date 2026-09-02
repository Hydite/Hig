#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { npmInvocation } from "./lib/npm-command.mjs";

const args = process.argv.slice(2);
const version = required(args, "--version");
const attempts = positiveInteger(args, "--attempts", 72);
const delayMs = positiveInteger(args, "--delay-ms", 5000);
const packages = [
  "@zorker/hig-darwin-universal",
  "@zorker/hig-linux-x64-gnu",
  "@zorker/hig-win32-x64-msvc"
];

for (const packageName of packages) {
  const specification = `${packageName}@${version}`;
  const invocation = npmInvocation(["view", specification, "version", "dist.integrity", "--json"]);
  let failure = "unreadable metadata";
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    const result = spawnSync(invocation.command, [...invocation.args, "--prefer-online"], {
      encoding: "utf8"
    });
    if (result.status === 0) {
      try {
        const response = JSON.parse(result.stdout);
        const metadata = Array.isArray(response) ? response[0] : response;
        if (metadata.version === version && typeof metadata["dist.integrity"] === "string") {
          process.stdout.write(`${specification} ${metadata["dist.integrity"]}\n`);
          failure = undefined;
          break;
        }
        failure = "incomplete metadata";
      } catch {
        failure = "invalid metadata response";
      }
    } else {
      failure = "package not yet readable";
    }
    if (attempt < attempts) {
      process.stderr.write(
        `waiting for registry propagation: ${specification} (${attempt}/${attempts})\n`
      );
      await sleep(delayMs);
    }
  }
  if (failure) {
    throw new Error(`registry package verification failed after ${attempts} attempts: ${specification} (${failure})`);
  }
}

function required(values, flag) {
  const index = values.indexOf(flag);
  const value = index >= 0 ? values[index + 1] : undefined;
  if (!value) throw new Error(`${flag} is required`);
  return value;
}

function positiveInteger(values, flag, fallback) {
  const index = values.indexOf(flag);
  if (index < 0) return fallback;
  const value = Number(values[index + 1]);
  if (!Number.isInteger(value) || value <= 0) throw new Error(`${flag} must be a positive integer`);
  return value;
}

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
