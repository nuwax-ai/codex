"""Validate framing and a real deadline with isolated subprocess fixtures."""

from pathlib import Path
import sys
import tempfile
import time
import unittest

from d4_stage_profiling import (
    build_upgrade_request,
    decode_server_frame,
    encode_client_text_frame,
    parse_websocket_banner,
    profile_once,
    strip_ansi,
)

HANDSHAKE = (
    "import json, sys\n"
    "assert sys.argv[1:] == ['--listen', 'stdio://']\n"
    "request = json.loads(sys.stdin.readline())\n"
    "assert request['method'] == 'initialize'\n"
    "print('starting', file=sys.stderr, flush=True)\n"
)


class StageProfilingTests(unittest.TestCase):
    def run_child(self, source: str, *, budget: float = 2):
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory, "child.py")
            script.write_text(source)
            return profile_once(
                [sys.executable, str(script)], rpc_budget_seconds=budget
            )

    def test_json_line_initialize_measures_successful_roundtrip(self):
        result = self.run_child(
            HANDSHAKE
            + "print(json.dumps({'id': request['id'], 'result': {'userAgent': 'fixture'}}), flush=True)\n"
            + "assert json.loads(sys.stdin.readline())['method'] == 'initialized'\n"
            + "sys.stdin.read()\n"
        )
        self.assertIsNotNone(result["initialize_roundtrip_s"])
        self.assertIsNotNone(result["first_stderr_s"])
        self.assertEqual(
            {key: result[key] for key in ("bind_report_s", "bind_phase")},
            {"bind_report_s": None, "bind_phase": "not_applicable_stdio"},
        )

    def test_error_response_is_not_a_successful_initialize(self):
        with self.assertRaisesRegex(RuntimeError, "successful result"):
            self.run_child(
                HANDSHAKE
                + "print(json.dumps({'id': 1, 'error': {'code': -32600}}), flush=True)\n"
            )

    def test_silent_open_stdout_obeys_the_response_deadline(self):
        started = time.perf_counter()
        with self.assertRaisesRegex(TimeoutError, "deadline expired"):
            self.run_child(HANDSHAKE + "sys.stdin.read()\n", budget=0.2)
        self.assertLess(time.perf_counter() - started, 3)

    def test_eof_before_initialize_cannot_report_a_measurement(self):
        with self.assertRaisesRegex(RuntimeError, "closed stdout"):
            self.run_child(HANDSHAKE)


class BannerParsingTests(unittest.TestCase):
    def test_plain_banner_line_yields_host_and_port(self):
        self.assertEqual(
            parse_websocket_banner("  listening on: ws://127.0.0.1:52341"),
            ("127.0.0.1", 52341),
        )

    def test_ansi_styled_banner_still_parses(self):
        line = "  \x1b[2mlistening on:\x1b[0m \x1b[32mws://127.0.0.1:0\x1b[0m"
        self.assertEqual(parse_websocket_banner(line), ("127.0.0.1", 0))

    def test_other_lines_do_not_parse(self):
        for line in (
            "codex app-server (WebSockets)",
            "  readyz: http://127.0.0.1:1/readyz",
            "",
        ):
            self.assertIsNone(parse_websocket_banner(line))

    def test_strip_ansi_removes_color_sequences_only(self):
        self.assertEqual(strip_ansi("\x1b[1mws://x\x1b[0m"), "ws://x")


class FrameCodecTests(unittest.TestCase):
    def test_small_frame_is_masked_and_recovers_payload(self):
        key = bytes(range(4))
        frame = encode_client_text_frame(b"hello", mask_key=key)
        self.assertEqual(frame[0], 0x81)
        self.assertEqual(frame[1], 0x80 | 5)
        self.assertEqual(frame[2:6], key)
        self.assertEqual(
            bytes(b ^ key[i % 4] for i, b in enumerate(frame[6:])), b"hello"
        )

    def test_medium_and_large_frames_use_extended_lengths(self):
        key = bytes(4)
        medium = encode_client_text_frame(b"x" * 200, mask_key=key)
        self.assertEqual(medium[1], 0x80 | 126)
        large = encode_client_text_frame(b"y" * 70000, mask_key=key)
        self.assertEqual(large[1], 0x80 | 127)
        for payload, frame in ((b"x" * 200, medium), (b"y" * 70000, large)):
            self.assertGreater(len(frame), len(payload))

    def test_mask_key_must_be_four_bytes(self):
        with self.assertRaises(ValueError):
            encode_client_text_frame(b"hi", mask_key=b"123")

    def test_decode_server_frame_roundtrip_and_errors(self):
        payload = b'{"id": 1, "result": {}}'
        frame = bytes([0x81, len(payload)]) + payload
        opcode, fin, decoded, consumed = decode_server_frame(frame + b"trailing")
        self.assertEqual(
            (opcode, fin, decoded, consumed), (1, True, payload, len(frame))
        )
        with self.assertRaisesRegex(ValueError, "masked"):
            decode_server_frame(bytes([0x81, 0x80 | 3]) + b"abc")
        with self.assertRaisesRegex(ValueError, "incomplete"):
            decode_server_frame(frame[:-1])


class UpgradeRequestTests(unittest.TestCase):
    def test_request_has_required_headers_and_no_origin(self):
        request = build_upgrade_request("127.0.0.1:1234").decode()
        self.assertIn("GET / HTTP/1.1\r\n", request)
        self.assertIn("Host: 127.0.0.1:1234\r\n", request)
        self.assertIn("Upgrade: websocket\r\n", request)
        self.assertIn("Connection: Upgrade\r\n", request)
        self.assertIn("Sec-WebSocket-Version: 13\r\n", request)
        self.assertNotIn("Origin", request)
        self.assertTrue(request.endswith("\r\n\r\n"))


class ModeDispatchTests(unittest.TestCase):
    def test_unknown_mode_fails_fast(self):
        with self.assertRaisesRegex(ValueError, "unknown mode"):
            profile_once(["true"], mode="carrier-pigeon")


if __name__ == "__main__":
    unittest.main()
