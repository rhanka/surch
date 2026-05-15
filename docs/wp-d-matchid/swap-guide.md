# Swap guide — OpenSearch → Surch for deces-backend

Operational playbook for the matchID team to migrate the
`deces-backend` reads from OpenSearch 2.17.1 to Surch without breaking
production. Three strategies are described in increasing order of
safety: env-var flip (1), shadow mode (2, recommended for the
transition), and incremental swap by workload (3). Rollback path (4)
and minimum observability (5) close the guide.

This document is **deliberate prose, not a SPEC**. The contractual
parity surface is in `docs/wp-d-matchid/dsl-translation-matrix.md` and
`docs/wp-d-matchid/gap-analysis.md`. The swap can only start once
those tables flip the required gaps to `implemented` (see §3 for the
per-workload prerequisite matrix).

## 1. Pre-requisites

### Surch version

- **Minimum Surch version for the first bulk-match swap**: 0.2.x
  (round 5 of `wp/a-optim` + `wp/b-test-auto` shipped, i.e. **A1, A3,
  A8 implemented + B1 replay fixture green**).
- **Minimum Surch version for the interactive-search swap**: 0.3.x
  (adds A9, A10, A11, A14).
- **Minimum Surch version for the autocomplete swap**: 0.4.x (adds
  A6, A13).
- **Minimum Surch version for the analytics tab swap**: 0.5.x (adds
  A12).
- **Minimum Surch version for the geo-filter swap**: 0.6.x (adds A2,
  full A13 with `geo_point`).

The exact version numbers will be confirmed by the Surch release
manifest once each round closes; track via `surch_version` in
`GET /` (see `crates/surch-api/src/root.rs`).

### Hardware budget

Single-node Surch, on the same VM shape as the production OS node:

- **vCPU:** ≥ 4 (matches the artillery rehearsal budget in
  `docs/wp-d-matchid/incoming/2026-05-15-deces-backend-dsl-inventory.md`
  §4).
- **RAM:** ≥ 32 GB (≥ 24 GB for Surch heap + page cache for the
  posting lists, ≥ 4 GB reserved for the kernel).
- **Disk:** raw-index size ≈ 1.6 × `_source` bytes for the INSEE
  `deces` corpus (≈ 8 GB / 25 M docs after compression, vs ≈ 14 GB
  for an OS 2.17 index of the same corpus). Add 1.2 × headroom for
  the `index_prefixes` fan-out once A13 lands.
- **Network:** loopback or same-AZ — Surch does not currently support
  cross-cluster reads (out of scope for WP-D).

These are working estimates derived from the artillery rehearsals and
the per-block stats persisted by `wp/a-optim`. A dedicated
`docs/ops/perf-optimization-plan.md` will land in a later round; until
then this section is the authoritative budget.

### deces-backend client compatibility

- `deces-backend` already targets the ES 7.x wire shape
  (`hits.total.{value,relation}`); no change is needed once A14
  lands. Confirm by reading `runRequest.ts:19-50`.
- The `@opensearch-project/opensearch` Node client must keep its
  `compatibility: true` flag (or equivalent) so it tolerates Surch's
  `version.number = "2.17.1"` returned by `GET /` while
  `surch_version` carries the real Surch binary version.

## 2. Strategy 1 — env-var flip

The simplest possible swap. matchID points its `ES_HOST` /
`ELASTIC_URL` environment variable at the Surch node and restarts the
`deces-backend` container.

```bash
# Today, against OpenSearch
export ELASTIC_URL=http://opensearch:9200

# After the flip, against Surch
export ELASTIC_URL=http://surch:9200
```

**Pre-condition:** every gap in `gap-analysis.md` listed under the
target workload(s) must be `implemented`, **and** the matchID replay
fixture (`tests/matchid_compat/replays/deces_v1.json`, gap B1) must be
green on the Surch version pinned in production.

**Status today (end of round 5):** **not yet available**. Round 5
brings A1, A3, A8 and B1 only — not enough for an unconditional flip.

**When it will be available:** end of round 7 at the earliest, once
A1..A15 + A14 are all `implemented` and the INSEE 10k slice (gap B2)
is the frozen fixture for the replay.

**Rollback for this strategy:** flip the env var back and restart.
Sub-minute rollback assumes the OS cluster is kept warm in parallel
during a quarantine window of ≥ 7 days after the flip.

## 3. Strategy 2 — shadow mode (recommended for the transition)

This is the strategy we recommend for matchID's first contact with
Surch in production. The user-visible response continues to come from
OpenSearch; Surch receives a copy of every read and the divergence is
logged out-of-band. Surch can be a single-node VM in shadow mode
without impact to the user.

### 3.1 Where the shadow code lives

A new middleware module in deces-backend, e.g.
`matchID/packages/deces-backend/src/shadow.ts`, wraps the
`client.search` / `client.scroll` / `client.msearch` calls issued from
`runRequest.ts`. Pseudocode:

```typescript
async function shadowedSearch(body, opts) {
  const [primary, shadow] = await Promise.allSettled([
    osClient.search(body, opts),
    surchClient.search(body, opts),
  ]);
  if (shadow.status === "fulfilled") {
    compareAndLog(body, primary.value, shadow.value);
  } else {
    log.warn("surch shadow rejected", { err: shadow.reason });
  }
  return primary.value; // user always sees OS
}
```

The comparator extracts the top-K hit ids from both responses (K=10
recommended) and emits a structured log line per query carrying:

- query DSL hash (sha256 of the canonicalised body)
- `top1_match` (boolean)
- `top10_jaccard` (float in [0, 1])
- `score_delta_top1` (numeric, OS `_score` − Surch `_score`)
- `os_took_ms`, `surch_took_ms`
- redacted query fields (PII-free)

### 3.2 SLOs of acceptable divergence

For the initial weeks of shadow mode, treat the following as the
**alert thresholds**, not as parity targets. They mirror the round 5
NDCG@10 ≥ 0.85 budget from
`docs/wp-d-matchid/incoming/2026-05-15-deces-backend-dsl-inventory.md`
§4.

| Metric | Initial SLO | Long-term target |
|---|---|---|
| `top1_match` | ≥ **95 %** of queries | ≥ 99 % |
| `top10_jaccard` ≥ 0.5 | ≥ **85 %** of queries | ≥ 95 % |
| Surch error rate (shadow side) | ≤ 1 % | ≤ 0.1 % |
| `surch_took_ms` p95 | ≤ 3 × `os_took_ms` p95 | ≤ 1.5 × OS p95 |

Read this as: 5 % top-1 divergence and 15 % top-10 divergence are
acceptable while gaps still land. Tighten on each round as more gaps
flip to `implemented` and the BM25 differences narrow.

### 3.3 What Surch can do to help (hors-scope WP-D, mentioned only)

A future Surch endpoint `POST /_surch/compare` could accept the OS
response body alongside the query and return the same structured diff
the matchID middleware computes today. This would shift comparator
load off the Node process. **Not in scope for WP-D round 5.** Filed
for a later intake batch on `wp/d-matchid` if matchID requests it.

## 4. Strategy 3 — incremental swap by workload

The recommended way to **transition from shadow mode to production
reads on Surch**: swap one workload at a time, in increasing order of
user-visibility. Each phase has a prerequisite set of gaps from
`gap-analysis.md`.

| Phase | Workload | Required gaps (in addition to all previous phases) | Risk |
|---|---|---|---|
| 1 | bulk-match (CSV download, off-path) | **A3, A4, A5, A14, A15** | low |
| 2 | interactive search (UI lookup) | + **A1, A8, A9, A10, A11** | medium |
| 3 | autocomplete (prefix queries) | + **A6, A13** (`index_prefixes`) | medium |
| 4 | analytics tab (aggregations) | + **A12** | low |
| 5 | geo-filter (UI "near …") | + **A2, A13** (`geo_point`) | low |

Phase 1 is the safest first step: bulk-match runs as a background job
and tolerates a 3× latency budget without UX impact. Phase 2 is the
real bet — if `top1_match` in shadow mode is ≥ 99 % by then, the swap
is essentially free; if it sits at 95 %, hold the phase and ship the
next ranking-quality fix on `wp/a-optim` first. Phases 3-5 land
independently.

For each phase: keep shadow mode on the **next** phase's workload
until ≥ 2 weeks of green SLOs before flipping it too.

## 5. Rollback

Each strategy has a rollback shape with a different time-to-recovery
(TTR).

### 5.1 Strategy 1 — env-var flip rollback (TTR ≤ 5 min)

1. Re-point `ELASTIC_URL` at the warm OS cluster.
2. Restart the `deces-backend` containers (`docker compose restart
   deces-backend` or k8s `kubectl rollout restart deployment/…`).
3. Watch the `5xx` rate on the matchID ingress for ≤ 60 s; the OS
   cluster was already warm, no cold-cache penalty.

**Pre-condition for ≤ 5 min TTR:** the OS cluster is kept indexed and
running in parallel for **at least 7 days** after the flip. Do not
decommission OS until shadow-mode SLOs on the next workload phase
prove out.

### 5.2 Strategy 2 — shadow mode rollback (TTR < 60 s)

Since OS is still serving user traffic, "rollback" is just disabling
the shadow middleware. Set the feature flag (e.g.
`SHADOW_SURCH_ENABLED=false`) and restart the Node process — no data
state to unwind.

### 5.3 Strategy 3 — incremental swap rollback (TTR ≤ 15 min)

Per phase: re-enable the workload's routing to OS only. If the index
diverged on the Surch side during the phase (it should not — Surch is
read-only in this flow, writes still go through dataprep `_bulk`),
the recovery path is:

1. Stop writes on Surch (it has no _bulk overlap by design).
2. Re-ingest from the matchID raw-index tarball (the same
   `make artifact-publish-dataprep-snapshot` flow today). Surch will
   expose its own raw-index export under C-SNAPSHOT-RAW (round 5).
3. If the tarball is unusable, fall back to NDJSON re-ingest from
   INSEE source.

## 6. Minimum observability

Scrape these metrics from Surch during the entire transition.
Exposed via `GET /_prometheus_metrics`
(`crates/surch-api/src/metrics.rs`).

| Metric (Prometheus) | Type | Why it matters during the swap |
|---|---|---|
| `surch_search_total{index, query_type}` | counter | volume per workload phase; check parity vs OS request log |
| `surch_search_duration_seconds` | histogram | p50/p95/p99 latency; the 3× ES SLO budget reads off this |
| `surch_search_cache_hit_total` | counter | cache hit ratio under shadow load; cold-cache anomalies show here |
| `surch_metrics_self_test_total` | counter | trivial health metric; confirms the exporter is alive |

Suggested matching alert rules (PromQL sketch, tune to local SLOs):

```promql
# p95 search > 3 × OS baseline (= 600 ms during the artillery profile)
histogram_quantile(0.95,
  sum by (le) (rate(surch_search_duration_seconds_bucket[5m]))) > 0.6

# cache hit ratio collapses (cold restart or eviction storm)
rate(surch_search_cache_hit_total[5m])
  / rate(surch_search_total[5m]) < 0.20
```

Wire the same panels into the matchID Grafana dashboard alongside the
existing OS counterparts; the shadow-mode comparator log lines from
§3.1 carry both `os_took_ms` and `surch_took_ms` so the side-by-side
latency view is straightforward.

## 7. Sanity checklist before any production swap

- [ ] `GET /` on Surch returns `version.number = "2.17.1"` and the
      pinned `surch_version` (per
      `crates/surch-api/src/root.rs`).
- [ ] Every gap in §3's prerequisite column is `implemented` in
      `gap-analysis.md`.
- [ ] `tests/matchid_compat/replays/deces_v1.json` (gap B1) is green
      on the deployed Surch version.
- [ ] INSEE 10k slice (gap B2) is loaded and checksum-verified.
- [ ] Shadow mode has been running for ≥ 7 days with top-1 match
      ≥ the SLO floor in §3.2.
- [ ] OS cluster is warm and kept indexed in parallel for the
      rollback window.
- [ ] Prometheus scraping `surch_search_*` and an alert on
      `surch_search_duration_seconds` p95 are wired.
- [ ] On-call runbook for "Surch down" links to this guide §5.
