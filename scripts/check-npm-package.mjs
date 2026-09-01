#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { npmInvocation } from "./lib/npm-command.mjs";

const args = parseArgs(process.argv.slice(2));
const packageName = required(args, "package");
const version = required(args, "version");
const specification = `${packageName}@${version}`;
const invocation = npmInvocation([
  "view",
  specification,
  "name",
  "version",
  "dist.integrity",
  "--json",
  "--prefer-online"
]);
const result = spawnSync(invocation.command, invocation.args, { encoding: "utf8" });

if (result.status !== 0) {
  const output = `${result.stderr}\n${result.stdout}`;
  if (output.includes("E404")) {
    process.stdout.write("false\n");
    process.exit(0);
  }
  throw new Error(`unable to query ${specification}: ${result.error?.message || result.stderr || result.stdout}`);
}

const response = JSON.parse(result.stdout);
const metadata = Array.isArray(response) ? response[0] : response;
if (
  metadata?.name !== packageName ||
  metadata.version !== version ||
  typeof metadata["dist.integrity"] !== "string"
) {
  throw new Error(`registry package metadata is incomplete: ${specification}`);
}

process.stdout.write("true\n");

function parseArgs(values) {
  const allowed = new Set(["package", "version"]);
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
