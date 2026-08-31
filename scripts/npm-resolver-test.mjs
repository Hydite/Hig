#!/usr/bin/env node
import assert from "node:assert/strict";
import { hasGlibcRuntime, platformPackageFor, resolveHigBinary } from "../packages/hig-mcp-server/lib/resolve-hig.js";

assert.equal(platformPackageFor("darwin", "arm64").name, "@zorker/hig-darwin-universal");
assert.equal(platformPackageFor("darwin", "x64").name, "@zorker/hig-darwin-universal");
assert.equal(platformPackageFor("linux", "x64").name, "@zorker/hig-linux-x64-gnu");
assert.equal(platformPackageFor("win32", "x64").name, "@zorker/hig-win32-x64-msvc");
assert.throws(() => platformPackageFor("linux", "arm64"), /Unsupported HIG npm platform/);
assert.throws(() => platformPackageFor("freebsd", "x64"), /Unsupported HIG npm platform/);

assert.equal(hasGlibcRuntime({ getReport: () => ({ header: { glibcVersionRuntime: "2.39" } }) }, "linux"), true);
assert.equal(hasGlibcRuntime({ getReport: () => ({ header: {} }) }, "linux"), false);
assert.equal(hasGlibcRuntime(null, "linux"), false);
assert.equal(hasGlibcRuntime(null, "darwin"), true);

const previous = process.env.HIG_BIN;
try {
  process.env.HIG_BIN = "/explicit/hig";
  assert.equal(resolveHigBinary(), "/explicit/hig");
} finally {
  if (previous === undefined) delete process.env.HIG_BIN;
  else process.env.HIG_BIN = previous;
}

process.stdout.write("hig-npm-resolver: PASS\n");
