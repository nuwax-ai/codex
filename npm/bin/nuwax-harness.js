#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { ensureBinary } from "./runtime.js";

try {
  const binaryPath = await ensureBinary();
  const result = spawnSync(binaryPath, process.argv.slice(2), {
    stdio: "inherit",
    windowsHide: true,
    // Internal install detection and updates continue targeting nuwax-codex.
    env: { ...process.env, CODEX_INSTALL_SOURCE: "npm_nuwax" },
  });
  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
} catch (error) {
  console.error(`nuwax-harness: ${error.message}`);
  process.exitCode = 1;
}
