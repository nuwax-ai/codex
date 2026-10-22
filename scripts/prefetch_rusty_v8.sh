#!/usr/bin/env bash
# Local rusty_v8 artifact prefetch entry (#24, 2026-10-22).
#
# Downloads (or reuses from a shared cache) the Codex-built rusty_v8 archive +
# src binding pair for one Rust target, verifying every byte against the
# in-repo trusted manifest third_party/v8/rusty_v8_<version>.sha256 before a
# file may be used. Mirrors the selection logic of
# .github/actions/setup-rusty-v8/action.yml (same version resolution via
# rusty_v8_bazel.py, same file naming, same ptrcomp_sandbox_release profile)
# so CI and local builds share one trust root; the action keeps its extra
# network-manifest layer for CI provenance, this script verifies against the
# committed manifest directly.
#
# Usage:
#   scripts/prefetch_rusty_v8.sh [--target <triple>] [--cache-dir <dir>]
#                                [--offline] [--env-file <path>] [--list]
#
#   --target     Rust target triple (default: rustc -vV host; x86_64-apple-darwin,
#                aarch64-unknown-linux-gnu, ...). No guessing from URL shapes.
#   --cache-dir  Shared cache (default: ${NUWAX_RUSTY_V8_CACHE:-~/.cache/nuwax/rusty_v8}).
#                Layout <cache>/<version>/<target>/<files>; safe to share across
#                checkouts and machines on the same fs.
#   --offline    Never touch the network. If a verified pair is already cached,
#                succeed; otherwise fail listing exactly which files are missing
#                and where they would come from.
#   --env-file   Additionally write a sourceable env file (RUSTY_V8_ARCHIVE=...,
#                RUSTY_V8_SRC_BINDING_PATH=...), matching the action's exports.
#                Load for Cargo with: set -a; source <path>; set +a
#   --list       Print the targets the trusted manifest covers; no downloads.
#
# Exit codes: 0 success (cache hit or verified download) · 2 checksum mismatch
# (bad file is deleted) · 3 offline with missing artifacts · 4 usage/manifest
# error. A failed verification never leaves a usable file behind.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION_HELPER="${REPO_ROOT}/.github/scripts/rusty_v8_bazel.py"
PROFILE="ptrcomp_sandbox_release"

TARGET=""
CACHE_DIR="${NUWAX_RUSTY_V8_CACHE:-}"
OFFLINE=0
ENV_FILE=""
LIST=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target) TARGET="${2:?}"; shift 2 ;;
    --cache-dir) CACHE_DIR="${2:?}"; shift 2 ;;
    --offline) OFFLINE=1; shift ;;
    --env-file) ENV_FILE="${2:?}"; shift 2 ;;
    --list) LIST=1; shift ;;
    -h|--help) sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 4 ;;
  esac
done

[[ -f "${VERSION_HELPER}" ]] || { echo "missing ${VERSION_HELPER}; run inside the codex checkout" >&2; exit 4; }

command -v python3 >/dev/null || { echo "python3 is required (manifest parsing)" >&2; exit 4; }

VERSION="$(python3 "${VERSION_HELPER}" resolved-v8-crate-version)" || {
  echo "could not resolve the rusty_v8 crate version" >&2; exit 4; }

MANIFEST="${REPO_ROOT}/third_party/v8/rusty_v8_${VERSION//./_}.sha256"
[[ -f "${MANIFEST}" ]] || { echo "trusted manifest ${MANIFEST} not found for v${VERSION}" >&2; exit 4; }

checksum_command=(sha256sum)
command -v sha256sum >/dev/null 2>&1 || checksum_command=(shasum -a 256)

expected_checksum() {
  # exactly one line for the name, else the manifest is ambiguous -> refuse
  awk -v name="$1" '$2 == name { print $1; count++ } END { if (count != 1) exit 1 }' "${MANIFEST}"
}

manifest_targets() {
  awk '$2 ~ /^librusty_v8_'"${PROFILE}"'_.*\.a\.gz$/ || $2 ~ /^rusty_v8_'"${PROFILE}"'_.*\.lib\.gz$/ {
    name = $2
    sub(/^librusty_v8_'"${PROFILE}"'_/, "", name); sub(/^rusty_v8_'"${PROFILE}"'_/, "", name)
    sub(/\.a\.gz$/, "", name); sub(/\.lib\.gz$/, "", name)
    print name
  }' "${MANIFEST}" | sort -u
}

if [[ "${LIST}" -eq 1 ]]; then
  echo "rusty_v8 v${VERSION} targets in ${MANIFEST#*third_party/}:"
  manifest_targets
  exit 0
fi

if [[ -z "${TARGET}" ]]; then
  command -v rustc >/dev/null 2>&1 || { echo "no --target given and rustc is unavailable to detect the host" >&2; exit 4; }
  TARGET="$(rustc -vV | sed -n 's/^host: //p')"
fi
if [[ -z "${TARGET}" || "${TARGET}" == *[[:space:]/]* ]]; then
  echo "invalid target triple: '${TARGET}'" >&2; exit 4
fi

if [[ "${TARGET}" == *-pc-windows-msvc ]]; then
  ARCHIVE_NAME="rusty_v8_${PROFILE}_${TARGET}.lib.gz"
else
  ARCHIVE_NAME="librusty_v8_${PROFILE}_${TARGET}.a.gz"
fi
BINDING_NAME="src_binding_${PROFILE}_${TARGET}.rs"

[[ -z "${CACHE_DIR}" ]] && CACHE_DIR="${HOME}/.cache/nuwax/rusty_v8"
# Keep exports valid after changing from the checkout root into codex-rs.
CACHE_DIR="$(python3 -c 'import os, sys; print(os.path.abspath(sys.argv[1]))' "${CACHE_DIR}")"
DEST_DIR="${CACHE_DIR}/${VERSION}/${TARGET}"
RELEASE_TAG="rusty-v8-v${VERSION}"
BASE_URL="https://github.com/openai/codex/releases/download/${RELEASE_TAG}"

for name in "${ARCHIVE_NAME}" "${BINDING_NAME}"; do
  expected_checksum "${name}" >/dev/null || {
    echo "the trusted manifest has no unique entry for ${name}; wrong target or stale manifest" >&2; exit 4; }
done

DOWNLOAD_TMP=""
cleanup_download() {
  if [[ -n "${DOWNLOAD_TMP}" ]]; then
    rm -f -- "${DOWNLOAD_TMP}"
  fi
}
trap cleanup_download EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Cache reuse requires re-verification: a cached file passes only if its bytes
# still match the committed manifest, so a corrupted or swapped cache never
# silently feeds a build.
fetch_file() {
  local name="$1" dest="${DEST_DIR}/$1"
  local expected
  expected="$(expected_checksum "${name}")"
  if [[ -f "${dest}" ]]; then
    if echo "${expected}  ${dest}" | "${checksum_command[@]}" --check - >/dev/null 2>&1; then
      echo "cache hit: ${dest}" >&2
      return 0
    fi
    echo "cached ${name} failed verification; deleting and refetching" >&2
    rm -f "${dest}"
  fi
  if [[ "${OFFLINE}" -eq 1 ]]; then
    echo "offline: ${name} is missing from ${DEST_DIR}" >&2
    echo "  would fetch: ${BASE_URL}/${name}" >&2
    MISSING=1
    return 0
  fi
  mkdir -p "${DEST_DIR}"
  # Concurrent checkouts must never write, verify, or remove one another's
  # partial download. Publish only this invocation's verified file atomically.
  DOWNLOAD_TMP="$(mktemp "${dest}.part.XXXXXXXX")"
  local tmp="${DOWNLOAD_TMP}"
  echo "downloading ${BASE_URL}/${name}" >&2
  curl -fsSL --retry 2 "${BASE_URL}/${name}" -o "${tmp}" || { rm -f "${tmp}"; echo "download failed: ${name}" >&2; exit 3; }
  if ! echo "${expected}  ${tmp}" | "${checksum_command[@]}" --check - >/dev/null 2>&1; then
    rm -f "${tmp}"
    echo "checksum mismatch for ${name} (expected ${expected}); refused" >&2
    exit 2
  fi
  mv "${tmp}" "${dest}"
  DOWNLOAD_TMP=""
}

MISSING=0
fetch_file "${ARCHIVE_NAME}"
fetch_file "${BINDING_NAME}"
if [[ "${MISSING}" -ne 0 ]]; then
  echo "offline mode cannot proceed: verified artifacts for ${TARGET} are not in the cache" >&2
  exit 3
fi

ARCHIVE_PATH="${DEST_DIR}/${ARCHIVE_NAME}"
BINDING_PATH="${DEST_DIR}/${BINDING_NAME}"

write_shell_env() {
  printf 'RUSTY_V8_ARCHIVE=%q\n' "${ARCHIVE_PATH}"
  printf 'RUSTY_V8_SRC_BINDING_PATH=%q\n' "${BINDING_PATH}"
}

if [[ -n "${ENV_FILE}" ]]; then
  write_shell_env > "${ENV_FILE}"
  echo "env file written: ${ENV_FILE}" >&2
fi

write_shell_env
