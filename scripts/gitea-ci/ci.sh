#!/usr/bin/env bash
#
# Drive the self-hosted Gitea CI for this repository.
#
#   scripts/gitea-ci/ci.sh check                 # instance, runners, latest run
#   scripts/gitea-ci/ci.sh runs [N]              # recent runs (default 5)
#   scripts/gitea-ci/ci.sh smoke                 # push HEAD to the Gitea mirror → smoke pipeline
#   scripts/gitea-ci/ci.sh release v0.18.30      # push a tag → release-gitea pipeline
#   scripts/gitea-ci/ci.sh logs RUN              # decompress one run's job log (needs ssh)
#   scripts/gitea-ci/ci.sh artifacts             # list built archives on the host
#   scripts/gitea-ci/ci.sh inventory             # read-only host inventory over ssh
#   scripts/gitea-ci/ci.sh up|down               # start/stop the Gitea stack on the host
#
# Every value can be overridden from the environment or a .env next to this
# script (see .env.example). Nothing here writes to GitHub.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "${here}/../.." && pwd)"

# shellcheck disable=SC1091
[ -f "${here}/.env" ] && . "${here}/.env"

GITEA_URL="${GITEA_URL:-http://192.168.31.23:3301}"
GITEA_USER="${GITEA_USER:-ci}"
GITEA_PASS="${GITEA_PASS:-}"
GITEA_TOKEN="${GITEA_TOKEN:-}"
GITEA_REPO="${GITEA_REPO:-ci/ci-smoke}"        # owner/name of the Gitea copy
GITEA_REMOTE="${GITEA_REMOTE:-gitea}"          # git remote pointing at that copy
GITEA_BRANCH="${GITEA_BRANCH:-main}"
GITEA_SSH="${GITEA_SSH:-swufe@192.168.31.23}"  # for logs/artifacts/inventory
CI_DATA_ROOT="${CI_DATA_ROOT:-/data/nvme1/gitea-ci}"
CI_ARTIFACTS_DIR="${CI_ARTIFACTS_DIR:-${CI_DATA_ROOT}/artifacts}"

api() {
  local path="$1"
  if [ -n "${GITEA_TOKEN}" ]; then
    curl -fsS -H "Authorization: token ${GITEA_TOKEN}" "${GITEA_URL}/api/v1${path}"
  elif [ -n "${GITEA_PASS}" ]; then
    curl -fsS -u "${GITEA_USER}:${GITEA_PASS}" "${GITEA_URL}/api/v1${path}"
  else
    curl -fsS "${GITEA_URL}/api/v1${path}"
  fi
}

repo_api() { api "/repos/${GITEA_REPO}$1"; }

runs_json() { repo_api "/actions/tasks?limit=${1:-5}"; }

cmd_check() {
  printf 'instance : %s\n' "${GITEA_URL}"
  local health
  health="$(curl -fsS -o /dev/null -w '%{http_code}' "${GITEA_URL}/api/healthz" || true)"
  printf 'healthz  : %s\n' "${health:-unreachable}"
  [ "${health}" = "200" ] || { echo "Gitea is not answering; 'make gitea-up' after fixing access" >&2; return 1; }

  printf 'repo     : %s\n' "${GITEA_REPO}"
  local json
  json="$(mktemp)"
  runs_json 3 > "${json}"
  python3 - "${json}" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
runs = d.get("workflow_runs") or []
print(f"runs     : {d.get('total_count', len(runs))} total; latest:")
for run in runs[:3]:
    print(f"           #{run['run_number']:<4} {run['workflow_id']:<18} {run['status']}")
PY
  rm -f "${json}"
  if command -v ssh >/dev/null 2>&1 && [ -n "${GITEA_SSH}" ]; then
    printf 'artifacts: '
    ssh -o BatchMode=yes -o ConnectTimeout=4 "${GITEA_SSH}" \
      "du -sh ${CI_ARTIFACTS_DIR} 2>/dev/null | cut -f1" 2>/dev/null || echo "(ssh unavailable)"
  fi
}

cmd_runs() {
  local limit="${1:-5}" json
  json="$(mktemp)"
  runs_json "${limit}" > "${json}"
  python3 - "${json}" <<'PY'
import json, sys
from collections import OrderedDict

tasks = json.load(open(sys.argv[1])).get("workflow_runs") or []
if not tasks:
    print("no runs yet")
    raise SystemExit(0)

# Gitea's tasks endpoint returns one entry per JOB, so runs repeat. Collapse
# them: a run is failed if any of its jobs failed, running while any job runs.
runs = OrderedDict()
for t in tasks:
    key = (t["workflow_id"], t["run_number"])
    entry = runs.setdefault(key, {"jobs": [], "title": t.get("display_title", ""), "event": t.get("event", "")})
    entry["jobs"].append(t["status"])

print(f"{'run':>4}  {'workflow':<14} {'status':<9} {'jobs':<10} title")
for (workflow, number), e in list(runs.items())[: int(sys.argv[2]) if len(sys.argv) > 2 else 10]:
    statuses = e["jobs"]
    if any(s in ("failure", "error") for s in statuses):
        status = "failure"
    elif any(s in ("running", "waiting") for s in statuses):
        status = "running"
    elif all(s == "success" for s in statuses):
        status = "success"
    else:
        status = statuses[0] if statuses else "?"
    ok = sum(1 for s in statuses if s == "success")
    print(f"{number:>4}  {workflow:<14} {status:<9} {ok}/{len(statuses):<8} {e['title'][:56]}")
PY
  rm -f "${json}"
}

need_remote() {
  case "${GITEA_REMOTE}" in
    *://*|*@*) return 0 ;;   # a URL can be pushed to directly
  esac
  git -C "${root}" remote get-url "${GITEA_REMOTE}" >/dev/null 2>&1 || {
    cat >&2 <<EOF
error: git remote '${GITEA_REMOTE}' is not configured.

Point one at the Gitea copy first, for example:

  git -C ${root} remote add ${GITEA_REMOTE} ${GITEA_URL}/${GITEA_REPO}.git

and make sure it already has content (push once by hand).
EOF
    return 1
  }
}

cmd_smoke() {
  need_remote
  echo "==> pushing $(git -C "${root}" rev-parse --short HEAD) to ${GITEA_REMOTE}/${GITEA_BRANCH}"
  git -C "${root}" push "${GITEA_REMOTE}" "HEAD:${GITEA_BRANCH}"
  echo "==> waiting for the smoke run"
  local before after
  local probe
  probe="$(mktemp)"
  runs_json 1 > "${probe}"
  before="$(python3 - "${probe}" <<'PY'
import json, sys
r = (json.load(open(sys.argv[1])).get("workflow_runs") or [{}])[0]
print(r.get("run_number", 0))
PY
)"
  for _ in $(seq 1 40); do
    runs_json 1 > "${probe}"
    after="$(python3 - "${probe}" <<'PY'
import json, sys
r = (json.load(open(sys.argv[1])).get("workflow_runs") or [{}])[0]
print(r.get("run_number", 0), r.get("status", ""))
PY
)"
    set -- ${after}
    if [ "${1:-0}" != "${before}" ]; then
      echo "    run #$1: $2"
      case "$2" in
        success) echo "SMOKE OK"; rm -f "${probe}"; return 0 ;;
        failure|cancelled|error) echo "SMOKE FAILED (see 'make gitea-logs RUN=$1')" >&2; return 1 ;;
      esac
    fi
    sleep 6
  done
  rm -f "${probe}"
  echo "timed out waiting for a new run" >&2
  return 1
}

cmd_release() {
  local tag="${1:-}"
  [ -n "${tag}" ] || { echo "usage: ci.sh release <tag>   (e.g. v0.18.30)" >&2; return 2; }
  [[ "${tag}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "error: '${tag}' is not a stable vX.Y.Z tag" >&2; return 2; }
  need_remote
  if ! git -C "${root}" rev-parse -q --verify "refs/tags/${tag}" >/dev/null; then
    echo "==> tag ${tag} does not exist locally; creating it on $(git -C "${root}" rev-parse --short HEAD)"
    git -C "${root}" tag -a "${tag}" -m "Gitea CI build ${tag}"
  fi
  echo "==> pushing ${tag} to ${GITEA_REMOTE}"
  git -C "${root}" push "${GITEA_REMOTE}" "refs/tags/${tag}"
  echo "==> the release-gitea pipeline starts on that tag; watch it with:"
  echo "    make gitea-runs"
}

cmd_logs() {
  local run="${1:-}"
  [ -n "${run}" ] || { echo "usage: ci.sh logs <run-number>" >&2; return 2; }

  # Gitea names the log file after the task's global id; the per-repository
  # run_number shown in the UI is different, so resolve it through the API.
  local json task_id
  json="$(mktemp)"
  runs_json 100 > "${json}"
  task_id="$(python3 - "${json}" "${run}" <<'PY'
import json, sys
runs = json.load(open(sys.argv[1])).get("workflow_runs") or []
wanted = int(sys.argv[2])
for run in runs:
    if run.get("run_number") == wanted:
        print(run.get("id", ""))
        break
PY
)"
  rm -f "${json}"
  [ -n "${task_id}" ] || { echo "run ${run} not found in ${GITEA_REPO}" >&2; return 1; }

  ssh "${GITEA_SSH}" "f=\$(find ${CI_DATA_ROOT}/gitea/gitea/actions_log/${GITEA_REPO} -name '${task_id}.log.zst' | head -1);
    [ -n \"\$f\" ] || { echo 'no log file for task ${task_id} yet' >&2; exit 1; };
    zstd -dcq \"\$f\" | tail -n 80"
}

cmd_artifacts() {
  ssh "${GITEA_SSH}" "ls -la ${CI_ARTIFACTS_DIR} 2>/dev/null || echo 'no artifacts yet'; \
    du -sh ${CI_ARTIFACTS_DIR} 2>/dev/null | cut -f1"
}

cmd_inventory() {
  ssh "${GITEA_SSH}" 'bash -s' < "${here}/host-inventory.sh"
}

cmd_up() {
  ssh "${GITEA_SSH}" "cd ${CI_DATA_ROOT} && RUNNER_TOKEN=\$(docker exec -u git gitea-ci gitea actions generate-runner-token | tail -1) docker compose up -d && docker ps --format '{{.Names}}\t{{.Status}}' | grep gitea"
}

cmd_down() {
  ssh "${GITEA_SSH}" "cd ${CI_DATA_ROOT} && RUNNER_TOKEN=x docker compose down"
}

case "${1:-}" in
  check) cmd_check ;;
  runs) shift; cmd_runs "${1:-5}" ;;
  smoke) cmd_smoke ;;
  release) shift; cmd_release "${1:-}" ;;
  logs) shift; cmd_logs "${1:-}" ;;
  artifacts) cmd_artifacts ;;
  inventory) cmd_inventory ;;
  up) cmd_up ;;
  down) cmd_down ;;
  *)
    sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit 2 ;;
esac
