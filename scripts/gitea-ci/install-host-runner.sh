#!/usr/bin/env bash
#
# Turn a Linux box into a Gitea CI build host: Rust toolchain, Zig, and an
# act_runner in HOST mode (label linux-host).
#
#   bash install-host-runner.sh --instance http://192.168.31.23:3301
#
# Host mode is what the release workflow needs: job containers cannot write to
# the host's artifact directory, cannot resolve the instance by its compose
# name, and (on this network) cannot reach github.com at all. A host-mode runner
# runs steps directly on the machine, so artifacts, caches and the pre-seeded
# V8 archives all behave like a local build.
#
# Re-runnable: skips the toolchain unless it is missing, keeps the runner's
# registration in place.

set -euo pipefail

instance=""
data_root="/data/nvme1/gitea-ci"
cache_root="/data/nvme0/ci-cache"
toolchain="${data_root}/toolchain"
rust_version="1.99.1"
zig_version="0.14.0"
targets="x86_64-unknown-linux-musl aarch64-unknown-linux-musl"

while [ $# -gt 0 ]; do
  case "$1" in
    --instance) instance="$2"; shift 2 ;;
    --data-root) data_root="$2"; shift 2 ;;
    --cache-root) cache_root="$2"; shift 2 ;;
    --rust) rust_version="$2"; shift 2 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

[ -n "${instance}" ] || { echo "error: --instance is required" >&2; exit 2; }
instance="${instance%/}"

say() { printf '\n==> %s\n' "$*"; }

say "directories"
mkdir -p "${toolchain}"/{cargo,rustup,zig} "${data_root}"/{bin,host-runner,artifacts} "${cache_root}"/{target,act}
export CARGO_HOME="${toolchain}/cargo"
export RUSTUP_HOME="${toolchain}/rustup"

say "cargo mirror (crates.io is blocked on this network)"
if [ ! -f "${CARGO_HOME}/config.toml" ]; then
  cat > "${CARGO_HOME}/config.toml" <<'TOML'
[source.crates-io]
replace-with = "rsproxy-sparse"
[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"
[registries.rsproxy]
index = "sparse+https://rsproxy.cn/index/"
[net]
git-fetch-with-cli = true
TOML
fi

say "rust ${rust_version}"
if [ ! -x "${CARGO_HOME}/bin/rustup" ]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
    sh -s -- -y --profile minimal --default-toolchain "${rust_version}" --no-modify-path
fi
"${CARGO_HOME}/bin/rustup" toolchain install "${rust_version}" --profile minimal --no-self-update
# shellcheck disable=SC2086
"${CARGO_HOME}/bin/rustup" target add --toolchain "${rust_version}" ${targets}
"${CARGO_HOME}/bin/rustc" --version

say "zig ${zig_version}"
if [ ! -x "${toolchain}/zig/zig" ]; then
  case "$(uname -m)" in
    x86_64) zig_arch=x86_64 ;;
    aarch64|arm64) zig_arch=aarch64 ;;
    *) echo "unsupported host arch: $(uname -m)" >&2; exit 1 ;;
  esac
  curl -fsSL "https://ziglang.org/download/${zig_version}/zig-linux-${zig_arch}-${zig_version}.tar.xz" |
    tar -xJ -C "${toolchain}/zig" --strip-components=1
fi
"${toolchain}/zig/zig" version

say "act_runner binary"
if [ ! -x "${data_root}/bin/act_runner" ]; then
  cid="$(docker create gitea/act_runner:0.2.11)"
  docker cp "${cid}:/usr/local/bin/act_runner" "${data_root}/bin/act_runner" 2>/dev/null ||
    docker cp "${cid}:/usr/bin/act_runner" "${data_root}/bin/act_runner"
  docker rm "${cid}" >/dev/null
  chmod +x "${data_root}/bin/act_runner"
fi
"${data_root}/bin/act_runner" --version

say "register the host runner"
cd "${data_root}/host-runner"
cat > config.yaml <<EOF
log:
  level: info
runner:
  file: .runner
  capacity: 2
cache:
  enabled: true
  dir: ${cache_root}/act
container:
  force_pull: false
EOF
if [ ! -f .runner ]; then
  token="$(docker exec -u git gitea-ci gitea actions generate-runner-token | tail -1 | tr -d '\r')"
  [ -n "${token}" ] || { echo "could not get a runner token (is the gitea container named gitea-ci?)" >&2; exit 1; }
  # A single label on purpose: jobs request `runs-on: linux-host`, matching the
  # label exactly. `[self-hosted, linux-host]` would need both labels declared.
  "${data_root}/bin/act_runner" register --no-interactive \
    --instance "${instance}" --token "${token}" \
    --name linux-host --labels linux-host:host
fi

say "start the daemon"
pkill -f "act_runner daemon" 2>/dev/null || true
sleep 1
nohup "${data_root}/bin/act_runner" daemon --config config.yaml > daemon.log 2>&1 &
sleep 5
tail -3 daemon.log

cat <<EOF

Host runner is up.

  labels:   linux-host
  rust:     ${CARGO_HOME}/bin/rustc (${rust_version})
  zig:      ${toolchain}/zig/zig
  artifacts:${data_root}/artifacts
  logs:     ${data_root}/host-runner/daemon.log

Workflows must request \`runs-on: linux-host\` and, in every job, put the
toolchain on PATH:

  echo "\${CARGO_HOME}/bin:\${data_root}/toolchain/zig" >> "\$GITHUB_PATH"
EOF
