"""Check probe classification with fake commands; no Linux capability is tested."""

import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("bwrap_capability_check.sh")
BASH = shutil.which("bash")
OPTIONS = (
    "--as-pid-1 --perms --new-session --die-with-parent --unshare-user "
    "--unshare-ipc --unshare-pid --unshare-net --ro-bind --proc --dev --cap-drop"
)


@unittest.skipUnless(
    os.name == "posix" and BASH, "POSIX bash is required for fake Linux probes"
)
class BwrapCapabilityCheckTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.call_log = self.directory / "calls"
        # macOS has /usr/bin/true. Patch only the availability guard in a
        # temporary copy; fake bwrap still receives the real Linux /bin/true.
        script_text = SCRIPT.read_text()
        availability_guard = "[[ ! -x /bin/true ]]"
        self.assertEqual(script_text.count(availability_guard), 1)
        local_true = shutil.which("true")
        self.assertIsNotNone(local_true)
        self.probe_script = self.directory / "probe.sh"
        self.probe_script.write_text(
            script_text.replace(
                availability_guard, f"[[ ! -x {shlex.quote(local_true)} ]]"
            )
        )
        self.environment = {
            **os.environ,
            "PATH": f"{self.directory}{os.pathsep}{os.environ.get('PATH', '')}",
            "FAKE_CALL_LOG": str(self.call_log),
            "FAKE_OPTIONS": OPTIONS,
            "FAKE_PLATFORM": "Linux",
            "FAKE_BASELINE_STATUS": "0",
            "FAKE_NETNS_STATUS": "0",
            "FAKE_HELP_STATUS": "0",
            "FAKE_BASELINE_OUTPUT": "",
            "FAKE_NETNS_OUTPUT": "",
            "FAKE_TIMEOUT_PHASE": "",
            "FAKE_TIMEOUT_STATUS": "124",
        }
        self.write_command("uname", 'printf "%s\\n" "$FAKE_PLATFORM"\n')
        self.write_command(
            "bwrap",
            """{ printf 'bwrap'; printf ' <%s>' "$@"; printf '\\n'; } >> "$FAKE_CALL_LOG"
if [[ "$1" == --help ]]; then
  printf '%s\\n' "$FAKE_OPTIONS"
  exit "$FAKE_HELP_STATUS"
fi
for arg in "$@"; do
  if [[ "$arg" == --unshare-net ]]; then
    printf '%s\\n' "$FAKE_NETNS_OUTPUT" >&2
    exit "$FAKE_NETNS_STATUS"
  fi
done
printf '%s\\n' "$FAKE_BASELINE_OUTPUT" >&2
exit "$FAKE_BASELINE_STATUS"
""",
        )
        self.write_command(
            "timeout",
            """{ printf 'timeout'; printf ' <%s>' "$@"; printf '\\n'; } >> "$FAKE_CALL_LOG"
[[ "$1" == --verbose && "$2" == --signal=TERM && "$3" == --kill-after=1s ]] || exit 125
shift 4
phase=baseline
for arg in "$@"; do
  [[ "$arg" == --help ]] && phase=help
  [[ "$arg" == --unshare-net ]] && phase=netns
done
if [[ "$phase" == "$FAKE_TIMEOUT_PHASE" ]]; then
  printf 'timeout: sending signal TERM to command fake-bwrap\\n' >&2
  if [[ "$FAKE_TIMEOUT_STATUS" == 137 ]]; then
    printf 'timeout: sending signal KILL to command fake-bwrap\\n' >&2
  fi
  exit "$FAKE_TIMEOUT_STATUS"
fi
exec "$@"
""",
        )

    def write_command(self, name, body):
        command = self.directory / name
        command.write_text(f"#!{BASH}\n{body}")
        command.chmod(0o755)
        return command

    def run_probe(self, *arguments, **environment):
        return subprocess.run(
            [BASH, str(self.probe_script), *arguments],
            env={**self.environment, **environment},
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )

    def test_success_uses_codex_options_and_a_deadline_for_every_invocation(self):
        result = self.run_probe("--timeout", "7")
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.call_log.read_text().splitlines()
        self.assertEqual(sum(line.startswith("timeout") for line in calls), 3)
        self.assertTrue(
            all("<7s>" in line for line in calls if line.startswith("timeout"))
        )
        for line in (
            line
            for line in calls
            if line.startswith("bwrap") and "<--help>" not in line
        ):
            for option in (
                "--new-session",
                "--die-with-parent",
                "--as-pid-1",
                "--cap-drop",
                "ALL",
                "--",
            ):
                self.assertIn(f"<{option}>", line)
        self.assertIn("acceptance remain unverified", result.stderr)

    def test_normal_and_verbose_preserve_network_permission_classification(self):
        for signature in ("RTM_NEWADDR", "RTM_NEWLINK"):
            with self.subTest(signature=signature):
                environment = {
                    "FAKE_NETNS_STATUS": "1",
                    "FAKE_NETNS_OUTPUT": f"bwrap: loopback: Failed {signature}: Operation not permitted",
                }
                normal = self.run_probe(**environment)
                verbose = self.run_probe("-v", **environment)
                self.assertEqual((normal.returncode, verbose.returncode), (3, 3))
                self.assertIn(signature, normal.stderr)
                self.assertIn(signature, verbose.stderr)

    def test_baseline_classifies_only_explicit_permission_evidence(self):
        cases = (
            ("bwrap: setting up uid map: Permission denied", 2),
            ("bwrap: No permissions to create new namespace", 2),
            ("bwrap: Creating new namespace failed: Operation not permitted", 2),
            ("bwrap: bind mount failed: Permission denied", 4),
            ("bwrap: Unknown option --as-pid-1", 4),
        )
        for output, expected in cases:
            with self.subTest(output=output):
                self.call_log.unlink(missing_ok=True)
                result = self.run_probe(
                    FAKE_BASELINE_STATUS="1", FAKE_BASELINE_OUTPUT=output
                )
                self.assertEqual(result.returncode, expected, result.stderr)
                self.assertNotIn("<--unshare-net>", self.call_log.read_text())

    def test_netlink_failure_without_permission_evidence_is_unexpected(self):
        result = self.run_probe(
            FAKE_NETNS_STATUS="1",
            FAKE_NETNS_OUTPUT="bwrap: loopback: Failed RTM_NEWADDR: Invalid argument",
        )
        self.assertEqual(result.returncode, 4, result.stderr)

    def test_timeout_is_distinct_in_every_phase_and_verbose_mode(self):
        for phase in ("help", "baseline", "netns"):
            for status in ("124", "137"):
                for arguments in ((), ("-v",)):
                    with self.subTest(phase=phase, status=status, arguments=arguments):
                        result = self.run_probe(
                            *arguments,
                            FAKE_TIMEOUT_PHASE=phase,
                            FAKE_TIMEOUT_STATUS=status,
                        )
                        self.assertEqual(result.returncode, 5, result.stderr)
                        self.assertIn(f"TIMEOUT: {phase}", result.stderr)

    def test_command_sigkill_without_timeout_diagnostic_is_unexpected(self):
        result = self.run_probe(FAKE_BASELINE_STATUS="137")
        self.assertEqual(result.returncode, 4, result.stderr)

    def test_incompatible_help_is_rejected_before_namespace_commands(self):
        for absent in ("--as-pid-1", "--perms", "--unshare-net"):
            with self.subTest(absent=absent):
                self.call_log.unlink(missing_ok=True)
                result = self.run_probe(FAKE_OPTIONS=OPTIONS.replace(absent, ""))
                self.assertEqual(result.returncode, 4, result.stderr)
                self.assertIn(f"does not advertise {absent}", result.stderr)
                self.assertNotIn("</bin/true>", self.call_log.read_text())

    def test_failed_help_and_similar_option_names_are_not_accepted(self):
        failed = self.run_probe(FAKE_HELP_STATUS="1")
        self.assertEqual(failed.returncode, 4, failed.stderr)
        self.call_log.unlink()
        similar = self.run_probe(
            FAKE_OPTIONS=OPTIONS.replace("--unshare-user", "--unshare-user-try")
        )
        self.assertEqual(similar.returncode, 4, similar.stderr)
        self.assertNotIn("</bin/true>", self.call_log.read_text())

    def test_explicit_executable_is_used_and_missing_executable_is_distinct(self):
        selected = self.directory / "selected-bwrap"
        (self.directory / "bwrap").rename(selected)
        result = self.run_probe("--bwrap", str(selected))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"Selected bwrap: {selected}", result.stderr)
        missing = self.run_probe("--bwrap", str(self.directory / "missing"))
        self.assertEqual(missing.returncode, 1, missing.stderr)

    def test_missing_timeout_fails_before_any_bwrap_invocation(self):
        (self.directory / "timeout").unlink()
        result = self.run_probe(PATH=str(self.directory))
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("GNU timeout is required", result.stderr)
        self.assertFalse(self.call_log.exists())

    def test_non_linux_is_skip_and_help_is_available_without_probing(self):
        skipped = self.run_probe(FAKE_PLATFORM="Darwin")
        self.assertEqual(skipped.returncode, 77, skipped.stderr)
        self.assertFalse(self.call_log.exists())
        help_result = self.run_probe("--help", FAKE_PLATFORM="Darwin")
        self.assertEqual(help_result.returncode, 0, help_result.stderr)
        self.assertIn("Usage:", help_result.stdout)
        self.assertFalse(self.call_log.exists())

    def test_invalid_arguments_fail_before_any_probe(self):
        for arguments in (
            ("--unknown",),
            ("--bwrap",),
            ("--bwrap", "-v"),
            ("--timeout", "0"),
            ("--timeout", "61"),
        ):
            with self.subTest(arguments=arguments):
                result = self.run_probe(*arguments)
                self.assertEqual(result.returncode, 64, result.stderr)
                self.assertFalse(self.call_log.exists())


if __name__ == "__main__":
    unittest.main()
