# Linux device-tree review receipt — 2026-10-22

Repo: `/Volumes/zhitai/git_workspace/fork-nuwax-codex`, baseline `850a87390`.
Only changed sources: `linux-sandbox/src/bwrap.rs`, `bwrap_tests.rs`, existing
`tests/suite/denied_files_tests.rs`. No sandbox policy, sysctl, host account, or
credential changes. No Windows changes. No workspace-wide test run.

## Reproduction

The original explicit `/dev` branch had three concrete defects:

1. Its writable, caller-owned tmpfs let the payload recreate the omitted `/dev/fd`
   with `ln -s /proc/self/fd /dev/fd`, restoring the denied lookup path.
2. A restricted `Read /dev` grant mounted the host directory over the explicit
   tree, restoring `/dev/fd` before payload startup; its mask was still skipped.
3. Binding the host `/dev/pts` exposed that instance, demonstrated using only a
   test-owned PTY's `st_dev`, `st_ino`, and `st_rdev`. No other terminal was read.
   This receipt does not claim that the old nodev bind permitted terminal I/O.

`.163` was reachable, but bwrap failed setting up the uid map under the existing
AppArmor restriction; sudo -n was unavailable. This was recorded, not bypassed.
Actual validation ran over normal key-auth SSH on `.45` as its configured soddy
user (bwrap 0.11.1; AppArmor sysctl remains 1).

Task mirror: `/tmp/nuwax-sandbox-review-20261022/repo`, from `git archive HEAD`,
then the three changed source files copied over. No reset or edits to Claude's
existing checkout. Its build cache was reused through `CARGO_TARGET_DIR`.

Raw bwrap reproduction: `dev_probe.py before` / `after`.
Real helper reproduction: `production_dev_probe.py <helper>` with identical
assertions for root_read, explicit_device_read, and writable_shm policies.

Before helper copy SHA256:
`ab9397c6db7fbbccc62cd7b7c4c4279357f15988130d61140aca9c9fdf387252`.
The original remote `bwrap.rs` exactly matched baseline committed `bwrap.rs`:
`2bfbe8037cfc935f138ae1a9e98a99fa4fc05fe81d4e81447b8b1404fc05fc85`.
The copied binary came from that existing build; no claim of a fresh baseline
build is made. Raw argv probes independently corroborated its failures.

Before outcomes: omitted fd was recreated (errno 0), explicit Read /dev already
contained it, and the test-owned host PTY metadata matched. Assertions failed.
Saved: `dev-probe-before-final.log`, `production-probe-before.log`.

## Fix and results

- Seal the explicit `/dev` parent read-only after all binds/masks (non-recursive).
- Keep the minimal tree when a restricted Read /dev grant would overwrite it.
- Reject incompatible later binds that restore omitted entries or host PTYs.
- Never substitute host devpts for a private instance. In this exceptional
  device-deny branch, keep `/dev/pts` empty and omit `/dev/ptmx`; new PTY allocation
  is unavailable. Inherited stdio and the optional own-console device survive.
- Ordinary policies still use the existing `--dev` private-devpts implementation.

Fresh helper build: Rust 1.99.1, `cargo build -p codex-linux-sandbox --bin
codex-linux-sandbox --locked`, 51.47s, exit 0. SHA256:
`b37e9874f4b2e3cf029bcf9edc3cce91b05f0d0f7a38089b18335a3abb337763`.

Real helper probe after: all three policies execute, fd starts and stays absent,
recreation fails EROFS (30), own host PTY is ENOENT (2), `/dev/ptmx` absent,
`/dev/null` writable, and explicitly writable `/dev/shm` writes survive the seal.
An additional ordinary-policy probe successfully allocates a private PTY and
does not initially expose the task-owned host sentinel.
Saved: `production-probe-final.log`.

`cargo nextest`, retries 0:

- bwrap/WSLg plus the three new real-helper integration cases: **76/76**,
  174 filtered/skipped, 1.399s.
- Entire codex-linux-sandbox lib: **178/178**, 0 skipped, 0.785s.
- Original exec-server `blocked_descriptor_path`: **1/1**, 76 filtered/skipped,
  1.136s. It remains operational.
- Final integration fixture marks both test-owned PTY fds CLOEXEC, preventing
  the test from inheriting host PTY handles into its payload. All three cases
  were rerun: **3/3**, 69 filtered/skipped, 0.915s.

New integration prerequisites use assertions, not an early success return.
They assert the payload completion marker, rejected recreation, own-host-PTY
absence, standard device operation, and writable-shm output.

Formatting and scoped diff check passed. Crate-scoped all-targets strict Clippy
passed (`--no-deps -- -D warnings`), including the final fixture, 1.04s warm.
The first broader Clippy invocation failed in unchanged dependency
`history/src/lib.rs:252` (`large_enum_variant`: 800-byte ResponseItem versus
488-byte EventMsg); `clippy-first.log` preserves that failure. It was not fixed
or hidden as a passing dependency-wide check.

Final source SHA256:

- bwrap.rs: `b4681bd711b9d3a6839ee908f52c497d782f1964f04e62d8fe55912376f17190`
- bwrap_tests.rs: `ffeeaad475a96a66a2aa2a0db299e202054d496660d4c22d56c0f6e289d01bd1`
- denied_files_tests.rs: `699cd1413edf09321dd36cd4d16a4b38597a5946ae231bfef9a28c5dddd82f51`

O3 narrow netns classification and O4 self-family Fatal were reviewed without
new defects. The accepted Windows legacy delete limitation remains untouched.
