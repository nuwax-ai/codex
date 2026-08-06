#!/usr/bin/env node
// Launcher for the nuwax-codex-ts npm package.
// Downloads the native `nuwax-codex` binary from GitHub Releases on first
// run and caches it locally. No platform-specific npm packages needed.

import { spawnSync } from "node:child_process";
import { createWriteStream, existsSync, mkdirSync, chmodSync } from "node:fs";
import { homedir } from "node:os";
import { join, dirname } from "node:path";
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
  // 等待写流的文件描述符真正释放、数据全部落盘后再返回。解压阶段(tar / yauzl)
  // 要读取这个归档文件,若 Node 仍持有写句柄,数据未必已刷盘、在 Windows 上还可能
  // 被独占打开拒绝。ws.end() 只是结束写入,fd 在 ‘close’ 事件时才异步关闭,必须 await。
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
  // 用成熟的 npm `tar` 库流式解压。仓库原先手写 tar 解析器,会把整个 entry 攒进
  // 一个 Buffer 再落盘,对 codex 这种 ~280MB 的单文件归档是 O(n²) 内存拷贝
  // (实测 15s 都写不出一个文件),在 macOS/Linux 上表现为“卡在 Extracting”。
  // `tar` 走流式管道、内存恒定,并按归档内记录的 mode 还原可执行位(已验证 0o755)。
  const tar = await import("tar");
  await tar.x({ file: archivePath, cwd: destDir, gzip: true });
}

async function extractZip(archivePath, destDir) {
  // Windows 上用纯 JS 的 yauzl 流式解压。原实现调用 PowerShell Expand-Archive,
  // 两个硬伤:老版本 Windows PowerShell(5.0 前)没有该 cmdlet;它用独占 FileStream
  // 打开 zip,极易撞“正由另一进程使用”(Node 写句柄未释放 / Defender 实时扫描)。
  // yauzl 经 Node fs 以共享只读方式读取,绕开这两类问题,且逐条目流式落盘、内存恒定。
  const yauzl = await import("yauzl");
  const { createWriteStream, mkdirSync } = await import("node:fs");
  const { dirname, join } = await import("node:path");

  await new Promise((resolve, reject) => {
    yauzl.open(archivePath, { lazyEntries: true, autoClose: true }, (err, zipfile) => {
      if (err) return reject(err);
      zipfile.on("error", reject);
      zipfile.on("entry", (entry) => {
        const outPath = join(destDir, entry.fileName);
        if (/\/$/.test(entry.fileName)) {
          mkdirSync(outPath, { recursive: true });
          zipfile.readEntry();
          return;
        }
        mkdirSync(dirname(outPath), { recursive: true });
        zipfile.openReadStream(entry, (e, readStream) => {
          if (e) return reject(e);
          const ws = createWriteStream(outPath);
          readStream.on("error", reject);
          ws.on("error", reject);
          ws.on("close", () => zipfile.readEntry());
          readStream.pipe(ws);
        });
      });
      zipfile.on("close", resolve);
      zipfile.readEntry();
    });
  });
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
