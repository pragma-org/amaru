#!/usr/bin/env python3
"""Turn Rust libtest JSON events into a node-diversity report.

The validation itself remains in the Rust test suite.  This adapter only makes
the test runner's final outcomes consumable by the presentation repository.
"""

import argparse
import json
import re
from datetime import datetime, timezone
from pathlib import Path


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", choices=("uplc-conformance", "ledger-rules"), required=True)
    parser.add_argument("--implementation", required=True)
    parser.add_argument("--corpus", required=True)
    parser.add_argument("--scenario-root", type=Path, default=None, help="Fixture directory used to restore stable scenario ids.")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--input",
        action="append",
        nargs=2,
        metavar=("SUITE", "PATH"),
        required=True,
        help="A libtest JSON stream and the suite name to attach to its scenarios; use 'auto' for a combined UPLC stream.",
    )
    return parser.parse_args()


def fixture_key(value):
    return re.sub(r"[^a-z0-9]+", "_", value.lower()).strip("_")


def fixture_index(root):
    if root is None:
        return {}
    index = {"by_id": {}, "by_name": {}}
    for path in root.glob("*.json"):
        match = re.match(r"(\d{5})-", path.stem)
        if not match:
            continue
        try:
            document = json.loads(path.read_text())
        except json.JSONDecodeError:
            document = {}
        fixture = {
            "id": path.stem,
            "title": document.get("title") or path.stem,
            "description": document.get("description"),
            "expected": document.get("expected", "Pass"),
        }
        short_id = fixture["id"][:5]
        if short_id in index["by_id"]:
            raise ValueError(f"duplicate ledger scenario id: {short_id}")
        index["by_id"][short_id] = fixture
        index["by_name"][fixture_key(fixture["title"])] = fixture
    return index


def load_events(path):
    events = []
    details = {}
    for line in path.read_text().splitlines():
        if line.startswith("UPLC_CONFORMANCE_RESULT "):
            try:
                detail = json.loads(line.removeprefix("UPLC_CONFORMANCE_RESULT "))
            except json.JSONDecodeError:
                continue
            detail_id = detail.get("id") or detail.get("name")
            if detail_id is not None:
                details.setdefault(detail_id, []).append(detail)
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("type") == "test" and event.get("event") in {"ok", "failed", "ignored", "timeout"}:
            events.append(event)
    return events, details


def detail_for_event(event_name, details):
    """Match a report detail to its protocol-qualified libtest function name."""
    direct = details.get(event_name)
    if direct:
        return direct[0]

    match = re.match(r"^(.*)_protocol_version_(\d+)$", event_name)
    if match is None:
        return None

    candidates = details.get(match.group(1), [])
    protocol_version = f"{match.group(2)}.0"
    return next((detail for detail in candidates if detail.get("protocol_version") == protocol_version), None)


def fallback_outcomes(status):
    return "success" if status == "ok" else status, None


def expected_result(fixture):
    expected = fixture.get("expected", "Pass")
    if expected == "Pass":
        return "pass"
    if isinstance(expected, dict):
        if expected.get("decoding_failure") is True:
            return "DecodingFailure"
        predicate = expected.get("predicate")
        if predicate:
            return predicate
    return "failed"


def title_without_group(name, group):
    prefix = "_".join(fixture_key(part) for part in (group or "").split("/") if part)
    if prefix and name.startswith(f"{prefix}_"):
        name = name[len(prefix) + 1 :]
    return name.replace("::", " / ").replace("_", " ")


def scenario(suite, event, scenario_name, fixtures, detail=None):
    name = event["name"]
    status = event["event"]
    scenario_id = name
    title = name.replace("::", " / ").replace("_", " ")
    if scenario_name == "ledger-rules":
        # The test-case name is an implementation detail. The five-digit id is
        # the stable key shared by the fixture and every node's report.
        match = re.search(r"(?<!\d)(\d{5})(?!\d)", name)
        fixture = fixtures["by_id"].get(match.group(1)) if match else None
        if fixture is None:
            match = re.search(r"test_case_\d+_(.+)$", name)
            fixture = fixtures["by_name"].get(fixture_key(match.group(1))) if match else None
        scenario_id = fixture["id"] if fixture else name
        return {
            "id": scenario_id[:5],
            "result": (
                expected_result(fixture)
                if fixture is not None and status == "ok"
                else event.get("stdout") or event.get("message") or status
            ),
        }
    item = {"id": scenario_id, "title": title}
    if scenario_name == "uplc-conformance":
        evaluation, budget = fallback_outcomes(status)
        group = None
        plutus_version = None
        protocol_version = None
        if detail is not None:
            scenario_id = detail.get("id") or detail.get("name") or scenario_id
            evaluation = detail.get("evaluation", evaluation)
            budget = detail.get("budget", budget)
            group = detail.get("group")
            plutus_version = detail.get("plutus_version")
            protocol_version = detail.get("protocol_version")
            title = title_without_group(scenario_id, group)
            item["title"] = title
            if isinstance(evaluation, dict) and "actual" in evaluation:
                evaluation = evaluation.get("actual")
            if isinstance(budget, dict) and "actual" in budget:
                budget = budget.get("actual")
        item["id"] = scenario_id
        item.update({
            "group": group or "other",
            "plutus_version": plutus_version,
            "protocol_version": protocol_version,
            "evaluation": evaluation,
            "budget": budget,
        })
    else:
        item.update({
            "expected": "pass",
            "actual": "pass" if status == "ok" else status,
            "passed": status == "ok",
        })
    if status != "ok":
        item["error"] = status
    return item


def main():
    args = arguments()
    fixtures = fixture_index(args.scenario_root)
    cases = {}
    for suite, raw_path in args.input:
        events, details = load_events(Path(raw_path))
        for event in events:
            detail = detail_for_event(event["name"], details)
            item = scenario(suite, event, args.scenario, fixtures, detail)
            key = (
                item["id"],
                item.get("plutus_version"),
                item.get("protocol_version"),
            ) if args.scenario == "uplc-conformance" else item["id"]
            cases[key] = item

    scenarios = sorted(cases.values(), key=lambda item: item["id"])
    if args.scenario == "ledger-rules":
        # Emit one entry for every known fixture. Missing test events are
        # failures, while unrelated events must never become scenarios.
        scenarios = [
            cases.get(identifier, {"id": identifier, "result": "missing"})
            for identifier in sorted(fixtures["by_id"])
        ]
        passed = sum(item["result"] == expected_result(fixtures["by_id"][item["id"]]) for item in scenarios)
        totals = {
            "passed": passed,
            "failed": len(scenarios) - passed,
            "total": len(scenarios),
        }
    else:
        totals = None

    report = {
        "schema_version": "1",
        "scenario": args.scenario,
        "generated_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "implementation": {"name": args.implementation},
        "corpus": args.corpus,
        "scenarios": scenarios,
    }
    if totals is not None:
        report["totals"] = totals

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
