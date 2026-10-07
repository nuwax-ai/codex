"""Summarize retries=0 nextest text logs without copying diagnostic payloads.

Identity is (binary, test name); a printed result belongs to (run ID, ordinal).
Nextest repeats failures after its Summary line. Identical repeated rows count
once. Conflicting rows are excluded and reported, rather than choosing a winner.
Different runs retain separate executions, including changed rerun outcomes.
"""

import argparse
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path
import re


ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
RUN = re.compile(r"Nextest run ID (\S+)")
RESULT = re.compile(
    r"^\s*(?P<status>[A-Z][A-Z0-9_-]*)\s+\[(?P<elapsed>[^]]+)\]\s+"
    r"\(\s*(?P<ordinal>\d+)/(?P<total>\d+)\)\s+"
    r"(?P<binary>\S+)\s+(?P<name>\S+)\s*$"
)
RESULT_CANDIDATE = re.compile(r"^\s*[A-Z][A-Z0-9_ -]*\s+\[[^]]+\]\s+\(\s*\d+/\d+\)")
SUMMARY = re.compile(r"^\s*Summary\s+\[[^]]+\]\s+(\d+) tests run:")


def parse_lines(lines):
    """Return metadata and terminal rows; never retain other stdout/stderr text."""
    slots = defaultdict(dict)
    runs = {}
    issues = []
    run_id = "unidentified"
    repeated = 0
    for line_number, raw in enumerate(lines, 1):
        line = ANSI.sub("", raw.rstrip("\r\n"))
        if match := RUN.search(line):
            run_id = match[1]
            runs.setdefault(run_id, {"totals": set(), "reported_run_counts": []})
        if match := SUMMARY.match(line):
            runs.setdefault(run_id, {"totals": set(), "reported_run_counts": []})[
                "reported_run_counts"
            ].append(int(match[1]))
        match = RESULT.match(line)
        if not match:
            if RESULT_CANDIDATE.match(line):
                issues.append({"kind": "unparsed_result", "line": line_number})
            continue
        row = match.groupdict()
        row["ordinal"] = int(row["ordinal"])
        row["total"] = int(row["total"])
        row["elapsed"] = row["elapsed"].strip()
        row["run_id"] = run_id
        runs.setdefault(run_id, {"totals": set(), "reported_run_counts": []})[
            "totals"
        ].add(row["total"])
        signature = tuple(row.items())
        alternatives = slots[(run_id, row["ordinal"])]
        if signature in alternatives:
            alternatives[signature]["repeated_line_count"] += 1
            repeated += 1
        else:
            alternatives[signature] = {
                **row,
                "source_line": line_number,
                "repeated_line_count": 0,
            }

    if not slots:
        issues.append({"kind": "no_result_rows"})
    results = []
    for (owner, ordinal), alternatives in slots.items():
        if len(alternatives) != 1:
            issues.append(
                {
                    "kind": "conflicting_result",
                    "run_id": owner,
                    "ordinal": ordinal,
                    "observations": list(alternatives.values()),
                }
            )
        else:
            results.extend(alternatives.values())

    run_reports = []
    for owner, metadata in runs.items():
        expected = metadata["totals"] | set(metadata["reported_run_counts"])
        ordinals = {ordinal for run, ordinal in slots if run == owner}
        missing = sorted(set(range(1, max(expected, default=0) + 1)) - ordinals)
        if len(expected) > 1:
            issues.append({"kind": "inconsistent_run_total", "run_id": owner})
        if missing:
            issues.append(
                {"kind": "missing_results", "run_id": owner, "ordinals": missing}
            )
        owner_results = [row for row in results if row["run_id"] == owner]
        cases = defaultdict(list)
        for row in owner_results:
            cases[(row["binary"], row["name"])].append(row["ordinal"])
        for (binary, name), indices in cases.items():
            if len(indices) > 1:
                issues.append(
                    {
                        "kind": "duplicate_case_execution",
                        "run_id": owner,
                        "binary": binary,
                        "name": name,
                        "ordinals": indices,
                    }
                )
        run_reports.append(
            {
                "run_id": owner,
                "expected_run_counts": sorted(expected),
                "observed_slots": len(ordinals),
                "counted_executions": len(owner_results),
                "statuses": dict(
                    sorted(Counter(row["status"] for row in owner_results).items())
                ),
            }
        )

    identities = defaultdict(set)
    for row in results:
        identities[(row["binary"], row["name"])].add(row["status"])
    non_pass = [row for row in results if row["status"] != "PASS"]
    return {
        "schema_version": 1,
        "complete_unambiguous": not issues,
        "counts": {
            "executions": len(results),
            "distinct_tests": len(identities),
            "non_pass_executions": len(non_pass),
            "non_pass_distinct_tests": len(
                {(row["binary"], row["name"]) for row in non_pass}
            ),
            "non_pass_unique_names": len({row["name"] for row in non_pass}),
            "statuses": dict(sorted(Counter(row["status"] for row in results).items())),
            "repeated_result_lines": repeated,
        },
        "runs": run_reports,
        "issues": issues,
        "changed_status_cases": [
            {"binary": binary, "name": name, "statuses": sorted(statuses)}
            for (binary, name), statuses in sorted(identities.items())
            if len(statuses) > 1
        ],
        "results": results,
    }


def parse_file(path):
    path = Path(path)
    digest = hashlib.sha256()
    with path.open("rb") as source:

        def lines():
            for raw in source:
                digest.update(raw)
                yield raw.decode("utf-8", errors="replace")

        result = parse_lines(lines())
    result["source"] = {"name": path.name, "sha256": digest.hexdigest()}
    return result


def classify_failures(initial, reruns):
    """Classify observed first-run failures by exact identity, without causal claims."""
    if not all(report["complete_unambiguous"] for report in [initial, *reruns]):
        raise ValueError("cannot classify incomplete or ambiguous logs")
    failures = {
        (row["binary"], row["name"])
        for row in initial["results"]
        if row["status"] != "PASS"
    }
    remaining = set(failures)
    seen = set()
    stages = []
    for report in reruns:
        statuses = defaultdict(set)
        for row in report["results"]:
            statuses[(row["binary"], row["name"])].add(row["status"])
        recovered = {key for key in remaining if statuses.get(key) == {"PASS"}}
        seen.update(statuses)
        stages.append(
            {
                "source": report["source"],
                "recovered_distinct_tests": len(recovered),
                "remaining_distinct_tests": len(remaining - recovered),
                "extra_tests_outside_initial_failures": [
                    {"binary": binary, "name": name}
                    for binary, name in sorted(statuses.keys() - failures)
                ],
                "mixed_outcome_tests": [
                    {"binary": binary, "name": name, "statuses": sorted(values)}
                    for (binary, name), values in sorted(statuses.items())
                    if len(values) > 1
                ],
            }
        )
        remaining -= recovered
    return {
        "initial_failure_distinct_tests": len(failures),
        "stages": stages,
        "remaining_with_rerun_result": len(remaining & seen),
        "never_rerun": [
            {"binary": binary, "name": name}
            for binary, name in sorted(remaining - seen)
        ],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("logs", type=Path, nargs="+")
    parser.add_argument("--output", type=Path)
    parser.add_argument(
        "--classify",
        action="store_true",
        help="first log is initial; later logs are rerun stages",
    )
    args = parser.parse_args()
    reports = [parse_file(path) for path in args.logs]
    evidence = {"logs": reports}
    if args.classify:
        if len(reports) < 2:
            parser.error(
                "--classify requires an initial log and at least one rerun log"
            )
        try:
            evidence["failure_classification"] = classify_failures(
                reports[0], reports[1:]
            )
        except ValueError as error:
            evidence["classification_error"] = str(error)
    for report in reports:
        report["non_pass_results"] = [
            row for row in report.pop("results") if row["status"] != "PASS"
        ]
    output = json.dumps(evidence, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(output, encoding="utf-8")
    else:
        print(output, end="")
    return 0 if all(report["complete_unambiguous"] for report in reports) else 2


if __name__ == "__main__":
    raise SystemExit(main())
