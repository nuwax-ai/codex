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
