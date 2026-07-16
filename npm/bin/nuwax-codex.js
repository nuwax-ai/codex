#!/usr/bin/env node
// Launcher for the nuwax-codex npm package.
// Resolves the platform-specific optional dependency that ships the native
// `nuwax-codex` binary (and the bundled `bwrap` on Linux) and spawns it with
// the forwarded argv. Both the npm command and the native binary are named
// "nuwax-codex".

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { familySync } from "detect-libc";

// Map Node.js platform/arch to the platform-specific npm package name.
function getPlatformPackage() {
  const platform = process.platform;
  const arch = process.arch;

  const platformMap = {
    darwin: {
      arm64: "nuwax-codex-darwin-arm64",
      x64: "nuwax-codex-darwin-x64",
    },
    linux: {
      // arm64 Linux is not shipped (GitHub arm64 runners are too flaky to build
      // it reliably); only x64 Linux is supported for now.
      x64: familySync() === "musl"
        ? "nuwax-codex-linux-x64-musl"
        : "nuwax-codex-linux-x64",
    },
    win32: {
      arm64: "nuwax-codex-win32-arm64",
      x64: "nuwax-codex-win32-x64",
    },
  };

  const packages = platformMap[platform];
  if (!packages) {
    console.error(`Unsupported platform: ${platform}`);
    process.exit(1);
  }

  const packageName = packages[arch];
  if (!packageName) {
    console.error(`Unsupported architecture: ${arch} on ${platform}`);
    process.exit(1);
  }

  return packageName;
}

// Locate the native `nuwax-codex` binary inside the resolved platform package.
function getBinaryPath() {
  const packageName = getPlatformPackage();
  const binaryName =
    process.platform === "win32" ? "nuwax-codex.exe" : "nuwax-codex";

  try {
    const binaryPath = fileURLToPath(
      import.meta.resolve(`${packageName}/bin/${binaryName}`),
    );

    if (existsSync(binaryPath)) {
      return binaryPath;
    }
  } catch (e) {
    console.error(`Error resolving package: ${e}`);
    // Package not found — fall through to the helpful error below.
  }

  console.error(
    `Failed to locate ${packageName} binary. This usually means the optional dependency was not installed.`,
  );
  console.error(`Platform: ${process.platform}, Architecture: ${process.arch}`);
  process.exit(1);
}

// Execute the binary, forwarding stdin/stdout/stderr and the exit code.
function run() {
  const binaryPath = getBinaryPath();
  const result = spawnSync(binaryPath, process.argv.slice(2), {
    stdio: "inherit",
    windowsHide: true,
  });

  if (result.error) {
    console.error(`Failed to execute ${binaryPath}:`, result.error);
    process.exit(1);
  }

  // result.status is null when the child was killed by a signal; treat that as
  // a failure (exit 1) rather than success.
  process.exit(result.status ?? 1);
}

run();
