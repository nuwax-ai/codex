#!/usr/bin/env bash
#
# Read-only inventory of a machine we intend to host Gitea + CI runners on.
# Nothing is changed: run it first, paste the output, then decide about disks.
#
#   bash host-inventory.sh
#
# Focus: what the release build needs (cpu/ram/disk), whether Docker is usable
# without sudo, and which block devices could host the data directory.

set -euo pipefail
say() { printf '\n=== %s\n' "$*"; }

say "os"
uname -srm
(. /etc/os-release 2>/dev/null && echo "${PRETTY_NAME:-}") || true

say "cpu / memory"
echo "cpus:   $(nproc)"
awk '/MemTotal/ {printf "memory: %.1f GiB\n", $2/1048576}' /proc/meminfo

say "block devices (lsblk)"
lsblk -o NAME,SIZE,FSTYPE,TYPE,MOUNTPOINT,MODEL 2>/dev/null || true

say "filesystems and free space"
df -hT | awk 'NR==1 || $2 ~ /(ext4|xfs|btrfs|zfs|ntfs|vfat)/'

say "candidate data disks (>= 500 GiB, not the root device)"
root_src="$(findmnt -n -o SOURCE / 2>/dev/null || true)"
lsblk -bno NAME,SIZE,TYPE,MOUNTPOINT 2>/dev/null | awk -v root="${root_src}" '
  $3=="disk" && $2 >= 500*1024*1024*1024 {
    printf "  /dev/%s  %.0f GiB  mounted-at=%s\n", $1, $2/1073741824, ($4==""?"-":$4)
  }'

say "docker"
if command -v docker >/dev/null 2>&1; then
  docker --version
  docker compose version 2>/dev/null || echo "docker compose plugin: MISSING"
  if docker info >/dev/null 2>&1; then echo "daemon: reachable as $(id -un)"; else echo "daemon: NOT reachable as $(id -un) (needs sudo or group membership)"; fi
else
  echo "docker: MISSING"
fi

say "network"
ip -brief addr show 2>/dev/null | awk '$1!="lo"' || true
for port in 3000 3300 3301 2222 2223 443 80; do
  if ss -ltn 2>/dev/null | awk '{print $4}' | grep -qE "[:.]${port}$"; then
    echo "port ${port}: IN USE"
  fi
done

say "toolchain"
for tool in git curl python3 node npm rustc cargo zig; do
  if command -v "${tool}" >/dev/null 2>&1; then
    printf '%-8s %s\n' "${tool}" "$("${tool}" --version 2>&1 | head -1)"
  else
    printf '%-8s MISSING\n' "${tool}"
  fi
done

say "done (nothing was modified)"
