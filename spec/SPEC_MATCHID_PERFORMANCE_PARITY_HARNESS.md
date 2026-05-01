# SPEC - MatchID Performance Parity Harness

## Purpose

Define the benchmark harness and summary contract used to compare Elasticsearch and Surch on MatchID-relevant workloads.

## Core Principle

Artillery may remain one source of evidence, but it is not sufficient by itself.

The performance harness must support:
- replay of MatchID-derived search requests
- repeated measurements
- latency percentile summaries
- throughput comparison
- pass/fail comparison against a baseline summary

## Benchmark Config Format

The benchmark config is JSON:

```json
{
  "label": "elastic-baseline",
  "corpus": "tests/matchid_parity/sample_corpus.jsonl",
  "iterations": 3,
  "warmup_iterations": 1,
  "timeout_seconds": 30
}
```

## Summary Output Format

The run output is JSON:

```json
{
  "label": "elastic-baseline",
  "requests": 6,
  "successes": 6,
  "failures": 0,
  "error_rate": 0.0,
  "latency_ms": {
    "p50": 10.0,
    "p95": 20.0,
    "p99": 25.0,
    "max": 25.0
  },
  "throughput_rps": 100.0
}
```

## Comparison Rules

Default pass/fail checks:
- candidate `error_rate` must not exceed baseline
- candidate `p95` must not exceed baseline by more than a configured percentage
- candidate `p99` must not exceed baseline by more than a configured percentage
- candidate `throughput_rps` must not be lower than baseline when `require_throughput_no_worse=true`

## Exit Codes

- `0` when candidate meets the configured thresholds
- `1` when candidate violates at least one threshold
