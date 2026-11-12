# macOS 构建机（darwin 两个目标）

macOS runner 一机承担 `aarch64-apple-darwin` 与 `x86_64-apple-darwin`。
在 Apple Silicon 上**直接交叉编译 x86_64 即可**，不需要 Intel 机器、也不需要模拟器：

* `x86_64-apple-darwin` 是标准交叉目标，macOS SDK 原生双架构，`clang -arch x86_64` 出码；
* 唯一硬要求是对应架构的 **rusty_v8 预编译归档**
  （`librusty_v8_<profile>_x86_64-apple-darwin.a.gz`），缺了会退回源码编译 v8（数小时）。

实测（M4，16 GB）：两个目标各约 **22–23 分钟**，与原生 arm64 差距在噪声范围内。

## 1. 一次性准备

```bash
# 工具链（版本与 codex-rs/rust-toolchain.toml 保持一致）
curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal
rustup toolchain install 1.99.1 --profile minimal
rustup target add --toolchain 1.99.1 aarch64-apple-darwin x86_64-apple-darwin

# Xcode CLT（clang / ld 都在里面）
xcode-select --install || true

# act_runner（宿主机模式）
mkdir -p ~/ci-toolchain/act_runner && cd ~/ci-toolchain/act_runner
curl -fsSLO https://gitea.com/gitea/act_runner/releases/download/v0.2.11/act_runner-0.2.11-darwin-arm64
chmod +x act_runner-0.2.11-darwin-arm64

# 注册（标签必须是 macos-host:host）
./act_runner-0.2.11-darwin-arm64 register --no-interactive \
  --instance http://<gitea-host>:3301 --token <token> \
  --labels macos-host:host --name mac-mini
```

`config.yaml` 里 **`capacity: 1`**：两个 mac 目标都要 `rustup target add`，共享同一个
`RUSTUP_HOME`，并发会互相抢锁。

## 2. 启动

```bash
cd ~/ci-toolchain/act_runner
setsid nohup ./act_runner-0.2.11-darwin-arm64 daemon --config config.yaml \
  >> daemon.log 2>&1 < /dev/null &
```

macOS 上没有 systemd，长期保活可以用 `launchd`（`~/Library/LaunchAgents/*.plist`，
`RunAtLoad` + `KeepAlive`），或用 `gitea-ops/runners/start-host-runner.sh` 的同样思路。

## 3. 该机的 ci 布局

工作流的 Darwin 分支**不需要** `CI_*` 变量，它直接用：

```
root=$HOME/ci
cache=$HOME/ci-cache
cargo_home=$HOME/ci-toolchain/cargo
rustup_home=$HOME/ci-toolchain/rustup
zig_dir=$HOME/ci-toolchain/zig
```

所以要在该机准备好 `~/ci`（artifacts 落点）与 `~/ci-toolchain`。

## 4. 排障

| 现象 | 原因 |
|---|---|
| `hashlib.file_digest` 不存在 | 打包脚本需要 Python ≥ 3.11；已改为流式 sha256，仍在旧脚本上会遇到 |
| job 里 `git` 连不上实例但 `curl` 正常 | 本机代理/证书；可用 `CI_GITEA_BASE` 指向可达地址，或退回 archive API |
| 两个 mac job 同时失败 | `capacity` 不是 1，`rustup target add` 抢 `RUSTUP_HOME` |
| 编译几小时 | 冷缓存（cache key 含 commit）+ GitHub/mac runner 抖动；自建机 22 分钟即完成 |
