#!/usr/bin/env bash
#
# Tie a CI host's vendored cargo sources to the checkout's Cargo.lock.
#
#   bash sync-vendor.sh --repo /path/to/codex-rs --vendor /data/.../ci-vendor --check
#   bash sync-vendor.sh --repo /path/to/codex-rs --vendor /data/.../ci-vendor \
#        --cargo-home /data/.../toolchain/cargo --toolchain 1.99.1
#
# Why this exists: `vendor-deps.sh` produces a snapshot by hand. Nothing tied
# that snapshot to Cargo.lock, so after the rig 0.46 -> 0.47 bump the three
# Linux release jobs failed ~40 minutes in with
#   failed to select a version for the requirement `rig-core = "=0.47"`
# while the workflow's precheck — a grep for "vendored-sources" — passed.
#
# The two halves run where each actually can:
#
#   --check    any host, including the CI box. Resolves the lockfile offline
#              against the vendor dir in seconds and names the exact crate
#              that is missing or at the wrong version. Put it in the job
#              before the build so a stale dir fails fast.
#
#   default    a host that can reach every git source, or whose cargo cache
#              already holds them. Re-vendors the checkout into a staging
#              dir, verifies that offline, then swaps it in.
#
# Why the refresh cannot run on the CI box even with a proxy:
# `cargo vendor` does not honour `replace-with` for git sources. It opens a
# real git database for every git dependency just to load the lockfile
# ("Updating git repository ..."), and rusty_v8 carries ~20 submodules (v8,
# LLVM, libc++), one on chromium.googlesource.com. Verified 2026-11-24: with
# every git source redirected to the vendor dir, `cargo metadata --offline`
# resolved in 3 s while `cargo vendor` started cloning. On the CI host that
# clone ran >15 min and was still fetching submodules; on a dev box whose
# cargo cache already held the pinned rev (19 of 20 submodules present) the
# same `cargo vendor` finished in 13 s for all 1328 crates.
#
# Refresh safety: the new tree is built beside the live one and verified
# offline before the swap; any failure leaves the live dir untouched.
# Concurrent callers serialize on a lock next to the vendor dir.
#
# Environment:
#   CI_VENDOR_MIRROR  optional registry mirror, used only when set explicitly
#                     (e.g. https://rsproxy.cn where crates.io answers 403).
#   CI_VENDOR_PROXY   optional HTTP proxy for the refresh only.

set -euo pipefail

repo=""
vendor=""
cargo_home=""
toolchain="${RUSTUP_TOOLCHAIN:-}"
check_only=0

while [ $# -gt 0 ]; do
  case "$1" in
    --repo) repo="$2"; shift 2 ;;
    --vendor) vendor="$2"; shift 2 ;;
    --cargo-home) cargo_home="$2"; shift 2 ;;
    --toolchain) toolchain="$2"; shift 2 ;;
    --check) check_only=1; shift ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

[ -n "${repo}" ] && [ -n "${vendor}" ] || {
  echo "error: --repo and --vendor are required" >&2; exit 2; }
# Put the toolchain on PATH instead of locating a single binary: the rustup
# proxies (cargo -> rustc) resolve sibling components through PATH, so calling
# cargo by absolute path alone fails with "could not execute process `rustc
# -vV`". In a CI job this script runs before the toolchain dir is added to
# PATH, which must not read as a broken vendor dir — so if no toolchain can be
# found, exit with a distinct code and say so.
toolchain_bin=""
for c in "${CARGO_HOME:-}/bin" "${RUSTUP_HOME:-}/../cargo/bin" \
         /data/nvme1/gitea-ci/toolchain/cargo/bin "$HOME/.cargo/bin"; do
  if [ -n "${c}" ] && [ -x "${c}/cargo" ] && [ -x "${c}/rustc" ]; then
    toolchain_bin="${c}"; break
  fi
done
if [ -z "${toolchain_bin}" ] && command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1; then
  toolchain_bin="$(dirname "$(command -v cargo)")"
fi
if [ -z "${toolchain_bin}" ]; then
  echo "sync-vendor: no rust toolchain found (looked in \$CARGO_HOME/bin," >&2
  echo "sync-vendor: \$RUSTUP_HOME/../cargo/bin, \$HOME/.cargo/bin, PATH)." >&2
  echo "sync-vendor: set CARGO_HOME/RUSTUP_HOME; this is a toolchain lookup" >&2
  echo "sync-vendor: failure, not a vendor-dir problem." >&2
  exit 3
fi
PATH="${toolchain_bin}:${PATH}"
# rustup proxies need RUSTUP_HOME to locate a toolchain; default to the CI layout.
export RUSTUP_HOME="${RUSTUP_HOME:-/data/nvme1/gitea-ci/toolchain/rustup}"
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-${CI_RUST_TOOLCHAIN:-1.99.1}}"

[ -f "${repo}/Cargo.lock" ] || {
  echo "error: no Cargo.lock in ${repo}" >&2; exit 2; }

# Write a cargo config that redirects every source Cargo.lock names to the
# vendor dir. The git sources are read from the lockfile instead of hardcoded:
# the host config and any hand-maintained copy both drifted behind the lock
# (5 of 9 sources listed on 2026-11-23), which is the failure this script
# exists to prevent.
write_redirect_config() {
  local home="$1" dir="$2"
  mkdir -p "${home}"
  {
    printf '[source.crates-io]\nreplace-with = "vendored-sources"\n\n'
    # A git source needs its own location too: `git = <url>`, `rev = <rev>`.
    # `cargo vendor` writes exactly these blocks; omitting them makes cargo
    # reject the config ("no source location specified").
    grep -oE 'git\+https://[^"# ]+\?rev=[0-9a-f]{7,40}' "${repo}/Cargo.lock" \
      | sort -u \
      | while IFS= read -r src; do
          url="${src#git+}"; url="${url%%\?rev=*}"
          rev="${src##*\?rev=}"
          printf '[source.%s]\n' "\"${src}\""
          printf 'git = "%s"\nrev = "%s"\nreplace-with = "vendored-sources"\n\n' "${url}" "${rev}"
        done
    printf '[source.vendored-sources]\ndirectory = "%s"\n' "${dir}"
  } > "${home}/config.toml"
}

# Resolve the lockfile offline against the vendor dir. Prints cargo's
# diagnostic on failure; succeeds quietly.
resolve_offline() {
  local dir="$1" home
  home="$(mktemp -d)"
  trap 'rm -rf "${home}"' RETURN
  write_redirect_config "${home}" "${dir}"
  local out
  if ! out="$(cd "${repo}" && CARGO_HOME="${home}" cargo metadata --locked --offline --format-version 1 2>&1 >/dev/null)"; then
    printf '%s\n' "${out}"
    return 1
  fi
}

if [ "${check_only}" = 1 ]; then
  if resolve_offline "${vendor}"; then
    echo "vendor dir satisfies Cargo.lock: ${vendor}"
  else
    echo "vendor dir does NOT satisfy Cargo.lock: ${vendor}" >&2
    exit 1
  fi
  exit 0
fi

# ── refresh ────────────────────────────────────────────────────────────────
# Must run where every git source is reachable (or already in the cargo
# cache): `cargo vendor` opens a git database per git dependency regardless of
# any `replace-with` redirect. See the header.
[ -n "${cargo_home}" ] || { echo "error: --cargo-home is required to refresh" >&2; exit 2; }
[ -n "${toolchain}" ] || { echo "error: --toolchain is required (or set RUSTUP_TOOLCHAIN)" >&2; exit 2; }
[ -d "${vendor}" ] || { echo "error: no vendor directory at ${vendor}" >&2; exit 1; }
[ -x "$(command -v flock)" ] || { echo "error: flock is required (util-linux)" >&2; exit 2; }

parent="$(dirname "${vendor}")"
name="$(basename "${vendor}")"
lock="${vendor}.lock"
mkdir -p "${parent}"
exec 9>"${lock}"
echo "==> waiting for the vendor lock ${lock}"
flock 9

if resolve_offline "${vendor}"; then
  echo "==> already up to date; nothing to do"
  exit 0
fi

staging="$(mktemp -d "${parent}/.vendor-staging.XXXXXX")"
cleanup() { rm -rf "${staging}"; rm -f "${lock}"; }
trap cleanup EXIT
staging_vendor="${staging}/${name}"

export RUSTUP_TOOLCHAIN="${toolchain}"
export CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse
mirror="${CI_VENDOR_MIRROR:-https://rsproxy.cn}"
if [ -n "${CI_VENDOR_MIRROR:-}" ]; then
  # Only honour an explicit mirror: an implicit one would silently bypass a
  # host's normal registry configuration.
  export CARGO_NET_GIT_FETCH_WITH_CLI=true
  mkdir -p "${CARGO_HOME:-$HOME/.cargo}"
  # shellcheck disable=SC2155
  export CARGO_HOME="$(mktemp -d "${parent}/.cargo-refresh.XXXXXX")"
  trap 'rm -rf "${staging}" "${CARGO_HOME}"; rm -f "${lock}"' EXIT
  mkdir -p "${CARGO_HOME}"
  printf '[source.crates-io]\nreplace-with = "mirror"\n\n[source.mirror]\nregistry = "%s"\n' "${mirror}" > "${CARGO_HOME}/config.toml"
fi
if [ -n "${CI_VENDOR_PROXY:-}" ]; then
  export https_proxy="${CI_VENDOR_PROXY}" http_proxy="${CI_VENDOR_PROXY}"
  echo "==> using proxy ${CI_VENDOR_PROXY} for the refresh"
fi

echo "==> vendoring ${repo} into ${staging_vendor}"
if ! ( cd "${repo}" && cargo vendor --versioned-dirs "${staging_vendor}" ) \
     > "${staging_vendor}/config.generated.toml"; then
  echo "cargo vendor failed; the live vendor dir is unchanged" >&2
  echo "note: this needs a host that can reach every git dependency's" >&2
  echo "      submodules (see the header); run --check on the CI box instead." >&2
  exit 1
fi

# Verify the staged result the way the build consumes it: fully offline.
resolve_offline "${staging_vendor}" \
  || { echo "refreshed vendor dir still fails to resolve; keeping the old one" >&2; exit 1; }

old="${staging}/old-${name}"
echo "==> swapping ${vendor} -> staged copy"
mv "${vendor}" "${old}"
mv "${staging_vendor}" "${vendor}"
chmod -R a+rX "${vendor}"
echo "==> done: $(ls -d "${vendor}"/*/ | wc -l) crates, $(du -sh "${vendor}" | cut -f1)"

here="$(cd "$(dirname "$0")" && pwd)"
bash "${here}/vendor-deps.sh" --install "${vendor}" --cargo-home "${cargo_home}" >/dev/null
echo "==> cargo config rewritten in ${cargo_home}/config.toml"
