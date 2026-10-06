"""Negative controls must observe network attempts, including just before exit."""

from pathlib import Path
import sys
import tempfile
import unittest

from bazel_bridge_gate_negative import check_binary


class BridgeNegativeControlTests(unittest.TestCase):
    def run_child(self, source: str) -> tuple[list[str], int]:
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory, "child.py")
            script.write_text(source)
            return check_binary([sys.executable, str(script)])

    def test_bridge_refusal_without_a_connection_passes(self):
        self.assertEqual(
            self.run_child(
                "import sys\nprint('requires a model bridge', file=sys.stderr)\nsys.exit(1)\n"
            ),
            ([], 0),
        )

    def test_connection_before_child_exit_is_rejected(self):
        problems, count = self.run_child(
            "import os, socket, sys\n"
            "from urllib.parse import urlsplit\n"
            "url = urlsplit(os.environ['NUWAX_BASE_URL'])\n"
            "with socket.create_connection((url.hostname, url.port)):\n    pass\n"
            "print('requires a model bridge', file=sys.stderr)\nsys.exit(1)\n"
        )
        self.assertEqual(count, 1)
        self.assertEqual(problems, ["listener saw 1 connection(s)"])

    def test_successful_exit_cannot_pass_the_negative_control(self):
        self.assertEqual(
            self.run_child(
                "import sys\nprint('requires a model bridge', file=sys.stderr)\n"
            ),
            (["process exited 0; a bridges-off binary must fail"], 0),
        )

    def test_unrelated_failure_cannot_pass_the_negative_control(self):
        self.assertEqual(
            self.run_child(
                "import sys\nprint('other failure', file=sys.stderr)\nsys.exit(1)\n"
            ),
            (["stderr does not name the missing model bridge"], 0),
        )


if __name__ == "__main__":
    unittest.main()
