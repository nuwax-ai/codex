#!/usr/bin/env bash
# 启动一个宿主机模式的 Gitea act_runner。
#
#   ./start-host-runner.sh <runner目录> <act_runner二进制> [日志文件]
#
# 三条必须做的事，少一条都会出问题（都是真实踩过的）：
#   1. 导出 CI_* 变量 —— 漏了 job 会去写默认的 /data 并报 Permission denied；
#   2. setsid + nohup + </dev/null —— 普通 nohup & 会随 ssh 会话结束被杀；
#   3. 日志追加到文件 —— 否则出问题无从查起。
#
# 已有同名实例在跑时直接退出，避免同一 .runner 注册被两个进程争抢。
set -euo pipefail

RUNNER_DIR="${1:?用法: start-host-runner.sh <runner目录> <act_runner二进制> [日志文件]}"
RUNNER_BIN="${2:?缺少 act_runner 二进制路径}"
LOG_FILE="${3:-$RUNNER_DIR/daemon.log}"

[ -d "$RUNNER_DIR" ] || { echo "runner 目录不存在: $RUNNER_DIR" >&2; exit 1; }
[ -x "$RUNNER_BIN" ] || { echo "act_runner 不可执行: $RUNNER_BIN" >&2; exit 1; }
[ -f "$RUNNER_DIR/.runner" ] || {
  echo "缺少 $RUNNER_DIR/.runner —— 先注册 runner（register --labels <label>:host）" >&2
  exit 1
}

# 该机的 ci 布局；未显式给出时按 <data>/toolchain, <cache> 推导。
export CI_DATA_ROOT="${CI_DATA_ROOT:-/data/nvme1/gitea-ci}"
export CI_CACHE_ROOT="${CI_CACHE_ROOT:-/data/nvme0/ci-cache}"
export CI_CARGO_HOME="${CI_CARGO_HOME:-$CI_DATA_ROOT/toolchain/cargo}"
export CI_RUSTUP_HOME="${CI_RUSTUP_HOME:-$CI_DATA_ROOT/toolchain/rustup}"
export CI_ZIG_DIR="${CI_ZIG_DIR:-$CI_DATA_ROOT/toolchain/zig}"

if pgrep -f "$RUNNER_BIN daemon" >/dev/null 2>&1; then
  echo "已有实例在运行（$RUNNER_BIN daemon），先停掉再启动："
  pgrep -af "$RUNNER_BIN daemon" || true
  echo
  echo "注意：如果正在跑 job，重启会直接杀死它（日志表现为停在编译中间且无 error）。"
  exit 1
fi

echo "CI_DATA_ROOT = $CI_DATA_ROOT"
echo "CI_CACHE_ROOT= $CI_CACHE_ROOT"
echo "CARGO_HOME   = $CI_CARGO_HOME"
echo "日志         = $LOG_FILE"

cd "$RUNNER_DIR"
setsid nohup "$RUNNER_BIN" daemon --config config.yaml >>"$LOG_FILE" 2>&1 </dev/null &
sleep 5

if pgrep -f "$RUNNER_BIN daemon" >/dev/null 2>&1; then
  echo "已启动："
  pgrep -af "$RUNNER_BIN daemon" || true
  echo "--- 最近日志 ---"
  tail -3 "$LOG_FILE"
else
  echo "启动失败，看日志：$LOG_FILE" >&2
  tail -20 "$LOG_FILE" >&2 || true
  exit 1
fi
