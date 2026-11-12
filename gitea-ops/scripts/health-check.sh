#!/usr/bin/env bash
# 巡检 Gitea CI：runner 可达性、负载、内存余量、是否有 job 在跑。
#
#   ./health-check.sh                 # 用 gitea-ops/.env 里的配置
#   HOSTS="a@1.2.3.4 b@5.6.7.8" ./health-check.sh
#
# 只读，不改任何东西。
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
[ -f "$here/../.env" ] && { set -a; . "$here/../.env"; set +a; }

HOSTS="${HOSTS:-}"   # 形如 "user@host1 user@host2"；留空则只查 Gitea API
warn=0

echo "== Gitea API =="
if [ -n "${GITEA_URL:-}" ] && [ -n "${GITEA_REPO:-}" ]; then
  out=$(curl -s -m 10 -u "${GITEA_USER}:${GITEA_PASS}" \
        "$GITEA_URL/api/v1/repos/$GITEA_REPO/actions/tasks?limit=30" 2>/dev/null)
  if [ -z "$out" ]; then
    echo "  ✗ 无法读取 $GITEA_URL（网络/凭据）"; warn=1
  else
    echo "$out" | python3 -c '
import sys, json, collections
d = json.load(sys.stdin)
rows = d.get("workflow_runs") or []
c = collections.Counter((r.get("name"), r.get("status")) for r in rows)
run = [(n, s) for (n, s), k in c.items() if s == "running"]
print(f"  最近 {len(rows)} 个任务，running={len(run)}")
for n, _ in sorted(run):
    print(f"    ▸ {n}")
'
  fi
else
  echo "  跳过（缺 GITEA_URL / GITEA_REPO）"
fi

echo "== 构建机 =="
if [ -z "$HOSTS" ]; then
  echo "  跳过（用 HOSTS=\"user@host ...\" 指定）"
else
  for h in $HOSTS; do
    printf '  %-28s ' "$h"
    info=$(ssh -o BatchMode=yes -o ConnectTimeout=8 "$h" \
      'printf "%s|%s|%s|%s|%s|%s" "$(cat /proc/loadavg | cut -d" " -f1)" \
         "$(free -g | sed -n 2p | awk "{print \$3}")" \
         "$(free -g | sed -n 2p | awk "{print \$2}")" \
         "$(pgrep -c rustc 2>/dev/null || echo 0)" \
         "$(command -v nuwax-upload >/dev/null 2>&1 && echo yes || echo no)" \
         "$(command -v aarch64-linux-gnu-strip >/dev/null 2>&1 && echo yes || echo no)"' 2>/dev/null)
    if [ -z "$info" ]; then
      echo "✗ SSH 不可达（若是 banner exchange 超时：机器被压垮，需重启并下调 capacity）"
      warn=1
      continue
    fi
    load=${info%%|*}; rest=${info#*|}
    used=${rest%%|*}; rest=${rest#*|}
    total=${rest%%|*}; rest=${rest#*|}
    rustc=${rest%%|*}; rest=${rest#*|}
    upload=${rest%%|*}; crossstrip=${rest#*|}
    pct=$(( total > 0 ? used * 100 / total : 0 ))
    flag="✓"
    [ "$pct" -ge 85 ] && { flag="⚠ 内存紧张"; warn=1; }
    # 缺发布工具时 job 能编译却发不出去（见 INCIDENTS.md §9/§10）
    [ "$upload" = yes ] || { flag="$flag ⚠ 缺 nuwax-upload"; warn=1; }
    [ "$crossstrip" = yes ] || { flag="$flag ⚠ 缺交叉 strip"; warn=1; }
    echo "load=$load  mem=${used}/${total}GB (${pct}%)  rustc=$rustc  $flag"
  done
fi

echo
[ "$warn" = 0 ] && echo "结论：未发现明显问题" || echo "结论：有告警，见上"
exit "$warn"
