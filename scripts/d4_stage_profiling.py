#!/usr/bin/env python3
"""Measure app-server startup phases per transport.

Modes:
- stdio: spawn, first stderr byte, initialize roundtrip. Stdio has no socket
  bind phase: bind_report_s is null with an explicit reason.
- websocket: spawn, first stderr byte, the "listening on: ws://..." banner as
  the accurate bind report (printed right after TcpListener::bind reports its
  actual port, so port 0 works), TCP connect, readyz HTTP roundtrip, websocket
  upgrade, initialize roundtrip over websocket frames.
- unix_socket: spawn, the control socket rendezvous path appearing as the bind
  report, Unix-domain connect, websocket upgrade over UDS, initialize
  roundtrip. readyz is HTTP-only and not applicable.

Usage: python3 scripts/d4_stage_profiling.py [--mode M] [samples] [app-server-binary]
"""

import argparse
import json
import os
from pathlib import Path
import queue
import re
import socket
import subprocess
import sys
import tempfile
import threading
import time

from d4_websocket import WebSocketClient
from d4_websocket import build_upgrade_request
from d4_websocket import decode_server_frame
from d4_websocket import encode_client_text_frame

RPC_BUDGET_SECONDS = 60.0
BIND_BUDGET_SECONDS = 30.0
CONTROL_SOCKET_RELATIVE_PATH = ("app-server-control", "app-server-control.sock")
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
ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
WEBSOCKET_BANNER = re.compile(r"listening on:\s*ws://(\d+\.\d+\.\d+\.\d+):(\d+)")


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


def strip_ansi(text: str) -> str:
    return ANSI_ESCAPE.sub("", text)


def parse_websocket_banner(line: str) -> tuple[str, int] | None:
    """Return (host, port) from a "listening on: ws://host:port" banner line.

    Tolerates ANSI styling around the label and the URL and rejects any other
    line, so callers can feed every stderr line through this filter.
    """
    match = WEBSOCKET_BANNER.search(strip_ansi(line))
    if match is None:
        return None
    return match.group(1), int(match.group(2))


def _child_environment(home: str) -> dict[str, str]:
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("NUWAX_", "CODEX_", "OPENAI_"))
    }
    env.update(CODEX_HOME=home, CODEX_SQLITE_HOME=home, RUST_LOG="info")
    return env


def _spawn_server(
    command: list[str], listen_url: str, home: str
) -> tuple[
    subprocess.Popen,
    queue.Queue[tuple[str, float, bytes]],
    list[threading.Thread],
]:
    child = subprocess.Popen(
        [*command, "--listen", listen_url],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=_child_environment(home),
        cwd=home,
    )
    events: queue.Queue[tuple[str, float, bytes]] = queue.Queue()

    def drain_stdout() -> None:
        assert child.stdout is not None
        for line in iter(child.stdout.readline, b""):
            events.put(("stdout", time.perf_counter(), line))
        events.put(("stdout_eof", time.perf_counter(), b""))

    def drain_stderr() -> None:
        assert child.stderr is not None
        first = child.stderr.read(1)
        if first:
            events.put(("first_stderr", time.perf_counter(), b""))
            line = first + child.stderr.readline()
            events.put(("stderr_line", time.perf_counter(), line))
        for line in iter(child.stderr.readline, b""):
            events.put(("stderr_line", time.perf_counter(), line))
        events.put(("stderr_eof", time.perf_counter(), b""))

    readers = [
        threading.Thread(target=drain_stdout, daemon=True),
        threading.Thread(target=drain_stderr, daemon=True),
    ]
    for reader in readers:
        reader.start()
    return child, events, readers


def _stop_server(child: subprocess.Popen, readers: list[threading.Thread]) -> None:
    if child.stdin is not None:
        child.stdin.close()
    if child.poll() is None:
        child.terminate()
    try:
        child.wait(timeout=10)
    except subprocess.TimeoutExpired:
        child.kill()
        child.wait(timeout=10)
    for reader in readers:
        reader.join(timeout=1)
    for stream in (child.stdout, child.stderr):
        if stream is not None:
            stream.close()


def _initialize_over_websocket(
    client: WebSocketClient, *, rpc_budget_seconds: float
) -> float:
    client.deadline = time.perf_counter() + rpc_budget_seconds
    init_sent = time.perf_counter()
    client.send_text(json.dumps(INITIALIZE_REQUEST))
    while True:
        message = json.loads(client.recv_text())
        if message.get("id") == INITIALIZE_REQUEST["id"]:
            if "error" in message or not isinstance(message.get("result"), dict):
                raise RuntimeError("initialize did not return a successful result")
            initialize_response = time.perf_counter()
            break
    client.send_text('{"jsonrpc":"2.0","method":"initialized"}')
    return initialize_response - init_sent


def _readyz_roundtrip(host: str, port: int) -> float:
    started = time.perf_counter()
    with socket.create_connection((host, port), timeout=BIND_BUDGET_SECONDS) as sock:
        sock.sendall(
            f"GET /readyz HTTP/1.1\r\nHost: {host}:{port}\r\n"
            f"Connection: close\r\n\r\n".encode()
        )
        deadline = started + BIND_BUDGET_SECONDS
        response = bytearray()
        while b"\r\n" not in response:
            remaining = deadline - time.perf_counter()
            if remaining <= 0:
                raise TimeoutError("readyz deadline expired")
            sock.settimeout(remaining)
            chunk = sock.recv(4096)
            if not chunk:
                raise RuntimeError("readyz closed before status line")
            response.extend(chunk)
            if len(response) > 8192:
                raise RuntimeError("readyz status exceeds diagnostic capture limit")
        status = bytes(response).split(b"\r\n", 1)[0]
    if b" 200 " not in status:
        raise RuntimeError(f"readyz did not return 200: {status!r}")
    return time.perf_counter() - started


def profile_once(
    command: list[str],
    *,
    rpc_budget_seconds: float = RPC_BUDGET_SECONDS,
    mode: str = "stdio",
) -> dict[str, float | str | None | int]:
    if mode == "stdio":
        return _profile_stdio(command, rpc_budget_seconds=rpc_budget_seconds)
    if mode == "websocket":
        return _profile_websocket(command, rpc_budget_seconds=rpc_budget_seconds)
    if mode == "unix_socket":
        return _profile_unix_socket(command, rpc_budget_seconds=rpc_budget_seconds)
    raise ValueError(f"unknown mode: {mode}")


def _profile_stdio(
    command: list[str], *, rpc_budget_seconds: float
) -> dict[str, float | str | None]:
    events: queue.Queue[tuple[str, float, bytes]] = queue.Queue()
    first_stderr = None
    initialize_response = None
    with tempfile.TemporaryDirectory() as home:
        Path(home, "config.toml").write_text(
            "features.plugins = false\nanalytics.enabled = false\n"
        )
        started = time.perf_counter()
        child = subprocess.Popen(
            [*command, "--listen", "stdio://"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=_child_environment(home),
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


def _wait_for_banner(
    events: queue.Queue[tuple[str, float, bytes]], started: float, bind_budget: float
) -> tuple[float, str, int, float | None]:
    """Wait for the websocket startup banner on stderr.

    Returns (bind_report_s, host, port, first_stderr_s).
    """
    first_stderr = None
    deadline = time.perf_counter() + bind_budget
    while True:
        remaining = deadline - time.perf_counter()
        if remaining <= 0:
            raise TimeoutError("websocket bind banner deadline expired")
        try:
            stream, observed, line = events.get(timeout=remaining)
        except queue.Empty as error:
            raise TimeoutError("websocket bind banner deadline expired") from error
        if stream == "first_stderr":
            first_stderr = observed - started
            continue
        if stream == "stderr_eof":
            raise RuntimeError("app-server closed stderr before binding")
        if stream != "stderr_line":
            continue
        parsed = parse_websocket_banner(line.decode("utf-8", "replace"))
        if parsed is not None:
            host, port = parsed
            return observed - started, host, port, first_stderr


def _profile_websocket(
    command: list[str], *, rpc_budget_seconds: float
) -> dict[str, float | str | None | int]:
    with tempfile.TemporaryDirectory() as home:
        Path(home, "config.toml").write_text(
            "features.plugins = false\nanalytics.enabled = false\n"
        )
        started = time.perf_counter()
        child, events, readers = _spawn_server(command, "ws://127.0.0.1:0", home)
        spawned = time.perf_counter()
        client = None
        sock = None
        try:
            bind_report_s, host, port, first_stderr = _wait_for_banner(
                events, started, BIND_BUDGET_SECONDS
            )
            readyz_s = _readyz_roundtrip(host, port)
            connect_started = time.perf_counter()
            sock = socket.create_connection((host, port), timeout=BIND_BUDGET_SECONDS)
            tcp_connect_s = time.perf_counter() - connect_started
            upgrade_started = time.perf_counter()
            client = WebSocketClient(
                sock, f"{host}:{port}", time.perf_counter() + rpc_budget_seconds
            )
            ws_upgrade_s = time.perf_counter() - upgrade_started
            initialize_s = _initialize_over_websocket(
                client, rpc_budget_seconds=rpc_budget_seconds
            )
            client.close()
            return {
                "spawn_call_s": spawned - started,
                "first_stderr_s": first_stderr,
                "bind_report_s": bind_report_s,
                "bind_phase": "websocket_banner_stderr",
                "bind_port": port,
                "tcp_connect_s": tcp_connect_s,
                "readyz_roundtrip_s": readyz_s,
                "ws_upgrade_s": ws_upgrade_s,
                "initialize_roundtrip_s": initialize_s,
                "total_s": time.perf_counter() - started,
            }
        finally:
            if client is not None:
                client.close()
            elif sock is not None:
                sock.close()
            _stop_server(child, readers)


def _profile_unix_socket(
    command: list[str], *, rpc_budget_seconds: float
) -> dict[str, float | str | None | str]:
    with tempfile.TemporaryDirectory() as home:
        Path(home, "config.toml").write_text(
            "features.plugins = false\nanalytics.enabled = false\n"
        )
        control_socket = Path(home, *CONTROL_SOCKET_RELATIVE_PATH)
        started = time.perf_counter()
        child, events, readers = _spawn_server(command, "unix://", home)
        spawned = time.perf_counter()
        client = None
        sock = None
        try:
            bind_deadline = time.perf_counter() + BIND_BUDGET_SECONDS
            while not control_socket.exists():
                if child.poll() is not None:
                    raise RuntimeError(
                        "app-server exited before control socket binding"
                    )
                if time.perf_counter() > bind_deadline:
                    raise TimeoutError("control socket bind deadline expired")
                time.sleep(0.005)
            bind_report_s = time.perf_counter() - started
            first_stderr = None
            while not events.empty():
                stream, observed, _ = events.get_nowait()
                if stream == "first_stderr" and first_stderr is None:
                    first_stderr = observed - started
                    break
            connect_started = time.perf_counter()
            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            sock.settimeout(BIND_BUDGET_SECONDS)
            sock.connect(str(control_socket))
            connect_s = time.perf_counter() - connect_started
            upgrade_started = time.perf_counter()
            client = WebSocketClient(
                sock,
                "app-server-control",
                time.perf_counter() + rpc_budget_seconds,
            )
            ws_upgrade_s = time.perf_counter() - upgrade_started
            initialize_s = _initialize_over_websocket(
                client, rpc_budget_seconds=rpc_budget_seconds
            )
            client.close()
            return {
                "spawn_call_s": spawned - started,
                "first_stderr_s": first_stderr,
                "bind_report_s": bind_report_s,
                "bind_phase": "unix_control_socket_appeared",
                "connect_s": connect_s,
                "readyz_roundtrip_s": None,
                "readyz_phase": "not_applicable_unix_socket",
                "ws_upgrade_s": ws_upgrade_s,
                "initialize_roundtrip_s": initialize_s,
                "total_s": time.perf_counter() - started,
            }
        finally:
            if client is not None:
                client.close()
            elif sock is not None:
                sock.close()
            _stop_server(child, readers)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--mode",
        choices=("stdio", "websocket", "unix_socket"),
        default="stdio",
        help="transport to profile (default: stdio)",
    )
    parser.add_argument("samples", nargs="?", type=int, default=5)
    parser.add_argument("binary", nargs="?")
    args = parser.parse_args()
    try:
        if args.samples <= 0:
            raise ValueError("samples must be positive")
        binary = resolve_binary(args.binary)
        print(
            json.dumps(
                {
                    "event": "start",
                    "mode": args.mode,
                    "binary": binary,
                    "load_before": current_load(),
                }
            )
        )
        for index in range(args.samples):
            result = profile_once([binary], mode=args.mode)
            print(
                json.dumps(
                    {
                        "event": "sample",
                        "index": index,
                        "load": current_load(),
                        **result,
                    }
                )
            )
        print(json.dumps({"event": "end", "load_after": current_load()}))
    except (OSError, RuntimeError, TimeoutError, ValueError, EOFError) as error:
        print(json.dumps({"event": "failure", "reason": str(error)}), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
