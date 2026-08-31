import fs from "node:fs";
import path from "node:path";

export function npmInvocation(args) {
  if (process.platform !== "win32") return { command: "npm", args };

  const candidates = [
    process.env.npm_execpath,
    path.join(path.dirname(process.execPath), "node_modules", "npm", "bin", "npm-cli.js")
  ].filter(Boolean);
  const npmCli = candidates.find((candidate) => fs.existsSync(candidate));
  if (!npmCli) {
    throw new Error(`unable to locate npm CLI beside ${process.execPath}`);
  }
  return { command: process.execPath, args: [npmCli, ...args] };
}
