#!/usr/bin/env python3
"""Agrège une paire P2 A/B sans masquer les séries alignées par corps."""

from __future__ import annotations

import argparse
import json
import math
import random
from pathlib import Path
from typing import Iterable


PHASE_KINDS = {
    "fixed": ("match",),
    "random": ("bool", "match"),
    "no_source": ("bool", "match"),
    "cold": ("bool", "match"),
}
METRICS = ("client", "took", "probe")


def nearest_rank(values: Iterable[float], quantile: float) -> float:
    ordered = sorted(values)
    if not ordered:
        raise ValueError("série vide")
    rank = max(1, math.ceil(len(ordered) * quantile))
    return ordered[rank - 1]


def summary(values: list[float], scale: float) -> dict[str, float]:
    return {
        "p50": nearest_rank(values, 0.50) * scale,
        "p95": nearest_rank(values, 0.95) * scale,
        "p99": nearest_rank(values, 0.99) * scale,
        "max": max(values) * scale,
    }


def read_series(path: Path) -> list[float]:
    try:
        values = [float(line) for line in path.read_text().splitlines() if line]
    except (OSError, ValueError) as error:
        raise ValueError(f"série illisible {path}: {error}") from error
    if not values:
        raise ValueError(f"série vide {path}")
    if not all(math.isfinite(value) for value in values):
        raise ValueError(f"série non finie {path}")
    return values


def series_path(run_dir: Path, phase: str, kind: str, metric: str) -> Path:
    suffix = "_s" if metric == "client" else "_ms"
    return run_dir / f"surch.p2.{phase}.{kind}.{metric}{suffix}"


def ratio_summary(a: dict[str, float], b: dict[str, float]) -> dict[str, float | None]:
    return {
        key: None if a[key] == 0 else b[key] / a[key]
        for key in ("p50", "p95", "p99", "max")
    }


def write_ratios(path: Path, a: list[float], b: list[float]) -> int:
    zero_denominator = 0
    with path.open("w", encoding="utf-8") as output:
        output.write("sequence\ta\tb\tb_over_a\n")
        for position, (a_value, b_value) in enumerate(zip(a, b), start=1):
            if a_value == 0:
                zero_denominator += 1
                ratio = "NA"
            else:
                ratio = f"{b_value / a_value:.12g}"
            output.write(f"{position}\t{a_value:.12g}\t{b_value:.12g}\t{ratio}\n")
    return zero_denominator


def bootstrap_primary(a: list[float], b: list[float], samples: int, seed: int, out: Path) -> dict[str, float | int | str]:
    if len(a) != len(b):
        raise ValueError("bootstrap: séries A/B de longueurs différentes")
    rng = random.Random(seed)
    count = len(a)
    ratios: list[float] = []
    with out.open("w", encoding="utf-8") as output:
        output.write("resample\tp95_a_ms\tp95_b_ms\tb_over_a\n")
        for resample in range(1, samples + 1):
            indices = [rng.randrange(count) for _ in range(count)]
            p95_a = nearest_rank((a[index] for index in indices), 0.95)
            p95_b = nearest_rank((b[index] for index in indices), 0.95)
            if p95_a == 0:
                raise ValueError("bootstrap: p95 A nul, ratio indéfini")
            ratio = p95_b / p95_a
            ratios.append(ratio)
            output.write(f"{resample}\t{p95_a:.12g}\t{p95_b:.12g}\t{ratio:.12g}\n")
    return {
        "metric": "took",
        "phase": "random",
        "kind": "bool",
        "quantile": "p95",
        "resamples": samples,
        "seed": seed,
        "ratio_median": nearest_rank(ratios, 0.50),
        "ci95_low": nearest_rank(ratios, 0.025),
        "ci95_high": nearest_rank(ratios, 0.975),
        "raw_file": str(out),
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Statistiques P2 appariées A/B, nearest-rank et bootstrap primaire.")
    parser.add_argument("--a", type=Path, required=True, help="OUT_DIR de A")
    parser.add_argument("--b", type=Path, required=True, help="OUT_DIR de B")
    parser.add_argument("--out", type=Path, required=True, help="répertoire de rapport de la paire")
    parser.add_argument("--bootstrap", type=int, default=10_000)
    parser.add_argument("--seed", type=int, default=20260726)
    args = parser.parse_args()

    if args.bootstrap != 10_000:
        raise SystemExit("P2 exige exactement 10 000 rééchantillonnages bootstrap")
    args.out.mkdir(parents=True, exist_ok=True)
    ratio_dir = args.out / "paired-ratios"
    ratio_dir.mkdir(exist_ok=True)

    records: list[dict[str, object]] = []
    primary_a: list[float] | None = None
    primary_b: list[float] | None = None
    for phase, kinds in PHASE_KINDS.items():
        for kind in kinds:
            for metric in METRICS:
                a_path = series_path(args.a, phase, kind, metric)
                b_path = series_path(args.b, phase, kind, metric)
                a_values = read_series(a_path)
                b_values = read_series(b_path)
                if len(a_values) != len(b_values):
                    raise SystemExit(
                        f"P2 invalide: longueurs A/B différentes pour {phase}/{kind}/{metric}: "
                        f"{len(a_values)} != {len(b_values)}")
                scale = 1000.0 if metric == "client" else 1.0
                a_stats = summary(a_values, scale)
                b_stats = summary(b_values, scale)
                ratio_file = ratio_dir / f"{phase}.{kind}.{metric}.tsv"
                zero_denominator = write_ratios(ratio_file, a_values, b_values)
                records.append({
                    "phase": phase,
                    "kind": kind,
                    "metric": metric,
                    "n": len(a_values),
                    "unit": "ms",
                    "a": a_stats,
                    "b": b_stats,
                    "b_over_a": ratio_summary(a_stats, b_stats),
                    "paired_ratios_file": str(ratio_file),
                    "paired_ratios_zero_denominator": zero_denominator,
                })
                if (phase, kind, metric) == ("random", "bool", "took"):
                    primary_a = [value * scale for value in a_values]
                    primary_b = [value * scale for value in b_values]

    if primary_a is None or primary_b is None:
        raise SystemExit("P2 invalide: série primaire random/bool/took absente")
    bootstrap = bootstrap_primary(
        primary_a,
        primary_b,
        args.bootstrap,
        args.seed,
        args.out / "bootstrap-random-bool-took-p95.tsv",
    )
    report = {
        "schema": "surch.bench.p2.pair.v1",
        "a_dir": str(args.a),
        "b_dir": str(args.b),
        "nearest_rank": True,
        "records": records,
        "primary_bootstrap": bootstrap,
    }
    (args.out / "pair-summary.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")

    primary = next(
        record for record in records
        if (record["phase"], record["kind"], record["metric"]) == ("random", "bool", "took")
    )
    markdown = [
        "# P2 — paire A/B",
        "",
        "La parité des réponses doit déjà avoir été validée par le pilote avant cette lecture.",
        "",
        "| Série primaire | A p95 took | B p95 took | B/A | IC95 bootstrap |",
        "|---|---:|---:|---:|---:|",
        "| random / bool / took | "
        f"{primary['a']['p95']:.2f} ms | {primary['b']['p95']:.2f} ms | "
        f"{primary['b_over_a']['p95']:.4f} | "
        f"[{bootstrap['ci95_low']:.4f}; {bootstrap['ci95_high']:.4f}] |",
        "",
        "Les statistiques complètes et les ratios par corps sont dans `pair-summary.json` et `paired-ratios/`.",
    ]
    (args.out / "README.md").write_text("\n".join(markdown) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
