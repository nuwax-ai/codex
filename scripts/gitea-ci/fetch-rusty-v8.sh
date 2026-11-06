#!/usr/bin/env bash
#
# Pre-seed rusty_v8 release archives on the CI host.
#
#   bash fetch-rusty-v8.sh --repo /path/to/checkout --target x86_64-unknown-linux-musl
#   bash fetch-rusty-v8.sh --repo /path/to/checkout --all-linux
#
# Why this exists: .github/actions/setup-rusty-v8 downloads the archives from
# GitHub Releases, and this network cannot reach github.com from CI jobs. The
# host itself can, so the archives are fetched once into
# $CACHE_ROOT/rusty_v8/<target>/ and the workflow points RUSTY_V8_ARCHIVE /
# RUSTY_V8_SRC_BINDING_PATH at them.
#
# The naming and the checksum verification mirror the GitHub action exactly: the
# manifest is checked against the trusted list committed under third_party/v8/.

set -euo pipefail

repo=""
targets=()
cache_root="${CI_CACHE_ROOT:-/data/nvme0/ci-cache}"
profile="ptrcomp_sandbox_release"

while [ $# -gt 0 ]; do
  case "$1" in
    --repo) repo="$2"; shift 2 ;;
    --target) targets+=("$2"); shift 2 ;;
    --all-linux) targets+=(x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-unknown-linux-gnu); shift ;;
    --cache-root) cache_root="$2"; shift 2 ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

[ -n "${repo}" ] || { echo "error: --repo <checkout> is required" >&2; exit 2; }
[ "${#targets[@]}" -gt 0 ] || { echo "error: give --target or --all-linux" >&2; exit 2; }

version="$(python3 "${repo}/.github/scripts/rusty_v8_bazel.py" resolved-v8-crate-version)"
base_url="https://github.com/openai/codex/releases/download/rusty-v8-v${version}"
trusted="${repo}/third_party/v8/rusty_v8_${version//./_}_release_manifests.sha256"
[ -f "${trusted}" ] || { echo "missing trusted checksum list: ${trusted}" >&2; exit 1; }

checksum_cmd=(sha256sum --check -)
command -v sha256sum >/dev/null 2>&1 || checksum_cmd=(shasum -a 256 --check -)

for target in "${targets[@]}"; do
  if [[ "${target}" == *-pc-windows-msvc ]]; then
    archive="rusty_v8_${profile}_${target}.lib.gz"
  else
    archive="librusty_v8_${profile}_${target}.a.gz"
  fi
  binding="src_binding_${profile}_${target}.rs"
  manifests="rusty_v8_${profile}_${target}.sha256"

  dest="${cache_root}/rusty_v8/${target}"
  mkdir -p "${dest}"
  echo "==> ${target} -> ${dest}"

  expected="$(awk -v name="${manifests}" '$2 == name { print $1; c++ } END { if (c != 1) exit 1 }' "${trusted}")" ||
    { echo "no unique trusted checksum for ${manifests}" >&2; exit 1; }

  curl -fsSL "${base_url}/${manifests}" -o "${dest}/${manifests}"
  actual="$(python3 -c 'import hashlib,pathlib,sys;print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())' "${dest}/${manifests}")"
  [ "${actual}" = "${expected}" ] || { echo "manifest checksum mismatch for ${target}" >&2; exit 1; }

  curl -fsSL "${base_url}/${archive}" -o "${dest}/${archive}"
  curl -fsSL "${base_url}/${binding}" -o "${dest}/${binding}"

  ( cd "${dest}" && tr -d '\r' < "${manifests}" | "${checksum_cmd[@]}" )
  echo "    ${archive} + ${binding} verified"
done
