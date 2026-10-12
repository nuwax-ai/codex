"""Exercise the action's actual shell with synthetic artifacts and real hashes."""

import hashlib
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest


ACTION = Path(__file__).resolve().parents[1] / "actions/setup-rusty-v8/action.yml"
BASH = shutil.which("bash")
HOST = "aarch64-apple-darwin"


def action_shell():
    lines = ACTION.read_text().splitlines()
    blocks = [
        (index, len(match[1]) + 2)
        for index, line in enumerate(lines)
        if (match := re.fullmatch(r"(\s+)run: \|", line))
    ]
    if len(blocks) != 1:
        raise ValueError("Expected exactly one action shell block")
    start, indentation = blocks[0]
    body = []
    for line in lines[start + 1 :]:
        if line.strip() and len(line) - len(line.lstrip()) < indentation:
            break
        body.append(line[indentation:] if line.strip() else "")
    return "\n".join(body) + "\n"


@unittest.skipUnless(os.name == "posix" and BASH, "POSIX bash is required")
@unittest.skipUnless(
    shutil.which("sha256sum") or shutil.which("shasum"), "SHA256 checker required"
)
class SetupRustyV8Tests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.workspace = self.root / "workspace"
        self.remote = self.root / "remote"
        self.bin = self.root / "bin"
        for directory in (self.workspace / ".github/scripts", self.remote, self.bin):
            directory.mkdir(parents=True)
        (self.workspace / ".github/scripts/rusty_v8_bazel.py").write_text(
            "import sys\nassert sys.argv[1:] == ['resolved-v8-crate-version']\nprint('150.4.0')\n"
        )
        self.trust = (
            self.workspace / "third_party/v8/rusty_v8_150_4_0_release_manifests.sha256"
        )
        self.trust.parent.mkdir(parents=True)
        self.calls = self.root / "calls"
        self.exports = self.root / "environment"
        self.script = self.root / "action.sh"
        self.script.write_text(action_shell())
        self.command("python3", f'exec {shlex.quote(sys.executable)} "$@"\n')
        self.command(
            "rustc",
            """printf 'rustc:%s\\n' "$*" >> "$CALL_LOG"
[[ "$*" == -vV ]] || exit 9
printf 'rustc 1.99.0\\nhost: %s\\n' "$FAKE_HOST"
""",
        )
        curl = self.root / "curl.py"
        curl.write_text("""import os, pathlib, shutil, sys
args = sys.argv[1:]
assert len(args) == 4 and args[0] == "-fsSL" and args[2] == "-o"
assert args[1].startswith("https://github.com/openai/codex/releases/download/rusty-v8-v150.4.0/")
with open(os.environ["CALL_LOG"], "a") as output:
    output.write("curl:" + args[1].rsplit("/", 1)[1] + "\\n")
shutil.copyfile(pathlib.Path(os.environ["FAKE_REMOTE"]) / args[1].rsplit("/", 1)[1], args[3])
""")
        self.command(
            "curl",
            f'exec {shlex.quote(sys.executable)} {shlex.quote(str(curl))} "$@"\n',
        )

    def command(self, name, body):
        path = self.bin / name
        path.write_text(f"#!{BASH}\n{body}")
        path.chmod(0o755)

    def seed_pair(self, target=HOST, crlf=False):
        profile = f"ptrcomp_sandbox_release_{target}"
        archive = (
            f"rusty_v8_{profile}.lib.gz"
            if target.endswith("-pc-windows-msvc")
            else f"librusty_v8_{profile}.a.gz"
        )
        binding = f"src_binding_{profile}.rs"
        manifest = f"rusty_v8_{profile}.sha256"
        rows = []
        for name, content in (
            (archive, b"synthetic archive"),
            (binding, b"synthetic binding"),
        ):
            (self.remote / name).write_bytes(content)
            rows.append(f"{hashlib.sha256(content).hexdigest()}  {name}")
        raw = (
            ("\r\n" if crlf else "\n").join(rows) + ("\r\n" if crlf else "\n")
        ).encode()
        (self.remote / manifest).write_bytes(raw)
        self.trust.write_text(f"{hashlib.sha256(raw).hexdigest()}  {manifest}\n")
        return archive, binding, manifest

    def run_action(self, target="host", host=HOST):
        self.calls.write_text("")
        self.exports.write_text("")
        result = subprocess.run(
            [BASH, str(self.script)],
            cwd=self.workspace,
            env={
                **os.environ,
                "PATH": f"{self.bin}{os.pathsep}{os.environ.get('PATH', '')}",
                "TARGET": target,
                "FAKE_HOST": host,
                "FAKE_REMOTE": str(self.remote),
                "CALL_LOG": str(self.calls),
                "GITHUB_WORKSPACE": str(self.workspace),
                "RUNNER_TEMP": str(self.root / "runner"),
                "GITHUB_ENV": str(self.exports),
            },
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        return result, self.calls.read_text().splitlines(), self.exports.read_text()

    def test_host_resolves_the_rustc_native_target_and_exports_verified_pair(self):
        for host in (HOST, "x86_64-unknown-linux-gnu"):
            with self.subTest(host=host):
                archive, binding, manifest = self.seed_pair(host)
                result, calls, exports = self.run_action(host=host)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(
                    calls,
                    [
                        "rustc:-vV",
                        f"curl:{manifest}",
                        f"curl:{archive}",
                        f"curl:{binding}",
                    ],
                )
                self.assertEqual(
                    exports,
                    f"RUSTY_V8_ARCHIVE={self.root}/runner/rusty_v8/{archive}\n"
                    f"RUSTY_V8_SRC_BINDING_PATH={self.root}/runner/rusty_v8/{binding}\n",
                )

    def test_explicit_windows_target_uses_lib_archive_and_accepts_crlf_manifest(self):
        target = "x86_64-pc-windows-msvc"
        archive, binding, manifest = self.seed_pair(target, crlf=True)
        result, calls, exports = self.run_action(target=target)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(archive.endswith(".lib.gz"))
        self.assertEqual(
            calls, [f"curl:{manifest}", f"curl:{archive}", f"curl:{binding}"]
        )
        self.assertIn(archive, exports)

    def test_unsupported_and_empty_host_fail_before_curl(self):
        self.seed_pair()
        for target, host in (
            ("host", ""),
            ("host", "unsupported-target"),
            ("host", "bad/target"),
            ("unsupported-target", HOST),
            ("", HOST),
        ):
            with self.subTest(target=target, host=host):
                result, calls, exports = self.run_action(target, host)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(any(call.startswith("curl:") for call in calls))
                self.assertEqual(exports, "")

    def test_manifest_bytes_mismatch_stops_before_artifacts_and_exports(self):
        _, _, manifest = self.seed_pair(crlf=True)
        path = self.remote / manifest
        path.write_bytes(path.read_bytes().replace(b"\r\n", b"\n"))
        result, calls, exports = self.run_action()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Checksum mismatch", result.stderr)
        self.assertEqual(calls, ["rustc:-vV", f"curl:{manifest}"])
        self.assertEqual(exports, "")

    def test_archive_and_binding_tampering_never_export_overrides(self):
        for asset in (0, 1):
            with self.subTest(asset=asset):
                names = self.seed_pair()
                (self.remote / names[asset]).write_bytes(b"tampered bytes")
                result, calls, exports = self.run_action()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(
                    calls,
                    [
                        "rustc:-vV",
                        f"curl:{names[2]}",
                        f"curl:{names[0]}",
                        f"curl:{names[1]}",
                    ],
                )
                self.assertEqual(exports, "")

    def test_duplicate_trusted_manifest_entry_is_rejected_before_curl(self):
        self.seed_pair()
        self.trust.write_text(self.trust.read_text() * 2)
        result, calls, exports = self.run_action()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("No unique trusted V8 checksum manifest", result.stderr)
        self.assertEqual(calls, ["rustc:-vV"])
        self.assertEqual(exports, "")


if __name__ == "__main__":
    unittest.main()
