#!/usr/bin/env python3
"""Applique les gates pré-engagées du protocole P2 après trois paires valides."""

from __future__ import annotations

import argparse
import json
import statistics
from pathlib import Path


def read_json(path: Path) -> dict:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"artefact illisible {path}: {error}") from error


def record(summary: dict, phase: str, kind: str, metric: str) -> dict:
    for item in summary["records"]:
        if (item["phase"], item["kind"], item["metric"]) == (phase, kind, metric):
            return item
    raise SystemExit(f"série absente: {phase}/{kind}/{metric}")


def ratios(summaries: list[dict], phase: str, kind: str, metric: str, quantile: str) -> list[float]:
    output: list[float] = []
    for summary in summaries:
        value = record(summary, phase, kind, metric)["b_over_a"][quantile]
        if value is None:
            raise SystemExit(f"ratio indéfini: {phase}/{kind}/{metric}/{quantile}")
        output.append(float(value))
    return output


def phase_status_valid(run_dir: Path) -> bool:
    score = read_json(run_dir / "surch.json")
    if score.get("measurement_valid") is not True or score.get("p2", {}).get("phase_records") != 5:
        return False
    status_path = Path(score["p2"]["phase_status_jsonl"])
    try:
        statuses = [json.loads(line) for line in status_path.read_text().splitlines() if line]
    except (OSError, json.JSONDecodeError):
        return False
    return len(statuses) == 5 and all(status.get("valid") is True for status in statuses)


def check(name: str, condition: bool, detail: str) -> dict[str, object]:
    return {"name": name, "pass": condition, "detail": detail}


def main() -> int:
    parser = argparse.ArgumentParser(description="Évalue les gates de la campagne P2 complète.")
    parser.add_argument("--campaign", type=Path, required=True)
    args = parser.parse_args()

    pair_dirs = sorted(path.parent for path in (args.campaign / "pairs").glob("*/pair-summary.json"))
    if len(pair_dirs) != 3:
        raise SystemExit(f"P2 exige exactement trois paires, trouvé {len(pair_dirs)}")

    summaries: list[dict] = []
    validity: list[bool] = []
    for pair_dir in pair_dirs:
        parity = read_json(pair_dir / "parity.json")
        a_dir = args.campaign / "runs" / parity["a_run"]
        b_dir = args.campaign / "runs" / parity["b_run"]
        valid = (
            parity.get("parity") is True
            and parity.get("a_manifest_sha256") == parity.get("b_manifest_sha256")
            and phase_status_valid(a_dir)
            and phase_status_valid(b_dir)
        )
        validity.append(valid)
        summaries.append(read_json(pair_dir / "pair-summary.json"))

    product_took_p95 = ratios(summaries, "random", "bool", "took", "p95")
    product_client_p95 = ratios(summaries, "random", "bool", "client", "p95")
    core_took_p95 = ratios(summaries, "no_source", "bool", "took", "p95")
    core_took_p99 = ratios(summaries, "no_source", "bool", "took", "p99")
    fixed_match_p95 = ratios(summaries, "fixed", "match", "took", "p95")
    random_match_p95 = ratios(summaries, "random", "match", "took", "p95")
    probe_delta = [
        abs(record(summary, "random", "bool", "probe")["b"]["p95"] - record(summary, "random", "bool", "probe")["a"]["p95"])
        for summary in summaries
    ]
    bootstrap_upper = [float(summary["primary_bootstrap"]["ci95_high"]) for summary in summaries]

    checks = [
        check("validité route/parité/count/segments", all(validity), f"paires valides: {validity}"),
        check("noyau size:0 bool p95", statistics.median(core_took_p95) <= 0.50,
              f"médiane={statistics.median(core_took_p95):.4f}, cible <= 0.50"),
        check("noyau size:0 bool p99", statistics.median(core_took_p99) <= 0.70,
              f"médiane={statistics.median(core_took_p99):.4f}, cible <= 0.70"),
        check("produit size:10 bool p95 took", statistics.median(product_took_p95) <= 0.70,
              f"médiane={statistics.median(product_took_p95):.4f}, cible <= 0.70"),
        check("produit size:10 bool p95 client", statistics.median(product_client_p95) <= 0.70,
              f"médiane={statistics.median(product_client_p95):.4f}, cible <= 0.70"),
        check("trois paires produit même sens <= 0.80", all(value <= 0.80 for value in product_took_p95),
              f"ratios={product_took_p95}"),
        check("IC95 bootstrap primaire", all(value < 0.90 for value in bootstrap_upper),
              f"bornes supérieures={bootstrap_upper}, cible < 0.90"),
        check("témoin fixed match", all(0.95 <= value <= 1.05 for value in fixed_match_p95),
              f"ratios={fixed_match_p95}"),
        check("témoin random match", all(0.95 <= value <= 1.05 for value in random_match_p95),
              f"ratios={random_match_p95}"),
        check("écart sonde p95", all(value <= 2.0 for value in probe_delta),
              f"écarts ms={probe_delta}, cible <= 2"),
    ]
    passed = all(item["pass"] for item in checks)
    routing_proven = all(validity)
    primary_refuted = routing_proven and statistics.median(product_took_p95) > 0.70
    verdict = "PASS P2" if passed else ("ÉCHEC P2" if primary_refuted else "INVALIDE P2")
    report = {
        "schema": "surch.bench.p2.campaign.v1",
        "verdict": verdict,
        "pair_directories": [str(path) for path in pair_dirs],
        "ratios": {
            "product_random_bool_took_p95": product_took_p95,
            "product_random_bool_client_p95": product_client_p95,
            "core_no_source_bool_took_p95": core_took_p95,
            "core_no_source_bool_took_p99": core_took_p99,
            "fixed_match_took_p95": fixed_match_p95,
            "random_match_took_p95": random_match_p95,
            "probe_p95_delta_ms": probe_delta,
            "bootstrap_primary_p95_took_ci95_upper": bootstrap_upper,
        },
        "medians": {
            "product_random_bool_took_p95": statistics.median(product_took_p95),
            "product_random_bool_client_p95": statistics.median(product_client_p95),
            "core_no_source_bool_took_p95": statistics.median(core_took_p95),
            "core_no_source_bool_took_p99": statistics.median(core_took_p99),
        },
        "checks": checks,
    }
    (args.campaign / "campaign-summary.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    lines = [
        "# P2 — verdict de campagne",
        "",
        f"Verdict: **{verdict}**.",
        "",
        "| Gate | Verdict | Détail |",
        "|---|---|---|",
    ]
    for item in checks:
        lines.append(f"| {item['name']} | {'pass' if item['pass'] else 'fail'} | {item['detail']} |")
    lines += [
        "",
        "Les nombres par paire et les IC bootstrap sont conservés sous `pairs/*/pair-summary.json`.",
    ]
    (args.campaign / "README.md").write_text("\n".join(lines) + "\n")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
