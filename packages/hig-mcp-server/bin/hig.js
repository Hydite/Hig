#!/usr/bin/env node
import { spawn } from "node:child_process";
import { resolveHigBinary } from "../lib/resolve-hig.js";

let executable;
try {
  executable = resolveHigBinary();
} catch (error) {
  process.stderr.write(`hig: ${error.message}\n`);
  process.exit(1);
}

const child = spawn(executable, process.argv.slice(2), {
  stdio: "inherit",
  env: process.env
});

for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(signal, () => {
    if (!child.killed) child.kill(signal);
  });
}

child.on("error", (error) => {
  process.stderr.write(`hig: unable to start ${executable}: ${error.message}\n`);
  process.exit(127);
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.stderr.write(`hig: native process terminated by ${signal}\n`);
    process.exit(1);
  }
  process.exit(code ?? 1);
});
