#!/usr/bin/env bash
#
# Bring up a throwaway Gitea + act_runner and verify that this fork's
# Gitea-side workflow actually runs, end to end, on a local machine.
#
#   bash scripts/gitea-ci/verify-local.sh
#
# It deliberately does NOT touch the GitHub repository or its workflows:
# everything happens in a temporary Gitea repository that only receives
# `.gitea/workflows/smoke.yml` and the small script it calls.
#
# Requirements: docker + docker compose, curl, git, python3 (for JSON).
# Leave OUTSIDE_DOCKER=1 unset to use the containerised runner in
# docker-compose.yml; set it to register a host-mode runner instead.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
compose=(docker compose -f "${repo_root}/scripts/gitea-ci/docker-compose.yml")
base_url="http://localhost:3301"
admin_user="ci"
admin_pass="ci-local-test-only"
slug="ci-smoke"

log() { printf '\n=== %s\n' "$*"; }

cleanup() {
  if [ "${KEEP_GITEA:-0}" = "1" ]; then
    log "KEEP_GITEA=1: leaving the stack running (${base_url})"
    return
  fi
  log "tearing down"
  RUNNER_TOKEN=unused "${compose[@]}" down -v >/dev/null 2>&1 || true
}
trap cleanup EXIT

log "starting gitea"
RUNNER_TOKEN=bootstrap "${compose[@]}" up -d gitea

log "waiting for gitea"
for _ in $(seq 1 60); do
  if curl -fsS "${base_url}/api/healthz" >/dev/null 2>&1; then break; fi
  sleep 2
done
curl -fsS "${base_url}/api/healthz" >/dev/null || { echo "gitea did not become healthy" >&2; exit 1; }

log "creating admin user ${admin_user}"
docker exec gitea-ci gitea admin user create \
  --username "${admin_user}" \
  --password "${admin_pass}" \
  --email "${admin_user}@example.invalid" \
  --admin --must-change-password=false >/dev/null 2>&1 || true

log "registering runner"
runner_token="$(docker exec gitea-ci gitea actions generate-runner-token | tail -1 | tr -d '\r')"
[ -n "${runner_token}" ] || { echo "could not generate a runner token" >&2; exit 1; }
RUNNER_TOKEN="${runner_token}" "${compose[@]}" up -d runner

log "creating ${slug} repository"
curl -fsS -u "${admin_user}:${admin_pass}" -X POST "${base_url}/api/v1/user/repos" \
  -H 'Content-Type: application/json' \
  -d "{\"name\":\"${slug}\",\"private\":true,\"auto_init\":false}" >/dev/null 2>&1 || true

log "pushing the smoke workflow"
work="$(mktemp -d)"
pushd "${work}" >/dev/null
git init -q -b main
git config user.email ci@example.invalid
git config user.name ci
mkdir -p .gitea/workflows scripts/gitea-ci
cp "${repo_root}/.gitea/workflows/smoke.yml" .gitea/workflows/
cp "${repo_root}/scripts/gitea-ci/runner-inventory.sh" scripts/gitea-ci/
git add -A
git commit -qm "ci: smoke workflow"
git remote add origin "http://${admin_user}:${admin_pass}@localhost:3301/${admin_user}/${slug}.git"
git push -q -fu origin main
popd >/dev/null

log "waiting for the workflow run"
run_json=""
for _ in $(seq 1 90); do
  run_json="$(curl -fsS -u "${admin_user}:${admin_pass}" \
    "${base_url}/api/v1/repos/${admin_user}/${slug}/actions/tasks?limit=1" || true)"
  status="$(printf '%s' "${run_json}" | python3 -c '
import json,sys
try:
    tasks=json.load(sys.stdin).get("workflow_runs") or json.load(sys.stdin).get("tasks") or []
except Exception:
    tasks=[]
if not tasks:
    print("none")
else:
    t=tasks[0]
    print(t.get("status") or t.get("conclusion") or "unknown")
' 2>/dev/null || echo none)"
  echo "  task status: ${status}"
  case "${status}" in
    success) echo; echo "SMOKE OK"; exit 0 ;;
    failure|cancelled|error) echo; echo "SMOKE FAILED" >&2; printf '%s\n' "${run_json}" | head -40 >&2; exit 1 ;;
  esac
  sleep 5
done

echo "SMOKE TIMEOUT: no terminal task status" >&2
exit 1
