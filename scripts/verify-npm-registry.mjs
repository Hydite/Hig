#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { npmInvocation } from "./lib/npm-command.mjs";

const version = required(process.argv.slice(2), "--version");
const packages = [
  "@zorker/hig-darwin-universal",
  "@zorker/hig-linux-x64-gnu",
  "@zorker/hig-win32-x64-msvc"
];

for (const packageName of packages) {
  const specification = `${packageName}@${version}`;
  const invocation = npmInvocation(["view", specification, "version", "dist.integrity", "--json"]);
  const result = spawnSync(invocation.command, invocation.args, { encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`registry package missing or unreadable: ${specification}`);
  }
  const metadata = JSON.parse(result.stdout);
  if (metadata.version !== version || typeof metadata["dist.integrity"] !== "string") {
    throw new Error(`registry package metadata is incomplete: ${specification}`);
  }
  process.stdout.write(`${specification} ${metadata["dist.integrity"]}\n`);
}

function required(values, flag) {
  const index = values.indexOf(flag);
  const value = index >= 0 ? values[index + 1] : undefined;
  if (!value) throw new Error(`${flag} is required`);
  return value;
}
