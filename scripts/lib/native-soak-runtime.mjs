import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

export function digestTree(root) {
  const hash = crypto.createHash("sha256");
  const visit = (directory, relative = "") => {
    const entries = fs.readdirSync(directory, { withFileTypes: true })
      .filter((entry) => !(relative === "" && entry.name === ".hig"))
      .sort((left, right) => left.name.localeCompare(right.name, "en"));
    for (const entry of entries) {
      const itemRelative = relative ? `${relative}/${entry.name}` : entry.name;
      const item = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        hash.update(`d\0${itemRelative}\0`);
        visit(item, itemRelative);
      } else if (entry.isFile()) {
        const bytes = fs.readFileSync(item);
        hash.update(`f\0${itemRelative}\0${bytes.length}\0`);
        hash.update(bytes);
      } else if (entry.isSymbolicLink()) {
        hash.update(`l\0${itemRelative}\0${fs.readlinkSync(item)}\0`);
      }
    }
  };
  visit(root);
  return hash.digest("hex");
}

export function countEntries(root) {
  if (!fs.existsSync(root)) return 0;
  let count = 0;
  const visit = (directory) => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      count += 1;
      if (entry.isDirectory()) visit(path.join(directory, entry.name));
    }
  };
  visit(root);
  return count;
}

export function countFiles(root, predicate = () => true) {
  if (!fs.existsSync(root)) return 0;
  let count = 0;
  const visit = (directory) => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      const item = path.join(directory, entry.name);
      if (entry.isDirectory()) visit(item);
      else if (entry.isFile() && predicate(entry.name)) count += 1;
    }
  };
  visit(root);
  return count;
}

export function fillDeterministic(buffer, seed) {
  let value = seed >>> 0;
  for (let offset = 0; offset < buffer.length; offset += 4) {
    value ^= value << 13;
    value ^= value >>> 17;
    value ^= value << 5;
    buffer.writeUInt32LE(value >>> 0, offset);
  }
}

export function spawnObserved(command, args, cwd) {
  const child = spawn(command, args, { cwd, stdio: ["ignore", "pipe", "pipe"] });
  child.capturedStdout = "";
  child.capturedStderr = "";
  child.stdout.on("data", (chunk) => { child.capturedStdout += chunk; });
  child.stderr.on("data", (chunk) => { child.capturedStderr += chunk; });
  return child;
}

export async function runProcess(command, args, { cwd, timeoutMs }) {
  const child = spawn(command, args, { cwd, stdio: ["ignore", "pipe", "pipe"] });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk) => { stdout += chunk; });
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  const timer = setTimeout(() => child.kill("SIGKILL"), timeoutMs);
  return new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      resolve({ code, signal, stdout, stderr });
    });
  });
}

export async function terminateProcess(child) {
  const closed = new Promise((resolve) => child.once("close", (code, signal) => resolve({
    code,
    signal,
    stdout: child.capturedStdout || "",
    stderr: child.capturedStderr || "",
  })));
  child.kill("SIGKILL");
  return Promise.race([
    closed,
    delay(10_000).then(() => { throw new Error("terminated child did not exit"); }),
  ]);
}

export async function waitFor(operation, timeoutMs, description) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = await operation();
    if (value) return value;
    await delay(20);
  }
  throw new Error(`timed out waiting for ${description}`);
}

export function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

export class McpClient {
  static async start({ higBin, mcpServer, workspace, allowedRoot }) {
    const client = new McpClient(higBin, mcpServer, workspace, allowedRoot);
    await client.initialize();
    return client;
  }

  constructor(higBin, mcpServer, workspace, allowedRoot) {
    this.nextId = 1;
    this.watcherSnapshots = 0;
    this.pending = new Map();
    this.output = Buffer.alloc(0);
    this.stderr = "";
    this.child = spawn(process.execPath, [mcpServer], {
      cwd: workspace,
      env: {
        ...process.env,
        HIG_BIN: higBin,
        HIG_MCP_ALLOWED_ROOTS: allowedRoot,
        HIG_MCP_WORKDIR: workspace,
        HIG_MCP_TIMEOUT_MS: "300000",
      },
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.child.stdout.on("data", (chunk) => {
      this.output = Buffer.concat([this.output, chunk]);
      this.drain();
    });
    this.child.stderr.on("data", (chunk) => { this.stderr += chunk.toString("utf8"); });
    this.child.on("close", (code, signal) => {
      for (const waiter of this.pending.values()) {
        waiter.reject(new Error(`MCP exited code=${code} signal=${signal}\n${this.stderr}`));
      }
      this.pending.clear();
    });
  }

  async initialize() {
    const response = await this.request("initialize", {
      protocolVersion: "2024-11-05",
      capabilities: {},
      clientInfo: { name: "hig-native-soak", version: "1" },
    });
    assert.equal(response.result?.protocolVersion, "2024-11-05", "MCP initialization failed");
  }

  async tool(name, args = {}) {
    const response = await this.request("tools/call", { name, arguments: args });
    assert.equal(response.error, undefined, `${name}: JSON-RPC error`);
    const payload = JSON.parse(response.result.content[0].text);
    assert.equal(response.result.isError, false, `${name}: ${payload.stderr || payload.error}`);
    assert.equal(payload.ok, true, `${name}: operation failed`);
    return payload;
  }

  request(method, params) {
    const id = this.nextId++;
    const bytes = Buffer.from(JSON.stringify({ jsonrpc: "2.0", id, method, params }), "utf8");
    this.child.stdin.write(`Content-Length: ${bytes.length}\r\n\r\n`);
    this.child.stdin.write(bytes);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`MCP request timed out: ${method}`));
      }, 300_000);
      this.pending.set(id, {
        resolve: (value) => { clearTimeout(timer); resolve(value); },
        reject: (error) => { clearTimeout(timer); reject(error); },
      });
    });
  }

  drain() {
    for (;;) {
      const headerEnd = this.output.indexOf("\r\n\r\n");
      if (headerEnd < 0) return;
      const header = this.output.subarray(0, headerEnd).toString("utf8");
      const match = /^Content-Length:\s*(\d+)/im.exec(header);
      assert(match, `MCP response is missing Content-Length: ${header}`);
      const length = Number(match[1]);
      const bodyStart = headerEnd + 4;
      if (this.output.length < bodyStart + length) return;
      const body = JSON.parse(this.output.subarray(bodyStart, bodyStart + length).toString("utf8"));
      this.output = this.output.subarray(bodyStart + length);
      const waiter = this.pending.get(body.id);
      if (waiter) {
        this.pending.delete(body.id);
        waiter.resolve(body);
      }
    }
  }

  async killHard() {
    if (this.child.exitCode !== null) return;
    await terminateProcess(this.child);
  }

  async close() {
    if (this.child.exitCode !== null) return;
    const closed = new Promise((resolve) => this.child.once("close", resolve));
    this.child.stdin.end();
    const graceful = await Promise.race([closed.then(() => true), delay(5000).then(() => false)]);
    if (!graceful && this.child.exitCode === null) await this.killHard();
  }
}
