# Gitea 自建 CI（方案 B）

目标：用**自有机器 + Gitea**跑本仓库的构建，不依赖 GitHub Actions 的排队与慢 runner；
`.github/workflows/` **完全不动**，GitHub 那条发布链路照旧可用。

## 目录内容

| 文件 | 作用 |
|---|---|
| `.gitea/workflows/smoke.yml` | 快速流水线（约 1 分钟）：验证 runner 可达 + 工具链齐全 |
| `.gitea/workflows/release.yml` | 7 平台 release 构建（GitHub `release.yml` 的移植版），产物落到宿主目录 |
| `scripts/gitea-ci/docker-compose.yml` | Gitea + act_runner 本地/单机编排 |
| `scripts/gitea-ci/install-gitea.sh` | 在目标 Linux 机器上安装（数据目录可指到 2T 盘） |
| `scripts/gitea-ci/host-inventory.sh` | **只读**体检：CPU/内存/磁盘/Docker/端口/工具链 |
| `scripts/gitea-ci/verify-local.sh` | 在任意装了 Docker 的机器上端到端自测（临时仓库，不碰 GitHub） |
| `scripts/gitea-ci/runner-inventory.sh` | 流水线里做的工具链体检 |

## 机器角色（按现有 5 台规划）

| 机器 | 角色 | 说明 |
|---|---|---|
| `192.168.31.23`（公司 Ubuntu 24，双 2T 外接盘） | **Gitea Server + Linux runner** | 数据与构建缓存都放 2T 盘 |
| `192.168.31.26`（Mac mini） | **macOS runner** | 两个 darwin 目标；Apple Silicon 上交叉 `x86_64-apple-darwin` 可行 |
| `192.168.32.235`（Windows） | **Windows runner** | 两个 msvc 目标 |
| `192.168.31.41` / `192.168.31.45` | 备用 Linux runner | 公司机器装常驻服务需先获批 |

## 1. 体检目标机器（只读）

```bash
ssh swufe@192.168.31.23 'bash -s' < scripts/gitea-ci/host-inventory.sh
```

重点看三块输出：

- `block devices` / `candidate data disks`：两块 2T 盘当前是否已分区、已挂载、文件系统类型；
- `filesystems and free space`：根盘剩余空间（Gitea 数据 + cargo 缓存放 2T 盘，根盘只需几个 GB）；
- `docker`：daemon 是否可直接访问（`daemon: NOT reachable` 就需要 `sudo` 或加入 docker 组）。

## 2. 把 2T 盘挂到固定挂载点

> **这一步会改磁盘，先确认盘上没有要保留的数据。** 下面按"整盘 ext4"举例，
> `/dev/sdX` 请用第 1 步 `lsblk` 的实际设备名替换，不要照抄。

```bash
# 确认这块盘没有挂载、也没有你的数据
lsblk -o NAME,SIZE,FSTYPE,MOUNTPOINT,MODEL

# 已经格式化过 ext4 → 直接挂；否则先 mkfs.ext4 -L ci-data /dev/sdX
sudo mkdir -p /mnt/ci
sudo mount /dev/sdX /mnt/ci
sudo chown -R "$USER":"$USER" /mnt/ci

# 开机自动挂载（用 UUID 更稳）
sudo blkid /dev/sdX
echo 'UUID=<uuid> /mnt/ci ext4 defaults,nofail 0 2' | sudo tee -a /etc/fstab
sudo systemctl daemon-reload && sudo mount -a
```

第二块 2T 盘可以留给**构建缓存**（cargo registry + target，几十 GB 起）：

```bash
sudo mkdir -p /mnt/ci-cache && sudo mount /dev/sdY /mnt/ci-cache
sudo chown -R "$USER":"$USER" /mnt/ci-cache
```

## 3. 安装 Gitea

```bash
sudo bash scripts/gitea-ci/install-gitea.sh \
  --data-dir /mnt/ci/gitea \
  --port 3301 --ssh-port 2223
```

脚本结束会打印：Web 地址、admin 账号（密码随机生成并打印一次）、compose 路径、数据目录。
它会创建：

```
/mnt/ci/gitea/{gitea,runner,artifacts}
```

`artifacts/` 就是 release 流水线的产物落点（替代 GitHub 的 `upload-artifact`）。

## 4. 注册 runner

Linux（Docker 模式，跑 `ubuntu-latest` 任务）：

```bash
# Gitea → Site Administration → Actions → Runners 里取 registration token
docker exec gitea-ci gitea actions generate-runner-token | tail -1
```

macOS / Windows 需要用 **host 模式**（它们构建的是本机目标，容器里跑不了）：

```bash
# macOS runner（官方二进制）
mkdir -p ~/act_runner && cd ~/act_runner
curl -fsSLO https://gitea.com/gitea/act_runner/releases/download/v0.2.11/act_runner-0.2.11-darwin-arm64
chmod +x act_runner-0.2.11-darwin-arm64
./act_runner-0.2.11-darwin-arm64 register \
  --no-interactive \
  --instance http://192.168.31.23:3301 \
  --token <REGISTRATION_TOKEN> \
  --labels macos:host \
  --name mac-mini
./act_runner-0.2.11-darwin-arm64 daemon --config config.yaml
```

Windows 同理，用 `act_runner-0.2.11-windows-amd64.exe`，标签 `windows:host`（需要 Git Bash，
流水线里用 `shell: bash`）。

Runner 标签要和 `release.yml` 的 `runs-on` 对上：`self-hosted` + `linux` / `macos` / `windows`。

## 5. 验证

**本机（不需要局域网）**：任意装了 Docker 的机器上跑

```bash
bash scripts/gitea-ci/verify-local.sh        # 起临时 Gitea + runner → 推 smoke workflow → 断言成功
KEEP_GITEA=1 bash scripts/gitea-ci/verify-local.sh   # 保留环境自己点开看
```

**目标机器上**：把仓库推到 Gitea 后，仓库 → Actions 里应出现 `smoke` 运行并变绿。
之后打 tag 即可触发 `release-gitea`：

```bash
git tag v0.18.30 && git push origin v0.18.30     # origin 指向 Gitea 时
```

## 6. 与 GitHub 版 release.yml 的差异

| GitHub | Gitea 版 | 原因 |
|---|---|---|
| `actions/setup-python` | 直接要求机器自带 python3（体检脚本会报） | 少一个外部 action 依赖 |
| `actions/cache` | 宿主目录 `$CI_CARGO_HOME` / `$CI_TARGET_DIR` | 自建机上持久缓存比 actions/cache 更可靠 |
| `mlugg/setup-zig` | curl 下 Zig 0.14.0 官方 tar | 固定版本、无第三方 action |
| `actions/upload-artifact` | 拷到 `$CI_ARTIFACTS_DIR` | Gitea 的 artifact 支持不保证可用 |
| `softprops/action-gh-release` | **不做** | 发布产物先人工核对 |
| OSS 上传 + npm publish | **不做**（见 `release.yml` 注释） | 需要凭据与已核验产物，避免半成品上架 |

发布闭环还缺的三步（后续要补）：产物上传 OSS、`npm publish`、Gitea Release 记录。

## 7. 安全提醒

- `.41` / `.23` 是公司机器：装常驻服务、跑能读 secrets 的构建前先确认合规；
- runner 上会落 `NPM_TOKEN` / OSS AK，建议只给 **automation token + 单包权限**；
- Gitea 默认 sqlite，适合单机；多人/多 runner 建议换 Postgres；
- 不要把 Gitea 直接暴露公网；内网用 HTTP + 反向代理加 TLS 更稳。

## 8. 实测记录（2026-11-06，`192.168.31.23`）

环境：Ubuntu 24.04 / 8 核 / 31 GB / Docker 29.9.2（`swufe` 可直接用 daemon）/
数据落 `/data/nvme1/gitea-ci`（1.7T 可用）、构建缓存计划落 `/data/nvme0/ci-cache`。

**已验证**：Gitea 1.23 + act_runner 0.2.11 起来，`smoke` 流水线 **success**，
job 日志里能看到 `8 cpus / 31.0 GiB / 1.5T free` 和工具链清单。

实践中踩到并已修进本目录脚本的坑：

| 现象 | 原因 | 修法 |
|---|---|---|
| `dial tcp 20.205.243.166:443: i/o timeout`（job 失败） | job 容器**访问不了 github.com**，而 `actions/checkout@v4` 要从 GitHub 克隆 action | 流水线**不用任何 GitHub action**：checkout 改成 `git clone`（见 `smoke.yml`），zig 自己下，缓存用宿主目录 |
| `fatal: unable to access 'http://gitea:3000/...': Could not resolve host` | act_runner 给每个 job 建**独立 docker 网络**，compose 里的 `gitea` 主机名在那不存在 | `GITEA__server__ROOT_URL` / runner 的 `GITEA_INSTANCE_URL` 用**主机 IP**（`--public-url`），不要用容器名 |
| `[actions] DEFAULT_ACTIONS_URL does not support "https://github.com"` | Gitea 1.23 只接受简写 | 配 `GITEA__actions__DEFAULT_ACTIONS_URL: github` |
| `Gitea is not supposed to be run as root`（容器内 CLI 报错） | `gitea` 命令行拒绝 root | `docker exec -u git gitea-ci gitea ...` |
| 镜像 `platform (linux/arm64) does not match host (linux/amd64)` | 从 Apple Silicon 上 `docker save` 的成像是 arm64 | 在能访问 registry 的机器上 `docker pull --platform linux/amd64`，再 `docker save \| ssh host docker load` |
| `免費節點當前繁忙`（拉镜像失败） | 内网 registry 加速器限流 | 同上：从可用机器流式灌入 |
| 改 ROOT_URL 后任务仍用旧地址 | runner 注册时把实例地址写进 `.runner` | 删 `runner/.runner` 重新注册 |

## 9. 真实构建还需要的离线准备

`smoke` 只验证 runner 与工具链。真要跑 `release-gitea.yml`，这台机器还需要：

1. **cargo 镜像**：主机访问 `crates.io` 返回 403，但 `rsproxy.cn` 可用。
   ```bash
   cat > /data/nvme0/ci-cache/cargo/config.toml <<'TOML'
   [source.crates-io]
   replace-with = "rsproxy"
   [source.rsproxy]
   registry = "sparse+https://rsproxy.cn/index/"
   [registries.rsproxy]
   index = "sparse+https://rsproxy.cn/index/"
   [net]
   git-fetch-with-cli = true
   TOML
   ```
   workflow 里已经把 `CARGO_HOME` 指到 `/srv/gitea-ci/cargo`（按需改成
   `/data/nvme0/ci-cache/cargo`）。

2. **rusty_v8 产物**：`.github/actions/setup-rusty-v8` 从 GitHub Releases 下载，
   job 容器访问不到。在**主机**上下载（主机能连 GitHub）后落到固定目录，
   流水线用 `RUSTY_V8_ARCHIVE` / `RUSTY_V8_SRC_BINDING_PATH` 指过去即可
   （本机 `scripts/prefetch_rusty_v8.sh` 生成的 env 文件就是这两个变量）。

3. **macOS / Windows runner**：见第 4 节，标签 `macos` / `windows`（host 模式）。

4. 发布闭环（OSS 上传 + `npm publish`）仍按第 6 节保持人工，等产物核验后再接。

## 10. 离线构建配方（已实测出包）

`192.168.31.23` 的网络限制：**job 容器**连不到 github.com/crates.io；**宿主机**能连
github.com 但 `crates.io` 返回 403、`chromium.googlesource.com` 超时。因此真实构建按
"宿主机预置 + 目录源"做：

| 依赖 | 做法 | 位置 | 实测 |
|---|---|---|---|
| Rust 1.99.1 + musl target | `install-host-runner.sh` | `/data/nvme1/gitea-ci/toolchain/{cargo,rustup}` | rustc/cargo 1.99.1 |
| Zig 0.14.0（musl 链接） | 同上 | `.../toolchain/zig` | 0.14.0 |
| crates 与 git 依赖（含 `rules_rust`/`nucleo`/`h3`/`mxc`/`rusty_v8`） | 有网机器 `cargo vendor`，整包搬过去 | `/data/nvme0/ci-cache/vendor/ci-vendor`（1.9G） | 目录源替代所有 registry |
| rusty_v8 归档 | `fetch-rusty-v8.sh`（校验 `third_party/v8` 清单） | `/data/nvme0/ci-cache/rusty_v8/<target>`（87M/3 target） | 校验 OK |
| 构建缓存 | `CARGO_TARGET_DIR` 指到大盘 | `/data/nvme0/ci-cache/target/<target>` | — |

**为什么必须 vendor**：`Cargo.lock` 把 `v8` pin 在 `seanh-oai/rusty_v8` 的 git rev 上，
那个 rev 的 `.gitmodules` 指向 `chromium.googlesource.com`（本网络超时）。git URL 重写
到本地副本没用——子模块属于**被 pin 的那个提交**，改不了历史。换成目录源后 cargo
根本不走 git，问题消失。

实测：在 `.23` 上对 `codex-code-mode-host`（gnu）跑
`cargo build --release` → **4 分 08 秒**出 207MB 二进制，全程无外网。

完整流程：

```bash
# 1) 有网机器：vendor + 采 V8
bash scripts/gitea-ci/vendor-deps.sh --repo codex-rs --out /tmp/ci-vendor
bash scripts/gitea-ci/fetch-rusty-v8.sh --repo . --cache-root /tmp/v8seed --all-linux

# 2) 搬到 CI 机
tar -C /tmp -czf - ci-vendor | ssh ci-host 'tar -C /data/nvme0/ci-cache/vendor -xzf -'
tar -C /tmp/v8seed -czf - rusty_v8 | ssh ci-host 'tar -C /data/nvme0/ci-cache -xzf -'

# 3) CI 机：装工具链与 host runner，写入目录源配置
bash install-host-runner.sh --instance http://<host>:3301
bash vendor-deps.sh --install /data/nvme0/ci-cache/vendor/ci-vendor \
  --cargo-home /data/nvme1/gitea-ci/toolchain/cargo
```

依赖变更（`Cargo.lock` 更新）后需要重跑第 1、2 步。

## 11. 用 make 触发（推荐入口）

`Makefile` 只做一层转发，逻辑在 `scripts/gitea-ci/ci.sh`；配置读环境变量或
`scripts/gitea-ci/.env`（已 gitignore，模板见 `.env.example`）。

```bash
make help                  # 列出全部目标
make gitea-check           # 实例健康 + runner + 最近 run + artifacts 体积
make gitea-runs N=10       # 最近 N 次运行
make gitea-smoke           # 把当前 HEAD 推到 Gitea 镜像并等 smoke 结果
make gitea-release TAG=v0.18.30   # 打 tag 并推送 → release-gitea 流水线
make gitea-logs RUN=12     # 解压并 tail 某次运行的 job 日志
make gitea-artifacts       # 宿主上已产出的归档
make gitea-inventory       # 只读体检目标机
make gitea-up / gitea-down # 启停 Gitea 栈
```

首次使用先把仓库镜像到 Gitea（只需一次）：

```bash
git remote add gitea http://<user>:<pass>@192.168.31.23:3301/<owner>/<repo>.git
git push gitea HEAD:main          # 注意：不要带 --tags，tag 会触发 release 流水线
```

之后 `make gitea-smoke` 就是 `git push gitea HEAD:main` + 轮询结果。

已知边界：`make gitea-release` 会触发 7 平台矩阵，而 U `macos-host` / `windows-host`
两个 runner 还没接入，所以现在它会在那两个 job 上排队/失败；要只验证 Linux 链路，先把
`release.yml` 的 matrix 收窄到 `linux-host` 的三个目标。

## 12. 多平台实测（2026-11-06）

三台 host 模式 runner 已接入：`linux-host`（192.168.31.23）、`macos-host`（Mac mini
192.168.31.26）、`windows-host`（192.168.32.235）。`release.yml` 的 matrix 是 7 个目标。

| 目标 | runner | 状态 |
|---|---|---|
| x86_64-unknown-linux-gnu | linux-host | ✅ **已产出归档**（`nuwax-codex-<ver>-x86_64-unknown-linux-gnu.tar.gz`，解出的 `nuwax-harness --version` 正常） |
| x86_64-unknown-linux-musl / aarch64-unknown-linux-musl | linux-host | ⏳ 交叉编译很慢（zig 链接，冷构建 40 分钟以上） |
| aarch64/x86_64-apple-darwin | macos-host | ✅ 编译通过；打包曾因 Python 3.9 缺 `hashlib.file_digest` 失败，已改成流式哈希 |
| x86_64-pc-windows-msvc | windows-host | ⏳ 编译中 |
| aarch64-pc-windows-msvc | windows-host | ❌ **缺 VS BuildTools 的 ARM64 组件**（`VC/Tools/MSVC/*/bin/Hostx64/arm64/cl.exe` 不存在）；只装 LLVM 不够，链接仍要 ARM64 的 MSVC 库 |

踩过的平台差异（每条都已在 workflow 里处理）：

1. `needs.<job>.outputs` 在 Gitea 求值为空 → 每个 job 自己从 `GITHUB_REF_NAME` 取版本。
2. cargo 步骤必须在 `codex-rs/` 下跑（GitHub 用 `defaults.run.working-directory` 提供）。
3. `install-musl-build-tools.sh` 需要 `TARGET` 环境变量。
4. `runner.os` 大小写与 GitHub 不一致 → 用 `uname -s` 判断平台。
5. **macOS job 里 `git` 连不上实例**（同一 job 内 `curl` 却 200/401）→ checkout 失败时回退到 Gitea archive API。
6. 两个 mac job 并发 `rustup target add` 抢同一个 RUSTUP_HOME → mac runner `capacity: 1`。
7. **Windows 在另一个网段且到不了 192.168.31.x**（TCP 能建但 HTTP 被 TUN 网关吞掉）→ 在同时有两个网段的机器上跑 3301 中继，runner 用 `CI_GITEA_BASE` 指向它。
8. Windows 的 `tar`：Git Bash 的 `/usr/bin/tar` 处理不了 `C:\...`（当成远程主机），用 `C:\Windows\System32\tar.exe`。
9. 打包脚本 `hashlib.file_digest` 需 Python 3.11+ → 改成流式 sha256（对所有平台安全）。

**aarch64-windows 的根因与现状（2026-11-11 复核）**

GitHub 那条链路**根本没有交叉编译**这个目标：`release.yml` 把它放在
`windows-11-arm`（原生 ARM64 runner，自带 ARM64 `cl.exe`）。我们只有一台 x64 的
`windows-host`，所以必然要走交叉编译，而交叉编译需要：

1. VS ARM64 工具链 `VC\Tools\MSVC\*\bin\Hostx64\arm64\cl.exe`（**当前没装**）；
2. `vcvarsall amd64_arm64`（为 ARM64 的 UCRT/SDK 设置 INCLUDE/LIB）；
3. C 依赖（ring 等）走 `clang-cl`。

2026-11-06 那次失败的实际报错是 `fatal error: 'assert.h' file not found`，
日志里的命令行是**裸 `clang` + GNU 风格参数**——说明 cc-rs 没拿到
`CC_aarch64_pc_windows_msvc`，回退到了 PATH 上的 clang，而它没有 MSVC 的 include 路径。
（这就是 README 早先写"缺 ARM64 组件"的实证来源。）

已做的两件事：

- `win-build.cmd` **不再静默回退**：先 preflight ARM64 `cl.exe`，缺失时直接打印要装的
  组件名并 `exit /b 1`；同时把 `CC/CXX/AR`（通用 + target 专属）都设上，并把
  `INCLUDE/LIB/clang-cl/llvm-lib` 的存在性打进日志，下次失败一眼能看出来。
- `release.yml` 里该目标**暂时注释掉**（矩阵 + verify 目标列表同步），pipeline 现在
  按 6 个平台变绿；补齐方式写在注释里：

  ```text
  VS Installer → Visual Studio Build Tools 2022 → Individual components →
  "MSVC v143 - VS 2022 C++ ARM64 build tools (latest)"   （约 1–2 GB，需管理员）
  ```

  装完把矩阵里那三行取消注释、verify 列表加回 `aarch64-pc-windows-msvc` 即可。

在那之前，`aarch64-pc-windows-msvc` 由 GitHub 的 `windows-11-arm` 负责——两条链路
各出一半，发布时把两边的产物合并即可。

### 多 Linux 并行（2026-11-06 追加）

3 个 Linux 目标原来挤在 Gitea 宿主的 2 个 runner 槽位里，musl 交叉一占就是 40 分钟，
gnu/musl 互相排队。现在多接了一台 `192.168.31.41`（**72 核 / 62 GB**）作为 `linux-host`：

- 它没有 `/data`，所以 workflow 的 Linux 分支改成读 `CI_DATA_ROOT` / `CI_CACHE_ROOT`
  （以及 `CI_CARGO_HOME`/`CI_RUSTUP_HOME`/`CI_ZIG_DIR`），默认仍是 Gitea 宿主的 `/data` 布局；
  该跑动器在启动 daemon 时 export 自己的路径（`$HOME/ci/{data,cache}`）。
- 每台 Linux runner 都需要：rustup 1.99.1 + 两个 musl target、Zig 0.14.0、
  **cargo vendor 目录**（1.9 G，`vendor-deps.sh`）、rusty_v8 的三个 Linux 归档、以及 act_runner。
- 国内下载 rustup 组件很慢，**用 `RUSTUP_DIST_SERVER=https://mirrors.ustc.edu.cn/rust-static`**
  （rsproxy 的 rustup 路径已 404，tuna 403）。
- 装完 zig 后要确认没有半写状态：job 报 `zig: Text file busy` 说明安装脚本还在写文件，
  等它结束再触发。
- 用 `setsid nohup ... &` 启动 daemon，普通 `nohup ... &` 在 ssh 会话结束时会被带走。

### 产物清单 manifest.json（2026-11-06 追加）

每个平台 job 打完包后会生成一份 `entry.json`（`file`/`url`/`sha256`/`size`/`target`），
和归档一起上传到：

```
ci-gitea/<version>/<target>/nuwax-codex-<version>-<target>.{tar.gz,zip}
ci-gitea/<version>/<target>/entry.json
```

`verify-artifacts` 校验完 7 个归档后，把 7 份 entry 合并成 **`ci-gitea/<version>/manifest.json`**
并上传：

```json
{
  "version": "0.1.2",
  "tag": "v0.1.2",
  "generated_at": "2026-11-06T09:47:51Z",
  "repository": "ci/codex",
  "commit": "<sha>",
  "base_url": "https://s3.nuwax.com:9443/nuwax-packages/ci-gitea/0.1.2",
  "targets": {
    "aarch64-apple-darwin": {
      "target": "aarch64-apple-darwin",
      "file": "nuwax-codex-0.1.2-aarch64-apple-darwin.tar.gz",
      "url": "…/ci-gitea/0.1.2/aarch64-apple-darwin/nuwax-codex-0.1.2-aarch64-apple-darwin.tar.gz",
      "sha256": "9c0e9f61…",
      "size": 135860575
    }
  }
}
```

这样发布脚本 / `nuwax-harness` 的 postinstall 只要读一个文件，不用在代码里硬编码 7 个地址，
也能用 `sha256` 校验下载完整性。生成脚本是 `scripts/gitea-ci/release-manifest.py`
（只用标准库，刻意兼容 Python 3.9 —— macOS runner 就是 3.9）。

> ⚠️ **vendor 目录必须带 `config.generated.toml`**。只替换 `[source.crates-io]` 时，
> `v8`/`crossterm`/`tokio-tungstenite` 这些**git 依赖仍会走网络**，于是又去拉
> `chromium.googlesource.com` 的 `buildtools` 子模块并超时失败。`vendor-deps.sh` 的
> vendor 模式会把该文件写进目录；如果是手工 `cargo vendor`，记得把输出的完整
> source 替换配置（9 个 `[source."git+…"]`）一起放进去，安装端会据此生成
> `$CARGO_HOME/config.toml`。
