#!/usr/bin/env python3
"""Hermetic shell-entry regressions; curl is replaced and never uses a network."""

import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest


SCRIPT = Path(__file__).with_name("prefetch_rusty_v8.sh")
VERSION = "152.2.0"
TARGET = "aarch64-apple-darwin"
ARCHIVE = f"librusty_v8_ptrcomp_sandbox_release_{TARGET}.a.gz"
BINDING = f"src_binding_ptrcomp_sandbox_release_{TARGET}.rs"


class PrefetchRustyV8Test(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(prefix="prefetch-v8-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.script = self.root / "scripts/prefetch_rusty_v8.sh"
        self.script.parent.mkdir()
        shutil.copy2(SCRIPT, self.script)
        helper = self.root / ".github/scripts/rusty_v8_bazel.py"
        helper.parent.mkdir(parents=True)
        helper.write_text(f"print({VERSION!r})\n")
        manifest = self.root / f"third_party/v8/rusty_v8_{VERSION.replace('.', '_')}.sha256"
        manifest.parent.mkdir(parents=True)
        self.payloads = self.root / "payloads"
        self.payloads.mkdir()
        manifest_lines = []
        for name in (ARCHIVE, BINDING):
            data = (name + "\n").encode()
            (self.payloads / name).write_bytes(data)
            manifest_lines.append(f"{hashlib.sha256(data).hexdigest()}  {name}\n")
        manifest.write_text("".join(manifest_lines))
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        curl = bin_dir / "curl"
        curl.write_text(
            f"#!{sys.executable}\n"
            + textwrap.dedent(
                """\
                import os
                from pathlib import Path
                import sys
                import time

                args = sys.argv[1:]
                name = next(arg for arg in args if arg.startswith('https://')).rsplit('/', 1)[1]
                output = Path(args[args.index('-o') + 1])
                payload = Path(os.environ['FIXTURE_PAYLOADS']) / name
                output.write_bytes(payload.read_bytes())
                if os.environ.get('FIXTURE_BAD_DOWNLOAD'):
                    output.write_bytes(b'corrupt')
                if os.environ.get('FIXTURE_DOWNLOAD_FAILURE'):
                    sys.exit(22)
                if os.environ.get('FIXTURE_CONCURRENT') and name.endswith('.a.gz'):
                    rendezvous = Path(os.environ['FIXTURE_RENDEZVOUS'])
                    (rendezvous / str(os.getpid())).touch()
                    deadline = time.monotonic() + 5
                    while len(list(rendezvous.iterdir())) < 2:
                        if time.monotonic() >= deadline:
                            sys.exit('second concurrent downloader never arrived')
                        time.sleep(.01)
                """
            )
        )
        curl.chmod(0o755)
        self.cache = self.root / "cache"
        self.env = {
            **os.environ,
            "PATH": str(bin_dir) + os.pathsep + os.environ["PATH"],
            "FIXTURE_PAYLOADS": str(self.payloads),
        }

    def command(self, *args: str) -> list[str]:
        return [
            "bash", str(self.script), "--target", TARGET,
            "--cache-dir", str(self.cache), *args,
        ]

    def run_script(self, *args: str, **env: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            self.command(*args), cwd=self.root, env={**self.env, **env},
            text=True, capture_output=True, timeout=15,
        )

    def cached(self, name: str) -> Path:
        return self.cache / VERSION / TARGET / name

    def populate_cache(self) -> None:
        self.cached(ARCHIVE).parent.mkdir(parents=True)
        for name in (ARCHIVE, BINDING):
            shutil.copyfile(self.payloads / name, self.cached(name))

    def test_shell_exports_round_trip_paths_without_evaluation(self) -> None:
        self.cache = self.root / "cache with spaces $dollar 'quote' $(touch SOURCE_EVALUATED)"
        self.populate_cache()
        env_file = self.root / "v8.env"
        result = self.run_script("--offline", "--env-file", str(env_file))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, env_file.read_text())
        sourced = subprocess.run(
            ["bash", "-c", 'set -ae; source "$1"; test -f "$RUSTY_V8_ARCHIVE"; '
             'test -f "$RUSTY_V8_SRC_BINDING_PATH"; '
             'printf "%s\\n%s\\n" "$RUSTY_V8_ARCHIVE" "$RUSTY_V8_SRC_BINDING_PATH"',
             "bash", str(env_file)],
            cwd=self.root, text=True, capture_output=True, timeout=5,
        )
        self.assertEqual(sourced.returncode, 0, sourced.stderr)
        self.assertEqual(sourced.stdout.splitlines(), [str(self.cached(ARCHIVE)), str(self.cached(BINDING))])
        self.assertFalse((self.root / "SOURCE_EVALUATED").exists())

    def test_relative_cache_exports_absolute_paths(self) -> None:
        self.cache = Path("relative-cache")
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(str(self.root / self.cached(ARCHIVE)), result.stdout)

    def test_concurrent_downloads_share_only_verified_destinations(self) -> None:
        rendezvous = self.root / "rendezvous"
        rendezvous.mkdir()
        env = {**self.env, "FIXTURE_CONCURRENT": "1", "FIXTURE_RENDEZVOUS": str(rendezvous)}
        processes = [subprocess.Popen(
            self.command(), cwd=self.root, env=env,
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        ) for _ in range(2)]
        try:
            for process in processes:
                _stdout, stderr = process.communicate(timeout=15)
                self.assertEqual(process.returncode, 0, stderr)
        finally:
            for process in processes:
                if process.poll() is None:
                    process.kill()
                    process.communicate()
        for name in (ARCHIVE, BINDING):
            self.assertEqual(self.cached(name).read_bytes(), (self.payloads / name).read_bytes())
        self.assertEqual(list(self.cache.rglob("*.part*")), [])

    def test_offline_missing_pair_fails_without_downloading(self) -> None:
        result = self.run_script("--offline")
        self.assertEqual(result.returncode, 3, result.stderr)
        self.assertIn(ARCHIVE, result.stderr)
        self.assertIn(BINDING, result.stderr)
        self.assertFalse(self.cache.exists())

    def test_offline_corrupt_cached_artifact_is_removed(self) -> None:
        self.populate_cache()
        self.cached(BINDING).write_bytes(b"corrupt")
        result = self.run_script("--offline")
        self.assertEqual(result.returncode, 3, result.stderr)
        self.assertFalse(self.cached(BINDING).exists())
        self.assertTrue(self.cached(ARCHIVE).exists())

    def test_bad_download_is_not_published_or_left_partial(self) -> None:
        result = self.run_script(FIXTURE_BAD_DOWNLOAD="1")
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertFalse(self.cached(ARCHIVE).exists())
        self.assertEqual(list(self.cache.rglob("*.part*")), [])

    def test_failed_download_cleans_only_its_own_partial_file(self) -> None:
        other_partial = self.cached(ARCHIVE).with_suffix(".part.another-process")
        other_partial.parent.mkdir(parents=True)
        other_partial.write_bytes(b"another process still owns this")
        result = self.run_script(FIXTURE_DOWNLOAD_FAILURE="1")
        self.assertEqual(result.returncode, 3, result.stderr)
        self.assertEqual(list(self.cache.rglob("*.part*")), [other_partial])
        self.assertFalse(self.cached(ARCHIVE).exists())

    def test_unknown_target_fails_before_downloading(self) -> None:
        result = self.run_script("--target", "unknown-target")
        self.assertEqual(result.returncode, 4, result.stderr)
        self.assertFalse(self.cache.exists())


if __name__ == "__main__":
    unittest.main()
