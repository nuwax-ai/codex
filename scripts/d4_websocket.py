"""Bounded RFC 6455 text client for local app-server startup diagnostics."""

import base64
import hashlib
import os
import select
import socket
import struct
import time

MAX_MESSAGE_BYTES = 1024 * 1024
MAX_UPGRADE_BYTES = 64 * 1024
WEBSOCKET_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


class IncompleteFrame(ValueError):
    """The buffered bytes do not yet contain a complete frame."""


def build_upgrade_request(host_header: str, *, key: str | None = None) -> bytes:
    if key is None:
        key = base64.b64encode(os.urandom(16)).decode()
    return (
        f"GET / HTTP/1.1\r\nHost: {host_header}\r\n"
        "Upgrade: websocket\r\nConnection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    ).encode()


def _encode_client_frame(opcode: int, payload: bytes, mask_key: bytes) -> bytes:
    if len(mask_key) != 4:
        raise ValueError("mask key must be 4 bytes")
    length = len(payload)
    if length < 126:
        header = struct.pack("!BB", 0x80 | opcode, 0x80 | length)
    elif length < 65536:
        header = struct.pack("!BBH", 0x80 | opcode, 0x80 | 126, length)
    else:
        header = struct.pack("!BBQ", 0x80 | opcode, 0x80 | 127, length)
    masked = bytes(byte ^ mask_key[index % 4] for index, byte in enumerate(payload))
    return header + mask_key + masked


def encode_client_text_frame(payload: bytes, mask_key: bytes | None = None) -> bytes:
    return _encode_client_frame(
        1, payload, os.urandom(4) if mask_key is None else mask_key
    )


def decode_server_frame(buffer: bytes) -> tuple[int, bool, bytes, int]:
    if len(buffer) < 2:
        raise IncompleteFrame("incomplete frame header")
    first, second = buffer[0], buffer[1]
    if first & 0x70:
        raise ValueError("websocket extensions were not negotiated")
    opcode, fin = first & 0x0F, bool(first & 0x80)
    if opcode not in (0, 1, 8, 9, 10):
        raise ValueError("unsupported websocket opcode")
    if second & 0x80:
        raise ValueError("server frame must not be masked")
    length, offset = second & 0x7F, 2
    if length == 126:
        if len(buffer) < 4:
            raise IncompleteFrame("incomplete extended length")
        length, offset = struct.unpack("!H", buffer[2:4])[0], 4
    elif length == 127:
        if len(buffer) < 10:
            raise IncompleteFrame("incomplete extended length")
        length, offset = struct.unpack("!Q", buffer[2:10])[0], 10
    if opcode >= 8 and (not fin or length > 125):
        raise ValueError("invalid websocket control frame")
    if length > MAX_MESSAGE_BYTES:
        raise ValueError("websocket frame exceeds diagnostic capture limit")
    if len(buffer) < offset + length:
        raise IncompleteFrame("incomplete frame payload")
    return opcode, fin, buffer[offset : offset + length], offset + length


class WebSocketClient:
    """A diagnostic client with one absolute deadline and a bounded message."""

    def __init__(self, sock: socket.socket, host_header: str, deadline: float):
        self._socket = sock
        self._socket.setblocking(False)
        self._buffer = b""
        self.deadline = deadline
        try:
            key = base64.b64encode(os.urandom(16)).decode()
            self._send(build_upgrade_request(host_header, key=key))
            while b"\r\n\r\n" not in self._buffer:
                if len(self._buffer) > MAX_UPGRADE_BYTES:
                    raise RuntimeError(
                        "websocket upgrade exceeds diagnostic capture limit"
                    )
                self._recv()
            headers, self._buffer = self._buffer.split(b"\r\n\r\n", 1)
            if len(headers) > MAX_UPGRADE_BYTES:
                raise RuntimeError("websocket upgrade exceeds diagnostic capture limit")
            lines = headers.decode("ascii").split("\r\n")
            if lines[0].split()[:2] != ["HTTP/1.1", "101"]:
                raise RuntimeError("websocket upgrade rejected")
            values = {}
            for line in lines[1:]:
                name, value = line.split(":", 1)
                name = name.lower()
                if name in values:
                    raise RuntimeError("duplicate websocket upgrade header")
                values[name] = value.strip()
            expected = base64.b64encode(
                hashlib.sha1((key + WEBSOCKET_GUID).encode()).digest()
            ).decode()
            if values.get("sec-websocket-accept") != expected:
                raise RuntimeError("invalid websocket upgrade accept")
            if values.get("upgrade", "").lower() != "websocket" or "upgrade" not in {
                token.strip().lower()
                for token in values.get("connection", "").split(",")
            }:
                raise RuntimeError("invalid websocket upgrade headers")
        except BaseException:
            self.close()
            raise

    def _wait(self, *, write: bool = False) -> None:
        remaining = self.deadline - time.perf_counter()
        if remaining <= 0:
            raise TimeoutError("initialize response deadline expired")
        readable, writable, _ = select.select(
            [] if write else [self._socket],
            [self._socket] if write else [],
            [],
            remaining,
        )
        if not readable and not writable:
            raise TimeoutError("initialize response deadline expired")

    def _send(self, data: bytes) -> None:
        remaining = memoryview(data)
        while remaining:
            self._wait(write=True)
            try:
                sent = self._socket.send(remaining)
            except BlockingIOError:
                continue
            if sent == 0:
                raise EOFError("websocket stream closed while sending")
            remaining = remaining[sent:]

    def _recv(self) -> None:
        while True:
            self._wait()
            try:
                chunk = self._socket.recv(4096)
            except BlockingIOError:
                continue
            if not chunk:
                raise EOFError("websocket stream closed")
            self._buffer += chunk
            return

    def _frame(self) -> tuple[int, bool, bytes]:
        while True:
            try:
                opcode, fin, payload, consumed = decode_server_frame(self._buffer)
            except IncompleteFrame:
                self._recv()
                continue
            self._buffer = self._buffer[consumed:]
            return opcode, fin, payload

    def send_text(self, message: str) -> None:
        self._send(encode_client_text_frame(message.encode()))

    def recv_text(self) -> str:
        fragments = bytearray()
        started = False
        while True:
            if time.perf_counter() >= self.deadline:
                raise TimeoutError("initialize response deadline expired")
            opcode, fin, chunk = self._frame()
            if opcode == 8:
                raise EOFError("websocket closed by server")
            if opcode == 9:
                self._send(_encode_client_frame(10, chunk, os.urandom(4)))
                continue
            if opcode == 10:
                continue
            if opcode == 1:
                if started:
                    raise RuntimeError(
                        "new websocket text frame during fragmented message"
                    )
                started = True
            elif not started:
                raise RuntimeError("websocket continuation without text frame")
            if len(fragments) + len(chunk) > MAX_MESSAGE_BYTES:
                raise RuntimeError("websocket message exceeds diagnostic capture limit")
            fragments.extend(chunk)
            if fin:
                return fragments.decode("utf-8")

    def close(self) -> None:
        self._socket.close()
