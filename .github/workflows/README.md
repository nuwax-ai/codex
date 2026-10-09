# Workflow Strategy (fork)

> Fork note (2026-10-01): this fork (`nuwax-ai/codex`) removed the upstream
> Bazel/Cargo PR-verification workflows (`bazel.yml`, `rust-ci.yml`). This
> README describes the workflows that actually exist here; the upstream PR
> CI description is retired. Registering a fork-local offline Cargo PR gate
> is tracked as future work (see `my-docs/codex-review-2026-10-01.md` R5).

## Workflows in this fork

- `fork-cargo-pr.yml` — fork-local offline Cargo PR gate (nextest on
  ubuntu/macos/windows incl. the fork-only bridge crates via
  `--features codex-core/rust-rig`; fmt + clippy on the fork surfaces).
  The macOS lane installs gstreamer 1.28 (brew) so `codex-voice-host` builds
  and unit-tests there; the ubuntu and windows lanes exclude that crate
  because their system package sources cannot provide the pinned gstreamer
  v1_28 API surface (ubuntu apt ships 1.24; the Windows installer route is
  not wired up). Newly added; no dispatch from this fork has happened yet —
  running it requires push/CI authorization (registered in
  `my-docs/rig-stability-next/`).
- `live-tests.yml` — manually dispatched real-vendor verification for the
  rig bridges (L1 bridge turns and L2 binary turns). Vendor credentials
  come from repository secrets / `.env.local`; vendors without hosted-search
  support are matrix-excluded with in-source evidence. No run of this
  workflow has been dispatched from this fork yet — local executions of the
  same entry points are the evidence recorded in `my-docs/`.
- `release.yml` / `release-beta.yml` — tag-driven npm publishing of the
  `nuwax-codex` package (`v*` tags).
- `python-sdk-build.yml` / `python-sdk-cli-release.yml` — python SDK build
  and CLI release paths.
- `rust-release-provisioned-macos.yml` — provisioned macOS release runner.

## Pull-request verification contract (local)

With no PR CI, verification is the developer's contract before pushing:

- `just fmt` after any code change; `just fix -p <crate>` before
  finalizing; `just test -p <changed crates>` (nextest), full workspace
  runs per `AGENTS.md`.
- Fork-only crates that upstream Bazel does not build
  (`codex-rust-rig-bridge`, `codex-rust-genai-bridge`, `codex-live-tests`)
  are Cargo-only; they carry no `BUILD.bazel` and are verified through the
  local contract above.
