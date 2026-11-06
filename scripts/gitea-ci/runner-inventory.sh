#!/usr/bin/env bash
#
# Reports what a CI runner can do. The Gitea smoke pipeline calls this so a
# mis-provisioned machine fails with a readable inventory instead of a build
# error twenty minutes in.
#
# Exit status is non-zero only when something the real build needs is missing
# in a way that is not simply "this target does not use it".

set -euo pipefail

missing=0
report() {
  local tool="$1"
  if command -v "${tool}" >/dev/null 2>&1; then
    printf '%-14s %s\n' "${tool}" "$(command -v "${tool}")"
    if "${tool}" --version >/dev/null 2>&1; then
      "${tool}" --version 2>&1 | head -1 | sed 's/^/               /'
    fi
  else
    printf '%-14s MISSING\n' "${tool}"
  fi
}

echo "os:      $(uname -srm)"
echo "cpus:    $(getconf _NPROCESSORS_ONLN 2>/dev/null || echo unknown)"
echo "memory:  $(awk '/MemTotal/ {printf "%.1f GiB", $2/1048576}' /proc/meminfo 2>/dev/null || echo unknown)"
echo "disk:    $(df -h "$PWD" | awk 'NR==2 {print $4 " free on " $6}')"
echo

for tool in git curl python3 node npm rustc cargo rustup strip; do
  report "${tool}"
done

# Zig is only needed for the musl targets; the build installs it itself when
# absent, so a missing zig is reported but not fatal.
if ! command -v zig >/dev/null 2>&1; then
  printf '%-14s not installed (installed by the musl build step)\n' zig
fi

# The rust toolchain must match rust-toolchain.toml when one is present.
if [ -f rust-toolchain.toml ]; then
  expected="$(python3 - <<'PY'
import pathlib, tomllib
print(tomllib.loads(pathlib.Path("rust-toolchain.toml").read_text())["toolchain"]["channel"])
PY
)"
  actual="$(rustc --version 2>/dev/null | awk '{print $2}')"
  printf '\nexpected rust: %s\nactual rust:   %s\n' "${expected}" "${actual:-missing}"
  # Advisory only: the release workflow installs the pinned toolchain itself,
  # so a machine without Rust is still a valid runner.
  case "${actual:-}" in
    "${expected}"*) ;;
    *) echo "note: rust toolchain differs from rust-toolchain.toml (the build installs it)" >&2 ;;
  esac
else
  echo "warning: no rust-toolchain.toml at the checkout root" >&2
fi

# python3 is required by the packaging step, so its absence IS fatal.
if command -v python3 >/dev/null 2>&1; then
  printf '%-14s %s\n' python3-ok "$(python3 -c 'import sys; print(sys.version.split()[0])')"
else
  echo "python3 is required by the release packaging step" >&2
  missing=1
fi

exit "${missing}"
