import hashlib
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile

from package_nuwax_release import TARGETS, bundled_ripgrep, package


class ReleasePackageContracts(unittest.TestCase):
    def fixture(self, root, target):
        build = root / "build"
        build.mkdir()
        ext = ".exe" if "windows" in target else ""
        names = ["codex", "codex-code-mode-host"]
        if "windows" in target:
            names += ["codex-windows-sandbox-setup", "codex-command-runner"]
        if "linux" in target:
            names += ["bwrap"]
        for name in names:
            path = build / (name + ext)
            path.write_bytes(("fresh binary " + name).encode())
            path.chmod(0o755)
        rg = build / ("bundled-rg" + ext)
        rg.write_bytes(b"fresh ripgrep")
        rg.chmod(0o755)
        return build, ext, rg

    def read_archive(self, path):
        if path.suffix == ".zip":
            with zipfile.ZipFile(path) as archive:
                return {name: archive.read(name) for name in archive.namelist()}
        with tarfile.open(path) as archive:
            return {entry.name: archive.extractfile(entry).read() for entry in archive}

    def test_all_seven_targets_ship_single_main_binary_and_required_helpers(self):
        for target in TARGETS:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                build, ext, rg = self.fixture(root, target)
                path = package(build, root / "out", "0.18.13", target, ripgrep=rg)
                entries = self.read_archive(path)
                self.assertEqual(entries["nuwax-harness" + ext], (build / ("codex" + ext)).read_bytes())
                self.assertNotIn("nuwax-codex" + ext, entries,
                                 "legacy alias must stay npm-layer only (JS forwarding)")
                self.assertIn("codex-code-mode-host" + ext, entries)
                if "linux" in target:
                    self.assertIn("codex-resources/bwrap", entries)
                if "windows" in target:
                    self.assertIn("codex-windows-sandbox-setup.exe", entries)
                    self.assertIn("codex-command-runner.exe", entries)
                # A package that describes itself is what lets the engine find
                # `codex-path` (bundled rg) and `codex-resources`.
                metadata = json.loads(entries["codex-package.json"])
                self.assertEqual(metadata["layoutVersion"], 1)
                self.assertEqual(metadata["entrypoint"], "nuwax-harness" + ext)
                self.assertEqual(metadata["pathDir"], "codex-path")
                self.assertEqual(metadata["resourcesDir"], "codex-resources")
                self.assertEqual(metadata["target"], target)
                self.assertEqual(entries["codex-path/rg" + ext], b"fresh ripgrep",
                                 "the bundled ripgrep must ship inside the archive")
                for line in entries["SHA256SUMS"].decode().splitlines():
                    digest, name = line.split("  ", 1)
                    self.assertEqual(digest, hashlib.sha256(entries[name]).hexdigest())
                self.assertTrue((build / ("codex" + ext)).is_file(), "packaging must not rename build outputs")

    def test_missing_runtime_helper_cannot_produce_a_publishable_archive(self):
        for target in TARGETS:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                build, ext, rg = self.fixture(root, target)
                (build / ("codex-code-mode-host" + ext)).unlink()
                with self.assertRaisesRegex(FileNotFoundError, "code-mode-host"):
                    package(build, root / "out", "0.18.13", target, ripgrep=rg)
                self.assertFalse((root / "out").exists())

    def test_missing_platform_helper_is_rejected(self):
        for target, name in (("x86_64-unknown-linux-musl", "bwrap"),
                             ("x86_64-pc-windows-msvc", "codex-command-runner.exe")):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                build, _, rg = self.fixture(root, target)
                (build / name).unlink()
                with self.assertRaises(FileNotFoundError):
                    package(build, root / "out", "0.18.13", target, ripgrep=rg)

    def test_invalid_version_and_stale_archive_fail_without_overwriting(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            target = "aarch64-apple-darwin"
            build, _, rg = self.fixture(root, target)
            with self.assertRaises(ValueError):
                package(build, root / "out", "../escape", target, ripgrep=rg)
            archive = package(build, root / "out", "0.18.13", target, ripgrep=rg)
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            with self.assertRaises(FileExistsError):
                package(build, root / "out", "0.18.13", target, ripgrep=rg)
            self.assertEqual(digest, hashlib.sha256(archive.read_bytes()).hexdigest())

    def test_replace_existing_lets_a_retry_overwrite_the_stale_archive(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            target = "x86_64-unknown-linux-gnu"
            build, _, rg = self.fixture(root, target)
            stale = package(build, root / "out", "0.18.13", target, ripgrep=rg)
            stale.write_bytes(b"leftover from an aborted run\n")
            rebuilt = package(build, root / "out", "0.18.13", target, ripgrep=rg, replace_existing=True)
            self.assertEqual(stale, rebuilt)
            self.assertNotIn(b"leftover", rebuilt.read_bytes())

    def test_target_without_an_official_musl_ripgrep_ships_metadata_but_no_rg(self):
        # aarch64 Linux musl has only a glibc rg upstream; shipping that inside
        # a static package would fail on musl-only hosts, which is worse than
        # letting the engine fall back to a system rg.
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            build, ext, _rg = self.fixture(root, "aarch64-unknown-linux-musl")
            path = package(build, root / "out", "0.18.13", "aarch64-unknown-linux-musl",
                           ripgrep=bundled_ripgrep("aarch64-unknown-linux-musl"))
            entries = self.read_archive(path)
            self.assertIn("codex-package.json", entries,
                          "the package stays self-describing either way")
            self.assertNotIn("codex-path/rg", entries)


if __name__ == "__main__":
    unittest.main()
