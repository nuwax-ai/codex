#!/usr/bin/env bash
#
# Prepare a fully offline cargo dependency set for the CI host.
#
#   # on a machine that can reach crates.io / github / googlesource
#   bash vendor-deps.sh --repo /path/to/codex-rs --out /tmp/ci-vendor
#
#   # then, on the CI host
#   bash vendor-deps.sh --install /tmp/ci-vendor --cargo-home /data/nvme1/gitea-ci/toolchain/cargo
#
# Why: Cargo.lock pins `v8` to a git dependency whose checkout has a git
# submodule under chromium.googlesource.com, which this network cannot reach.
# `cargo vendor` copies every crate (including git dependencies) into a
# directory source, so neither the submodule fetch nor the registry is needed at
# build time. Combined with a pre-seeded RUSTY_V8_ARCHIVE (fetch-rusty-v8.sh)
# the whole build runs offline.

set -euo pipefail

mode=""
repo=""
out=""
cargo_home=""

while [ $# -gt 0 ]; do
  case "$1" in
    --repo) mode=vendor; repo="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --install) mode=install; out="$2"; shift 2 ;;
    --cargo-home) cargo_home="$2"; shift 2 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ "${mode}" = "vendor" ]; then
  [ -n "${repo}" ] && [ -n "${out}" ] || { echo "error: --repo and --out are required" >&2; exit 2; }
  mkdir -p "${out}"
  echo "==> vendoring ${repo} into ${out}"
  ( cd "${repo}" && cargo vendor --versioned-dirs "${out}" ) > "${out}/config.generated.toml"
  echo "==> vendored $(du -sh "${out}" | cut -f1); config at ${out}/config.generated.toml"
  exit 0
fi

[ "${mode}" = "install" ] || { echo "error: use --repo/--out or --install/--cargo-home" >&2; exit 2; }
[ -n "${out}" ] && [ -n "${cargo_home}" ] || { echo "error: --install and --cargo-home are required" >&2; exit 2; }
[ -d "${out}" ] || { echo "error: no vendor directory at ${out}" >&2; exit 1; }

mkdir -p "${cargo_home}"
config="${cargo_home}/config.toml"
echo "==> writing ${config}"

# Start from the replacements cargo printed, then point the directory source at
# the path this host actually uses.
python3 - "${out}" "${config}" <<'PY'
import pathlib, sys
vendor_dir, config_path = sys.argv[1], sys.argv[2]
generated = pathlib.Path(vendor_dir, "config.generated.toml")
text = generated.read_text() if generated.exists() else (
    '[source.crates-io]\nreplace-with = "vendored-sources"\n')
# Drop any previous directory block; rewrite it for this host.
text = text.split("[source.vendored-sources]")[0].rstrip()
text += f'\n\n[source.vendored-sources]\ndirectory = "{vendor_dir}"\n'
pathlib.Path(config_path).write_text(text)
print(text[-200:])
PY

echo "==> done; builds in ${cargo_home}'s project now resolve from ${out}"
