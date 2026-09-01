#!/usr/bin/env node
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const hig = process.env.HIG_BIN || path.resolve("target", "release", process.platform === "win32" ? "hig.exe" : "hig");
const work = fs.mkdtempSync(path.join(os.tmpdir(), "hig-recovery-runbook-"));
const source = path.join(work, "source");
const expected = path.join(work, "expected");
const primary = path.join(work, "primary");
const survivor = path.join(work, "survivor");
const replacement = path.join(work, "replacement");
const restored = path.join(work, "restored");

function run(args) {
  return JSON.parse(execFileSync(hig, [...args, "--json"], {
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024
  }));
}

function treeDigest(root) {
  const hash = crypto.createHash("sha256");
  const visit = (relative) => {
    const current = path.join(root, relative);
    for (const entry of fs.readdirSync(current, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const child = path.join(relative, entry.name);
      hash.update(entry.isDirectory() ? `d:${child}\0` : `f:${child}\0`);
      if (entry.isDirectory()) visit(child);
      else hash.update(fs.readFileSync(path.join(root, child)));
    }
  };
  visit("");
  return hash.digest("hex");
}

try {
  fs.mkdirSync(path.join(source, "src"), { recursive: true });
  fs.writeFileSync(path.join(source, "README.md"), "Recovery Vault operator drill\n");
  fs.writeFileSync(path.join(source, "src", "critical.bin"), Buffer.from([0, 1, 2, 255, 10, 0, 42]));
  fs.cpSync(source, expected, { recursive: true });

  const initialized = run(["repo", "init", source]);
  run(["repo", "snapshot", source, "--message", "operator drill baseline"]);
  run(["recovery", "init", "--vault-root", primary, "--mirror", survivor]);
  const capture = run(["recovery", "capture", source, "--vault-root", primary]);
  assert.equal(capture.schema, 1);
  assert.equal(capture.recovery_point.durability, "protected");
  const repositoryId = Buffer.from(initialized.repository_id).toString("hex");
  const pointId = capture.recovery_point.recovery_point_id;

  fs.rmSync(source, { recursive: true, force: true });
  fs.rmSync(primary, { recursive: true, force: true });
  const survivorStatus = run(["recovery", "status", "--vault-root", survivor]);
  assert.equal(survivorStatus.recovery_points, 1);
  run(["recovery", "verify", repositoryId, pointId, "--vault-root", survivor]);

  const promotion = run(["recovery", "promote", "--vault-root", survivor, "--mirror", replacement]);
  assert.equal(promotion.schema, 1);
  assert.equal(promotion.durability, "protected");
  run(["recovery", "scrub", "--vault-root", survivor]);
  fs.rmSync(survivor, { recursive: true, force: true });

  run(["recovery", "verify", repositoryId, pointId, "--vault-root", replacement]);
  run(["recovery", "restore", repositoryId, pointId, "--vault-root", replacement, "--output-dir", restored]);
  assert.equal(treeDigest(restored), treeDigest(expected));
  const audit = run(["recovery", "audit", "--vault-root", replacement]);
  assert.equal(audit.incomplete_operation_ids.length, 0);
  console.log(`hig-recovery-runbook: PASS digest=${treeDigest(restored)}`);
} finally {
  fs.rmSync(work, { recursive: true, force: true });
}
