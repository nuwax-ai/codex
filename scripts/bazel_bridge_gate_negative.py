#!/usr/bin/env python3
"""Verify that a bridges-off binary fails before connecting to the model."""

import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading

MISSING_BRIDGE_MARKER = "requires a model bridge"
PROCESS_BUDGET_SECONDS = 60.0


def check_binary(
    command: list[str], *, process_budget_seconds: float = PROCESS_BUDGET_SECONDS
) -> tuple[list[str], int]:
    connections = 0
    stopped = threading.Event()
    accept_errors: list[str] = []
    problems: list[str] = []
    completed = None
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(4)
        listener.settimeout(0.05)
        port = listener.getsockname()[1]

        def accept_loop() -> None:
            nonlocal connections
            while True:
                try:
                    conn, _ = listener.accept()
                except socket.timeout:
                    # After process completion, drain all queued connections
                    # before accepting a zero count as evidence.
                    if stopped.is_set():
                        return
                    continue
                except OSError:
                    accept_errors.append("counting listener failed")
                    return
                connections += 1
                conn.close()

        reader = threading.Thread(target=accept_loop, daemon=True)
        reader.start()
        try:
            with tempfile.TemporaryDirectory() as home:
                Path(home, "config.toml").write_text(
                    "features.plugins = false\nanalytics.enabled = false\n"
                )
                env = {
                    key: value
                    for key, value in os.environ.items()
                    if not key.startswith(("NUWAX_", "CODEX_", "OPENAI_"))
                }
                env.update(
                    CODEX_HOME=home,
                    CODEX_SQLITE_HOME=home,
                    CODEX_API_KEY="dummy",
                    NUWAX_BASE_URL=f"http://127.0.0.1:{port}/v1",
                    NUWAX_WIRE_API="chat",
                    NUWAX_API_KEY="bridge-gate-negative-key",
                    NUWAX_MODEL="bridge-gate-negative-model",
                )
                try:
                    completed = subprocess.run(
                        [
                            *command,
                            "--skip-git-repo-check",
                            "--color",
                            "never",
                            "-C",
                            home,
                            "reply with ok",
                        ],
                        env=env,
                        cwd=home,
                        capture_output=True,
                        text=True,
                        timeout=process_budget_seconds,
                    )
                except subprocess.TimeoutExpired:
                    problems.append("codex-exec did not exit within the budget")
        finally:
            stopped.set()
            reader.join(timeout=5)
            if reader.is_alive():
                problems.append("counting listener did not finish draining")
            problems.extend(accept_errors)

    if completed is not None:
        if completed.returncode == 0:
            problems.append("process exited 0; a bridges-off binary must fail")
        if MISSING_BRIDGE_MARKER not in completed.stderr:
            problems.append("stderr does not name the missing model bridge")
    if connections:
        problems.append(f"listener saw {connections} connection(s)")
    return problems, connections


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: bazel_bridge_gate_negative.py <binary>", file=sys.stderr)
        return 2
    problems, _ = check_binary([os.path.abspath(sys.argv[1])])
    if problems:
        for problem in problems:
            print(f"negative control: {problem}", file=sys.stderr)
        return 1
    print("negative control: OK (non-zero exit, bridge named, zero connections)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
