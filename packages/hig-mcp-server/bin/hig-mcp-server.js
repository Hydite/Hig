#!/usr/bin/env node
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const VERSION = "1.10.0";
const PROTOCOL_VERSION = "2024-11-05";
const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const packageRoot = path.resolve(__dirname, "..");
const bundledHig = path.join(packageRoot, "bin", process.platform === "win32" ? "hig.exe" : "hig");

const MAX_OUTPUT_BYTES = Number(process.env.HIG_MCP_MAX_OUTPUT_BYTES || 1_000_000);
const DEFAULT_TIMEOUT_MS = Number(process.env.HIG_MCP_TIMEOUT_MS || 20 * 60 * 1000);
const allowAnyPath = process.env.HIG_MCP_ALLOW_ANY_PATH === "1";
const allowedRoots = computeAllowedRoots();
const repositoryWatchers = new Map();
let shuttingDown = false;

const tools = [
  {
    name: "hig_version",
    description: "Return the bundled or configured Hig CLI version.",
    inputSchema: objectSchema({})
  },
  {
    name: "hig_help",
    description: "Show Hig CLI help for a command path such as ['pack'] or ['daemon','status'].",
    inputSchema: objectSchema({
      command: {
        type: "array",
        items: { type: "string" },
        description: "Optional Hig command path."
      }
    })
  },
  {
    name: "hig_init_project",
    description: "Initialize Hig project metadata and optional excludes for a directory.",
    inputSchema: objectSchema({
      dir: pathProp("Project directory. Defaults to workspace root."),
      cacheDir: pathProp("Optional Hig cache directory."),
      excludes: {
        type: "array",
        items: { type: "string" },
        description: "Exclude patterns passed as repeated --exclude values."
      }
    })
  },
  {
    name: "hig_project_status",
    description: "Return project status as JSON.",
    inputSchema: objectSchema({
      dir: pathProp("Project directory. Defaults to workspace root.")
    })
  },
  {
    name: "hig_project_rebuild",
    description: "Submit or wait for a project snapshot rebuild.",
    inputSchema: objectSchema({
      dir: pathProp("Project directory. Defaults to workspace root."),
      wait: { type: "boolean", description: "Wait for rebuild completion." }
    })
  },
  {
    name: "hig_project_policy_show",
    description: "Return the versioned IDE automatic snapshot policy.",
    inputSchema: objectSchema({
      dir: pathProp("Project directory. Defaults to workspace root.")
    })
  },
  {
    name: "hig_project_policy_set",
    description: "Atomically update IDE automatic snapshot policy and apply it to the daemon.",
    inputSchema: objectSchema({
      dir: pathProp("Project directory. Defaults to workspace root."),
      enabled: { type: "boolean" },
      quiescenceMs: { type: "integer", minimum: 0 },
      periodicIntervalMs: { type: "integer", minimum: 0 },
      maxPendingEvents: { type: "integer", minimum: 1 },
      maxPendingFiles: { type: "integer", minimum: 1 },
      resourceEnabled: { type: "boolean" },
      minAvailableMemoryBytes: { type: "integer", minimum: 0 },
      resumeAvailableMemoryBytes: { type: "integer", minimum: 0 },
      resourcePollIntervalMs: { type: "integer", minimum: 1 }
    })
  },
  {
    name: "hig_daemon_status",
    description: "Return daemon status for a cache directory.",
    inputSchema: objectSchema({
      cacheDir: pathProp("Optional Hig cache directory.")
    })
  },
  {
    name: "hig_daemon_start",
    description: "Start a Hig daemon for a cache directory.",
    inputSchema: objectSchema({
      cacheDir: pathProp("Optional Hig cache directory."),
      ttlSecs: { type: "integer", minimum: 1, description: "Optional daemon TTL in seconds." }
    })
  },
  {
    name: "hig_daemon_stop",
    description: "Stop a Hig daemon for a cache directory.",
    inputSchema: objectSchema({
      cacheDir: pathProp("Optional Hig cache directory.")
    })
  },
  {
    name: "hig_session_status",
    description: "Return in-memory session status for a cache directory.",
    inputSchema: objectSchema({
      cacheDir: pathProp("Optional Hig cache directory.")
    })
  },
  {
    name: "hig_session_unlock",
    description: "Unlock an in-memory session key. Prefer this over passing passwords repeatedly to pack.",
    inputSchema: objectSchema({
      password: { type: "string", minLength: 1, description: "Archive password. Not logged by this adapter." },
      cacheDir: pathProp("Optional Hig cache directory."),
      ttlSecs: { type: "integer", minimum: 1, description: "Optional session TTL in seconds." },
      kdfProfile: { type: "string", enum: ["secure", "interactive", "fast-bench"], description: "KDF profile." }
    }, ["password"])
  },
  {
    name: "hig_session_clear",
    description: "Clear an in-memory session key.",
    inputSchema: objectSchema({
      cacheDir: pathProp("Optional Hig cache directory.")
    })
  },
  {
    name: "hig_pack",
    description: "Create a .hig archive. Defaults to HIGV2, secure encryption, daemon auto, and JSON output.",
    inputSchema: objectSchema({
      inputDir: pathProp("Directory to archive."),
      output: pathProp("Output .hig archive path."),
      password: { type: "string", description: "Archive password. Omit when useSession is true or encryption is none." },
      encryption: { type: "string", enum: ["password", "none"], description: "Encryption mode." },
      cacheDir: pathProp("Optional Hig cache directory."),
      threads: { type: "integer", minimum: 1 },
      level: { type: "integer" },
      noCache: { type: "boolean" },
      format: { type: "string", enum: ["higv1", "higv2"] },
      manifestFormat: { type: "string", enum: ["json", "compact"] },
      noBatch: { type: "boolean" },
      noChunk: { type: "boolean" },
      speed: { type: "string", enum: ["balanced", "fastest"] },
      kdfProfile: { type: "string", enum: ["secure", "interactive", "fast-bench"] },
      trustMetadata: { type: "boolean" },
      useSession: { type: "boolean" },
      daemon: { type: "string", enum: ["auto", "off", "required"] },
      project: { type: "string", enum: ["auto", "off", "required"] },
      solid: { type: "string", enum: ["auto", "off"] }
    }, ["inputDir", "output"])
  },
  {
    name: "hig_unpack",
    description: "Unpack a .hig archive into a directory.",
    inputSchema: objectSchema({
      archiveFile: pathProp("Input .hig archive path."),
      outputDir: pathProp("Destination directory."),
      password: { type: "string", description: "Archive password when required." },
      overwrite: { type: "boolean", description: "Allow replacing existing files." }
    }, ["archiveFile", "outputDir"])
  },
  {
    name: "hig_inspect",
    description: "Inspect archive metadata. JSON output is enabled by default.",
    inputSchema: objectSchema({
      archiveFile: pathProp("Input .hig archive path."),
      password: { type: "string", description: "Archive password when required." },
      json: { type: "boolean", description: "Return JSON. Defaults to true." }
    }, ["archiveFile"])
  },
  {
    name: "hig_migrate",
    description: "Verify and atomically migrate an HIGV1 or HIGV2 archive to a new HIGV2 archive.",
    inputSchema: objectSchema({
      source: pathProp("Source .hig archive."),
      output: pathProp("Target .hig archive."),
      password: { type: "string", description: "Source password; also used as target password when targetPassword is omitted." },
      targetPassword: { type: "string", description: "Optional target password." },
      encryption: { type: "string", enum: ["password", "none"] },
      overwrite: { type: "boolean", description: "Replace an existing target only after successful verification." }
    }, ["source", "output"])
  },
  {
    name: "hig_cache_status",
    description: "Return Hig cache status.",
    inputSchema: objectSchema({
      cacheDir: pathProp("Optional Hig cache directory.")
    })
  },
  {
    name: "hig_cache_gc",
    description: "Run or preview cache garbage collection.",
    inputSchema: objectSchema({
      cacheDir: pathProp("Optional Hig cache directory."),
      dryRun: { type: "boolean", description: "Preview only. Defaults to true." }
    })
  },
  {
    name: "hig_cache_compact",
    description: "Run or preview cache compaction.",
    inputSchema: objectSchema({
      cacheDir: pathProp("Optional Hig cache directory."),
      dryRun: { type: "boolean", description: "Preview only. Defaults to true." }
    })
  },
  {
    name: "hig_task_list",
    description: "List daemon tasks.",
    inputSchema: objectSchema({
      cacheDir: pathProp("Optional Hig cache directory."),
      includeCompleted: { type: "boolean" }
    })
  },
  {
    name: "hig_task_status",
    description: "Get daemon task status.",
    inputSchema: objectSchema({
      taskId: { type: "string", minLength: 1 },
      cacheDir: pathProp("Optional Hig cache directory.")
    }, ["taskId"])
  },
  {
    name: "hig_task_cancel",
    description: "Cancel daemon task.",
    inputSchema: objectSchema({
      taskId: { type: "string", minLength: 1 },
      cacheDir: pathProp("Optional Hig cache directory.")
    }, ["taskId"])
  },
  {
    name: "hig_task_result",
    description: "Get daemon task result.",
    inputSchema: objectSchema({
      taskId: { type: "string", minLength: 1 },
      cacheDir: pathProp("Optional Hig cache directory.")
    }, ["taskId"])
  },
  {
    name: "hig_repo_init",
    description: "Initialize independent immutable HIG repository history.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      excludes: { type: "array", items: { type: "string" }, description: "Additional excluded path components." }
    })
  },
  {
    name: "hig_repo_snapshot",
    description: "Create an atomic repository snapshot with byte and semantic indexes.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      message: { type: "string", description: "Snapshot message." },
      author: { type: "string", description: "Optional author identity." }
    })
  },
  {
    name: "hig_repo_refs",
    description: "List HEAD, branches, and tags with full commit IDs and active state.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root.")
    })
  },
  {
    name: "hig_repo_migrate",
    description: "Upgrade a legacy direct-HEAD repository to HEAD plus refs/heads/main without rewriting objects.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root.")
    })
  },
  {
    name: "hig_repo_watch_start",
    description: "Start IDE-managed automatic repository snapshots for an allowed workspace.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      debounceMs: { type: "integer", minimum: 1, description: "Quiet period before snapshot. Defaults to 750 ms." },
      message: { type: "string", minLength: 1, description: "Automatic snapshot message." },
      author: { type: "string", minLength: 1, description: "Optional author identity." }
    })
  },
  {
    name: "hig_repo_watch_status",
    description: "Return lifecycle state and the latest automatic snapshot for an IDE-managed repository watcher.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root.")
    })
  },
  {
    name: "hig_repo_watch_stop",
    description: "Stop the IDE-managed repository watcher for a workspace. Repeated stops are safe.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root.")
    })
  },
  {
    name: "hig_repo_branch_list",
    description: "List repository branches.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root.")
    })
  },
  {
    name: "hig_repo_branch_create",
    description: "Create a branch at a revision. Defaults to the current HEAD.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      name: { type: "string", minLength: 1, description: "Branch name." },
      from: { type: "string", description: "Optional source revision." }
    }, ["name"])
  },
  {
    name: "hig_repo_branch_switch",
    description: "Switch the active repository branch.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      name: { type: "string", minLength: 1, description: "Branch name." }
    }, ["name"])
  },
  {
    name: "hig_repo_branch_delete",
    description: "Delete an inactive repository branch.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      name: { type: "string", minLength: 1, description: "Branch name." }
    }, ["name"])
  },
  {
    name: "hig_repo_tag_list",
    description: "List immutable repository tags.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root.")
    })
  },
  {
    name: "hig_repo_tag_create",
    description: "Create an immutable tag at a revision. Defaults to the current HEAD.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      name: { type: "string", minLength: 1, description: "Tag name." },
      from: { type: "string", description: "Optional source revision." }
    }, ["name"])
  },
  {
    name: "hig_repo_tag_delete",
    description: "Delete an immutable repository tag.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      name: { type: "string", minLength: 1, description: "Tag name." }
    }, ["name"])
  },
  {
    name: "hig_repo_log",
    description: "List repository commits as JSON.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      limit: { type: "integer", minimum: 1, description: "Maximum commits. Defaults to 20." }
    })
  },
  {
    name: "hig_repo_diff",
    description: "Return file and exact byte-range changes between revisions.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      from: { type: "string", description: "Source revision. Defaults to target parent." },
      to: { type: "string", description: "Target revision. Defaults to HEAD." }
    })
  },
  {
    name: "hig_repo_path_history",
    description: "Query committed rename-aware history for one path without scanning all commits.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      path: pathProp("Repository-relative path."),
      limit: { type: "integer", minimum: 1, description: "Maximum entries. Defaults to 50." }
    }, ["path"])
  },
  {
    name: "hig_repo_restore",
    description: "Restore a complete revision or selected path into a staged output directory.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      revision: { type: "string", description: "Revision. Defaults to HEAD." },
      outputDir: pathProp("Destination directory."),
      path: pathProp("Optional repository-relative path."),
      overwrite: { type: "boolean", description: "Replace an existing destination." }
    }, ["outputDir"])
  },
  {
    name: "hig_repo_restore_range",
    description: "Restore an exact byte range from a file at a revision.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      revision: { type: "string", description: "Revision. Defaults to HEAD." },
      path: pathProp("Repository-relative file path."),
      start: { type: "integer", minimum: 0, description: "Start byte offset." },
      len: { type: "integer", minimum: 0, description: "Optional byte length; defaults to EOF." },
      output: pathProp("Destination file."),
      overwrite: { type: "boolean", description: "Replace an existing destination." }
    }, ["path", "start", "output"])
  },
  {
    name: "hig_repo_storage_tree",
    description: "Inspect per-path chunk reuse and compressed object storage for a revision.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      revision: { type: "string", description: "Revision. Defaults to HEAD." }
    })
  },
  {
    name: "hig_repo_symbols",
    description: "List parser-derived functions, methods, classes, and Rust type symbols.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      revision: { type: "string", description: "Revision. Defaults to HEAD." },
      path: pathProp("Optional repository-relative source path.")
    })
  },
  {
    name: "hig_repo_symbol_history",
    description: "Query rename-aware function or symbol history from the committed semantic index.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      symbol: { type: "string", minLength: 1, description: "Symbol ID, unique prefix, qualified name, or unique short name." },
      limit: { type: "integer", minimum: 1, description: "Maximum entries. Defaults to 50." }
    }, ["symbol"])
  },
  {
    name: "hig_repo_restore_symbol",
    description: "Restore a function, method, class, or Rust type as exact historical bytes.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      revision: { type: "string", description: "Revision. Defaults to HEAD." },
      symbol: { type: "string", minLength: 1, description: "Symbol query." },
      output: pathProp("Destination file."),
      overwrite: { type: "boolean", description: "Replace an existing destination." }
    }, ["symbol", "output"])
  },
  {
    name: "hig_repo_verify",
    description: "Verify every repository object reachable from all refs.",
    inputSchema: objectSchema({ dir: pathProp("Repository root. Defaults to workspace root.") })
  },
  {
    name: "hig_repo_gc",
    description: "Preview or apply deletion of unreachable repository objects. Defaults to preview.",
    inputSchema: objectSchema({
      dir: pathProp("Repository root. Defaults to workspace root."),
      apply: { type: "boolean", description: "Delete unreachable objects when true." }
    })
  },
  {
    name: "hig_bench",
    description: "Run Hig benchmark. Use carefully; can be long-running.",
    inputSchema: objectSchema({
      inputDir: pathProp("Directory to benchmark."),
      output: pathProp("Output archive path."),
      password: { type: "string", description: "Benchmark password when encryption is password." },
      encryption: { type: "string", enum: ["password", "none"] },
      cacheDir: pathProp("Optional cache directory."),
      benchDir: pathProp("Benchmark temporary directory."),
      compare: { type: "boolean" },
      benchSuite: {
        type: "string",
        enum: ["source", "lobehub", "lobehub-watch", "small500", "textmix", "repeat4m", "random8m", "binarymix", "all"]
      },
      useSession: { type: "boolean" },
      daemon: { type: "string", enum: ["auto", "off", "required"] },
      speed: { type: "string", enum: ["balanced", "fastest"] },
      kdfProfile: { type: "string", enum: ["secure", "interactive", "fast-bench"] }
    }, ["inputDir"])
  }
];

if (process.argv.includes("--smoke")) {
  const result = await runHig(["--version"]);
  process.stdout.write(`${result.stdout.trim()}\n`);
  process.exit(result.code === 0 ? 0 : 1);
}

let nextId = 1;
let inputBuffer = Buffer.alloc(0);
process.stdin.on("data", (chunk) => {
  inputBuffer = Buffer.concat([inputBuffer, chunk]);
  for (;;) {
    const message = readMessage();
    if (!message) break;
    void handleMessage(message).catch((error) => {
      if (message.id !== undefined) sendError(message.id, -32603, String(error?.message || error));
    });
  }
});

process.stdin.resume();

function computeAllowedRoots() {
  const raw = process.env.HIG_MCP_ALLOWED_ROOTS;
  if (raw && raw.trim()) {
    return raw.split(path.delimiter).flatMap((part) => part.split(",")).filter(Boolean).map((root) => path.resolve(root));
  }
  const roots = new Set([process.cwd()]);
  if (process.env.PWD) roots.add(process.env.PWD);
  return [...roots].map((root) => path.resolve(root));
}

function objectSchema(properties, required = []) {
  return {
    type: "object",
    additionalProperties: false,
    properties,
    required
  };
}

function pathProp(description) {
  return { type: "string", description };
}

function readMessage() {
  const headerEnd = inputBuffer.indexOf("\r\n\r\n");
  if (headerEnd >= 0) {
    const header = inputBuffer.subarray(0, headerEnd).toString("utf8");
    const match = /^Content-Length:\s*(\d+)/im.exec(header);
    if (!match) throw new Error("Invalid MCP frame: missing Content-Length");
    const length = Number(match[1]);
    const bodyStart = headerEnd + 4;
    if (inputBuffer.length < bodyStart + length) return null;
    const body = inputBuffer.subarray(bodyStart, bodyStart + length).toString("utf8");
    inputBuffer = inputBuffer.subarray(bodyStart + length);
    return JSON.parse(body);
  }

  const newline = inputBuffer.indexOf("\n");
  if (newline < 0) return null;
  const line = inputBuffer.subarray(0, newline).toString("utf8").trim();
  inputBuffer = inputBuffer.subarray(newline + 1);
  if (!line) return null;
  return JSON.parse(line);
}

async function handleMessage(message) {
  if (!message || message.jsonrpc !== "2.0") return;
  if (message.method?.startsWith("notifications/")) return;
  if (message.id === undefined) return;

  try {
    switch (message.method) {
      case "initialize":
        sendResult(message.id, {
          protocolVersion: PROTOCOL_VERSION,
          capabilities: { tools: {} },
          serverInfo: { name: "hig-mcp-server", version: VERSION }
        });
        break;
      case "tools/list":
        sendResult(message.id, { tools });
        break;
      case "tools/call": {
        const { name, arguments: args = {} } = message.params || {};
        const result = await callTool(name, args || {});
        sendResult(message.id, {
          content: [{ type: "text", text: stringifyToolResult(result) }],
          isError: result.code !== 0
        });
        break;
      }
      case "ping":
        sendResult(message.id, {});
        break;
      default:
        sendError(message.id, -32601, `Unsupported method: ${message.method}`);
    }
  } catch (error) {
    sendResult(message.id, {
      content: [{ type: "text", text: JSON.stringify({ error: String(error?.message || error) }, null, 2) }],
      isError: true
    });
  }
}

async function callTool(name, args) {
  switch (name) {
    case "hig_version":
      return runHig(["--version"]);
    case "hig_help":
      return runHig([...(args.command || []).map(String), "--help"]);
    case "hig_init_project":
      return runHig(["init", resolveInputPath(args.dir || "."), ...optionPath("--cache-dir", args.cacheDir), ...repeatOption("--exclude", args.excludes)]);
    case "hig_project_status":
      return runHig(["project", "status", resolveInputPath(args.dir || "."), "--json"], { parseJson: true });
    case "hig_project_rebuild":
      return runHig(["project", "rebuild", resolveInputPath(args.dir || "."), ...(args.wait ? ["--wait"] : [])]);
    case "hig_project_policy_show":
      return runHig(["project", "policy", "show", resolveInputPath(args.dir || "."), "--json"], { parseJson: true });
    case "hig_project_policy_set":
      return runHig([
        "project", "policy", "set", resolveInputPath(args.dir || "."),
        ...optionValue("--enabled", args.enabled),
        ...optionValue("--quiescence-ms", args.quiescenceMs),
        ...optionValue("--periodic-interval-ms", args.periodicIntervalMs),
        ...optionValue("--max-pending-events", args.maxPendingEvents),
        ...optionValue("--max-pending-files", args.maxPendingFiles),
        ...optionValue("--resource-enabled", args.resourceEnabled),
        ...optionValue("--min-available-memory-bytes", args.minAvailableMemoryBytes),
        ...optionValue("--resume-available-memory-bytes", args.resumeAvailableMemoryBytes),
        ...optionValue("--resource-poll-interval-ms", args.resourcePollIntervalMs),
        "--json"
      ], { parseJson: true });
    case "hig_daemon_status":
      return runHig(["daemon", "status", ...optionPath("--cache-dir", args.cacheDir)]);
    case "hig_daemon_start":
      return runHig(["daemon", "start", ...optionPath("--cache-dir", args.cacheDir), ...optionValue("--ttl-secs", args.ttlSecs)]);
    case "hig_daemon_stop":
      return runHig(["daemon", "stop", ...optionPath("--cache-dir", args.cacheDir)]);
    case "hig_session_status":
      return runHig(["session", "status", ...optionPath("--cache-dir", args.cacheDir)]);
    case "hig_session_unlock":
      return runHig(["session", "unlock", "--password", stringArg(args.password, "password"), ...optionPath("--cache-dir", args.cacheDir), ...optionValue("--ttl-secs", args.ttlSecs), ...optionValue("--kdf-profile", args.kdfProfile)]);
    case "hig_session_clear":
      return runHig(["session", "clear", ...optionPath("--cache-dir", args.cacheDir)]);
    case "hig_pack":
      return runHig(buildPackArgs(args), { parseJson: true });
    case "hig_unpack":
      return runHig(["unpack", resolveInputPath(args.archiveFile), "--output-dir", resolveOutputPath(args.outputDir), ...optionValue("--password", args.password), ...(args.overwrite ? ["--overwrite"] : [])]);
    case "hig_inspect":
      return runHig(["inspect", resolveInputPath(args.archiveFile), ...optionValue("--password", args.password), ...(args.json === false ? [] : ["--json"])], { parseJson: args.json !== false });
    case "hig_migrate":
      return runHig([
        "migrate",
        resolveInputPath(args.source),
        "--output",
        resolveOutputPath(args.output),
        ...optionValue("--password", args.password),
        ...optionValue("--target-password", args.targetPassword),
        ...optionValue("--encryption", args.encryption),
        ...(args.overwrite ? ["--overwrite"] : []),
        "--json"
      ], { parseJson: true });
    case "hig_cache_status":
      return runHig(["cache", "status", ...optionPath("--cache-dir", args.cacheDir)]);
    case "hig_cache_gc":
      return runHig(["cache", "gc", ...optionPath("--cache-dir", args.cacheDir), ...(args.dryRun === false ? [] : ["--dry-run"])]);
    case "hig_cache_compact":
      return runHig(["cache", "compact", ...optionPath("--cache-dir", args.cacheDir), ...(args.dryRun === false ? [] : ["--dry-run"])]);
    case "hig_task_list":
      return runHig(["task", "list", ...optionPath("--cache-dir", args.cacheDir), ...(args.includeCompleted ? ["--include-completed"] : [])]);
    case "hig_task_status":
      return runHig(["task", "status", stringArg(args.taskId, "taskId"), ...optionPath("--cache-dir", args.cacheDir)]);
    case "hig_task_cancel":
      return runHig(["task", "cancel", stringArg(args.taskId, "taskId"), ...optionPath("--cache-dir", args.cacheDir)]);
    case "hig_task_result":
      return runHig(["task", "result", stringArg(args.taskId, "taskId"), ...optionPath("--cache-dir", args.cacheDir)]);
    case "hig_repo_init":
      return runHig(["repo", "init", resolveInputPath(args.dir || "."), ...repeatOption("--exclude", args.excludes), "--json"], { parseJson: true });
    case "hig_repo_snapshot":
      return runHig(["repo", "snapshot", resolveInputPath(args.dir || "."), ...optionValue("--message", args.message), ...optionValue("--author", args.author), "--json"], { parseJson: true });
    case "hig_repo_refs":
      return runHig(["repo", "refs", resolveInputPath(args.dir || "."), "--json"], { parseJson: true });
    case "hig_repo_migrate":
      return runHig(["repo", "migrate", resolveInputPath(args.dir || "."), "--json"], { parseJson: true });
    case "hig_repo_watch_start":
      return startRepositoryWatcher(args);
    case "hig_repo_watch_status":
      return repositoryWatcherStatus(resolveInputPath(args.dir || "."));
    case "hig_repo_watch_stop":
      return stopRepositoryWatcher(resolveInputPath(args.dir || "."));
    case "hig_repo_branch_list":
      return runHig(["repo", "branch", "list", resolveInputPath(args.dir || "."), "--json"], { parseJson: true });
    case "hig_repo_branch_create":
      return runHig(["repo", "branch", "create", stringArg(args.name, "name"), resolveInputPath(args.dir || "."), ...optionValue("--from", args.from), "--json"], { parseJson: true });
    case "hig_repo_branch_switch":
      return runHig(["repo", "branch", "switch", stringArg(args.name, "name"), resolveInputPath(args.dir || "."), "--json"], { parseJson: true });
    case "hig_repo_branch_delete":
      return runHig(["repo", "branch", "delete", stringArg(args.name, "name"), resolveInputPath(args.dir || "."), "--json"], { parseJson: true });
    case "hig_repo_tag_list":
      return runHig(["repo", "tag", "list", resolveInputPath(args.dir || "."), "--json"], { parseJson: true });
    case "hig_repo_tag_create":
      return runHig(["repo", "tag", "create", stringArg(args.name, "name"), resolveInputPath(args.dir || "."), ...optionValue("--from", args.from), "--json"], { parseJson: true });
    case "hig_repo_tag_delete":
      return runHig(["repo", "tag", "delete", stringArg(args.name, "name"), resolveInputPath(args.dir || "."), "--json"], { parseJson: true });
    case "hig_repo_log":
      return runHig(["repo", "log", resolveInputPath(args.dir || "."), ...optionValue("--limit", args.limit), "--json"], { parseJson: true });
    case "hig_repo_diff":
      return runHig(["repo", "diff", resolveInputPath(args.dir || "."), ...optionValue("--from", args.from), ...optionValue("--to", args.to), "--json"], { parseJson: true });
    case "hig_repo_path_history":
      return runHig(["repo", "history", resolveInputPath(args.dir || "."), "--path", stringArg(args.path, "path"), ...optionValue("--limit", args.limit), "--json"], { parseJson: true });
    case "hig_repo_restore":
      return runHig(["repo", "restore", resolveInputPath(args.dir || "."), ...optionValue("--revision", args.revision), "--output-dir", resolveOutputPath(args.outputDir), ...optionValue("--path", args.path), ...(args.overwrite ? ["--overwrite"] : []), "--json"], { parseJson: true });
    case "hig_repo_restore_range":
      return runHig(["repo", "restore-range", resolveInputPath(args.dir || "."), ...optionValue("--revision", args.revision), "--path", stringArg(args.path, "path"), "--start", integerArg(args.start, "start", 0), ...optionValue("--len", args.len), "--output", resolveOutputPath(args.output), ...(args.overwrite ? ["--overwrite"] : []), "--json"], { parseJson: true });
    case "hig_repo_storage_tree":
      return runHig(["repo", "storage-tree", resolveInputPath(args.dir || "."), ...optionValue("--revision", args.revision), "--json"], { parseJson: true });
    case "hig_repo_symbols":
      return runHig(["repo", "symbols", resolveInputPath(args.dir || "."), ...optionValue("--revision", args.revision), ...optionValue("--path", args.path), "--json"], { parseJson: true });
    case "hig_repo_symbol_history":
      return runHig(["repo", "symbol-history", resolveInputPath(args.dir || "."), "--symbol", stringArg(args.symbol, "symbol"), ...optionValue("--limit", args.limit), "--json"], { parseJson: true });
    case "hig_repo_restore_symbol":
      return runHig(["repo", "restore-symbol", resolveInputPath(args.dir || "."), ...optionValue("--revision", args.revision), "--symbol", stringArg(args.symbol, "symbol"), "--output", resolveOutputPath(args.output), ...(args.overwrite ? ["--overwrite"] : []), "--json"], { parseJson: true });
    case "hig_repo_verify":
      return runHig(["repo", "verify", resolveInputPath(args.dir || "."), "--json"], { parseJson: true });
    case "hig_repo_gc":
      return runHig(["repo", "gc", resolveInputPath(args.dir || "."), ...(args.apply ? ["--apply"] : []), "--json"], { parseJson: true });
    case "hig_bench":
      return runHig(buildBenchArgs(args), { parseJson: true, timeoutMs: Number(process.env.HIG_MCP_BENCH_TIMEOUT_MS || 60 * 60 * 1000) });
    default:
      throw new Error(`Unknown tool: ${name}`);
  }
}

function buildPackArgs(args) {
  return [
    "pack",
    resolveInputPath(args.inputDir),
    "--output",
    resolveOutputPath(args.output),
    "--json",
    ...optionValue("--password", args.password),
    ...optionValue("--encryption", args.encryption),
    ...optionPath("--cache-dir", args.cacheDir),
    ...optionValue("--threads", args.threads),
    ...optionValue("--level", args.level),
    ...(args.noCache ? ["--no-cache"] : []),
    ...optionValue("--format", args.format),
    ...optionValue("--manifest-format", args.manifestFormat),
    ...(args.noBatch ? ["--no-batch"] : []),
    ...(args.noChunk ? ["--no-chunk"] : []),
    ...optionValue("--speed", args.speed),
    ...optionValue("--kdf-profile", args.kdfProfile),
    ...(args.trustMetadata ? ["--trust-metadata"] : []),
    ...(args.useSession ? ["--use-session"] : []),
    ...optionValue("--daemon", args.daemon),
    ...optionValue("--project", args.project),
    ...optionValue("--solid", args.solid)
  ];
}

function buildBenchArgs(args) {
  return [
    "bench",
    resolveInputPath(args.inputDir),
    "--json",
    ...optionPath("-o", args.output),
    ...optionValue("--password", args.password),
    ...optionValue("--encryption", args.encryption),
    ...optionPath("--cache-dir", args.cacheDir),
    ...optionPath("--bench-dir", args.benchDir),
    ...(args.compare ? ["--compare"] : []),
    ...optionValue("--bench-suite", args.benchSuite),
    ...(args.useSession ? ["--use-session"] : []),
    ...optionValue("--daemon", args.daemon),
    ...optionValue("--speed", args.speed),
    ...optionValue("--kdf-profile", args.kdfProfile)
  ];
}

function optionValue(flag, value) {
  if (value === undefined || value === null || value === "") return [];
  return [flag, String(value)];
}

function optionPath(flag, value) {
  if (value === undefined || value === null || value === "") return [];
  return [flag, resolveOutputPath(value)];
}

function repeatOption(flag, values) {
  if (!Array.isArray(values)) return [];
  return values.flatMap((value) => [flag, String(value)]);
}

function stringArg(value, name) {
  if (typeof value !== "string" || value.length === 0) throw new Error(`${name} is required`);
  return value;
}

function integerArg(value, name, minimum) {
  if (!Number.isInteger(value) || value < minimum) throw new Error(`${name} must be an integer >= ${minimum}`);
  return String(value);
}

function resolveInputPath(value) {
  return resolveCheckedPath(value, false);
}

function resolveOutputPath(value) {
  return resolveCheckedPath(value, true);
}

function resolveCheckedPath(value, output) {
  if (typeof value !== "string" || value.length === 0) throw new Error("path value is required");
  const absolute = path.resolve(value);
  if (!allowAnyPath && !allowedRoots.some((root) => isInside(absolute, root))) {
    throw new Error(`Path is outside allowed roots: ${absolute}. Set HIG_MCP_ALLOWED_ROOTS or HIG_MCP_ALLOW_ANY_PATH=1.`);
  }
  if (!output && !fs.existsSync(absolute)) {
    throw new Error(`Input path does not exist: ${absolute}`);
  }
  return absolute;
}

function isInside(target, root) {
  const relative = path.relative(root, target);
  return relative === "" || (!!relative && !relative.startsWith("..") && !path.isAbsolute(relative));
}

async function runHig(args, options = {}) {
  const higBin = process.env.HIG_BIN || (fs.existsSync(bundledHig) ? bundledHig : "hig");
  const timeoutMs = options.timeoutMs || DEFAULT_TIMEOUT_MS;
  return new Promise((resolve) => {
    const child = spawn(higBin, args, {
      cwd: process.env.HIG_MCP_WORKDIR || process.cwd(),
      env: { ...process.env, NO_COLOR: "1" },
      stdio: ["ignore", "pipe", "pipe"]
    });
    let stdout = Buffer.alloc(0);
    let stderr = Buffer.alloc(0);
    const timer = setTimeout(() => {
      child.kill("SIGTERM");
      setTimeout(() => child.kill("SIGKILL"), 2000).unref();
    }, timeoutMs);
    child.stdout.on("data", (chunk) => {
      stdout = appendBounded(stdout, chunk);
    });
    child.stderr.on("data", (chunk) => {
      stderr = appendBounded(stderr, chunk);
    });
    child.on("error", (error) => {
      clearTimeout(timer);
      resolve({ code: 127, stdout: "", stderr: String(error.message), data: null });
    });
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      const out = stdout.toString("utf8");
      const err = stderr.toString("utf8");
      let data = null;
      if (options.parseJson && out.trim()) {
        try {
          data = JSON.parse(out);
        } catch {
          data = null;
        }
      }
      resolve({ code: code ?? 1, signal, stdout: out, stderr: err, data });
    });
  });
}

async function startRepositoryWatcher(args) {
  const root = resolveInputPath(args.dir || ".");
  const existing = repositoryWatchers.get(root);
  if (existing?.active) {
    return watcherResult(existing, { reused: true });
  }

  const preflight = await runHig(["repo", "refs", root, "--json"], { parseJson: true });
  if (preflight.code !== 0) return preflight;

  const debounceMs = args.debounceMs ?? 750;
  if (!Number.isInteger(debounceMs) || debounceMs < 1) {
    throw new Error("debounceMs must be an integer >= 1");
  }
  const message = args.message || "IDE automatic snapshot";
  const author = args.author || null;
  const higBin = process.env.HIG_BIN || (fs.existsSync(bundledHig) ? bundledHig : "hig");
  const command = [
    "repo", "watch", root,
    "--debounce-ms", String(debounceMs),
    "--message", message,
    ...(author ? ["--author", author] : []),
    "--catch-up", "true",
    "--lifecycle-stdin",
    "--json"
  ];
  const child = spawn(higBin, command, {
    cwd: process.env.HIG_MCP_WORKDIR || process.cwd(),
    env: { ...process.env, NO_COLOR: "1" },
    stdio: ["pipe", "pipe", "pipe"]
  });
  const watcher = {
    root,
    child,
    active: true,
    stopping: false,
    startedAt: new Date().toISOString(),
    debounceMs,
    message,
    author,
    snapshots: 0,
    lastSnapshot: null,
    stdout: Buffer.alloc(0),
    stdoutLine: "",
    stderr: Buffer.alloc(0),
    exitCode: null,
    signal: null
  };
  repositoryWatchers.set(root, watcher);
  child.stdout.on("data", (chunk) => consumeWatcherOutput(watcher, chunk));
  child.stderr.on("data", (chunk) => {
    watcher.stderr = appendBounded(watcher.stderr, chunk);
  });
  child.on("error", (error) => {
    watcher.active = false;
    watcher.stderr = appendBounded(watcher.stderr, Buffer.from(String(error.message)));
  });
  child.on("close", (code, signal) => {
    watcher.active = false;
    watcher.exitCode = code;
    watcher.signal = signal;
  });

  await new Promise((resolve) => setTimeout(resolve, 100));
  if (!watcher.active) return watcherResult(watcher, { code: watcher.exitCode ?? 1 });
  return watcherResult(watcher);
}

function consumeWatcherOutput(watcher, chunk) {
  watcher.stdout = appendBounded(watcher.stdout, chunk);
  watcher.stdoutLine += Buffer.from(chunk).toString("utf8");
  const lines = watcher.stdoutLine.split(/\r?\n/);
  watcher.stdoutLine = lines.pop() || "";
  for (const line of lines) {
    if (!line.trim()) continue;
    try {
      watcher.lastSnapshot = JSON.parse(line);
      watcher.snapshots += 1;
    } catch {
      watcher.stderr = appendBounded(
        watcher.stderr,
        Buffer.from(`invalid watcher JSON output: ${line}\n`)
      );
    }
  }
}

function repositoryWatcherStatus(root) {
  const watcher = repositoryWatchers.get(root);
  if (!watcher) {
    return { code: 0, stdout: "", stderr: "", data: { root, active: false, managed: false } };
  }
  return watcherResult(watcher);
}

async function stopRepositoryWatcher(root) {
  const watcher = repositoryWatchers.get(root);
  if (!watcher) {
    return { code: 0, stdout: "", stderr: "", data: { root, active: false, managed: false } };
  }
  await terminateRepositoryWatcher(watcher);
  const result = watcherResult(watcher);
  repositoryWatchers.delete(root);
  return result;
}

function watcherResult(watcher, overrides = {}) {
  return {
    code: overrides.code ?? 0,
    signal: watcher.signal,
    stdout: watcher.stdout.toString("utf8"),
    stderr: watcher.stderr.toString("utf8"),
    data: {
      root: watcher.root,
      managed: true,
      active: watcher.active,
      reused: overrides.reused || false,
      started_at: watcher.startedAt,
      debounce_ms: watcher.debounceMs,
      message: watcher.message,
      author: watcher.author,
      snapshots: watcher.snapshots,
      last_snapshot: watcher.lastSnapshot,
      exit_code: watcher.exitCode,
      signal: watcher.signal
    }
  };
}

async function terminateRepositoryWatcher(watcher) {
  if (!watcher.active || watcher.stopping) return;
  watcher.stopping = true;
  const closed = new Promise((resolve) => watcher.child.once("close", resolve));
  watcher.child.kill("SIGTERM");
  const graceful = await Promise.race([
    closed.then(() => true),
    new Promise((resolve) => setTimeout(() => resolve(false), 2000))
  ]);
  if (!graceful && watcher.active) {
    watcher.child.kill("SIGKILL");
    await Promise.race([closed, new Promise((resolve) => setTimeout(resolve, 2000))]);
  }
  watcher.active = false;
  watcher.stopping = false;
}

async function shutdown(exitCode) {
  if (shuttingDown) return;
  shuttingDown = true;
  await Promise.all([...repositoryWatchers.values()].map(terminateRepositoryWatcher));
  process.exit(exitCode);
}

function appendBounded(current, chunk) {
  const next = Buffer.concat([current, Buffer.from(chunk)]);
  if (next.length <= MAX_OUTPUT_BYTES) return next;
  return next.subarray(next.length - MAX_OUTPUT_BYTES);
}

function stringifyToolResult(result) {
  const payload = {
    ok: result.code === 0,
    code: result.code,
    signal: result.signal || null,
    data: result.data,
    stdout: result.stdout.trim(),
    stderr: result.stderr.trim()
  };
  return JSON.stringify(payload, null, 2);
}

function sendResult(id, result) {
  send({ jsonrpc: "2.0", id, result });
}

function sendError(id, code, message) {
  send({ jsonrpc: "2.0", id, error: { code, message } });
}

function send(message) {
  const body = Buffer.from(JSON.stringify(message), "utf8");
  process.stdout.write(`Content-Length: ${body.length}\r\n\r\n`);
  process.stdout.write(body);
}

process.on("uncaughtException", (error) => {
  const id = nextId++;
  sendError(id, -32603, String(error?.message || error));
});

process.stdin.on("end", () => void shutdown(0));
process.on("SIGTERM", () => void shutdown(0));
process.on("SIGINT", () => void shutdown(0));
