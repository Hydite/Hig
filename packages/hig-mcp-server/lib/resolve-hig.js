import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const defaultPackageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const PLATFORM_PACKAGES = new Map([
  ["darwin-arm64", { name: "@zorker/hig-darwin-universal", executable: "bin/hig" }],
  ["darwin-x64", { name: "@zorker/hig-darwin-universal", executable: "bin/hig" }],
  ["linux-x64", { name: "@zorker/hig-linux-x64-gnu", executable: "bin/hig", glibc: true }],
  ["win32-x64", { name: "@zorker/hig-win32-x64-msvc", executable: "bin/hig.exe" }]
]);

export function resolveHigBinary(options = {}) {
  if (process.env.HIG_BIN) return process.env.HIG_BIN;

  const packageRoot = options.packageRoot || defaultPackageRoot;
  const bundled = path.join(packageRoot, "bin", process.platform === "win32" ? "hig.exe" : "hig");
  if (isFile(bundled)) return bundled;

  const platformPackage = platformPackageFor(process.platform, process.arch);
  if (platformPackage.glibc && !hasGlibcRuntime(process.report, process.platform)) {
    throw new Error("HIG Linux packages require glibc; musl and other libc runtimes are not supported by v1.10.0.");
  }

  try {
    const executable = require.resolve(`${platformPackage.name}/${platformPackage.executable}`);
    if (!isFile(executable)) throw new Error("resolved path is not a file");
    return executable;
  } catch (error) {
    throw new Error(
      `Missing native HIG package ${platformPackage.name}@1.10.0 for ${process.platform}/${process.arch}. `
      + `Reinstall @zorker/hig with optional dependencies enabled or set HIG_BIN explicitly. Cause: ${error.message}`
    );
  }
}

export function platformPackageFor(platform, arch) {
  const key = `${platform}-${arch}`;
  const selected = PLATFORM_PACKAGES.get(key);
  if (selected) return selected;
  throw new Error(`Unsupported HIG npm platform: ${platform}/${arch}`);
}

export function hasGlibcRuntime(report = process.report, platform = process.platform) {
  if (platform !== "linux") return true;
  const header = report?.getReport?.().header;
  return typeof header?.glibcVersionRuntime === "string" && header.glibcVersionRuntime.length > 0;
}

function isFile(file) {
  try {
    return fs.statSync(file).isFile();
  } catch {
    return false;
  }
}
