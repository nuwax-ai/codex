#!/usr/bin/env bash
# 打 tag 推到 Gitea 触发 release-gitea，并轮询到 6 个构建任务出结果。
#
#   ./trigger-build.sh v0.18.48 [轮询间隔秒=240]
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
[ -f "$here/../.env" ] && { set -a; . "$here/../.env"; set +a; }
set -u

TAG="${1:?用法: trigger-build.sh <tag> [间隔秒]}"
INTERVAL="${2:-240}"
: "${GITEA_URL:?}" "${GITEA_REPO:?}" "${GITEA_REMOTE:=gitea}"

echo "[trigger] 推 tag $TAG"
git tag "$TAG" 2>/dev/null || echo "  (tag 已存在，直接推)"
git push "$GITEA_REMOTE" "$TAG" || { echo "推送失败" >&2; exit 1; }

while true; do
  sleep "$INTERVAL"
  out=$(curl -s -m 15 -u "${GITEA_USER}:${GITEA_PASS}" \
        "$GITEA_URL/api/v1/repos/$GITEA_REPO/actions/tasks?limit=40" | \
    python3 -c "
import sys,json
d=json.load(sys.stdin)
rows=[r for r in (d.get('workflow_runs') or []) if r.get('head_branch')=='$TAG' and r.get('name','').startswith('build ')]
done=[r for r in rows if r.get('status') in ('success','failure')]
print(f'{len(done)}/{len(rows)}', '|', '; '.join(f\"{r['name'].replace('build ','')}={r['status']}\" for r in sorted(rows,key=lambda x:x['name'])))
")
  echo "$(date +%H:%M:%S) $out"
  case "$out" in 6/6*) echo "[trigger] 全部完成"; break ;; esac
done
