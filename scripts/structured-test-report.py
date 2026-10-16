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
    parser.add_argument("--protocol-version", default=None)
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
            details[detail["name"]] = detail
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("type") == "test" and event.get("event") in {"ok", "failed", "ignored", "timeout"}:
            events.append(event)
    return events, details


def fallback_outcomes(status):
    passed = status == "ok"
    return (
        {"expected": "pass", "actual": "pass" if passed else status, "passed": passed},
        {"expected": None, "actual": None, "passed": None},
    )


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
        # test-case turns the fixture's kebab-case filename into a Rust
        # identifier. Restore the stable fixture id used by the Haskell report.
        match = re.search(r"test_case_\d+_(.+)$", name)
        fixture = fixtures["by_name"].get(fixture_key(match.group(1))) if match else None
        scenario_id = fixture["id"] if fixture else name
        return {
            "id": scenario_id[:5],
            "result": "pass" if status == "ok" else event.get("stdout") or event.get("message") or status,
        }
    item = {"id": scenario_id, "title": title}
    if scenario_name == "uplc-conformance":
        evaluation, budget = fallback_outcomes(status)
        cost_model_id = None
        group = None
        if detail is not None:
            evaluation = detail.get("evaluation", evaluation)
            budget = detail.get("budget", budget)
            cost_model_id = detail.get("cost_model_id")
            group = detail.get("group")
            title = title_without_group(name, group)
            item["title"] = title
        item.update({
            "group": group or "other",
            "evaluation": evaluation,
            "budget": budget,
            "cost_model_id": cost_model_id,
            # Some vectors are expected to stop during evaluation. They still
            # contribute to evaluation conformance, but there is no measured
            # budget outcome to compare for those vectors.
            "passed": evaluation.get("passed") is True and (
                budget.get("passed") is None or budget.get("passed") is True
            ),
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


def dimension_totals(scenarios, dimension):
    outcomes = [item[dimension].get("passed") for item in scenarios]
    return {
        "passed": sum(value is True for value in outcomes),
        "failed": sum(value is False for value in outcomes),
        "total": len(outcomes),
    }


def main():
    args = arguments()
    fixtures = fixture_index(args.scenario_root)
    cases = {}
    cost_models = {}
    for suite, raw_path in args.input:
        events, details = load_events(Path(raw_path))
        for event in events:
            item = scenario(suite, event, args.scenario, fixtures, details.get(event["name"]))
            detail = details.get(event["name"])
            model = detail.get("cost_model") if detail else None
            if model and model.get("id"):
                cost_models[model["id"]] = model
            cases[item["id"]] = item

    scenarios = sorted(cases.values(), key=lambda item: item["id"])
    if args.scenario == "ledger-rules":
        fixture_ids = set(fixtures["by_id"])
        scenarios = [item for item in scenarios if item["id"] in fixture_ids]
        overall_failed = sum(item["result"] != "pass" for item in scenarios)
        totals = {
            "passed": sum(item["result"] == "pass" for item in scenarios),
            "failed": len(fixture_ids) - sum(item["result"] == "pass" for item in scenarios),
            "total": len(fixture_ids),
        }
    else:
        overall_failed = sum(not item["passed"] for item in scenarios)
        totals = {
            "passed": sum(item["passed"] for item in scenarios),
            "failed": overall_failed,
            "total": len(scenarios),
        }
    if args.scenario == "uplc-conformance":
        totals["evaluation"] = dimension_totals(scenarios, "evaluation")
        totals["budget"] = dimension_totals(
            [item for item in scenarios if item["budget"].get("passed") is not None],
            "budget",
        )

    report = {
        "schema_version": "1",
        "scenario": args.scenario,
        "generated_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "implementation": {"name": args.implementation},
        "corpus": args.corpus,
        "totals": totals,
        "scenarios": scenarios,
    }
    if args.scenario == "uplc-conformance":
        report["protocol_version"] = args.protocol_version or "unknown"
        report["cost_models"] = sorted(cost_models.values(), key=lambda model: model["id"])

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
