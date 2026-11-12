# Gitea 自建 CI：初始化与运维手册

本目录记录**自建 Gitea + act_runner 跑 nuwax-harness 多平台构建**的完整做法、
踩过的坑和当前配置。代码侧的流水线定义在 `.gitea/workflows/`，本目录只讲
**机器怎么装、runner 怎么起、出问题怎么查**。

> 与 `scripts/gitea-ci/` 的关系：那边是早期的安装/打包脚本（含 docker-compose
> 与离线 vendor 工具）；本目录是**运维手册 + 修正后的启动脚本**，以本目录为准。
>
> **不含任何口令、token、AK/SK。** 真实值放本地 `.env`（已 gitignore），
> 模板见 [`env.example`](env.example)。

## 0. 从零重建（TL;DR）

换机器/重装时按这个顺序，每步都有对应脚本：

```bash
# ① Gitea 服务端（一台常开的 Linux）
sudo gitea-ops/bootstrap/gitea-server.sh --data-dir /srv/gitea --port 3301
#    → 记下打印的 admin 口令，并生成 runner token：
#      docker exec -u git <容器> gitea actions generate-runner-token

# ② 每台 Linux 构建机
sudo gitea-ops/bootstrap/linux-build-host.sh \
  --data-root /srv/ci/data --cache-root /srv/ci/cache \
  --runner-name linux-a \
  --instance http://<gitea-host>:3301 --token <token>
#    → 无外网的机器加 --warm-vendor/--warm-rusty-v8/--musl-cross-tarball

# ③ macOS 构建机：见 bootstrap/macos-runner.md
# ④ Windows 构建机：见 bootstrap/windows-runner.md

# ⑤ 验证
cp gitea-ops/env.example gitea-ops/.env && $EDITOR gitea-ops/.env
HOSTS="user@host-a user@host-b" ./gitea-ops/scripts/health-check.sh
./gitea-ops/scripts/trigger-build.sh v0.18.48
```

装完之后**必须自检这四条**，它们是最常见的初始化失败点：

| 检查 | 命令 |
|---|---|
| runner 标签是 `linux-host:host` | `python3 -c "import json;print(json.load(open('<runner>/.runner'))['labels'])"` |
| runner 进程带着 `CI_*` 环境 | `tr '\0' '\n' < /proc/<pid>/environ \| grep CI_DATA_ROOT` |
| `CARGO_HOME/config.toml` 指向 vendor | `grep -c vendored-sources <CARGO_HOME>/config.toml` |
| `capacity` × 并行单元 ≤ 内存 65% | `grep capacity <runner>/config.yaml` + `free -g` |

## 1. 拓扑

| 角色 | 平台 | runner 标签 | 说明 |
|---|---|---|---|
| Gitea 服务端 | Linux x86_64 | — | Web + API + Actions 调度，数据落大盘 |
| Linux 构建机 A（大内存） | Linux x86_64 | `linux-host` | 主构建机，跑全部 Linux 目标 |
| Linux 构建机 B | Linux x86_64 | `linux-host` | 宿主机模式，分担 Linux 目标（**标签必须是 `linux-host`**） |
| 同一台的容器 runner | Linux x86_64 | `ubuntu-latest` | 容器模式，只接 `inventory` 这类轻任务 |
| macOS 构建机 | macOS arm64 | `macos-host` | 两个 darwin 目标；**x86_64 在 arm64 上交叉编译** |
| Windows 构建机 | Windows x86_64 | `windows-host` | `x86_64-pc-windows-msvc`；ARM64 需另装 VS 组件 |

调度只看标签：`runs-on: linux-host` 的 job **只会**落到带 `linux-host` 的 runner 上。

## 2. 一次性初始化

### 2.1 Gitea 服务端

```bash
bash scripts/gitea-ci/install-gitea.sh --data-dir <数据目录> --port 3301 --ssh-port 2223
```

脚本结束会打印 Web 地址与初始 admin 口令（只打印一次，自行保存）。

关键服务端配置项（踩过的）：

| 配置 | 值 | 原因 |
|---|---|---|
| `GITEA__server__ROOT_URL` | `http://<主机IP>:3301` | 用 IP，不要用 compose 服务名——job 容器的网络里没有那个名字 |
| `GITEA__actions__DEFAULT_ACTIONS_URL` | `github` | 1.23 只接受简写 |
| 容器内 CLI | `docker exec -u git gitea-ci gitea ...` | 直接跑会报 "should not be run as root" |

### 2.2 注册 runner

在 Gitea → Site Administration → Actions → Runners 取 registration token，
或 `docker exec -u git <容器> gitea actions generate-runner-token`。

**宿主机模式**（构建用，标签必须带 `:host` 后缀）：

```bash
./act_runner register --no-interactive \
  --instance http://<主机IP>:3301 \
  --token <REGISTRATION_TOKEN> \
  --labels linux-host:host \
  --name <runner 名称>
```

**容器模式**（只跑轻任务）：标签写 `ubuntu-latest`。

> 标签写错是最隐蔽的故障：runner 明明在线、机器空闲，job 却永远排队。
> 判断方法见 §5「runner 在线但不接任务」。

### 2.3 发行版支持

引导脚本会自动识别包管理器并按族映射包名：

| 族 | 识别方式 | 安装命令 |
|---|---|---|
| Debian / Ubuntu | `apt-get` | `apt-get install -y` |
| Fedora / RHEL / Rocky | `dnf` 或 `yum` | `dnf install -y` |
| Arch | `pacman` | `pacman -Sy --noconfirm` |

包名不通用（`libssl-dev` ↔ `openssl-devel` ↔ `openssl`），脚本内已按族列好清单。
四者都不是时报错并打印需要手工安装的能力清单（C 工具链、clang、lld、libcap、
libc++、openssl、musl 工具链）。

> 已验证：Ubuntu 24 / Ubuntu 26。
> 未实测：Fedora / RHEL / Arch —— 包名映射是按官方仓库名写的，首次在上面跑时
> 请留意 `dnf install` 的解析结果；musl 交叉编译在非 Debian 系上建议直接用
> musl.cc 的工具链包（`--musl-cross-tarball`）。

### 2.4 构建机的工具链

Linux 宿主机模式每台都要有：rustup + 目标 target、Zig、cargo vendor 目录、
rusty_v8 预编译归档。`scripts/gitea-ci/install-host-runner.sh` 负责大部分。

若该机磁盘布局不是默认的 `/data/nvme1/gitea-ci` + `/data/nvme0/ci-cache`，
**必须在启动 runner 时导出**这些变量（见下一节），否则 job 会去写 `/data` 并报
`mkdir: cannot create directory '/data': Permission denied`。

```bash
CI_DATA_ROOT       # 该机的 ci 数据根（toolchain、artifacts）
CI_CACHE_ROOT      # 该机的构建缓存根（cargo target、vendor、rusty_v8、act）
CI_CARGO_HOME      # CARGO_HOME（里面要有指向 vendor 的 config.toml）
CI_RUSTUP_HOME     # RUSTUP_HOME
CI_ZIG_DIR         # Zig 所在目录
```

## 3. 启动 / 保活 runner

用 [`runners/start-host-runner.sh`](runners/start-host-runner.sh)：

```bash
CI_DATA_ROOT=/srv/ci/data CI_CACHE_ROOT=/srv/ci/cache \
  ./gitea-ops/runners/start-host-runner.sh /srv/ci/data/runner /srv/ci/bin/act_runner
```

它做了三件必须做的事，少一件都会出问题：

1. **导出上面五个变量**（漏了 → `/data` 权限拒绝）；
2. `setsid nohup ... < /dev/null &`（普通 `nohup &` 会随 ssh 会话结束被杀）；
3. 把 stdout/stderr 追加到 `daemon.log`，便于事后排查。

长期运行用 systemd：[`runners/gitea-runner-host.service`](runners/gitea-runner-host.service)。
**当前正在跑 job 时不要重启 runner**——会直接杀死该 job，日志表现为
「停在编译中间、没有任何 error」。

## 4. 并发与内存（重要）

构建 codex 是**内存**瓶颈，不是 CPU 瓶颈。两个旋钮：

| 旋钮 | 位置 | 含义 |
|---|---|---|
| `runner.capacity` | runner 的 `config.yaml` | 这台机器**同时**接几个 job |
| `CARGO_BUILD_JOBS` | `.gitea/workflows/release.yml` | 每个 job 内 cargo 并行单元数 |

`CARGO_BUILD_JOBS` 已按机器内存自适应（工作流里计算，勿写死）：

```bash
mem_gb=$(awk '/MemTotal/ {printf "%d", $2/1048576}' /proc/meminfo)
jobs=$(( mem_gb / 30 ))   # 每个并行单元预算 ~30 GB（含 v8 的 rustc 峰值约 10 GB）
[ "$jobs" -lt 1 ] && jobs=1
[ "$jobs" -gt 3 ] && jobs=3
```

| 机器内存 | 每 job 单元 | capacity=2 时峰值 |
|---|---|---|
| 62 GB | 2 | ~40 GB ✅ |
| 31 GB | 1 | ~20 GB ✅ |

**反例（真实事故）**：`capacity: 3` + 全开 `-j`（=72）→ 3 个 job × 72 rustc，
内存 61/62 GB、swap 跑满、load 236，机器 sshd 无法完成握手（ping 和 22 端口仍正常）。
调整后必须**重启 runner 才生效**（配置只在启动时读取）。

## 5. 排障手册

### job 报 `mkdir: cannot create directory '/data': Permission denied`
runner 启动时没带 `CI_*` 变量，工作流回落到了默认路径。用 §3 的脚本重启。

### runner 在线、机器空闲，但 job 一直排队
标签不匹配。查该 runner 的注册标签：

```bash
python3 -c "import json;print(json.load(open('<runner 目录>/.runner'))['labels'])"
```

`runs-on: linux-host` 只认 `linux-host:host`。

### Linux 目标一起失败，日志停在编译中且无 error
这台机器被压垮或 runner 被重启。看 `load average` 与 `free -g`；若 SSH 报
`Connection timed out during banner exchange` 而 ping/22 端口正常，就是 sshd
拿不到 CPU，需要重启机器并下调 `capacity`。

### `unable to update registry crates-io / transfer too slow: 0 bytes in 30s`
该 runner 的 `CARGO_HOME/config.toml` 没有指向 vendor 目录，cargo 走了网络镜像。
生成方式：

```bash
bash scripts/gitea-ci/vendor-deps.sh --install <vendor目录> --cargo-home <CARGO_HOME>
```

工作流里已有 preflight，会**立刻**报错并打印这条命令，不再白跑 40 分钟。

### `final link failed: memory exhausted`（musl 目标）
musl-cross 带的 GNU ld 默认把每个输入文件的符号表留在内存里，链接峰值很高。
工作流已为 musl 目标加：

```
-C link-arg=-Wl,--no-keep-memory -C link-arg=-Wl,--reduce-memory-overheads
```

> 换 lld 走不通：该 gcc driver 只在自身工具链内找 `ld`，`-fuse-ld=lld` 报
> `cannot find 'ld'`；`-B/usr/bin` 又会误用宿主 `as`。

### Windows 构建报「系统找不到指定的路径」
两个独立原因，都出现过：

1. **`.cmd` 被写成 LF 换行** → cmd.exe 解析 `|| exit /b 1` 异常。
   `.gitattributes` 已锁 `*.cmd/*.bat/*.ps1 text eol=crlf`。
2. **`shift` 会连带移位 `%0`** → 之后再用 `%~dp0` 拿到的是第一个参数的路径。
   必须先把 `%~dp0` 存进变量再 `shift`。

### Windows runner 连不上 Gitea
Windows 机器在另一网段时，用一台双栈机器做 3301 端口中继，runner 注册时
`--instance` 指向中继地址。

## 6. 触发与查看

```bash
git push gitea v0.18.47          # 推 tag 触发 release-gitea
```

查看任务状态（示例，需本地 `.env` 里的地址与账号）：

```bash
set -a; . ./gitea-ops/.env; set +a
curl -s -u "$GITEA_USER:$GITEA_PASS" \
  "$GITEA_URL/api/v1/repos/$GITEA_REPO/actions/tasks?limit=30" \
  | python3 -c 'import sys,json;[print(r["name"],r["status"]) for r in json.load(sys.stdin)["workflow_runs"]]'
```

读任务日志（日志在 Gitea 宿主机上，zstd 压缩，文件名是 task id）：

```bash
find <CI_DATA_ROOT>/gitea/gitea/actions_log/<owner>/<repo> -name "<task-id>.log.zst" \
  | xargs zstd -dcq | tail -50
```

辅助脚本：[`scripts/health-check.sh`](scripts/health-check.sh)（巡检）、
[`scripts/trigger-build.sh`](scripts/trigger-build.sh)（打 tag 并轮询到结束）。

## 7. 各平台实测耗时（参考）

| 目标 | runner | 耗时 |
|---|---|---|
| aarch64-apple-darwin | mac mini (arm64) | ~22 min |
| x86_64-apple-darwin | mac mini (arm64，交叉) | ~23 min |
| x86_64-pc-windows-msvc | windows | ~25 min |
| x86_64-unknown-linux-gnu | linux | ~40 min |
| x86_64-unknown-linux-musl | linux | ~60 min（含 musl tools 冷构建） |
| aarch64-unknown-linux-musl | linux | ~60 min（交叉 + musl tools） |

对比 GitHub Actions 同构矩阵 5+ 小时、且 mac job 常被取消——**自建 Gitea 是主通道**，
GitHub 保留为备份/兜底（尤其 `aarch64-pc-windows-msvc`）。

## 9. sudo 策略（构建机需要 root 权限时怎么办）

工作流里唯一需要 root 的地方是 **musl 目标的 apt 依赖**（`musl-tools`/`clang`/`lld`/
`libc++-dev` …）。三种做法，按推荐度排列：

### ✅ 方案 A（首选）：一次性手工装齐，CI 不再碰 sudo

在机器上执行一次即可，之后每次构建都跳过 apt：

```bash
sudo apt-get update
sudo apt-get install -y ca-certificates curl musl-tools pkg-config libcap-dev \
  g++ clang libc++-dev libc++abi-dev lld xz-utils perl make
```

工作流的 `install-musl-build-tools.sh` 现在会**先检查包是否齐全**：

* 齐了 → 打印「依赖已齐，跳过 apt」直接继续（完全不调用 sudo）；
* 缺了且**没有免密 sudo** → 立刻失败并打印需要执行的那条命令，
  不再像以前那样卡 17 分钟 × 3 次后报一句 `sudo: 3 incorrect password attempts`。

检查一台机器是否就绪：

```bash
for p in ca-certificates curl musl-tools pkg-config libcap-dev g++ clang \
         libc++-dev libc++abi-dev lld xz-utils perl make; do
  dpkg -s "$p" >/dev/null 2>&1 || echo "MISS $p"
done
```

### ✅ 方案 B：只给这一条命令配免密 sudo

比 `NOPASSWD: ALL` 安全得多，且机器重装后仍然有效：

```bash
# /etc/sudoers.d/gitea-runner-apt
Cmnd_Alias CI_APT = /usr/bin/apt-get update, /usr/bin/apt-get install
<构建用户> ALL=(root) NOPASSWD: CI_APT
```

校验：`sudo -n apt-get --version && echo ok`。

### ❌ 不要把密码交给 CI

* **不要**在 workflow 里 `echo <密码> | sudo -S ...` —— 密码会进 job 日志和
  `actions_log/*.log.zst`，任何能看构建日志的人都能读到；
* **不要**把密码写进 runner 的 config/systemd；
* **不要**为了省事给构建用户 `NOPASSWD: ALL`（等于让 CI 拥有 root）。

> 如果某台机器既装不了包、又不能配 sudo，就别把它加入 `linux-host` 标签——
> 它会让每个 musl 目标都失败。这类机器只适合挂 `ubuntu-latest` 跑轻任务。

## 8. 已知未完成

- `aarch64-pc-windows-msvc`：需在 Windows 机装 VS BuildTools 的 ARM64 组件
  （`Microsoft.VisualStudio.Component.VC.Tools.ARM64`，约 1–2 GB，需管理员），
  当前从 Gitea 矩阵中摘除，由 GitHub 构建。
- musl 目标的 GNU ld 内存参数刚加，尚待一轮完整验证。
- runner 的 `capacity` 改动需要重启才能生效，暂无热加载。

详细事故复盘见 [`INCIDENTS.md`](INCIDENTS.md)。
