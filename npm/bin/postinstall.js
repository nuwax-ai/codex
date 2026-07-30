#!/usr/bin/env node
// Postinstall script: pre-downloads the native `nuwax-codex` binary from
// Alibaba Cloud OSS so the first CLI invocation is instant.

import { createWriteStream, existsSync, mkdirSync, chmodSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, dirname } from "node:path";
import { createGunzip } from "node:zlib";
import { pipeline } from "node:stream/promises";
import { Transform } from "node:stream";
import { createReadStream } from "node:fs";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const VERSION = require("../package.json").version;
const OSS_CDN_BASE = "https://nuwa-packages.oss-rg-china-mainland.aliyuncs.com/nuwax-codex";

// ---------------------------------------------------------------------------
// Platform helpers
// ---------------------------------------------------------------------------

function getTargetTriple() {
  const p = process.platform;
  const a = process.arch;
  if (p === "darwin") {
    return a === "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin";
  }
  if (p === "linux") {
    if (a !== "x64") throw new Error(`Unsupported Linux arch: ${a}`);
    const { familySync } = require("detect-libc");
    return familySync() === "musl"
      ? "x86_64-unknown-linux-musl"
      : "x86_64-unknown-linux-gnu";
  }
  if (p === "win32") {
    return a === "arm64" ? "aarch64-pc-windows-msvc" : "x86_64-pc-windows-msvc";
  }
  throw new Error(`Unsupported platform: ${p}`);
}

function getArchiveExt() {
  return process.platform === "win32" ? "zip" : "tar.gz";
}

function getBinaryName() {
  return process.platform === "win32" ? "nuwax-codex.exe" : "nuwax-codex";
}

function cacheDir() {
  return join(homedir(), ".nuwax-codex-cache", VERSION);
}

function cachedBinaryPath() {
  return join(cacheDir(), getBinaryName());
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

async function downloadBinary(url, outPath) {
  mkdirSync(dirname(outPath), { recursive: true });
  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok) {
    throw new Error(
      `Failed to download binary: HTTP ${res.status} ${res.statusText}\nURL: ${url}`,
    );
  }
  const total = parseInt(res.headers.get("content-length") || "0", 10);
  let downloaded = 0;
  const reader = res.body.getReader();
  const ws = createWriteStream(outPath);
  const logInterval = setInterval(() => {
    if (total > 0) {
      process.stderr.write(
        `\r  Downloading nuwax-codex ${VERSION} … ${((downloaded / total) * 100).toFixed(0)}%`,
      );
    }
  }, 500);
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      ws.write(value);
      downloaded += value.length;
    }
  } finally {
    clearInterval(logInterval);
    ws.end();
  }
  process.stderr.write("\n");
  if (process.platform !== "win32") {
    chmodSync(outPath, 0o755);
  }
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

async function extractTarGz(archivePath, destDir) {
  const gunzip = createGunzip();
  const rs = createReadStream(archivePath);
  let buffer = Buffer.alloc(0);

  await pipeline(
    rs,
    gunzip,
    new Transform({
      transform(chunk, _enc, cb) {
        buffer = Buffer.concat([buffer, chunk]);
        while (buffer.length >= 512) {
          const name = buffer.toString("utf8", 0, 100).replace(/\0.*$/, "");
          const sizeStr = buffer.toString("utf8", 124, 136).replace(/\0.*$/, "");
          const size = parseInt(sizeStr, 8);
          if (isNaN(size) || size < 0) break;
          const totalSize = Math.ceil((512 + size) / 512) * 512;
          if (buffer.length < totalSize) break;
          if (name && !name.endsWith("/") && size > 0) {
            const fileData = buffer.subarray(512, 512 + size);
            const outPath = join(destDir, name);
            mkdirSync(dirname(outPath), { recursive: true });
            writeFileSync(outPath, fileData);
            if (process.platform !== "win32") {
              chmodSync(outPath, 0o755);
            }
          }
          buffer = buffer.subarray(totalSize);
        }
        cb();
      },
    }),
  );
}

async function extractZip(archivePath, destDir) {
  // extractZip 仅在 Windows 上触发（getArchiveExt 对 win32 返回 zip）。
  // Windows 没有内置 `unzip` 命令，改用 PowerShell 的 Expand-Archive 解压，
  // 否则 Windows 用户安装后预下载会被静默吞掉、首次运行 CLI 直接报错退出。
  const { execSync } = await import("node:child_process");
  const psArchive = archivePath.replace(/'/g, "''");
  const psDest = destDir.replace(/'/g, "''");
  execSync(
    `powershell -NoProfile -NonInteractive -Command "Expand-Archive -LiteralPath '${psArchive}' -DestinationPath '${psDest}' -Force"`,
    { stdio: "inherit" },
  );
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  const cached = cachedBinaryPath();
  if (existsSync(cached)) {
    process.stderr.write(`nuwax-codex ${VERSION} binary already cached, skipping download.\n`);
    return;
  }

  const target = getTargetTriple();
  const ext = getArchiveExt();
  const url = `${OSS_CDN_BASE}/v${VERSION}/nuwax-codex-${VERSION}-${target}.${ext}`;

  process.stderr.write(`Pre-downloading nuwax-codex ${VERSION} for ${target} …\n`);
  process.stderr.write(`  ${url}\n`);

  const dir = cacheDir();
  mkdirSync(dir, { recursive: true });
  const archivePath = join(dir, `nuwax-codex.${ext}`);

  await downloadBinary(url, archivePath);

  process.stderr.write("  Extracting …\n");
  if (ext === "tar.gz") {
    await extractTarGz(archivePath, dir);
  } else {
    await extractZip(archivePath, dir);
  }

  if (existsSync(cached)) {
    process.stderr.write(`✓ nuwax-codex ${VERSION} ready at ${cached}\n`);
  } else {
    process.stderr.write(`⚠ nuwax-codex ${VERSION} download completed but binary not found at ${cached}\n`);
  }
}

main().catch((err) => {
  process.stderr.write(`nuwax-codex postinstall: ${err.message}\n`);
  // Never fail the install — binary will be downloaded on first run instead
});
