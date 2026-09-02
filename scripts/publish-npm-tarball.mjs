#!/usr/bin/env node
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { npmInvocation } from "./lib/npm-command.mjs";

const REGISTRY_PROPAGATION_ATTEMPTS = 72;
const REGISTRY_PROPAGATION_DELAY_MS = 5000;

const args = parseArgs(process.argv.slice(2));
const tarball = path.resolve(required(args, "tarball"));
const packageName = required(args, "package");
const version = required(args, "version");
const specification = `${packageName}@${version}`;
const integrity = `sha512-${createHash("sha512").update(fs.readFileSync(tarball)).digest("base64")}`;
const view = npmInvocation(["view", specification, "dist.integrity", "--json", "--prefer-online"]);
const existing = await registryIntegrity(view, specification);

if (existing !== null) {
  if (existing !== integrity) {
    throw new Error(
      `${specification} already exists with different integrity: ${existing}`
    );
  }
  process.stdout.write(`${specification} already published with matching integrity; skipping\n`);
  process.exit(0);
}

const publish = npmInvocation(["publish", tarball, "--access", "public", "--provenance"]);
const published = spawnSync(publish.command, publish.args, { stdio: "inherit" });
if (published.status !== 0) process.exit(published.status || 1);

const publishedIntegrity = await registryIntegrity(
  view,
  specification,
  REGISTRY_PROPAGATION_ATTEMPTS,
  REGISTRY_PROPAGATION_DELAY_MS
);
if (publishedIntegrity === null) {
  throw new Error(`published ${specification} but registry verification timed out`);
}
if (publishedIntegrity !== integrity) {
  throw new Error(`registry integrity mismatch after publishing ${specification}`);
}
process.stdout.write(`${specification} published and verified: ${integrity}\n`);

async function registryIntegrity(invocation, packageSpec, attempts = 1, delayMs = 0) {
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    const result = spawnSync(invocation.command, invocation.args, { encoding: "utf8" });
    if (result.status === 0) {
      const output = result.stdout.trim();
      return output ? normalizeIntegrity(JSON.parse(output)) : null;
    }
    const lastError = result.error?.message || result.stderr || result.stdout;
    if (!`${result.stderr}\n${result.stdout}`.includes("E404")) {
      throw new Error(`unable to query ${packageSpec}: ${lastError}`);
    }
    if (attempt + 1 < attempts) await delay(delayMs);
  }
  if (attempts === 1) return null;
  return null;
}

function normalizeIntegrity(response) {
  if (Array.isArray(response)) return normalizeIntegrity(response[0]);
  if (typeof response === "string") return response;
  if (response && typeof response["dist.integrity"] === "string") {
    return response["dist.integrity"];
  }
  return null;
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function parseArgs(values) {
  const allowed = new Set(["tarball", "package", "version"]);
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
