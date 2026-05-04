#!/usr/bin/env python3

import argparse
import json
import math
import time
import urllib.request
from pathlib import Path


def load_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def load_jsonl(path: Path):
    with path.open("r", encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def percentile(sorted_values, pct):
    if not sorted_values:
        return 0.0
    index = max(0, min(len(sorted_values) - 1, math.ceil(len(sorted_values) * pct) - 1))
    return sorted_values[index]


def run_request(base_url, case, timeout_seconds):
    request_def = case["request"]
    body_bytes = json.dumps(request_def.get("json", {})).encode("utf-8")
    request = urllib.request.Request(
        base_url.rstrip("/") + request_def["path"],
        data=body_bytes,
        method=request_def.get("method", "POST"),
        headers={"Content-Type": "application/json"},
    )

    start = time.perf_counter()
    try:
        with urllib.request.urlopen(request, timeout=timeout_seconds) as response:
            response.read()
            duration_ms = (time.perf_counter() - start) * 1000.0
            return True, duration_ms, response.status
    except Exception:
        duration_ms = (time.perf_counter() - start) * 1000.0
        return False, duration_ms, None


def build_summary(label, latencies, successes, failures, started_at, finished_at):
    sorted_latencies = sorted(latencies)
    elapsed_seconds = max(finished_at - started_at, 1e-9)
    request_count = successes + failures

    return {
        "label": label,
        "requests": request_count,
        "successes": successes,
        "failures": failures,
        "error_rate": failures / request_count if request_count else 0.0,
        "latency_ms": {
            "p50": percentile(sorted_latencies, 0.50),
            "p95": percentile(sorted_latencies, 0.95),
            "p99": percentile(sorted_latencies, 0.99),
            "max": max(sorted_latencies) if sorted_latencies else 0.0,
        },
        "throughput_rps": request_count / elapsed_seconds,
    }


def command_run(args):
    config = load_json(Path(args.config))
    corpus = load_jsonl(Path(config["corpus"]))
    iterations = config.get("iterations", 1)
    warmup_iterations = config.get("warmup_iterations", 0)
    timeout_seconds = config.get("timeout_seconds", 30)

    for _ in range(warmup_iterations):
        for case in corpus:
            run_request(args.base_url, case, timeout_seconds)

    latencies = []
    successes = 0
    failures = 0
    started_at = time.perf_counter()

    for _ in range(iterations):
        for case in corpus:
            ok, duration_ms, _status = run_request(args.base_url, case, timeout_seconds)
            latencies.append(duration_ms)
            if ok:
                successes += 1
            else:
                failures += 1

    finished_at = time.perf_counter()
    summary = build_summary(
        config.get("label", "benchmark"),
        latencies,
        successes,
        failures,
        started_at,
        finished_at,
    )
    Path(args.out).write_text(json.dumps(summary, indent=2), encoding="utf-8")
    return 0


def command_compare(args):
    baseline = load_json(Path(args.baseline))
    candidate = load_json(Path(args.candidate))

    max_regression_pct = args.max_latency_regression_pct / 100.0
    require_throughput_no_worse = args.require_throughput_no_worse

    failures = []

    if candidate["error_rate"] > baseline["error_rate"]:
        failures.append("error_rate")

    for metric in ["p95", "p99"]:
        baseline_value = baseline["latency_ms"][metric]
        candidate_value = candidate["latency_ms"][metric]
        allowed = baseline_value * (1.0 + max_regression_pct)
        if candidate_value > allowed:
            failures.append(metric)

    if require_throughput_no_worse and candidate["throughput_rps"] < baseline["throughput_rps"]:
        failures.append("throughput_rps")

    report = {
        "baseline_label": baseline["label"],
        "candidate_label": candidate["label"],
        "failures": failures,
    }
    if args.report:
        Path(args.report).write_text(json.dumps(report, indent=2), encoding="utf-8")
    else:
        print(json.dumps(report, indent=2))

    return 0 if not failures else 1


def build_parser():
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    run = subparsers.add_parser("run")
    run.add_argument("--config", required=True)
    run.add_argument("--base-url", required=True)
    run.add_argument("--out", required=True)
    run.set_defaults(func=command_run)

    compare = subparsers.add_parser("compare")
    compare.add_argument("--baseline", required=True)
    compare.add_argument("--candidate", required=True)
    compare.add_argument("--max-latency-regression-pct", type=float, default=5.0)
    compare.add_argument("--require-throughput-no-worse", action="store_true")
    compare.add_argument("--report")
    compare.set_defaults(func=command_compare)

    return parser


def main(argv=None):
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
