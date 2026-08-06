#!/usr/bin/env node
// Launcher for the nuwax-codex-ts npm package.
// Downloads the native `nuwax-codex` binary from GitHub Releases on first
// run and caches it locally. No platform-specific npm packages needed.

import { spawnSync } from "node:child_process";
import { createWriteStream, existsSync, mkdirSync, chmodSync } from "node:fs";
import { homedir } from "node:os";
import { join, dirname } from "node:path";
import { pipeline } from "node:stream/promises";
import { createGunzip } from "node:zlib";
import { familySync } from "detect-libc";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const VERSION = require("../package.json").version;
const OSS_CDN_BASE = "https://nuwa-packages.oss-rg-china-mainland.aliyuncs.com/nuwax-codex";

// -- platform helpers -------------------------------------------------------

function getTargetTriple() {
  const p = process.platform;
  const a = process.arch;

  if (p === "darwin") {
    return a === "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin";
  }
  if (p === "linux") {
    if (a !== "x64") throw new Error(`Unsupported Linux arch: ${a}`);
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

// -- download & cache -------------------------------------------------------

function cacheDir() {
  return join(homedir(), ".nuwax-codex-cache", VERSION);
}

function cachedBinaryPath() {
  return join(cacheDir(), getBinaryName());
}

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
  // 等待写流的文件描述符真正释放后再返回。Windows 上 Expand-Archive 以独占方式
  // 打开 zip;若 Node 仍持有写句柄,解压会报“正由另一进程使用”。ws.end() 只是
  // 结束写入,fd 在 ‘close’ 事件时才异步关闭,因此这里必须 await。
  await new Promise((resolve, reject) => {
    ws.once("close", resolve);
    ws.once("error", reject);
  });
  process.stderr.write("\n");

  if (process.platform !== "win32") {
    chmodSync(outPath, 0o755);
  }
}

async function extractTarGz(archivePath, destDir) {
  // Simple tar.gz extraction: members are listed sequentially as
  // [header(512B)][content(padded to 512B)]...
  const { createReadStream } = await import("node:fs");
  const { createGunzip } = await import("node:zlib");
  const { pipeline } = await import("node:stream/promises");
  const { Transform } = await import("node:stream");
  const { writeFileSync } = await import("node:fs");

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
          // Parse tar header
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
  // Windows 没有内置 `unzip` 命令，改用 PowerShell 的 Expand-Archive 解压。
  const { execSync } = await import("node:child_process");
  const psArchive = archivePath.replace(/'/g, "''");
  const psDest = destDir.replace(/'/g, "''");
  const cmd =
    `powershell -NoProfile -NonInteractive -Command "Expand-Archive -LiteralPath '${psArchive}' -DestinationPath '${psDest}' -Force"`;

  // Expand-Archive 用独占 FileStream 打开 zip。即便 Node 已关闭写句柄,Windows
  // Defender 仍可能在下载完成的瞬间锁住文件做实时扫描,导致解压报“正由另一进程
  // 使用”。做有限次重试给杀软让出窗口;仅最后一次失败才把原始错误抛出。
  const maxAttempts = 3;
  let lastErr;
  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    const isLast = attempt === maxAttempts;
    try {
      execSync(cmd, { stdio: isLast ? "inherit" : "pipe" });
      return;
    } catch (err) {
      lastErr = err;
      if (!isLast) {
        console.error(`  Expand-Archive 被占用(第 ${attempt}/${maxAttempts} 次),重试…`);
        await new Promise((r) => setTimeout(r, 500 * attempt));
      }
    }
  }
  throw lastErr;
}

// -- main -------------------------------------------------------------------

async function ensureBinary() {
  const cached = cachedBinaryPath();
  if (existsSync(cached)) {
    return cached;
  }

  const target = getTargetTriple();
  const ext = getArchiveExt();
  const url = `${OSS_CDN_BASE}/v${VERSION}/nuwax-codex-${VERSION}-${target}.${ext}`;

  console.error(`Downloading nuwax-codex ${VERSION} for ${target} …`);
  console.error(`  ${url}`);

  const dir = cacheDir();
  mkdirSync(dir, { recursive: true });
  const archivePath = join(dir, `nuwax-codex.${ext}`);

  await downloadBinary(url, archivePath);

  console.error("  Extracting …");
  if (ext === "tar.gz") {
    await extractTarGz(archivePath, dir);
  } else {
    await extractZip(archivePath, dir);
  }

  if (!existsSync(cached)) {
    console.error("  Error: binary not found after extraction at", cached);
    process.exit(1);
  }

  return cached;
}

function run() {
  ensureBinary().then((binaryPath) => {
    const result = spawnSync(binaryPath, process.argv.slice(2), {
      stdio: "inherit",
      windowsHide: true,
    });
    if (result.error) {
      console.error(`Failed to execute ${binaryPath}:`, result.error);
      process.exit(1);
    }
    process.exit(result.status ?? 1);
  }).catch((err) => {
    console.error(err.message);
    process.exit(1);
  });
}

run();
