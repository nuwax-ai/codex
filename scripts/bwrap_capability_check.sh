#!/usr/bin/env bash
# Classify whether this environment can run codex's Linux sandbox, and if
# not, exactly which capability boundary it hits.
#
# The codex Linux sandbox launches bubblewrap with (at least)
# --unshare-user --unshare-ipc --unshare-pid --unshare-net; bwrap brings the
# loopback interface up inside the new network namespace, which needs
# CAP_NET_ADMIN in that namespace. GitHub-hosted container runners allow
# user namespaces but deny that operation ("bwrap: loopback: Failed
# RTM_NEWADDR: Operation not permitted"), which is what the fork's CI logs
# show for every sandbox-dependent test (see
# my-docs/rig-production-completion-2026-10-10/ci-run6-attribution-2026-10-11.md).
#
# Exit codes:
#   0  full capability - sandbox-dependent tests should work
#   1  bwrap (or /bin/true) missing - install bubblewrap first
#   2  baseline user-namespace sandbox denied - userns blocked entirely
#   3  network namespace denied - the RTM_NEWADDR boundary (hosted runners);
#      sandbox tests will fail the same way CI does
#   4  unexpected failure - inspect the raw output printed above
#
# Usage: scripts/bwrap_capability_check.sh [-v]

set -uo pipefail

verbose=0
[[ "${1:-}" == "-v" ]] && verbose=1

log() { printf '%s\n' "$*" >&2; }

if [[ "$(uname -s)" != "Linux" ]]; then
  log "SKIP: not Linux (uname: $(uname -s)); the sandbox is Linux-only."
  exit 1
fi

if ! command -v bwrap >/dev/null 2>&1; then
  log "MISSING: bwrap not found in PATH; install the bubblewrap package."
  exit 1
fi

if [[ ! -x /bin/true ]]; then
  log "MISSING: /bin/true not available for probe commands."
  exit 1
fi

# Baseline: user + ipc + pid namespaces, no network namespace.
baseline_args=(
  --unshare-user
  --unshare-ipc
  --unshare-pid
  --ro-bind / /
  --proc /proc
  --dev /dev
  /bin/true
)

# The codex configuration adds the network namespace; bwrap then configures
# the namespace loopback, which is the operation hosted runners deny.
netns_args=(
  --unshare-user
  --unshare-ipc
  --unshare-pid
  --unshare-net
  --ro-bind / /
  --proc /proc
  --dev /dev
  /bin/true
)

if (( verbose )); then
  log "== baseline: bwrap ${baseline_args[*]}"
  bwrap "${baseline_args[@]}" 2>&1 | sed 's/^/  | /' >&2
  baseline_status=${PIPESTATUS[0]}
  log "== netns: bwrap ${netns_args[*]}"
  bwrap "${netns_args[@]}" 2>&1 | sed 's/^/  | /' >&2
  netns_status=${PIPESTATUS[0]}
else
  baseline_output="$(bwrap "${baseline_args[@]}" 2>&1)"
  baseline_status=$?
  netns_output="$(bwrap "${netns_args[@]}" 2>&1)"
  netns_status=$?
fi

if (( ! verbose )); then
  # Reproduce the failure text for the report even without -v.
  if (( baseline_status != 0 )); then
    log "baseline output: ${baseline_output}"
  fi
  if (( netns_status != 0 )); then
    log "netns output: ${netns_output}"
  fi
fi

if (( baseline_status != 0 )); then
  log "DENIED: the baseline user-namespace sandbox failed (status ${baseline_status})."
  log "User namespaces are blocked in this environment; no bwrap-based sandbox can run."
  exit 2
fi

if (( netns_status != 0 )); then
  if printf '%s' "${netns_output:-}" | grep -q "RTM_NEWADDR"; then
    log "DENIED: network namespace loopback setup failed (RTM_NEWADDR)."
    log "This is the GitHub-hosted-runner boundary: sandbox tests will fail here."
    log "Self-hosted/privileged runners or environments granting CAP_NET_ADMIN fix it."
    exit 3
  fi
  log "UNEXPECTED: the netns sandbox failed without the RTM_NEWADDR signature (status ${netns_status})."
  exit 4
fi

log "OK: user namespaces and the network namespace (with loopback) both work."
log "Sandbox-dependent tests should be viable on this environment."
exit 0
