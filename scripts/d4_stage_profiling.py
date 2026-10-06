#!/usr/bin/env python3
"""Measure app-server stdio startup and a successful initialize RPC.

Stdio has no socket bind phase: bind_report_s is null with an explicit reason.
The response deadline applies even if stdout stays open without a full line.
Usage: python3 scripts/d4_stage_profiling.py [samples] [app-server-binary]
"""

import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time

RPC_BUDGET_SECONDS = 60.0
INITIALIZE_REQUEST = {
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
        "clientInfo": {
            "name": "d4-stage-profiling",
            "title": "D4 Stage Profiling",
            "version": "0.1.0",
        }
    },
}


def current_load() -> float | None:
    try:
        return os.getloadavg()[0]
    except (AttributeError, OSError):
        return None


def resolve_binary(override: str | None) -> str:
    if override:
        return str(Path(override).resolve())
    target = Path(os.environ.get("CARGO_TARGET_DIR", "codex-rs/target"))
    for profile in ("debug", "release"):
        candidate = target / profile / "codex-app-server"
        if candidate.is_file():
            return str(candidate.resolve())
    raise ValueError("no codex-app-server binary found; pass a built binary path")


def profile_once(
    command: list[str], *, rpc_budget_seconds: float = RPC_BUDGET_SECONDS
) -> dict[str, float | str | None]:
    events: queue.Queue[tuple[str, float, bytes]] = queue.Queue()
    first_stderr = None
    initialize_response = None
    with tempfile.TemporaryDirectory() as home:
        Path(home, "config.toml").write_text(
            "features.plugins = false\nanalytics.enabled = false\n"
        )
        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(("NUWAX_", "CODEX_", "OPENAI_"))
        }
        env.update(CODEX_HOME=home, CODEX_SQLITE_HOME=home, RUST_LOG="info")
        started = time.perf_counter()
        child = subprocess.Popen(
            [*command, "--listen", "stdio://"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            cwd=home,
        )
        spawned = time.perf_counter()

        def drain_stdout() -> None:
            assert child.stdout is not None
            for line in iter(child.stdout.readline, b""):
                events.put(("stdout", time.perf_counter(), line))
            events.put(("stdout_eof", time.perf_counter(), b""))

        def drain_stderr() -> None:
            assert child.stderr is not None
            if child.stderr.read(1):
                events.put(("first_stderr", time.perf_counter(), b""))
            # Keep draining: log volume must never block initialize.
            while child.stderr.read(4096):
                pass

        readers = [
            threading.Thread(target=drain_stdout, daemon=True),
            threading.Thread(target=drain_stderr, daemon=True),
        ]
        for reader in readers:
            reader.start()
        try:
            assert child.stdin is not None
            init_sent = time.perf_counter()
            # The app-server stdio transport accepts newline-delimited JSON,
            # not LSP Content-Length headers.
            child.stdin.write((json.dumps(INITIALIZE_REQUEST) + "\n").encode())
            child.stdin.flush()
            deadline = init_sent + rpc_budget_seconds
            while initialize_response is None:
                remaining = deadline - time.perf_counter()
                if remaining <= 0:
                    raise TimeoutError("initialize response deadline expired")
                try:
                    stream, observed, line = events.get(timeout=remaining)
                except queue.Empty as error:
                    raise TimeoutError(
                        "initialize response deadline expired"
                    ) from error
                if stream == "first_stderr":
                    first_stderr = observed - started
                elif stream == "stdout_eof":
                    raise RuntimeError("app-server closed stdout before initialize")
                else:
                    message = json.loads(line)
                    if message.get("id") == INITIALIZE_REQUEST["id"]:
                        if "error" in message or not isinstance(
                            message.get("result"), dict
                        ):
                            raise RuntimeError(
                                "initialize did not return a successful result"
                            )
                        initialize_response = observed
            # Notifications are sent only after a successful handshake.
            child.stdin.write(b'{"jsonrpc":"2.0","method":"initialized"}\n')
            child.stdin.flush()
        finally:
            if child.stdin is not None:
                child.stdin.close()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=10)
            for reader in readers:
                reader.join(timeout=1)
            if child.stdout is not None:
                child.stdout.close()
            if child.stderr is not None:
                child.stderr.close()

        if first_stderr is None:
            while not events.empty():
                stream, observed, _ = events.get_nowait()
                if stream == "first_stderr":
                    first_stderr = observed - started
        return {
            "spawn_call_s": spawned - started,
            "first_stderr_s": first_stderr,
            "bind_report_s": None,
            "bind_phase": "not_applicable_stdio",
            "initialize_roundtrip_s": initialize_response - init_sent,
            "total_s": time.perf_counter() - started,
        }


def main() -> int:
    try:
        samples = int(sys.argv[1]) if len(sys.argv) > 1 else 5
        if samples <= 0:
            raise ValueError("samples must be positive")
        binary = resolve_binary(sys.argv[2] if len(sys.argv) > 2 else None)
        print(
            json.dumps(
                {"event": "start", "binary": binary, "load_before": current_load()}
            )
        )
        for index in range(samples):
            result = profile_once([binary])
            print(json.dumps({"event": "sample", "index": index, **result}))
        print(json.dumps({"event": "end", "load_after": current_load()}))
    except (OSError, RuntimeError, TimeoutError, ValueError) as error:
        print(json.dumps({"event": "failure", "reason": str(error)}), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
