#!/usr/bin/env node
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const server = path.join(root, "packages", "hig-mcp-server", "bin", "hig-mcp-server.js");
const work = fs.mkdtempSync(path.join(os.tmpdir(), "hig-mcp-security-"));
const outside = fs.mkdtempSync(path.join(os.tmpdir(), "hig-mcp-security-outside-"));
const fakeScript = path.join(work, "fake-hig.mjs");
const capture = path.join(work, "capture.jsonl");
const events = path.join(work, "events.jsonl");
const fakeBinary = createFakeBinary();

function createFakeBinary() {
  fs.writeFileSync(fakeScript, `
import fs from "node:fs";
const args = process.argv.slice(2);
let stdin = "";
for await (const chunk of process.stdin) stdin += chunk;
const now = () => Number(process.hrtime.bigint());
const append = (file, value) => file && fs.appendFileSync(file, JSON.stringify(value) + "\\n");
append(process.env.HIG_TEST_CAPTURE, {
  args,
  stdin,
  envSecret: process.env.HIG_TEST_SECRET || null,
  enforcedRoots: process.env.HIG_MCP_ENFORCED_ROOTS || null
});
append(process.env.HIG_TEST_EVENTS, { kind: "start", pid: process.pid, at: now() });
await new Promise((resolve) => setTimeout(resolve, Number(process.env.HIG_TEST_DELAY_MS || 0)));
append(process.env.HIG_TEST_EVENTS, { kind: "end", pid: process.pid, at: now() });
if (args[0] === "--version") {
  process.stdout.write("hig 1.10.0\\n");
} else {
  process.stdout.write(JSON.stringify({ fake: true, mirror_roots: [] }) + "\\n");
}
`);

  if (process.platform === "win32") {
    const command = path.join(work, "fake-hig.cmd");
    fs.writeFileSync(command, `@echo off\r\n"${process.execPath}" "${fakeScript}" %*\r\n`);
    return command;
  }
  const command = path.join(work, "fake-hig");
  fs.writeFileSync(
    command,
    `#!/bin/sh\nexec "${process.execPath}" "${fakeScript}" "$@"\n`,
    { mode: 0o700 }
  );
  return command;
}

async function rejectsOversizedFrame() {
  const child = startServer({ HIG_MCP_MAX_REQUEST_BYTES: "128" });
  let stderr = "";
  child.stderr.on("data", (chunk) => { stderr += chunk.toString("utf8"); });
  child.stdin.end("Content-Length: 129\r\n\r\n");
  const result = await waitForExit(child, 5000);
  assert.equal(result.code, 1, `oversized frame exit: ${JSON.stringify(result)}`);
  assert.match(stderr, /Content-Length exceeds the configured limit/);
}

async function rejectsExcessiveResourceLimit() {
  const child = startServer({ HIG_MCP_MAX_OUTPUT_BYTES: String(64 * 1024 * 1024 + 1) });
  let stderr = "";
  child.stderr.on("data", (chunk) => { stderr += chunk.toString("utf8"); });
  const result = await waitForExit(child, 5000);
  assert.equal(result.code, 1, `excessive resource limit exit: ${JSON.stringify(result)}`);
  assert.match(stderr, /HIG_MCP_MAX_OUTPUT_BYTES must be a positive safe integer <= 67108864/);
}

async function keepsSecretsOutOfArguments() {
  fs.rmSync(capture, { force: true });
  const client = new McpClient(startServer({ HIG_TEST_CAPTURE: capture }));
  const secret = "mcp-secret-value-42";
  try {
    await client.initialize();
    const response = await client.request("tools/call", {
      name: "hig_pack",
      arguments: {
        inputDir: work,
        output: path.join(work, "secret-test.hig"),
        password: secret
      }
    });
    assert.equal(response.result.isError, false, JSON.stringify(response));
  } finally {
    await client.close();
  }
  const records = readJsonLines(capture);
  assert.equal(records.length, 1);
  assert(!records[0].args.includes(secret), "secret leaked into child arguments");
  assert.equal(records[0].stdin, `${secret}\n`);
  assert.equal(records[0].envSecret, null);
  const physicalWork = fs.realpathSync.native(work);
  assert.equal(records[0].args[1], physicalWork);
  assert.equal(records[0].args[3], path.join(physicalWork, "secret-test.hig"));
  assert.equal(records[0].enforcedRoots, physicalWork);
}

async function boundsConcurrentProcesses() {
  fs.rmSync(events, { force: true });
  const client = new McpClient(startServer({
    HIG_MCP_MAX_INFLIGHT_TOOLS: "2",
    HIG_MCP_MAX_QUEUED_TOOLS: "8",
    HIG_TEST_EVENTS: events,
    HIG_TEST_DELAY_MS: "250"
  }));
  try {
    await client.initialize();
    const responses = await Promise.all(Array.from({ length: 6 }, () => (
      client.request("tools/call", { name: "hig_version", arguments: {} })
    )));
    assert(responses.every((response) => response.result.isError === false));
  } finally {
    await client.close();
  }
  let active = 0;
  let peak = 0;
  for (const event of readJsonLines(events).sort((left, right) => left.at - right.at)) {
    active += event.kind === "start" ? 1 : -1;
    peak = Math.max(peak, active);
  }
  assert.equal(active, 0);
  assert(peak <= 2, `observed ${peak} concurrent HIG processes`);
}

async function boundsQueuedCalls() {
  const client = new McpClient(startServer({
    HIG_MCP_MAX_INFLIGHT_TOOLS: "1",
    HIG_MCP_MAX_QUEUED_TOOLS: "1",
    HIG_TEST_DELAY_MS: "300"
  }));
  try {
    await client.initialize();
    const responses = await Promise.all(Array.from({ length: 4 }, () => (
      client.request("tools/call", { name: "hig_version", arguments: {} })
    )));
    const rejected = responses.filter((response) => response.result.isError);
    assert(rejected.length >= 2, `expected queue rejection: ${JSON.stringify(responses)}`);
    for (const response of rejected) {
      assert.match(response.result.content[0].text, /tool queue is full/);
    }
  } finally {
    await client.close();
  }
}

async function rejectsRecoveryAuthenticationOutsideAllowedRoots() {
  const vault = path.join(work, "recovery-vault");
  const client = new McpClient(startServer({
    HIG_RECOVERY_AUTH_DIR: path.join(outside, "recovery-auth")
  }));
  try {
    await client.initialize();
    const response = await client.request("tools/call", {
      name: "hig_recovery_init",
      arguments: { vaultRoot: vault }
    });
    assert.equal(response.result.isError, true, JSON.stringify(response));
    assert.match(response.result.content[0].text, /outside allowed roots/i);
    assert.equal(fs.existsSync(vault), false);
  } finally {
    await client.close();
  }
}

function startServer(extraEnv = {}) {
  return spawn(process.execPath, [server], {
    cwd: work,
    env: {
      ...process.env,
      HIG_BIN: fakeBinary,
      HIG_MCP_ALLOWED_ROOTS: work,
      HIG_MCP_WORKDIR: work,
      HIG_MCP_TIMEOUT_MS: "5000",
      ...extraEnv
    },
    stdio: ["pipe", "pipe", "pipe"]
  });
}

class McpClient {
  constructor(child) {
    this.child = child;
    this.nextId = 1;
    this.output = Buffer.alloc(0);
    this.pending = new Map();
    this.stderr = "";
    child.stdout.on("data", (chunk) => {
      this.output = Buffer.concat([this.output, chunk]);
      this.drain();
    });
    child.stderr.on("data", (chunk) => { this.stderr += chunk.toString("utf8"); });
    child.on("exit", (code, signal) => {
      for (const pending of this.pending.values()) {
        pending.reject(new Error(`MCP exited code=${code} signal=${signal}\n${this.stderr}`));
      }
      this.pending.clear();
    });
  }

  initialize() {
    return this.request("initialize", {
      protocolVersion: "2024-11-05",
      capabilities: {},
      clientInfo: { name: "hig-security-test", version: "1" }
    });
  }

  request(method, params = {}) {
    const id = this.nextId++;
    const body = Buffer.from(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    this.child.stdin.write(`Content-Length: ${body.length}\r\n\r\n`);
    this.child.stdin.write(body);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`MCP request timed out: ${method}\n${this.stderr}`));
      }, 10000);
      this.pending.set(id, {
        resolve: (value) => { clearTimeout(timer); resolve(value); },
        reject: (error) => { clearTimeout(timer); reject(error); }
      });
    });
  }

  drain() {
    for (;;) {
      const headerEnd = this.output.indexOf("\r\n\r\n");
      if (headerEnd < 0) return;
      const header = this.output.subarray(0, headerEnd).toString("utf8");
      const match = /^Content-Length:\s*(\d+)/im.exec(header);
      assert(match, `missing response Content-Length: ${header}`);
      const length = Number(match[1]);
      const bodyStart = headerEnd + 4;
      if (this.output.length < bodyStart + length) return;
      const response = JSON.parse(this.output.subarray(bodyStart, bodyStart + length));
      this.output = this.output.subarray(bodyStart + length);
      const pending = this.pending.get(response.id);
      if (pending) {
        this.pending.delete(response.id);
        pending.resolve(response);
      }
    }
  }

  async close() {
    if (this.child.exitCode !== null) return;
    this.child.stdin.end();
    await waitForExit(this.child, 5000);
  }
}

function readJsonLines(file) {
  if (!fs.existsSync(file)) return [];
  return fs.readFileSync(file, "utf8").trim().split(/\r?\n/).filter(Boolean).map(JSON.parse);
}

function waitForExit(child, timeoutMs) {
  if (child.exitCode !== null) return Promise.resolve({ code: child.exitCode, signal: child.signalCode });
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error("child exit timed out"));
    }, timeoutMs);
    child.once("exit", (code, signal) => {
      clearTimeout(timer);
      resolve({ code, signal });
    });
  });
}

try {
  await rejectsOversizedFrame();
  await rejectsExcessiveResourceLimit();
  await keepsSecretsOutOfArguments();
  await boundsConcurrentProcesses();
  await boundsQueuedCalls();
  await rejectsRecoveryAuthenticationOutsideAllowedRoots();
  console.log("hig-mcp-security: PASS");
} finally {
  fs.rmSync(work, { recursive: true, force: true });
  fs.rmSync(outside, { recursive: true, force: true });
}
