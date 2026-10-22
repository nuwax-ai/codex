// Shared native bundle installer for postinstall and both npm commands.
import { createWriteStream, existsSync, mkdirSync, mkdtempSync, renameSync, rmdirSync, rmSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, join } from "node:path";
import { createRequire } from "node:module";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { setTimeout as delay } from "node:timers/promises";
import { familySync } from "detect-libc";

const require = createRequire(import.meta.url);
const VERSION = require("../package.json").version;
const OSS_CDN_BASE = "https://nuwa-packages.oss-rg-china-mainland.aliyuncs.com/nuwax-codex";

export function getTargetTriple(platform = process.platform, arch = process.arch, libc = familySync()) {
  if (platform === "darwin") {
    if (arch === "arm64") return "aarch64-apple-darwin";
    if (arch === "x64") return "x86_64-apple-darwin";
  } else if (platform === "linux") {
    if (arch === "arm64") return "aarch64-unknown-linux-musl";
    if (arch === "x64") {
      return libc === "musl" ? "x86_64-unknown-linux-musl" : "x86_64-unknown-linux-gnu";
    }
  } else if (platform === "win32") {
    if (arch === "arm64") return "aarch64-pc-windows-msvc";
    if (arch === "x64") return "x86_64-pc-windows-msvc";
  }
  throw new Error(`Unsupported platform/architecture: ${platform}/${arch}`);
}

export function requiredBundleFiles(platform = process.platform) {
  const ext = platform === "win32" ? ".exe" : "";
  const files = [`nuwax-harness${ext}`, `codex-code-mode-host${ext}`];
  if (platform === "linux") files.push("codex-resources/bwrap");
  if (platform === "win32") files.push("codex-windows-sandbox-setup.exe", "codex-command-runner.exe");
  return files;
}

function missingBundleFiles(directory) {
  return requiredBundleFiles().filter((name) => {
    try {
      return !statSync(join(directory, name)).isFile();
    } catch {
      return true;
    }
  });
}

async function extractArchive(archivePath, destination) {
  if (process.platform !== "win32") {
    const tar = await import("tar");
    await tar.x({ file: archivePath, cwd: destination, gzip: true });
    return;
  }
  const yauzl = await import("yauzl");
  await new Promise((resolve, reject) => {
    yauzl.open(archivePath, { lazyEntries: true, autoClose: true, strictFileNames: true }, (error, archive) => {
      if (error) return reject(error);
      const fail = (failure) => {
        archive.close();
        reject(failure);
      };
      archive.on("error", fail);
      archive.on("entry", (entry) => {
        try {
          if (!entry.fileName || entry.fileName.includes("\\") || entry.fileName.includes("\0") || entry.fileName.startsWith("/") || /^[a-zA-Z]:/.test(entry.fileName) || entry.fileName.split("/").includes("..")) {
            throw new Error(`Unsafe ZIP entry: ${entry.fileName}`);
          }
          const destinationPath = join(destination, entry.fileName);
          if (entry.fileName.endsWith("/")) {
            mkdirSync(destinationPath, { recursive: true });
            archive.readEntry();
            return;
          }
          mkdirSync(dirname(destinationPath), { recursive: true });
          archive.openReadStream(entry, (readError, stream) => {
            if (readError) return fail(readError);
            pipeline(stream, createWriteStream(destinationPath)).then(() => archive.readEntry()).catch(fail);
          });
        } catch (entryError) {
          fail(entryError);
        }
      });
      // On Windows the archive cannot be removed until yauzl closes its fd.
      archive.on("close", resolve);
      archive.readEntry();
    });
  });
}

async function publishBundle(stagingDirectory, directory, stagingRoot) {
  const lock = `${directory}.publish-lock`;
  let acquired = false;
  // Only serialize the short publication step; downloads/extraction stay parallel.
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      mkdirSync(lock);
      acquired = true;
      break;
    } catch (error) {
      if (error.code !== "EEXIST") throw error;
    }
    await delay(50);
  }
  if (!acquired) {
    throw new Error(`Native bundle publish lock is busy: ${lock}. Retry after the other installer exits; an interrupted installer may leave this empty lock directory behind.`);
  }
  try {
    // A concurrent install may already have published the same complete bundle.
    if (missingBundleFiles(directory).length === 0) return;
    if (existsSync(directory)) renameSync(directory, join(stagingRoot, "previous-incomplete-cache"));
    renameSync(stagingDirectory, directory);
    if (missingBundleFiles(directory).length > 0) throw new Error("Native bundle cache is incomplete after installation");
  } finally {
    rmdirSync(lock);
  }
}

export async function ensureBinary() {
  const target = getTargetTriple();
  const directory = join(homedir(), ".nuwax-harness-cache", VERSION, target);
  const binaryPath = join(directory, requiredBundleFiles()[0]);
  if (missingBundleFiles(directory).length === 0) return binaryPath;

  // Archive names and the OSS namespace stay compatible with existing clients.
  const ext = process.platform === "win32" ? "zip" : "tar.gz";
  const url = `${OSS_CDN_BASE}/v${VERSION}/nuwax-codex-${VERSION}-${target}.${ext}`;
  console.error(`Downloading nuwax-harness ${VERSION} for ${target} …`);
  mkdirSync(dirname(directory), { recursive: true });
  const stagingRoot = mkdtempSync(`${directory}.stage-`);
  const stagingDirectory = join(stagingRoot, "bundle");
  const archivePath = join(stagingRoot, `nuwax-codex.${ext}`);
  mkdirSync(stagingDirectory, { recursive: true });
  try {
    const response = await fetch(url, { redirect: "follow" });
    if (!response.ok || !response.body) {
      throw new Error(`Failed to download ${basename(archivePath)}: HTTP ${response.status}`);
    }
    // pipeline waits for disk writes/close and propagates ENOSPC/EACCES.
    await pipeline(Readable.fromWeb(response.body), createWriteStream(archivePath));
    await extractArchive(archivePath, stagingDirectory);
    const missing = missingBundleFiles(stagingDirectory);
    if (missing.length > 0) throw new Error(`Incomplete native bundle: missing ${missing.join(", ")}`);

    // Check again under a bounded lock before atomically quarantining a bad
    // cache. One repair must never remove another installer's complete winner.
    await publishBundle(stagingDirectory, directory, stagingRoot);
    return binaryPath;
  } finally {
    rmSync(stagingRoot, { recursive: true, force: true });
  }
}
