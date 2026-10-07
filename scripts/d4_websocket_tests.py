"""Exercise the diagnostic client through real stream sockets."""

import base64
from contextlib import contextmanager
import hashlib
import socket
import struct
import threading
import time
import unittest
from unittest.mock import patch

from d4_websocket import (
    MAX_MESSAGE_BYTES,
    WEBSOCKET_GUID,
    WebSocketClient,
    decode_server_frame,
)


def server_frame(payload: bytes, *, opcode: int = 1, fin: bool = True) -> bytes:
    return bytes([(0x80 if fin else 0) | opcode, len(payload)]) + payload


@contextmanager
def peer(action, *, bad_accept=False, budget=2):
    server, client_socket = socket.socketpair()
    errors = []

    def serve():
        try:
            request = b""
            while b"\r\n\r\n" not in request:
                request += server.recv(4096)
            key = next(
                line.split(b":", 1)[1].strip()
                for line in request.split(b"\r\n")
                if line.lower().startswith(b"sec-websocket-key:")
            )
            accept = base64.b64encode(
                hashlib.sha1(key + WEBSOCKET_GUID.encode()).digest()
            )
            if bad_accept:
                accept = b"not-the-request-key"
            server.sendall(
                b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: "
                + accept
                + b"\r\n\r\n"
            )
            action(server)
        except (BrokenPipeError, ConnectionResetError):
            pass
        except BaseException as error:
            errors.append(error)
        finally:
            server.close()

    worker = threading.Thread(target=serve, daemon=True)
    worker.start()
    client = None
    try:
        client = WebSocketClient(
            client_socket, "local-test", time.perf_counter() + budget
        )
        yield client
    finally:
        if client is not None:
            client.close()
        else:
            client_socket.close()
        worker.join(timeout=3)
        if worker.is_alive():
            raise AssertionError("fixture peer did not stop")
        if errors:
            raise errors[0]


class WebSocketClientTests(unittest.TestCase):
    def test_pong_is_not_returned_as_application_text(self):
        with peer(
            lambda sock: sock.sendall(
                server_frame(b"heartbeat", opcode=10) + server_frame(b'{"id":1}')
            )
        ) as client:
            self.assertEqual(client.recv_text(), '{"id":1}')

    def test_fragmented_text_preserves_bytes_across_ping_and_pong(self):
        captured = []

        def send(sock):
            sock.sendall(
                server_frame(b'{"id":', fin=False)
                + server_frame(b"echo", opcode=9)
                + server_frame(b"ignored", opcode=10)
                + server_frame(b"1}", opcode=0)
            )
            reply = b""
            while len(reply) < 10:
                reply += sock.recv(4096)
            captured.append(reply)

        with peer(send) as client:
            self.assertEqual(client.recv_text(), '{"id":1}')
        frame = captured[0]
        self.assertEqual(frame[:2], bytes([0x8A, 0x84]))
        mask = frame[2:6]
        self.assertEqual(
            bytes(byte ^ mask[i % 4] for i, byte in enumerate(frame[6:10])), b"echo"
        )

    def test_wrong_upgrade_accept_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, "upgrade accept"):
            with peer(lambda sock: None, bad_accept=True):
                pass

    def test_continuation_without_initial_text_is_rejected(self):
        with peer(lambda sock: sock.sendall(server_frame(b"bad", opcode=0))) as client:
            with self.assertRaisesRegex(RuntimeError, "continuation without"):
                client.recv_text()

    def test_new_text_during_fragmented_message_is_rejected(self):
        with peer(
            lambda sock: sock.sendall(
                server_frame(b"first", fin=False) + server_frame(b"second")
            )
        ) as client:
            with self.assertRaisesRegex(RuntimeError, "during fragmented"):
                client.recv_text()

    def test_trickled_frame_cannot_reset_absolute_deadline(self):
        def trickle(sock):
            for byte in server_frame(b"x" * 80):
                sock.sendall(bytes([byte]))
                time.sleep(0.01)

        started = time.perf_counter()
        with peer(trickle, budget=0.15) as client:
            with self.assertRaisesRegex(TimeoutError, "deadline"):
                client.recv_text()
        self.assertLess(time.perf_counter() - started, 0.7)

    def test_aggregate_message_limit_applies_across_fragments(self):
        with patch("d4_websocket.MAX_MESSAGE_BYTES", 8):
            with peer(
                lambda sock: sock.sendall(
                    server_frame(b"123456", fin=False)
                    + server_frame(b"abcdef", opcode=0)
                )
            ) as client:
                with self.assertRaisesRegex(RuntimeError, "message exceeds"):
                    client.recv_text()

    def test_invalid_control_and_oversized_frames_fail_before_payload_read(self):
        for frame in [
            server_frame(b"ping", opcode=9, fin=False),
            bytes([0x89, 126]) + struct.pack("!H", 126),
        ]:
            with self.assertRaisesRegex(ValueError, "control frame"):
                decode_server_frame(frame)
        with self.assertRaisesRegex(ValueError, "capture limit"):
            decode_server_frame(
                bytes([0x81, 127]) + struct.pack("!Q", MAX_MESSAGE_BYTES + 1)
            )


if __name__ == "__main__":
    unittest.main()
