# Swap guide — Elasticsearch 8.6.1 → Surch for deces-backend

Operational playbook for the matchID team to migrate `deces-backend`
reads from Elasticsearch 8.6.1 to Surch.

**Pre-production state today (2026-05-15)**: Surch is **not yet ready**
for a matchID swap. See `docs/wp-d-matchid/gap-analysis.md` for the
gap-by-gap status. The bascule cannot start before every row in that
table reads `implemented` (or `declined` by mutual agreement).

Until full compat is reached, the matchID team should **not run any
production traffic** against Surch — not even in shadow mode. The
artillery rehearsal in `docs/wp-d-matchid/incoming/2026-05-15-deces-backend-dsl-inventory.md`
§4 is the only sanctioned workload during the implementation phase,
and it runs against an internal Surch fixture, never against a
production matchID dataset.

## 1. Pre-requisites (before any flip)

### 1.1 Surch readiness

- Every row in `gap-analysis.md` reads `implemented`.
- `tests/matchid_compat/replays/deces_v1.json` (gap B1) carries at
  least 30 representative searches, all green, and the expectations
  are cross-checked against Elasticsearch 8.6.1 (not just against
  Surch HEAD).
- The INSEE slice (gap B2) is the real INSEE `deces-2020-m01.txt.gz`
  10 k extract, not the synthetic AWK-seeded slice.

### 1.2 Surch version

- `surch_version` reported by `GET /` is ≥ the version pinned in the
  matchID release manifest (TBD once the swap window is scheduled).
- `opensearch_compat_version` reports `2.17.1` (pinned in
  `crates/surch-api/src/root.rs`).

### 1.3 Hardware budget

Single-node Surch, on the same VM shape as the production OS node:

- **vCPU**: ≥ 4 (matches the artillery rehearsal budget).
- **RAM**: ≥ 32 GB (≥ 24 GB for Surch heap + page cache for the
  posting lists, ≥ 4 GB reserved for the kernel).
- **Disk**: raw-index size ≈ 1.6 × `_source` bytes for the INSEE
  `deces` corpus (≈ 8 GB / 25 M docs after compression, vs ≈ 14 GB
  for an OS 2.17 index of the same corpus). Add 1.2 × headroom for
  the `index_prefixes` fan-out once A13 ships.
- **Network**: loopback or same-AZ — Surch does not currently
  support cross-cluster reads (out of scope for WP-D).

### 1.4 deces-backend client compatibility

- `deces-backend` already targets the Elasticsearch 8.6.1 wire shape
  (`hits.total.{value,relation}`); no client change is needed once
  A14 lands. Confirm by reading `runRequest.ts:19-50` in the
  matchID repo.
- The `@opensearch-project/opensearch` Node client must keep its
  `compatibility: true` flag (or equivalent) so it tolerates
  Surch's `version.number = "2.17.1"` returned by `GET /`.

## 2. The cutover — env-var flip

The only swap strategy supported by this guide. **Not available
before full compat.**

matchID points its `ES_HOST` / `ELASTIC_URL` environment variable at
the Surch node and restarts the `deces-backend` container.

```bash
# Today, against Elasticsearch
export ELASTIC_URL=http://elasticsearch:9200

# After the flip, against Surch
export ELASTIC_URL=http://surch:9200
```

**Pre-condition (mandatory)**: §1 above is fully satisfied. If any
row in `gap-analysis.md` is not `implemented`, abort the swap.

**Cutover window**: ≤ 10 minutes — restart `deces-backend`, confirm
the artillery scenario passes against the new endpoint, confirm the
30-query replay fixture matches Elasticsearch 8.6.1 expectations, then declare
the swap complete.

**Keep Elasticsearch hot for the quarantine window**: ≥ 7 days after the flip.
During quarantine, both Surch and Elasticsearch run in parallel; matchID
operators can flip the env var back at any time without data loss.

### Shadow mode / incremental swap — explicitly out of scope

Earlier drafts of this guide proposed a shadow-mode strategy
(double-write to OS + Surch, log divergence, gradually shift traffic)
and an incremental-by-workload strategy (bulk-match first, then
interactive, then autocomplete, etc.). Both are **withdrawn**:

- they require client changes inside `deces-backend` (a shadow
  middleware) and a divergence-logging infrastructure matchID does
  not have today;
- they assume partial compat is operationally safe, which it is
  not for an end-user-facing search surface — divergent top hits
  are a UX regression even if total counts agree.

The agreed strategy is **all or nothing**: Surch reaches 100% compat,
then matchID flips the env var. If a later batch of intake from
matchID changes that decision, this guide is amended.

## 3. Rollback

**TTR ≤ 5 min**:

1. Flip `ELASTIC_URL` back to the OS cluster (kept warm during the
   quarantine window).
2. Restart `deces-backend`.
3. Confirm OS is serving traffic via `_cluster/health`.

If the quarantine window is over and OS has been decommissioned, the
rollback path is to re-deploy OS from its last snapshot — far slower
(hours), and assumes matchID retains its OS snapshots. **Do not
decommission OS before ≥ 30 days of green Surch operation.**

## 4. Minimum observability

Even though no shadow mode runs, matchID needs visibility into Surch
during the cutover window and the quarantine that follows.

### 4.1 Prometheus metrics

Surch exposes `/metrics` on port 7700 (configurable via
`SURCH_METRICS_ADDR`). Scrape it from matchID's existing Prometheus.
Key series:

- `surch_search_requests_total{outcome=…}` — request counts by
  outcome.
- `surch_search_duration_seconds_bucket` — latency histogram.
- `surch_index_documents_total{index=…}` — doc count per index.

### 4.2 Health endpoints

- `GET /` → `{ surch_version, opensearch_compat_version, … }` — a
  cheap liveness probe.
- `GET /_cluster/health` → green / yellow / red, matches the OS
  shape so existing alerts keep working.

### 4.3 Tracing (optional)

Set `OTEL_EXPORTER_OTLP_ENDPOINT` to export OTLP traces (see
`docs/ops/observability.md`). The OTLP gRPC exporter is pending
phase 2 — meanwhile the env var is accepted (warning logged) and
the local `tracing_subscriber` stays installed.

## 5. Sanity checklist before any production swap

- [ ] Every row in `docs/wp-d-matchid/gap-analysis.md` reads
  `implemented` or `declined`.
- [ ] B1 replay fixture is green against OS 2.17.1 expectations,
  not just Surch HEAD.
- [ ] B2 INSEE slice is the real INSEE `deces-2020-m01.txt.gz`
  10 k extract.
- [ ] The artillery scenario from
  `docs/wp-d-matchid/incoming/2026-05-15-deces-backend-dsl-inventory.md`
  §4 passes on the candidate Surch version with p95 ≤ 200 ms /
  max ≤ 500 ms / errors ≤ 1 %.
- [ ] OS cluster is hot and reachable for a ≥ 7 day quarantine
  rollback.
- [ ] Prometheus scrape job for Surch is wired and producing series.
- [ ] matchID team has the runbook for §3 rollback memorised /
  pinned in the on-call channel.

When all boxes are ticked, the swap can proceed during a low-traffic
window. Until then, **do not flip**.
