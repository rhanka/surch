# B1 phase 3 — Elasticsearch 8.6.1 oracle cross-check (plan)

Tracked under `docs/wp-d-matchid/gap-analysis.md` row `B1`. The current
status there is:

> implemented (30 requests executed on Surch HEAD; Elasticsearch 8.6.1
> oracle cross-check still pending)

Phase 3 closes that note by running the deterministic
`tests/matchid_compat/replays/deces_v1.json` fixture against **both**
Surch and Elasticsearch 8.6.1, on the **same** INSEE 10k slice, and
asserting structural parity on the response shapes Surch's tests
already pin against Surch HEAD.

## Scope

- Same 30-request fixture, same `tests/matchid_compat/deces/mapping.json`,
  same `tests/matchid_compat/deces/slice-10000.ndjson.gz` bulk-load on
  both engines. No request text translation: matchID wire shapes are
  Elasticsearch 8.6.1 compatible by construction.
- Compare per-request:
  - `hits.total.value` and `hits.total.relation`
  - `hits.hits[0]._id` (when the request expects ranking)
  - `aggregations.<name>` shape for the A12 entries
  - `_scroll_id` non-empty for the A4 scroll initiators
- Any divergence becomes a row in the gap-analysis ("Notes" column),
  not a blocker. The cross-check is a baseline witness, not a CI gate
  (for now).

## Deliverables

1. **Manifest** `deploy/k8s/jobs/b1-oracle-gate.yaml`:
   - Two init engines: Surch (current image) + Elasticsearch 8.6.1
     (image `docker.elastic.co/elasticsearch/elasticsearch:8.6.1`,
     `discovery.type=single-node`, `xpack.security.enabled=false`,
     `ES_JAVA_OPTS=-Xms1g -Xmx1g`).
   - One driver `b1-oracle-driver` (Surch bench-driver image):
     bootstrap Surch + ES with mapping + slice, then run a new
     binary `b1_oracle` against both URLs, persist
     `b1-oracle-{surch,es}.out` + `b1-oracle.diff.json` to `/reports`.
   - Resource budget: ES needs ~2 GiB RSS at this slice size; bump
     pod memory limit to ~5 GiB total (Surch 512 Mi + ES 2 Gi + driver
     256 Mi + 1 GiB headroom).
2. **Binary** `crates/surch-demo/src/bin/b1_oracle.rs`:
   - Reuses `tests/matchid_compat::Request` / `Replay` types (extract
     them to a small `crates/matchid-replay` library crate so both the
     existing test and this driver share the parser).
   - Hits each replay request against `--url-a` and `--url-b`, captures
     the four diff axes above, emits a `surch.bench.b1_oracle.v1`
     report.
3. **`bench_report` extension**: new `b1_oracle` section that surfaces
   any divergence count and lists the first 3 divergent request names.
   SLO: `divergences_count == 0` for the matchID-compatible subset
   (with the known A2 / A5 / A12 partials excluded explicitly by name).
4. **Workflow**: add `b1-oracle-gate` to the `ci-k8s.yml` job choices
   list (`workflow_dispatch` only — Phase 3 does not run on every
   push).

## Risk + ordering

- Elasticsearch 8.6.1 startup is ~30 s on this pod shape; bulk-loading 10k INSEE
  docs is ~10 s. Total cold-start budget should fit in the 30 min
  K8s job cap with comfortable headroom.
- `b1_oracle` is the only **new** runtime code; everything else is
  manifest + bench_report glue. Land it behind unit tests against
  `tests/opensearch_compat/oracle` (the existing oracle fixture
  harness) first.
- Do NOT merge the existing `tests/matchid_compat::Request` parser
  into the new `b1_oracle` until the matchID replay still passes
  unchanged. The library extract is a refactor lot of its own.

## Out of scope (Phase 4 follow-ups)

- A2 `geo_bounding_box` / `geo_polygon` request shapes (Surch returns
  400 today, ES would return hits).
- A5 `linear` / `exp` decay, `script_score`, `random_score`.
- A12 composite `date_histogram` source.
- These three rows are already documented as `partial` in
  `gap-analysis.md`; Phase 3 explicitly skips them rather than logging
  expected divergences.

## When to start

- ndcg-gate K8s green with TREC-COVID extension (run `26116693319` or
  successor).
- insee-bench K8s green on `df3b0aa`+ (already true:
  `2026-05-19-insee-10k-k8s/`).
- Some bandwidth: this is ~2-3 commits worth of work plus an image
  rebuild.
