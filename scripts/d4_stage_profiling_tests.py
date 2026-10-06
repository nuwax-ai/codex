"""Validate framing and a real deadline with isolated subprocess fixtures."""

from pathlib import Path
import sys
import tempfile
import time
import unittest

from d4_stage_profiling import profile_once

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


if __name__ == "__main__":
    unittest.main()
