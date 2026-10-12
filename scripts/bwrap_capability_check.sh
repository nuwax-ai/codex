#!/usr/bin/env bash
# Probe namespace permissions using the system bwrap, or an explicitly selected
# executable. This is not complete Codex sandbox acceptance: Codex can select a
# bundled bwrap and also enforces filesystem, network-proxy and other policies.
# Each bwrap invocation is bounded by GNU timeout, including its --help probe.
#
# Exit codes: 0 probes passed; 1 missing executable/tool; 2 explicit baseline
# namespace permission denial; 3 explicit network loopback permission denial;
# 4 unexpected failure/incompatible bwrap; 5 probe timeout; 64 invalid arguments;
# 77 skipped because the host is not Linux.

set -uo pipefail

log() { printf '%s\n' "$*" >&2; }
usage() {
  cat <<'USAGE'
Usage: scripts/bwrap_capability_check.sh [-v] [--bwrap EXECUTABLE] [--timeout SECONDS]
  -v, --verbose       Print probe commands and captured output.
  --bwrap EXECUTABLE  Select a system or bundled executable; default: bwrap on PATH.
  --timeout SECONDS   Per-probe deadline, an integer from 1 to 60; default: 10.
  -h, --help          Show this help.

Requires Linux, GNU timeout and /bin/true. Passing probes only establishes the
selected bwrap's namespace capability; it does not establish Codex acceptance.
Exit codes: 0 pass, 1 missing tool, 2 baseline permission denial, 3 network
permission denial, 4 unexpected/incompatible, 5 timeout, 64 usage, 77 non-Linux.
USAGE
}

verbose=0
bwrap_program=bwrap
probe_timeout=10
while (( $# )); do
  case "$1" in
    -v|--verbose) verbose=1; shift ;;
    --bwrap|--timeout)
      option="$1"
      if (( $# < 2 )) || [[ -z "$2" || "$2" == -* ]]; then
        log "USAGE: ${option} requires a value."
        exit 64
      fi
      if [[ "$option" == --bwrap ]]; then
        bwrap_program="$2"
      else
        if [[ ! "$2" =~ ^([1-9]|[1-5][0-9]|60)$ ]]; then
          log "USAGE: --timeout must be an integer from 1 to 60."
          exit 64
        fi
        probe_timeout="$2"
      fi
      shift 2
      ;;
    -h|--help) usage; exit 0 ;;
    *) log "USAGE: unknown argument: $1"; usage >&2; exit 64 ;;
  esac
done

platform="$(uname -s)"
if [[ "$platform" != Linux ]]; then
  log "SKIP: not Linux (uname: ${platform}); no namespace probe was run."
  exit 77
fi

if ! bwrap_path="$(command -v -- "$bwrap_program")" || [[ ! -x "$bwrap_path" ]]; then
  log "MISSING: selected bwrap executable is unavailable: ${bwrap_program}."
  exit 1
fi
if ! command -v timeout >/dev/null 2>&1; then
  log "MISSING: GNU timeout is required to bound each probe."
  exit 1
fi
if [[ ! -x /bin/true ]]; then
  log "MISSING: /bin/true is unavailable."
  exit 1
fi
log "Selected bwrap: ${bwrap_path}; per-probe timeout: ${probe_timeout}s."

# Capture every invocation before verbose printing so diagnostics cannot change
# classification. GNU timeout's verbose kill diagnostic distinguishes a timed
# out command requiring SIGKILL from a command that dies by SIGKILL itself.
run_probe() {
  local probe_name="$1"
  shift
  if (( verbose )); then
    log "== ${probe_name}: ${bwrap_path} $*"
  fi
  probe_output="$(LC_ALL=C timeout --verbose --signal=TERM --kill-after=1s \
    "${probe_timeout}s" "$bwrap_path" "$@" 2>&1)"
  probe_status=$?
  if (( verbose || probe_status != 0 )); then
    log "${probe_name} output: ${probe_output}"
  fi
  if (( probe_status == 124 )) || {
    (( probe_status == 137 )) && [[ "$probe_output" == *"timeout: sending signal KILL to command"* ]]
  }; then
    log "TIMEOUT: ${probe_name} exceeded its ${probe_timeout}s deadline (status ${probe_status})."
    exit 5
  fi
}

run_probe help --help
if (( probe_status != 0 )); then
  log "UNEXPECTED: selected bwrap --help failed (status ${probe_status})."
  exit 4
fi
# Codex checks --as-pid-1 and --perms before selecting a system bwrap. Also
# verify every option used below rather than mistaking old CLI failures for
# kernel permission denials.
for required_option in --as-pid-1 --perms --new-session --die-with-parent \
  --unshare-user --unshare-ipc --unshare-pid --unshare-net --ro-bind --proc \
  --dev --cap-drop; do
  if ! grep -Eq -- "(^|[[:space:]])${required_option}([[:space:]=]|$)" <<< "$probe_output"; then
    log "INCOMPATIBLE: selected bwrap --help does not advertise ${required_option}."
    exit 4
  fi
done

# Mirror the common Codex namespace/launcher options, including --as-pid-1.
# Real Codex mount policies and managed proxy setup are outside this probe.
common_args=(--new-session --die-with-parent --unshare-user --unshare-ipc
  --unshare-pid --as-pid-1 --ro-bind / / --proc /proc --dev /dev --cap-drop ALL)
run_probe baseline "${common_args[@]}" -- /bin/true
if (( probe_status != 0 )); then
  if grep -Eq -- 'bwrap: (setting up (uid|gid) map: (Permission denied|Operation not permitted)|No permissions to create (a )?new namespace|Creating new namespace failed: (Operation not permitted|Permission denied))' <<< "$probe_output"; then
    log "DENIED: explicit baseline namespace permission failure (status ${probe_status})."
    exit 2
  fi
  log "UNEXPECTED: baseline failed without a namespace permission signature (status ${probe_status})."
  exit 4
fi

run_probe netns "${common_args[@]}" --unshare-net -- /bin/true
if (( probe_status != 0 )); then
  if grep -Eq -- 'bwrap: loopback: Failed RTM_NEW(ADDR|LINK): (Operation not permitted|Permission denied)' <<< "$probe_output"; then
    log "DENIED: explicit network namespace loopback permission failure (status ${probe_status})."
    exit 3
  fi
  log "UNEXPECTED: netns failed without a loopback permission signature (status ${probe_status})."
  exit 4
fi

log "OK: selected bwrap passed baseline and network namespace probes."
log "Filesystem policies, proxy setup and full Codex sandbox acceptance remain unverified."
exit 0
