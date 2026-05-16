# In-process scoring bench (`search_hot_path`)

Criterion harness that measures `surch_api::search::run_search` directly,
bypassing the axum router and the HTTP stack. The end-to-end script
`target/bench-mon/bench.sh` (BAN 25k) reports `took=0` on most queries
because the HTTP round-trip dominates the wall clock — useless for
comparing scoring-side optimisations. This bench fixes that.

## Run

```bash
cargo bench --bench search_hot_path -p surch-api
```

Setup indexes a 5 000-doc corpus once (BEIR SciFact when present,
synthetic fallback otherwise). Default runtime: ~5–10 min for the four
groups (`match_all`, `match_simple`, `bool_must_2`, `multi_match`).

Override the SciFact path with:

```bash
SURCH_BEIR_SCIFACT_PATH=/path/to/corpus.jsonl \
  cargo bench --bench search_hot_path -p surch-api
```

## Reading results

Criterion writes HTML reports under `target/criterion/<group>/<id>/report/index.html`.
The summary index is `target/criterion/report/index.html`. Each report
shows mean / median / p99 with confidence intervals and PDF plots.

For quick checks the terminal output is enough:

```
match_simple/title=rust,size=10
    time:   [1.234 ms 1.245 ms 1.257 ms]
    thrpt:  [795.5 elem/s 803.2 elem/s 810.4 elem/s]
```

## Regression workflow

1. Before the change, capture a baseline:

   ```bash
   cargo bench --bench search_hot_path -p surch-api -- --save-baseline before
   ```

2. Apply the patch, then compare against the saved baseline:

   ```bash
   cargo bench --bench search_hot_path -p surch-api -- --baseline before
   ```

Criterion flags every group with the delta (`-5.2%`, `+0.3%`, …) and a
statistical significance verdict (`No change`, `Improvement`, `Regression`).
The HTML report also gains a side-by-side view.

## CI

This bench is **manual**: 5–10 min per run is too slow for the default
PR gate. A commented-out `bench-regression-gate` skeleton lives in
`.github/workflows/ci.yml` for future opt-in on a dedicated runner.
