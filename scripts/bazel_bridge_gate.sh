#!/usr/bin/env bash
# Fork bridge gate (P1 follow-up 2026-10-06): proves the Bazel-built public
# binaries actually serve third-party model traffic on all three wires, and
# that compiling the model bridges out fails BEFORE any network request.
#
# What it runs:
#   1. bazelisk build //codex-rs/cli:codex //codex-rs/exec:codex-exec
#      (default flags: //:enable_model_bridges=true)
#   2. bazelisk test //codex-rs/exec:exec-all-test --test_filter=nuwax_env
#      — the REAL nuwax_env wire matrix against Bazel-built binaries (the
#      runfiles-resolved codex-exec), asserting per wire: path, credential,
#      model, cap field, exactly one POST, config bytes unchanged.
#      Fails if fewer than CODEX_BAZEL_GATE_MIN_TESTS tests matched (0-match
#      cannot pass the gate) or if any test fails.
#   3. Negative control: rebuild codex-exec with --//:enable_model_bridges=false
#      and drive one NUWAX turn against a counting TCP listener. The process
#      must exit non-zero, mention the missing bridge, and the listener must
#      observe ZERO connections (failure before any network traffic).
set -euo pipefail
cd "$(dirname "$0")/.."

command -v bazelisk >/dev/null 2>&1 || {
    echo "bridge gate: bazelisk not found on PATH" >&2
    exit 1
}

JOBS="${CODEX_BAZEL_JOBS:-8}"
MIN_NUWAX_TESTS="${CODEX_BAZEL_GATE_MIN_TESTS:-19}"
if ! [[ "$MIN_NUWAX_TESTS" =~ ^[1-9][0-9]*$ ]]; then
    echo "bridge gate: CODEX_BAZEL_GATE_MIN_TESTS must be a positive integer" >&2
    exit 1
fi

echo "== [bridge gate] building public binaries (bridges on) =="
bazelisk build --jobs="$JOBS" --//:enable_model_bridges=true //codex-rs/cli:codex //codex-rs/exec:codex-exec

echo "== [bridge gate] nuwax_env wire matrix under Bazel =="
LOG="$(mktemp -t codex-bazel-bridge-gate.XXXXXX)"
NEGATIVE_BUILD_STARTED=0
restore_public_binaries() {
    local gate_exit_status=$?
    rm -f "$LOG"
    if [ "$NEGATIVE_BUILD_STARTED" -eq 1 ]; then
        # The negative build changes bazel-bin symlinks too. Leave the public
        # binaries with bridges enabled, including when the negative check fails.
        if ! bazelisk build --jobs="$JOBS" --//:enable_model_bridges=true \
            //codex-rs/cli:codex //codex-rs/exec:codex-exec; then
            gate_exit_status=1
        fi
    fi
    exit "$gate_exit_status"
}
trap restore_public_binaries EXIT
bazelisk test --jobs="$JOBS" --test_sharding_strategy=disabled \
    --test_filter=nuwax_env --test_output=streamed --cache_test_results=no \
    --//:enable_model_bridges=true \
    //codex-rs/exec:exec-all-test | tee "$LOG"

PASSED="$(sed -n 's/.*test result: ok\. \([0-9][0-9]*\) passed.*/\1/p' "$LOG" | tail -n1 || true)"
FAILED="$(sed -n 's/.*; \([0-9][0-9]*\) failed.*/\1/p' "$LOG" | tail -n1 || true)"
PASSED="${PASSED:-0}"
FAILED="${FAILED:-1}"
echo "[bridge gate] nuwax_env selected/executed: ${PASSED}, failed: ${FAILED}"
if [ "$PASSED" -lt "$MIN_NUWAX_TESTS" ]; then
    echo "bridge gate: expected at least ${MIN_NUWAX_TESTS} nuwax_env tests, matched ${PASSED}" >&2
    exit 1
fi
if [ "$FAILED" -ne 0 ]; then
    echo "bridge gate: ${FAILED} nuwax_env test(s) failed under Bazel" >&2
    exit 1
fi
for WIRE_TEST in \
    nuwax_env_chat_wire_reaches_the_chat_endpoint \
    nuwax_env_anthropic_wire_reaches_the_messages_endpoint \
    nuwax_env_responses_wire_reaches_the_responses_endpoint; do
    if ! grep -Eq "test .*::${WIRE_TEST} \\.\\.\\. ok" "$LOG"; then
        echo "bridge gate: required wire test did not pass: ${WIRE_TEST}" >&2
        exit 1
    fi
done

echo "== [bridge gate] negative control: bridges compiled out =="
NEGATIVE_BUILD_STARTED=1
bazelisk build --jobs="$JOBS" --//:enable_model_bridges=false //codex-rs/exec:codex-exec
NEG_BIN="$(bazelisk cquery --//:enable_model_bridges=false --output=files //codex-rs/exec:codex-exec | head -n1)"
case "$NEG_BIN" in
    /*) ;;
    *) NEG_BIN="$(bazelisk info execution_root)/$NEG_BIN" ;;
esac
if [ -z "$NEG_BIN" ] || [ ! -x "$NEG_BIN" ]; then
    echo "bridge gate: could not resolve the bridges-off codex-exec binary" >&2
    exit 1
fi
python3 scripts/bazel_bridge_gate_negative.py "$NEG_BIN"

echo "[bridge gate] OK: three-wire matrix green under Bazel; bridges-off fails before any network request."
