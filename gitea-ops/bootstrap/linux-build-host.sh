#!/usr/bin/env bash
# 把一台**新的 Linux 机器**装成 Gitea 的宿主机模式构建 runner。
#
# 幂等：每一步都先检测，已具备就跳过。可以重复执行用来"补齐"。
#
#   sudo ./linux-build-host.sh --help
#   sudo ./linux-build-host.sh \
#     --data-root /data/nvme1/gitea-ci \
#     --cache-root /data/nvme0/ci-cache \
#     --runner-name linux-host-2 \
#     --instance http://<gitea-host>:3301 \
#     --token <registration-token> \
#     --warm-vendor /seed/ci-vendor \
#     --warm-rusty-v8 /seed/rusty_v8
#
# 无外网的机器：用 --warm-vendor / --warm-rusty-v8 / --musl-cross-tarball
# 指向本地文件（在能上网的机器上先用 scripts/gitea-ci/ 里的脚本采好）。
#
# 装什么：
#   * 编译依赖（build-essential / clang / lld / cmake / perl / zstd ...）
#   * rustup + 固定版本 toolchain + musl/gnu target
#   * Zig（musl 链接用）
#   * cargo vendor 目录 + CARGO_HOME/config.toml（指向 vendor，避免走网络镜像）
#   * rusty_v8 预编译归档（每个 target 一份）
#   * aarch64-linux-musl 交叉工具链
#   * act_runner + .runner 注册 + config.yaml + systemd 单元
set -euo pipefail

TOOLCHAIN="${TOOLCHAIN:-1.99.1}"
ZIG_VERSION="${ZIG_VERSION:-0.14.0}"
RUNNER_LABEL="${RUNNER_LABEL:-linux-host}"
RUNNER_CAPACITY="${RUNNER_CAPACITY:-2}"
ACT_RUNNER_VERSION="${ACT_RUNNER_VERSION:-0.2.11}"
DRY_RUN=0

DATA_ROOT=""; CACHE_ROOT=""; RUNNER_NAME=""; INSTANCE=""; TOKEN=""
WARM_VENDOR=""; WARM_V8=""; MUSL_CROSS_TARBALL=""; START_RUNNER=1

log()  { printf '\033[1;34m[bootstrap]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[bootstrap] WARN\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[bootstrap] ERROR\033[0m %s\n' "$*" >&2; exit 1; }
run()  { if [ "$DRY_RUN" = 1 ]; then printf '  [dry-run] %s\n' "$*"; else "$@"; fi; }

# 需要 root 才能装包。要么整个脚本用 sudo 跑，要么该用户有免密 sudo；
# 两者都不满足时**立刻报清楚**，不要让调用者对着 apt 的权限错误猜。
if [ "$(id -u)" -ne 0 ]; then
  if command -v sudo >/dev/null && sudo -n true 2>/dev/null; then
    APT_SUDO=(sudo -n)
  else
    APT_SUDO=()
    NEED_MANUAL_APT=1
  fi
else
  APT_SUDO=()
fi

usage() {
  sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
}

while [ $# -gt 0 ]; do
  case "$1" in
    --data-root) DATA_ROOT="${2:?}"; shift 2 ;;
    --cache-root) CACHE_ROOT="${2:?}"; shift 2 ;;
    --runner-name) RUNNER_NAME="${2:?}"; shift 2 ;;
    --instance) INSTANCE="${2:?}"; shift 2 ;;
    --token) TOKEN="${2:?}"; shift 2 ;;
    --label) RUNNER_LABEL="${2:?}"; shift 2 ;;
    --capacity) RUNNER_CAPACITY="${2:?}"; shift 2 ;;
    --toolchain) TOOLCHAIN="${2:?}"; shift 2 ;;
    --warm-vendor) WARM_VENDOR="${2:?}"; shift 2 ;;
    --warm-rusty-v8) WARM_V8="${2:?}"; shift 2 ;;
    --musl-cross-tarball) MUSL_CROSS_TARBALL="${2:?}"; shift 2 ;;
    --no-start) START_RUNNER=0; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage ;;
    *) die "未知参数: $1（--help 看用法）" ;;
  esac
done

[ -n "$DATA_ROOT" ]  || die "缺少 --data-root"
[ -n "$CACHE_ROOT" ] || die "缺少 --cache-root"

CARGO_HOME_DIR="$DATA_ROOT/toolchain/cargo"
RUSTUP_HOME_DIR="$DATA_ROOT/toolchain/rustup"
ZIG_DIR="$DATA_ROOT/toolchain/zig"
VENDOR_DIR="$CACHE_ROOT/vendor/ci-vendor"
V8_DIR="$CACHE_ROOT/rusty_v8"
RUNNER_DIR="$DATA_ROOT/runner"
BIN_DIR="$DATA_ROOT/bin"
TARGETS=(x86_64-unknown-linux-gnu x86_64-unknown-linux-musl aarch64-unknown-linux-musl)
V8_TARGETS=(x86_64-unknown-linux-gnu x86_64-unknown-linux-musl aarch64-unknown-linux-musl)

log "data=$DATA_ROOT  cache=$CACHE_ROOT  toolchain=$TOOLCHAIN  label=$RUNNER_LABEL"

# ---------------------------------------------------------------- 1. 基础依赖
# 自动识别包管理器：Debian/Ubuntu 用 apt，Fedora/RHEL 用 dnf/yum，Arch 用 pacman。
# 包名在三族之间并不通用（libssl-dev ↔ openssl-devel ↔ openssl），所以按族映射。
if command -v apt-get >/dev/null 2>&1; then
  PKG_MGR=apt
elif command -v dnf >/dev/null 2>&1; then
  PKG_MGR=dnf
elif command -v yum >/dev/null 2>&1; then
  PKG_MGR=yum
elif command -v pacman >/dev/null 2>&1; then
  PKG_MGR=pacman
else
  die "不支持的发行版：找不到 apt-get / dnf / yum / pacman。
   请手工装好这些再重跑（或参考 gitea-ops/README.md 的依赖清单）：
     C 工具链(gcc/g++/make)、perl、pkg-config、clang、lld、cmake、curl、git、zstd、xz、
     libcap 开发包、libc++/libc++abi 开发包、openssl 开发包、musl 工具链"
fi
log "包管理器: $PKG_MGR"

case "$PKG_MGR" in
  apt)    pkgs=(build-essential pkg-config libssl-dev musl-tools clang lld cmake perl curl git zstd xz-utils libc++-dev libc++abi-dev libcap-dev) ;;
  dnf|yum)
          pkgs=(gcc gcc-c++ make perl pkgconf-pkg-config openssl-devel clang lld cmake curl git zstd xz libcxx-devel libcxxabi-devel libcap-devel musl-gcc) ;;
  pacman) pkgs=(base-devel pkgconf openssl clang lld cmake perl curl git zstd xz libc++ libcap musl) ;;
esac

pkg_installed() {
  case "$PKG_MGR" in
    apt)    dpkg -s "$1" >/dev/null 2>&1 ;;
    dnf|yum) rpm -q "$1" >/dev/null 2>&1 ;;
    pacman) pacman -Qi "$1" >/dev/null 2>&1 ;;
  esac
}

missing=()
for p in "${pkgs[@]}"; do pkg_installed "$p" || missing+=("$p"); done

if [ ${#missing[@]} -gt 0 ]; then
  # 与 CI 共用同一把锁：工作流的 musl 步骤用 `flock /tmp/codex-musl-tools.lock`
  # 串行化 apt，引导脚本不拿锁就会和正在跑的构建撞在 dpkg 前端锁上
  # （实测报 "E: 无法获得锁 /var/lib/dpkg/lock-frontend"）。
  if [ "$PKG_MGR" = apt ] && command -v flock >/dev/null 2>&1; then
    exec 9>/tmp/codex-musl-tools.lock
    if ! flock -w 900 9; then
      die "等待 /tmp/codex-musl-tools.lock 超时：有别的 apt 或构建正在跑，稍后重试。"
    fi
    log "已取得 apt 锁（/tmp/codex-musl-tools.lock）"
  fi
  log "安装系统依赖: ${missing[*]}"
  run "${APT_SUDO[@]}" true  # 先探一次权限，失败信息更清楚
  case "$PKG_MGR" in
    apt)
      run "${APT_SUDO[@]}" apt-get update -y
      run "${APT_SUDO[@]}" apt-get install -y "${missing[@]}" ;;
    dnf)    run "${APT_SUDO[@]}" dnf install -y "${missing[@]}" ;;
    yum)    run "${APT_SUDO[@]}" yum install -y "${missing[@]}" ;;
    pacman) run "${APT_SUDO[@]}" pacman -Sy --noconfirm "${missing[@]}" ;;
  esac
else
  log "系统依赖已齐"
fi
command -v cc >/dev/null || die "找不到 C 编译器"

# ---------------------------------------------------------------- 2. Rust
export RUSTUP_HOME="$RUSTUP_HOME_DIR" CARGO_HOME="$CARGO_HOME_DIR"
mkdir -p "$CARGO_HOME_DIR/bin" "$RUSTUP_HOME_DIR"
if [ ! -x "$CARGO_HOME_DIR/bin/cargo" ]; then
  log "安装 rustup 到 $CARGO_HOME_DIR"
  # 国内网络慢时可用 RUSTUP_DIST_SERVER=https://mirrors.ustc.edu.cn/rust-static
  run bash -c "curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain none"
else
  log "rustup 已存在"
fi
export PATH="$CARGO_HOME_DIR/bin:$PATH"
run rustup toolchain install "$TOOLCHAIN" --profile minimal --no-self-update
run rustup default "$TOOLCHAIN"
for t in "${TARGETS[@]}"; do
  if rustup target list --installed 2>/dev/null | grep -qx "$t"; then
    log "target 已有: $t"
  else
    run rustup target add --toolchain "$TOOLCHAIN" "$t"
  fi
done

# ---------------------------------------------------------------- 3. Zig
if [ -x "$ZIG_DIR/zig" ]; then
  log "Zig 已存在: $($ZIG_DIR/zig version)"
else
  case "$(uname -m)" in
    x86_64) zig_arch=x86_64 ;;
    aarch64|arm64) zig_arch=aarch64 ;;
    *) die "不支持的架构: $(uname -m)" ;;
  esac
  log "下载 Zig $ZIG_VERSION ($zig_arch)"
  mkdir -p "$ZIG_DIR"
  tmp="$(mktemp -d)"
  run curl -fsSL "https://ziglang.org/download/$ZIG_VERSION/zig-linux-$zig_arch-$ZIG_VERSION.tar.xz" -o "$tmp/zig.tar.xz"
  run tar -xJf "$tmp/zig.tar.xz" -C "$tmp"
  run cp -a "$tmp/zig-linux-$zig_arch-$ZIG_VERSION/." "$ZIG_DIR/"
  rm -rf "$tmp"
  [ -x "$ZIG_DIR/zig" ] || die "Zig 安装失败"
fi

# ---------------------------------------------------------------- 4. cargo vendor
if [ -d "$VENDOR_DIR" ] && [ -f "$VENDOR_DIR/config.generated.toml" ]; then
  log "vendor 已存在: $VENDOR_DIR"
elif [ -n "$WARM_VENDOR" ] && [ -d "$WARM_VENDOR" ]; then
  log "从种子复制 vendor: $WARM_VENDOR"
  mkdir -p "$(dirname "$VENDOR_DIR")"
  run cp -a "$WARM_VENDOR" "$VENDOR_DIR"
else
  # vendor 采集脚本在 fork-nuwax-codex 仓库里（scripts/gitea-ci/）。本目录可能被复制到
  # 别的仓库使用，所以先探测；找不到就要求用 --warm-vendor 提供现成目录。
  repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  vendor_tool="$repo_root/scripts/gitea-ci/vendor-deps.sh"
  if [ -f "$vendor_tool" ] && [ -d "$repo_root/codex-rs" ]; then
    log "本地 vendor 生成（需要能访问 crates.io / git）"
    run bash "$vendor_tool" --repo "$repo_root/codex-rs" --out "$VENDOR_DIR"
  else
    die "本仓库没有 scripts/gitea-ci/vendor-deps.sh（它在 fork-nuwax-codex 里）。
     两种做法：
       1) 在 fork-nuwax-codex 仓库根目录执行本脚本；或
       2) 用 --warm-vendor <现成的 ci-vendor 目录> 传入（在能上网的机器上用
          scripts/gitea-ci/vendor-deps.sh 采好再拷过来）。"
  fi
fi

log "写 CARGO_HOME/config.toml（指向 vendor；不写就会走网络镜像然后超时）"
if [ "$DRY_RUN" = 0 ]; then
  python3 - "$VENDOR_DIR" "$CARGO_HOME_DIR/config.toml" <<'PY'
import pathlib, sys
vendor_dir, cfg = sys.argv[1], sys.argv[2]
gen = pathlib.Path(vendor_dir, "config.generated.toml")
text = gen.read_text() if gen.exists() else '[source.crates-io]\nreplace-with = "vendored-sources"\n'
text = text.split("[source.vendored-sources]")[0].rstrip()
text += f'\n\n[source.vendored-sources]\ndirectory = "{vendor_dir}"\n'
pathlib.Path(cfg).write_text(text)
print(f"  -> {cfg} ({text.count(chr(10))} 行)")
PY
fi

# ---------------------------------------------------------------- 5. rusty_v8
mkdir -p "$V8_DIR"
for t in "${V8_TARGETS[@]}"; do
  if ls "$V8_DIR/$t"/*.gz >/dev/null 2>&1; then
    log "rusty_v8 已有: $t"
    continue
  fi
  if [ -n "$WARM_V8" ] && [ -d "$WARM_V8/$t" ]; then
    log "从种子复制 rusty_v8: $t"
    mkdir -p "$V8_DIR/$t"
    run cp -a "$WARM_V8/$t/." "$V8_DIR/$t/"
  else
    warn "缺少 $t 的 rusty_v8 归档且没有种子；稍后用 scripts/gitea-ci/fetch-rusty-v8.sh 补齐："
    warn "  bash scripts/gitea-ci/fetch-rusty-v8.sh --repo <repo> --cache-root $CACHE_ROOT --all-linux"
  fi
done

# ---------------------------------------------------------------- 6. musl 交叉工具链
MUSL_CROSS="$DATA_ROOT/tools/aarch64-linux-musl-cross"
if [ -x "$MUSL_CROSS/bin/aarch64-linux-musl-gcc" ]; then
  log "aarch64 musl 交叉工具链已存在"
elif [ -n "$MUSL_CROSS_TARBALL" ] && [ -f "$MUSL_CROSS_TARBALL" ]; then
  log "从本地包安装 musl 交叉工具链"
  mkdir -p "$DATA_ROOT/tools"
  run tar -xzf "$MUSL_CROSS_TARBALL" -C "$DATA_ROOT/tools"
else
  warn "没有 aarch64 musl 交叉工具链。musl.cc 在国内可能极慢（108 MB 常超时），"
  warn "建议在能上网的机器下载 aarch64-linux-musl-cross.tgz 后用 --musl-cross-tarball 传入。"
  warn "缺失时 aarch64-unknown-linux-musl 目标会失败（链接器找不到）。"
fi

# ---------------------------------------------------------------- 7. act_runner
ACT_RUNNER="$BIN_DIR/act_runner"
mkdir -p "$BIN_DIR" "$RUNNER_DIR"
if [ -x "$ACT_RUNNER" ]; then
  log "act_runner 已存在"
else
  case "$(uname -m)" in x86_64) ar_arch=amd64 ;; aarch64|arm64) ar_arch=arm64 ;; *) die "不支持的架构" ;; esac
  log "下载 act_runner v$ACT_RUNNER_VERSION ($ar_arch)"
  run curl -fsSL "https://gitea.com/gitea/act_runner/releases/download/v$ACT_RUNNER_VERSION/act_runner-$ACT_RUNNER_VERSION-linux-$ar_arch" -o "$ACT_RUNNER"
  run chmod +x "$ACT_RUNNER"
fi

if [ ! -f "$RUNNER_DIR/.runner" ]; then
  [ -n "$INSTANCE" ] && [ -n "$TOKEN" ] && [ -n "$RUNNER_NAME" ] || \
    die "尚未注册 runner：需要 --instance / --token / --runner-name（或先手工 register）"
  log "注册 runner: $RUNNER_NAME  labels=[$RUNNER_LABEL:host]"
  run "$ACT_RUNNER" register --no-interactive \
      --instance "$INSTANCE" --token "$TOKEN" \
      --labels "$RUNNER_LABEL:host" --name "$RUNNER_NAME" \
      --config "$RUNNER_DIR/config.yaml"
else
  log "runner 已注册: $(python3 -c "import json,sys;d=json.load(open('$RUNNER_DIR/.runner'));print(d.get('name'),d.get('labels'))" 2>/dev/null || echo '?')"
fi

if [ "$DRY_RUN" = 0 ] && [ ! -f "$RUNNER_DIR/config.yaml" ]; then
  cat > "$RUNNER_DIR/config.yaml" <<EOF
log:
  level: info
runner:
  file: $RUNNER_DIR/.runner
  capacity: $RUNNER_CAPACITY
cache:
  enabled: true
  dir: $CACHE_ROOT/act
container:
  force_pull: false
EOF
  log "写入 config.yaml（capacity=$RUNNER_CAPACITY）"
fi

# ---------------------------------------------------------------- 8. 启动
if [ "$DRY_RUN" = 1 ]; then
  log "dry-run: 跳过写 $RUNNER_DIR/start.sh 与 systemd 单元"
else
cat > "$RUNNER_DIR/start.sh" <<EOF
#!/usr/bin/env bash
# 由 gitea-ops/bootstrap/linux-build-host.sh 生成。
# 关键：导出 CI_* 供工作流定位本机布局；setsid+nohup 保证 ssh 断开不被杀。
export CI_DATA_ROOT=$DATA_ROOT
export CI_CACHE_ROOT=$CACHE_ROOT
export CI_CARGO_HOME=$CARGO_HOME_DIR
export CI_RUSTUP_HOME=$RUSTUP_HOME_DIR
export CI_ZIG_DIR=$ZIG_DIR
cd $RUNNER_DIR || exit 1
exec $ACT_RUNNER daemon --config config.yaml
EOF
chmod +x "$RUNNER_DIR/start.sh"
fi

if [ "$START_RUNNER" = 1 ] && [ ! -f /etc/systemd/system/gitea-runner-host.service ] && [ "$DRY_RUN" = 0 ]; then
  cat > /tmp/gitea-runner-host.service <<EOF
[Unit]
Description=Gitea act_runner ($RUNNER_LABEL, host mode)
After=network-online.target
Wants=network-online.target
[Service]
Type=simple
User=$(id -un)
Group=$(id -gn)
WorkingDirectory=$RUNNER_DIR
Environment=CI_DATA_ROOT=$DATA_ROOT
Environment=CI_CACHE_ROOT=$CACHE_ROOT
Environment=CI_CARGO_HOME=$CARGO_HOME_DIR
Environment=CI_RUSTUP_HOME=$RUSTUP_HOME_DIR
Environment=CI_ZIG_DIR=$ZIG_DIR
ExecStart=$ACT_RUNNER daemon --config config.yaml
Restart=always
RestartSec=10
[Install]
WantedBy=multi-user.target
EOF
  cp /tmp/gitea-runner-host.service /etc/systemd/system/ 2>/dev/null \
    && systemctl daemon-reload \
    && systemctl enable --now gitea-runner-host.service \
    && log "已安装并启动 systemd 单元 gitea-runner-host.service" \
    || warn "无法写 systemd 单元（没有 root？），可手工执行 $RUNNER_DIR/start.sh"
fi

log "完成。检查项："
cat <<EOF

  cargo        $CARGO_HOME_DIR/bin/cargo  ($("$CARGO_HOME_DIR/bin/cargo" --version 2>/dev/null || echo '未安装'))
  rustup home  $RUSTUP_HOME_DIR
  zig          $ZIG_DIR/zig
  vendor       $VENDOR_DIR  ($(du -sh "$VENDOR_DIR" 2>/dev/null | cut -f1 || echo '缺失'))
  rusty_v8     $V8_DIR
  musl cross   $MUSL_CROSS
  runner       $RUNNER_DIR (.runner + config.yaml, capacity=$RUNNER_CAPACITY)
  启动         $RUNNER_DIR/start.sh   或   systemctl status gitea-runner-host

下一步：
  1) 在 Gitea 里确认该 runner 显示 Online，且标签是 [$RUNNER_LABEL:host]；
  2) 触发一次构建验证：git push gitea vX.Y.Z
  3) 用 gitea-ops/scripts/health-check.sh 巡检内存与容量。

注意：capacity 是启动时读取的，改了要重启 runner；
      正在跑 job 时重启会直接杀死它。
EOF
