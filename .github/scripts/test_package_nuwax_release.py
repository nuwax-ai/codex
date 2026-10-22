import hashlib
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile

from package_nuwax_release import TARGETS, package


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
        return build, ext

    def read_archive(self, path):
        if path.suffix == ".zip":
            with zipfile.ZipFile(path) as archive:
                return {name: archive.read(name) for name in archive.namelist()}
        with tarfile.open(path) as archive:
            return {entry.name: archive.extractfile(entry).read() for entry in archive}

    def test_all_seven_targets_include_matching_main_alias_and_required_helpers(self):
        for target in TARGETS:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                build, ext = self.fixture(root, target)
                path = package(build, root / "out", "0.18.13", target)
                entries = self.read_archive(path)
                self.assertEqual(entries["nuwax-harness" + ext], (build / ("codex" + ext)).read_bytes())
                self.assertEqual(entries["nuwax-codex" + ext], entries["nuwax-harness" + ext])
                self.assertIn("codex-code-mode-host" + ext, entries)
                if "linux" in target:
                    self.assertIn("codex-resources/bwrap", entries)
                if "windows" in target:
                    self.assertIn("codex-windows-sandbox-setup.exe", entries)
                    self.assertIn("codex-command-runner.exe", entries)
                for line in entries["SHA256SUMS"].decode().splitlines():
                    digest, name = line.split("  ", 1)
                    self.assertEqual(digest, hashlib.sha256(entries[name]).hexdigest())
                self.assertTrue((build / ("codex" + ext)).is_file(), "packaging must not rename build outputs")

    def test_missing_runtime_helper_cannot_produce_a_publishable_archive(self):
        for target in TARGETS:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                build, ext = self.fixture(root, target)
                (build / ("codex-code-mode-host" + ext)).unlink()
                with self.assertRaisesRegex(FileNotFoundError, "code-mode-host"):
                    package(build, root / "out", "0.18.13", target)
                self.assertFalse((root / "out").exists())

    def test_missing_platform_helper_is_rejected(self):
        for target, name in (("x86_64-unknown-linux-musl", "bwrap"),
                             ("x86_64-pc-windows-msvc", "codex-command-runner.exe")):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                build, _ = self.fixture(root, target)
                (build / name).unlink()
                with self.assertRaises(FileNotFoundError):
                    package(build, root / "out", "0.18.13", target)

    def test_invalid_version_and_stale_archive_fail_without_overwriting(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            target = "aarch64-apple-darwin"
            build, _ = self.fixture(root, target)
            with self.assertRaises(ValueError):
                package(build, root / "out", "../escape", target)
            archive = package(build, root / "out", "0.18.13", target)
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            with self.assertRaises(FileExistsError):
                package(build, root / "out", "0.18.13", target)
            self.assertEqual(digest, hashlib.sha256(archive.read_bytes()).hexdigest())


if __name__ == "__main__":
    unittest.main()
