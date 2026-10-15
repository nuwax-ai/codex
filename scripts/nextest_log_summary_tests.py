"""Exercise counting boundaries with realistic nextest result lines."""

import unittest

from nextest_log_summary import classify_failures, parse_lines


def result(status, ordinal, total, binary="codex-core", name="test_case"):
    return f"    {status} [  30.174s] ({ordinal:3}/{total}) {binary} {name}\n"


class NextestLogSummaryTests(unittest.TestCase):
    def test_empty_or_diagnostic_only_logs_are_not_complete_evidence(self):
        for lines in ([], ["stderr: fixture failed before nextest ran\n"]):
            with self.subTest(lines=lines):
                report = parse_lines(lines)
                self.assertFalse(report["complete_unambiguous"])
                self.assertEqual(report["issues"], [{"kind": "no_result_rows"}])

    def test_all_terminal_statuses_and_summary_reprints_count_once(self):
        rows = [
            result(status, i, 5, name=f"case_{i}")
            for i, status in enumerate(
                ["PASS", "FAIL", "TIMEOUT", "SIGABRT", "SIGSEGV"], 1
            )
        ]
        report = parse_lines(
            [
                " Nextest run ID run-1 with nextest profile: local\n",
                " SLOW [> 30.000s] (───────) codex-core case_4\n",
                *rows,
                " stderr: SECRET_REQUEST_PAYLOAD\n",
                " Summary [ 60.000s] 5 tests run: 1 passed, 3 failed, 1 timed out\n",
                *rows[1:],
            ]
        )
        self.assertEqual(
            report["counts"],
            {
                "executions": 5,
                "distinct_tests": 5,
                "non_pass_executions": 4,
                "non_pass_distinct_tests": 4,
                "non_pass_unique_names": 4,
                "statuses": {
                    "FAIL": 1,
                    "PASS": 1,
                    "SIGABRT": 1,
                    "SIGSEGV": 1,
                    "TIMEOUT": 1,
                },
                "repeated_result_lines": 4,
            },
        )
        self.assertTrue(report["complete_unambiguous"])
        self.assertNotIn("SECRET_REQUEST_PAYLOAD", str(report))

    def test_same_name_in_two_binaries_is_two_tests(self):
        report = parse_lines(
            [
                result("PASS", 1, 2, "codex-rmcp-client::discovery"),
                result("FAIL", 2, 2, "codex-rmcp-client::limits"),
            ]
        )
        self.assertEqual(report["counts"]["distinct_tests"], 2)
        self.assertEqual(report["changed_status_cases"], [])

    def test_compound_failure_and_summary_reprints_are_normalized(self):
        report = parse_lines(
            [
                "Nextest run ID windows-run with nextest profile: ci\n",
                result("PASS", 1, 3, name="normal"),
                result(
                    "FAIL + LEAK",
                    2,
                    3,
                    "codex-core::all",
                    "suite::cli_stream::responses_api_stream_cli",
                ),
                result("LEAK", 3, 3, name="leaked_process"),
                "Summary [60.000s] 3 tests run: 2 passed (1 leaky), 1 failed\n",
                result(
                    "FAIL+  LEAK",
                    2,
                    3,
                    "codex-core::all",
                    "suite::cli_stream::responses_api_stream_cli",
                ),
            ]
        )
        self.assertTrue(report["complete_unambiguous"])
        self.assertEqual(
            report["counts"],
            {
                "executions": 3,
                "distinct_tests": 3,
                "non_pass_executions": 2,
                "non_pass_distinct_tests": 2,
                "non_pass_unique_names": 2,
                "statuses": {"FAIL + LEAK": 1, "LEAK": 1, "PASS": 1},
                "repeated_result_lines": 1,
            },
        )
        self.assertEqual(report["results"][1]["status"], "FAIL + LEAK")

    def test_unknown_compound_status_is_reported_instead_of_ignored(self):
        for status in ("FAIL + UNKNOWN", "PASS+LEAK", "FAIL ++ LEAK"):
            with self.subTest(status=status):
                report = parse_lines(
                    [result("PASS", 1, 2), result(status, 2, 2, name="unknown")]
                )
                self.assertFalse(report["complete_unambiguous"])
                self.assertEqual(report["counts"]["statuses"], {"PASS": 1})
                self.assertEqual(
                    report["issues"],
                    [
                        {"kind": "unparsed_result", "line": 2},
                        {
                            "kind": "missing_results",
                            "run_id": "unidentified",
                            "ordinals": [2],
                        },
                    ],
                )

    def test_truncated_compound_log_without_summary_keeps_observed_failure(self):
        report = parse_lines(
            [
                "Nextest run ID interrupted with nextest profile: ci\n",
                result("PASS", 1, 3, name="normal"),
                result("FAIL + LEAK", 2, 3, name="failed_before_disk_full"),
                "Error: There is not enough space on the disk. (os error 112)\n",
            ]
        )
        self.assertFalse(report["complete_unambiguous"])
        self.assertEqual(report["counts"]["statuses"], {"FAIL + LEAK": 1, "PASS": 1})
        self.assertEqual(
            report["issues"],
            [{"kind": "missing_results", "run_id": "interrupted", "ordinals": [3]}],
        )

    def test_compound_retry_attempt_is_not_counted_as_a_terminal_result(self):
        report = parse_lines(
            [
                "TRY 1 FAIL + LEAK [1.000s] (1/1) codex-core test_case\n",
                result("PASS", 1, 1),
            ]
        )
        self.assertFalse(report["complete_unambiguous"])
        self.assertEqual(report["counts"]["statuses"], {"PASS": 1})
        self.assertEqual(report["issues"], [{"kind": "unparsed_result", "line": 1}])

    def test_conflicting_slot_is_reported_and_not_counted_as_pass_or_fail(self):
        report = parse_lines([result("PASS", 1, 1), result("SIGABRT", 1, 1)])
        self.assertFalse(report["complete_unambiguous"])
        self.assertEqual(report["counts"]["executions"], 0)
        self.assertEqual(report["issues"][0]["kind"], "conflicting_result")
        self.assertEqual(
            [row["status"] for row in report["issues"][0]["observations"]],
            ["PASS", "SIGABRT"],
        )

    def test_duplicate_case_ordinals_are_explicit(self):
        report = parse_lines([result("PASS", 1, 2), result("PASS", 2, 2)])
        self.assertEqual(report["counts"]["executions"], 2)
        self.assertEqual(report["counts"]["distinct_tests"], 1)
        self.assertEqual(report["issues"][0]["kind"], "duplicate_case_execution")
        self.assertFalse(report["complete_unambiguous"])

    def test_separate_reruns_preserve_outcomes_and_report_status_changes(self):
        report = parse_lines(
            [
                "Nextest run ID run-1 with nextest profile: local\n",
                result("FAIL", 1, 1),
                "Nextest run ID run-2 with nextest profile: local\n",
                result("PASS", 1, 1),
            ]
        )
        self.assertEqual(report["counts"]["executions"], 2)
        self.assertEqual(
            report["changed_status_cases"],
            [
                {
                    "binary": "codex-core",
                    "name": "test_case",
                    "statuses": ["FAIL", "PASS"],
                }
            ],
        )
        self.assertTrue(report["complete_unambiguous"])

    def test_truncation_and_retry_rows_cannot_silently_report_complete(self):
        report = parse_lines(
            [
                "\x1b[31m" + result("FAIL", 1, 3) + "\x1b[0m",
                " TRY 1 FAIL [ 1.000s] (2/3) codex-core retry_case\n",
            ]
        )
        self.assertEqual(report["counts"]["statuses"], {"FAIL": 1})
        self.assertEqual(
            [issue["kind"] for issue in report["issues"]],
            ["unparsed_result", "missing_results"],
        )
        self.assertEqual(report["issues"][1]["ordinals"], [2, 3])

    def test_classification_keeps_unrerun_abort_and_does_not_merge_binaries(self):
        initial = parse_lines(
            [
                result("FAIL", 1, 3, "binary-a", "shared"),
                result("FAIL", 2, 3, "binary-b", "shared"),
                result("SIGABRT", 3, 3, name="stack_overflow"),
            ]
        )
        rerun = parse_lines(
            [
                result("PASS", 1, 2, "binary-a", "shared"),
                result("FAIL", 2, 2, "binary-b", "shared"),
            ]
        )
        rerun["source"] = {"name": "rerun.log"}
        summary = classify_failures(initial, [rerun])
        self.assertEqual(
            summary,
            {
                "initial_failure_distinct_tests": 3,
                "stages": [
                    {
                        "source": {"name": "rerun.log"},
                        "recovered_distinct_tests": 1,
                        "remaining_distinct_tests": 2,
                        "extra_tests_outside_initial_failures": [],
                        "mixed_outcome_tests": [],
                    }
                ],
                "remaining_with_rerun_result": 1,
                "never_rerun": [{"binary": "codex-core", "name": "stack_overflow"}],
            },
        )


if __name__ == "__main__":
    unittest.main()
