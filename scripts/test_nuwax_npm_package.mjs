#!/usr/bin/env node
// Offline installation contracts. Never changes the user's HOME or npm cache.
// Usage: NUWAX_NPM_TEST_NODE_MODULES=/absolute/node_modules node --test scripts/test_nuwax_npm_package.mjs
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { cp, mkdir, mkdtemp, readFile, readdir, rm, symlink, writeFile, chmod } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { promisify } from "node:util";
import { setTimeout as delay } from "node:timers/promises";
import { after, test } from "node:test";

const execute = promisify(execFile);
const repository = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const scratchParent = process.env.NUWAX_NPM_TEST_TMPDIR || "/Volumes/zhitai/tmp";
await mkdir(scratchParent, { recursive: true });
const scratch = await mkdtemp(join(scratchParent, "nuwax-npm-contract-"));
const packageRoot = join(scratch, "package");
await cp(join(repository, "npm"), packageRoot, { recursive: true });
const modules = process.env.NUWAX_NPM_TEST_NODE_MODULES || join(repository, "node_modules");
await symlink(resolve(modules), join(packageRoot, "node_modules"), "dir");
const require = createRequire(join(packageRoot, "package.json"));
const tar = require("tar");
const metadata = JSON.parse(await readFile(join(packageRoot, "package.json"), "utf8"));
const runtime = await import(pathToFileURL(join(packageRoot, "bin/runtime.js")));
let archive = Buffer.alloc(0);
let responseStatus = 200;
const requests = [];
const server = createServer((request, response) => {
  requests.push(request.url);
  response.writeHead(responseStatus, { "content-type": "application/octet-stream" });
  response.end(archive);
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const fixtureOrigin = `http://127.0.0.1:${server.address().port}`;
const preload = join(scratch, "fixture-preload.mjs");
await writeFile(preload, `
const originalFetch = globalThis.fetch;
Object.defineProperty(process, "platform", { value: process.env.FIXTURE_PLATFORM || "darwin" });
Object.defineProperty(process, "arch", { value: process.env.FIXTURE_ARCH || "arm64" });
globalThis.fetch = (input, options) => {
  const url = new URL(typeof input === "string" ? input : input.url);
  if (url.origin !== "https://nuwa-packages.oss-rg-china-mainland.aliyuncs.com") {
    throw new Error("Unexpected external request in offline npm contract: " + url.origin);
  }
  return originalFetch(process.env.FIXTURE_ORIGIN + url.pathname, options);
};
`);

after(async () => {
  await new Promise((resolve) => server.close(resolve));
  await rm(scratch, { recursive: true, force: true });
});

async function bundle({ omit = [], platform = "darwin", extraEntries = [] } = {}) {
  const root = await mkdtemp(join(scratch, "archive-"));
  const ext = platform === "win32" ? ".exe" : "";
  const main = `#!${process.execPath}
const {spawnSync} = require("node:child_process");
const {dirname, join} = require("node:path");
const host = spawnSync(join(dirname(process.argv[1]), "codex-code-mode-host${ext}"), ["probe"], {encoding:"utf8"});
console.log(JSON.stringify({binary:process.argv[1], args:process.argv.slice(2), source:process.env.CODEX_INSTALL_SOURCE, host:host.stdout?.trim(), hostStatus:host.status}));
process.exit(Number(process.env.FIXTURE_NATIVE_EXIT || 0));
`;
  const host = `#!${process.execPath}\nconsole.log("fixture-host");\n`;
  const files = runtime.requiredBundleFiles(platform).filter((name) => !omit.includes(name));
  for (const name of files) {
    const path = join(root, name);
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, name.startsWith("nuwax-harness") ? main : host);
    await chmod(path, 0o755);
  }
  const path = `${root}.${platform === "win32" ? "zip" : "tar.gz"}`;
  if (platform === "win32") {
    await execute("python3", ["-c", "import json,pathlib,sys,zipfile; root=pathlib.Path(sys.argv[1]); z=zipfile.ZipFile(sys.argv[2], 'w', zipfile.ZIP_DEFLATED); [z.write(root/name,name) for name in sys.argv[4:]]; [z.writestr(name,'escape-probe') for name in json.loads(sys.argv[3])]; z.close()", root, path, JSON.stringify(extraEntries), ...files]);
  } else {
    await tar.c({ file: path, cwd: root, gzip: true }, files);
  }
  archive = await readFile(path);
  responseStatus = 200;
}

async function home() {
  return mkdtemp(join(scratch, "home-"));
}

function cache(homeDirectory, target = "aarch64-apple-darwin") {
  return join(homeDirectory, ".nuwax-harness-cache", metadata.version, target);
}

async function invoke(command, homeDirectory, additionalEnv = {}) {
  try {
    const result = await execute(process.execPath, ["--import", preload, join(packageRoot, "bin", command), "--fixture", "argument with spaces"], {
      cwd: scratch,
      env: { ...process.env, HOME: homeDirectory, USERPROFILE: homeDirectory, FIXTURE_ORIGIN: fixtureOrigin, ...additionalEnv },
      timeout: 15_000,
      maxBuffer: 1_000_000,
    });
    return { code: 0, ...result };
  } catch (error) {
    return { code: error.code, stdout: error.stdout || "", stderr: error.stderr || "" };
  }
}

test("maps all seven release targets and rejects unsupported architectures", () => {
  const examples = [
    ["darwin", "arm64", null, "aarch64-apple-darwin"],
    ["darwin", "x64", null, "x86_64-apple-darwin"],
    ["linux", "x64", "glibc", "x86_64-unknown-linux-gnu"],
    ["linux", "x64", "musl", "x86_64-unknown-linux-musl"],
    ["linux", "arm64", "glibc", "aarch64-unknown-linux-musl"],
    ["linux", "arm64", "musl", "aarch64-unknown-linux-musl"],
    ["win32", "x64", null, "x86_64-pc-windows-msvc"],
    ["win32", "arm64", null, "aarch64-pc-windows-msvc"],
  ];
  for (const [platform, arch, libc, expected] of examples) {
    assert.equal(runtime.getTargetTriple(platform, arch, libc), expected);
  }
  assert.throws(() => runtime.getTargetTriple("linux", "riscv64", "glibc"), /Unsupported/);
  assert.throws(() => runtime.getTargetTriple("darwin", "ia32", null), /Unsupported/);
  assert.deepEqual(runtime.requiredBundleFiles("win32"), ["nuwax-harness.exe", "codex-code-mode-host.exe", "codex-windows-sandbox-setup.exe", "codex-command-runner.exe"]);
});

test("fresh new command downloads the unchanged archive name and launches the renamed binary with its host", async () => {
  await bundle();
  const directory = await home();
  const start = requests.length;
  const result = await invoke("nuwax-harness.js", directory);
  assert.equal(result.code, 0, result.stderr);
  const receipt = JSON.parse(result.stdout);
  assert.equal(receipt.binary, join(cache(directory), "nuwax-harness"));
  assert.deepEqual(receipt.args, ["--fixture", "argument with spaces"]);
  assert.equal(receipt.source, "npm_nuwax");
  assert.equal(receipt.host, "fixture-host");
  assert.equal(receipt.hostStatus, 0);
  assert.deepEqual(requests.slice(start), [`/nuwax-codex/v${metadata.version}/nuwax-codex-${metadata.version}-aarch64-apple-darwin.tar.gz`]);
  const oldAlias = await invoke("nuwax-codex.js", directory);
  assert.equal(oldAlias.code, 0, oldAlias.stderr);
  assert.equal(JSON.parse(oldAlias.stdout).binary, receipt.binary);
  assert.equal(requests.length, start + 1, "old command reuses the complete new cache");
});

test("postinstall and launcher share a complete bundle and forward native exit status", async () => {
  await bundle();
  const directory = await home();
  const start = requests.length;
  const install = await invoke("postinstall.js", directory);
  assert.equal(install.code, 0, install.stderr);
  assert.match(install.stderr, /ready at/);
  const run = await invoke("nuwax-harness.js", directory, { FIXTURE_NATIVE_EXIT: "7" });
  assert.equal(run.code, 7, run.stderr);
  assert.equal(JSON.parse(run.stdout).hostStatus, 0);
  assert.equal(requests.length, start + 1);
});

test("both entry points reject a cache with only the main binary when download fails", async () => {
  const directory = await home();
  await mkdir(cache(directory), { recursive: true });
  await writeFile(join(cache(directory), "nuwax-harness"), "#!/bin/sh\necho WRONG-PARTIAL-CACHE\n");
  await chmod(join(cache(directory), "nuwax-harness"), 0o755);
  responseStatus = 503;
  const start = requests.length;
  const install = await invoke("postinstall.js", directory);
  assert.equal(install.code, 0, "postinstall failure stays recoverable");
  assert.doesNotMatch(install.stderr, /ready at/);
  assert.match(install.stderr, /HTTP 503/);
  const run = await invoke("nuwax-harness.js", directory);
  assert.equal(run.code, 1);
  assert.equal(run.stdout, "", "incomplete cache executable must never run");
  assert.equal(requests.length, start + 2);
});

test("a later valid download repairs an incomplete cache as a whole", async () => {
  await bundle();
  const directory = await home();
  await mkdir(cache(directory), { recursive: true });
  await writeFile(join(cache(directory), "nuwax-harness"), "partial");
  const result = await invoke("nuwax-codex.js", directory);
  assert.equal(result.code, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout).host, "fixture-host");
  assert.equal((await readdir(dirname(cache(directory)))).filter((name) => name.includes(".stage-")).length, 0);
});

test("concurrent repairs serialize publication and both launch the same complete winner", async () => {
  await bundle();
  const directory = await home();
  await mkdir(cache(directory), { recursive: true });
  await writeFile(join(cache(directory), "nuwax-harness"), "incomplete-cache");
  const lock = `${cache(directory)}.publish-lock`;
  await mkdir(lock);
  const pending = [invoke("nuwax-harness.js", directory), invoke("nuwax-codex.js", directory)];
  let preparationError;
  try {
    // Hold our own lock until both real installers have extracted their bundles.
    // This forces the competing repair paths to wait at the publication boundary.
    const deadline = Date.now() + 5_000;
    while (true) {
      const staged = (await readdir(dirname(cache(directory)))).filter((name) => name.startsWith("aarch64-apple-darwin.stage-"));
      const complete = await Promise.all(staged.map(async (name) => {
        try {
          await readFile(join(dirname(cache(directory)), name, "bundle/codex-code-mode-host"));
          return true;
        } catch {
          return false;
        }
      }));
      if (staged.length === 2 && complete.every(Boolean)) break;
      assert.ok(Date.now() < deadline, "both installers must reach the real publication boundary");
      await delay(20);
    }
  } catch (error) {
    preparationError = error;
  } finally {
    await rm(lock, { recursive: true });
  }
  const results = await Promise.all(pending);
  if (preparationError) throw preparationError;
  for (const result of results) {
    assert.equal(result.code, 0, result.stderr);
    const receipt = JSON.parse(result.stdout);
    assert.equal(receipt.binary, join(cache(directory), "nuwax-harness"));
    assert.equal(receipt.hostStatus, 0);
    assert.equal(receipt.host, "fixture-host");
  }
  assert.deepEqual((await readdir(cache(directory))).sort(), runtime.requiredBundleFiles("darwin").sort());
  assert.deepEqual(await readdir(dirname(cache(directory))), ["aarch64-apple-darwin"]);
});

test("an archive missing code-mode-host never publishes a final cache or a ready receipt", async () => {
  await bundle({ omit: ["codex-code-mode-host"] });
  const directory = await home();
  const install = await invoke("postinstall.js", directory);
  assert.equal(install.code, 0);
  assert.match(install.stderr, /missing codex-code-mode-host/);
  assert.doesNotMatch(install.stderr, /ready at/);
  const run = await invoke("nuwax-harness.js", directory);
  assert.equal(run.code, 1);
  await assert.rejects(readFile(join(cache(directory), "nuwax-harness")), { code: "ENOENT" });
  assert.deepEqual(await readdir(dirname(cache(directory))), []);
});

test("postinstall and launcher both support Linux arm64 using the musl bundle", async () => {
  await bundle({ platform: "linux" });
  const directory = await home();
  const start = requests.length;
  const environment = { FIXTURE_PLATFORM: "linux", FIXTURE_ARCH: "arm64" };
  const install = await invoke("postinstall.js", directory, environment);
  assert.equal(install.code, 0, install.stderr);
  assert.match(install.stderr, /ready at/);
  const run = await invoke("nuwax-harness.js", directory, environment);
  assert.equal(run.code, 0, run.stderr);
  assert.equal(JSON.parse(run.stdout).binary, join(cache(directory, "aarch64-unknown-linux-musl"), "nuwax-harness"));
  assert.deepEqual(requests.slice(start), [`/nuwax-codex/v${metadata.version}/nuwax-codex-${metadata.version}-aarch64-unknown-linux-musl.tar.gz`]);
});

test("Linux bundles missing bwrap fail before executing the main binary", async () => {
  await bundle({ platform: "linux", omit: ["codex-resources/bwrap"] });
  const run = await invoke("nuwax-harness.js", await home(), { FIXTURE_PLATFORM: "linux", FIXTURE_ARCH: "arm64" });
  assert.equal(run.code, 1);
  assert.equal(run.stdout, "");
  assert.match(run.stderr, /codex-resources\/bwrap/);
});

test("Windows ZIP postinstall requires the renamed main executable, host and both sandbox helpers", async () => {
  await bundle({ platform: "win32" });
  const directory = await home();
  const start = requests.length;
  const environment = { FIXTURE_PLATFORM: "win32", FIXTURE_ARCH: "arm64" };
  const install = await invoke("postinstall.js", directory, environment);
  assert.equal(install.code, 0, install.stderr);
  assert.match(install.stderr, /ready at/);
  assert.deepEqual((await readdir(cache(directory, "aarch64-pc-windows-msvc"))).sort(), runtime.requiredBundleFiles("win32").sort());
  assert.deepEqual(requests.slice(start), [`/nuwax-codex/v${metadata.version}/nuwax-codex-${metadata.version}-aarch64-pc-windows-msvc.zip`]);

  await bundle({ platform: "win32", omit: ["codex-command-runner.exe"] });
  const incomplete = await home();
  const rejected = await invoke("postinstall.js", incomplete, environment);
  assert.equal(rejected.code, 0);
  assert.doesNotMatch(rejected.stderr, /ready at/);
  assert.match(rejected.stderr, /missing codex-command-runner.exe/);
  await assert.rejects(readFile(join(cache(incomplete, "aarch64-pc-windows-msvc"), "nuwax-harness.exe")), { code: "ENOENT" });
});

test("real ZIP archives with traversal, absolute, drive or backslash entries fail closed", async () => {
  const absoluteEscape = join(scratch, "absolute-escape-probe");
  const names = ["../../relative-escape-probe", absoluteEscape, "C:/drive-escape-probe", "safe\\..\\backslash-escape-probe"];
  for (const name of names) {
    await bundle({ platform: "win32", extraEntries: [name] });
    const directory = await home();
    const result = await invoke("postinstall.js", directory, { FIXTURE_PLATFORM: "win32", FIXTURE_ARCH: "arm64" });
    assert.equal(result.code, 0, "invalid archives remain recoverable install failures");
    assert.doesNotMatch(result.stderr, /ready at/);
    assert.match(result.stderr, /Unsafe ZIP entry|invalid relative path|absolute path|invalid characters/);
    await assert.rejects(readFile(join(cache(directory, "aarch64-pc-windows-msvc"), "nuwax-harness.exe")), { code: "ENOENT" });
    assert.deepEqual(await readdir(dirname(cache(directory))), []);
  }
  await assert.rejects(readFile(absoluteEscape), { code: "ENOENT" });
});

test("npm pack includes both public commands, shared runtime and postinstall without changing the package name", async () => {
  assert.equal(metadata.name, "nuwax-codex");
  assert.deepEqual(metadata.bin, { "nuwax-harness": "bin/nuwax-harness.js", "nuwax-codex": "bin/nuwax-codex.js" });
  const { stdout } = await execute("npm", ["pack", "--dry-run", "--ignore-scripts", "--json", "--cache", join(scratch, "npm-cache")], {
    cwd: packageRoot,
    env: { ...process.env, HOME: scratch, USERPROFILE: scratch },
    timeout: 30_000,
    maxBuffer: 1_000_000,
  });
  const [packed] = JSON.parse(stdout);
  for (const expected of ["bin/nuwax-harness.js", "bin/nuwax-codex.js", "bin/postinstall.js", "bin/runtime.js"]) {
    assert.ok(packed.files.some((file) => file.path === expected), `${expected} missing from npm package`);
  }
  assert.equal(packed.name, "nuwax-codex");
  assert.ok(packed.files.every((file) => !file.path.includes("node_modules")));
});
