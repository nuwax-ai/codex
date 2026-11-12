#!/usr/bin/env bash
# 在**新机器**上装 Gitea 服务端（单机 compose），并给出注册 runner 的后续步骤。
#
#   sudo ./gitea-server.sh --data-dir /srv/gitea --port 3301 --ssh-port 2223
#
# 实际安装逻辑复用 scripts/gitea-ci/install-gitea.sh（那边维护 compose/卷/初始 admin）。
# 本脚本只做环境检查 + 调用 + 把必须记住的三件事打在最后。
set -euo pipefail

DATA_DIR=""; PORT=3301; SSH_PORT=2223
while [ $# -gt 0 ]; do
  case "$1" in
    --data-dir) DATA_DIR="${2:?}"; shift 2 ;;
    --port) PORT="${2:?}"; shift 2 ;;
    --ssh-port) SSH_PORT="${2:?}"; shift 2 ;;
    -h|--help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "未知参数: $1" >&2; exit 1 ;;
  esac
done
[ -n "$DATA_DIR" ] || { echo "缺少 --data-dir" >&2; exit 1; }

command -v docker >/dev/null || { echo "需要先装 Docker" >&2; exit 1; }
docker info >/dev/null 2>&1 || { echo "当前用户无法访问 docker daemon（加 docker 组或用 sudo）" >&2; exit 1; }

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
echo "[gitea-server] 调用 $repo_root/scripts/gitea-ci/install-gitea.sh"
bash "$repo_root/scripts/gitea-ci/install-gitea.sh" \
  --data-dir "$DATA_DIR" --port "$PORT" --ssh-port "$SSH_PORT"

host_ip="$(ip route get 1.1.1.1 2>/dev/null | awk '{print $7; exit}')"
cat <<EOF

[gitea-server] 安装结束。三件必须确认的事：

1) ROOT_URL 用**主机 IP**，不要用 compose 服务名：
     http://${host_ip:-<主机IP>}:$PORT
   否则 job 容器里解析不到 gitea 这个名字（fatal: Could not resolve host）。

2) DEFAULT_ACTIONS_URL 必须是简写 github（1.23 只接受这个写法）。

3) 生成 runner 注册 token：
     docker exec -u git <gitea容器> gitea actions generate-runner-token

然后到每台构建机执行：
     sudo gitea-ops/bootstrap/linux-build-host.sh \
       --data-root <该机data> --cache-root <该机cache> \
       --runner-name <名字> --instance http://${host_ip:-<主机IP>}:$PORT --token <token>
EOF
