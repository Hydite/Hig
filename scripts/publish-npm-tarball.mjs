#!/usr/bin/env node
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { npmInvocation } from "./lib/npm-command.mjs";

const args = parseArgs(process.argv.slice(2));
const tarball = path.resolve(required(args, "tarball"));
const packageName = required(args, "package");
const version = required(args, "version");
const specification = `${packageName}@${version}`;
const integrity = `sha512-${createHash("sha512").update(fs.readFileSync(tarball)).digest("base64")}`;
const view = npmInvocation(["view", specification, "dist.integrity", "--json"]);
const existing = spawnSync(view.command, view.args, {
  encoding: "utf8"
});

if (existing.status === 0) {
  const publishedIntegrity = JSON.parse(existing.stdout);
  if (publishedIntegrity !== integrity) {
    throw new Error(
      `${specification} already exists with different integrity: ${publishedIntegrity}`
    );
  }
  process.stdout.write(`${specification} already published with matching integrity; skipping\n`);
  process.exit(0);
}

const notFound = `${existing.stderr}\n${existing.stdout}`.includes("E404");
if (!notFound) {
  throw new Error(`unable to query ${specification}: ${existing.stderr || existing.stdout}`);
}

const publish = npmInvocation(["publish", tarball, "--access", "public", "--provenance"]);
const published = spawnSync(publish.command, publish.args, { stdio: "inherit" });
if (published.status !== 0) process.exit(published.status || 1);

const verified = spawnSync(view.command, view.args, {
  encoding: "utf8"
});
if (verified.status !== 0) {
  throw new Error(`published ${specification} but registry verification failed`);
}
const publishedIntegrity = JSON.parse(verified.stdout);
if (publishedIntegrity !== integrity) {
  throw new Error(`registry integrity mismatch after publishing ${specification}`);
}
process.stdout.write(`${specification} published and verified: ${integrity}\n`);

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
