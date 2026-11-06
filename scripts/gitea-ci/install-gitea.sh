#!/usr/bin/env bash
#
# Install the Gitea + act_runner stack on a Linux host, keeping every byte of
# state on a chosen data directory (point it at the big disk).
#
#   sudo bash install-gitea.sh --data-dir /mnt/ci/gitea --port 3301
#
# Non-destructive: it never formats or mounts a device. Mount your 2 TB disk
# first (see README.md, "使用第二块盘"), then pass the mount point here.
#
# What it does:
#   1. checks docker + compose are usable
#   2. creates <data-dir>/{gitea,runner,artifacts} owned by the current user
#   3. writes a compose file pinned to those paths and starts both services
#   4. prints the admin credentials, the runner token and the web URL

set -euo pipefail

data_dir="/srv/gitea-ci"
port="3301"
ssh_port="2223"
admin_user="ci"
admin_pass="${GITEA_ADMIN_PASSWORD:-}"
compose_src="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/docker-compose.yml"

usage() {
  cat <<'EOF'
usage: install-gitea.sh [--data-dir DIR] [--port PORT] [--ssh-port PORT]
                        [--admin-user USER] [--admin-password PASS]

  --data-dir DIR        where gitea data, runner state and CI artifacts live
                        (default /srv/gitea-ci; put this on the 2 TB disk)
  --port PORT           web port (default 3301)
  --ssh-port PORT       git-over-ssh port (default 2223)
  --admin-user USER     initial admin login (default ci)
  --admin-password PASS admin password (default: generated, printed once)
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --data-dir) data_dir="$2"; shift 2 ;;
    --port) port="$2"; shift 2 ;;
    --ssh-port) ssh_port="$2"; shift 2 ;;
    --admin-user) admin_user="$2"; shift 2 ;;
    --admin-password) admin_pass="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage; exit 2 ;;
  esac
done

die() { echo "error: $*" >&2; exit 1; }

command -v docker >/dev/null 2>&1 || die "docker is not installed (apt-get install docker.io docker-compose-v2)"
docker info >/dev/null 2>&1 || die "cannot talk to the docker daemon as $(id -un); re-run with sudo or add the user to the docker group"
docker compose version >/dev/null 2>&1 || die "docker compose plugin is missing"

[ -n "${admin_pass}" ] || admin_pass="$(head -c 24 /dev/urandom | base64 | tr -d '/+=' | head -c 24)"

echo "==> preparing ${data_dir}"
mkdir -p "${data_dir}/gitea" "${data_dir}/runner" "${data_dir}/artifacts"
# The gitea image runs as uid 1000 inside the container.
if [ "$(id -u)" = "0" ]; then
  chown -R 1000:1000 "${data_dir}/gitea" || true
fi
avail="$(df -h --output=avail "${data_dir}" | tail -1 | tr -d ' ')"
echo "==> ${data_dir} has ${avail} available"

compose_file="${data_dir}/docker-compose.yml"
python3 - "${compose_src}" "${compose_file}" "${data_dir}" "${port}" "${ssh_port}" <<'PY'
import pathlib, sys
src, dst, data_dir, port, ssh_port = sys.argv[1:6]
text = pathlib.Path(src).read_text()
text = text.replace('"3301:3000"', f'"{port}:3000"')
text = text.replace('"2223:22"', f'"{ssh_port}:22"')
text = text.replace('GITEA__server__SSH_PORT: "2223"', f'GITEA__server__SSH_PORT: "{ssh_port}"')
text = text.replace('http://localhost:3301/', f'http://localhost:{port}/')
text = text.replace("      - gitea-data:/data", f"      - {data_dir}/gitea:/data")
text = text.replace("      - runner-data:/data", f"      - {data_dir}/runner:/data")
text = text.replace("volumes:\n  gitea-data:\n  runner-data:\n", "")
pathlib.Path(dst).write_text(text)
PY

echo "==> starting gitea"
cd "${data_dir}"
RUNNER_TOKEN=bootstrap docker compose -f docker-compose.yml up -d gitea

for _ in $(seq 1 60); do
  curl -fsS "http://localhost:${port}/api/healthz" >/dev/null 2>&1 && break
  sleep 2
done
curl -fsS "http://localhost:${port}/api/healthz" >/dev/null 2>&1 ||
  die "gitea did not become healthy; see: docker logs gitea-ci"

echo "==> creating admin user ${admin_user}"
docker exec gitea-ci gitea admin user create \
  --username "${admin_user}" --password "${admin_pass}" \
  --email "${admin_user}@example.invalid" --admin --must-change-password=false \
  >/dev/null 2>&1 || echo "    (user may already exist)"

echo "==> registering runner"
runner_token="$(docker exec gitea-ci gitea actions generate-runner-token | tail -1 | tr -d '\r')"
RUNNER_TOKEN="${runner_token}" docker compose -f docker-compose.yml up -d runner

cat <<EOF

Gitea is up.

  web:      http://$(hostname -I | awk '{print $1}'):${port}
  admin:    ${admin_user} / ${admin_pass}
  compose:  ${compose_file}
  data:     ${data_dir}  (gitea/, runner/, artifacts/)

Next:
  1. create a repository and push this checkout to it
  2. watch the smoke pipeline in the repo's Actions tab
  3. artifacts/ is the hand-off directory for release archives
EOF
