# Surch workpackages

Surch development runs in three parallel workpackages. Each has its own
long-lived branch under `wp/` and its own reporting block at the bottom
of this document. Each commit lands on its WP branch first, then merges
to `main` as soon as it is green — fast iterative merges while we are
still pre-production.

## WP definitions

### WP-A — Optimisation

Branch: **`wp/a-optim`**

Scope:

- Query engine performance (scoring path, postings layout, top-K, WAND,
  Block-Max WAND, FST term dictionary, Roaring bitmaps, BM25 LUT,
  caching, BP doc-id reorder, …)
- Memory footprint reductions on the in-memory index (RAM win,
  duplicate state elimination, packed encoding, …)
- Disk format groundwork that unlocks the persistence milestone (per
  `docs/poc/perf-optimization-plan.md`)

Out of scope: every benchmark harness change (→ WP-B) and every
packaging / deployment artifact change (→ WP-C).

### WP-B — Automatisation des tests de performance

Branch: **`wp/b-test-auto`**

Scope:

- Bench harness scripts (BAN, INSEE, SciFact, TREC-COVID, mMARCO-fr,
  artillery, RSS sampling)
- Makefile + Rust binaries (`bench_report`, `bench_aggregate`)
- Scaleway `scw` remote runners with strict cost guardrails (see
  `docs/ops/test-automation-plan.md`)
- Test corpora ingestion, NDCG / Recall computation, regression
  detection vs baseline
- SLO targets and their tracking

Out of scope: optim work proper (→ WP-A) and the production runtime
itself (→ WP-C).

### WP-C — Opérationnalisation produit

Branch: **`wp/c-ops`**

Scope:

- Binary distribution (cargo-dist, multi-target builds, signatures)
- Docker image (Dockerfile, distroless, multi-arch, ghcr.io, cosign)
- Helm chart (`charts/surch/`)
- CI workflows (fmt, clippy, test, release)
- Snapshots S3 compatible with the Elasticsearch SLM surface
- Kubernetes operator (kube-rs, `SurchCluster`, `SurchSnapshot`)
- Versioning policy, `opensearch_compat_version`, SBOM, attestations
- Future: Terraform provider, marketplace, hosted offering

Out of scope: code generation in the engine path (→ WP-A) and the
bench harness (→ WP-B).

## Branch + merge policy

- Three long-lived branches: `wp/a-optim`, `wp/b-test-auto`, `wp/c-ops`.
- Direct commits stay possible on `main` for tiny cross-cutting
  changes (typos, README), but anything implementation-bearing lands
  on its WP branch first.
- Each commit carries a `[wp-a]`, `[wp-b]`, `[wp-c]` prefix in the
  subject so retrospective reading stays simple.
- Merges from a WP branch to `main` should happen as soon as the
  commit is green (cargo test + the WP-relevant bench / chart lint).
  No PR ceremony until the first production release.
- Worktrees live under `.worktrees/` (already in `.gitignore`):
  `.worktrees/wp-a`, `.worktrees/wp-b`, `.worktrees/wp-c`.

## Reporting

The retrospective below covers every commit currently on `main`,
grouped by workpackage.

### WP-A — Optimisation

#### Fait

| Commit | Title | Axe | Effet observé |
|---|---|---|---|
| `0dc30ad` | perf(api): use postings for match candidates | vélocité | 1ère étape post-grosse phase, prépare le top-K |
| `1b2e380` | perf(api): BM25 stats at index time | vélocité | tokenization sortie du chemin search |
| `3157afb` | perf(api): top-K + lazy `_source` hydration | vélocité | 18 k clones JSON éliminés sur Rue Payenne |
| `ed76014` | perf(api): MaxScore-style WAND skipping | vélocité | Rue Payenne ~120 ms → ~30 ms |
| `d778ee1` | perf(api): sorted `Vec<(u32, u64)>` scoring stats | vélocité | ~30 ms → ~16 ms |
| `65ccfbe` | perf(api): drop builder + WAND MultiMatch | RAM + vélocité | ~150 MB postings duplicate freed, MultiMatch joins the WAND path |
| `8757288` | perf(api): dedup repeated query tokens with boost | vélocité | half the posting walks for duplicate-token queries |
| `651e22a` | perf(index): drop StoredDocument duplicate | RAM | RSS 260 → 231 MB on BAN 25k |
| `644f62b` | perf(api): per-index LRU search response cache | vélocité (warm) | cache hit ≈ 0 ms; invalidation on every mutation |
| `3e907cf` | feat(api): default `track_total_hits` cap=10 000 | vélocité | matches OS default; less work past the cap |
| `14b7118` | perf(api): BoolMust intersect ascending size | vélocité | tighter intersections |
| `e38bf91` | perf(api): Block-Max WAND (per-128 max contribs) | vélocité | Tantivy priority #1; SciFact NDCG@10 unchanged |

#### Reste

Ordered by priority gain/effort on the BAN + INSEE + SciFact workloads:

| ID | Title | Effort | Vélocité | RAM | Disque |
|---|---|---|---|---|---|
| **C** | Block-128 FoR postings codec | 3 j | 1.3-1.6× scan | -50 to -80 MB | unlocks on-disk format |
| **SK** | Skip list + true Block-Max WAND on codec | 1.5 j | 1.5-3× selective | small | required for `advance` from disk |
| **D** | FST term dictionary (`fst` crate) | 2 j | µs lookup, ~3-5× on prefix | -5 to -10 MB | unlocks mmap term dict |
| **P2** | Roaring bitmaps for dense postings | 2-3 j | 2-10× AND / OR | adaptive | neutral |
| **P3** | BM25 8-bit quantised LUT | 3-4 j | removes `log` from hot path | small | neutral |
| **EF** | Elias-Fano codec on dense postings | 2 j | ≈ | -10 MB | quasi-succinct |
| **VBMW** | Variable Block-Max WAND | 1 j on top of C+SK | +30-50 % throughput | neutral | depends on C |
| **P7** | Recursive Graph Bisection doc-id reorder | 3-4 j | composes with P4 | indirect | -18 % postings disk |

#### Attendu

- Branch `wp/a-optim` runs ahead of main; every merge to main carries
  a SciFact NDCG@10 parity check (must stay ≥ 0.65) and a Rue Payenne
  search latency check (must not regress past v2.12 ~16 ms cold).
- Next pick is **C** (block-128 FoR) because it is the gate for SK,
  VBMW, snapshot format and P7. Concrete branch starts there.

### WP-B — Automatisation des tests perfs

#### Fait

| Commit | Title | Notes |
|---|---|---|
| `cf53377` | docs(perf): API performance diagnosis report | — |
| `4c35045` | test(perf): BAN HTTP benchmark runner | — |
| `019d91e` | test(perf): publish ban HTTP smoke report | — |
| `3485cd0` | test(perf): document Paris BAN blockers | — |
| `dd4ba8c` | test(beir): SciFact NDCG@10 parity gate | Surch 0.6576 vs OS 0.6537 vs Anserini 0.688 |
| `51b8383` | bench(matchid): artillery-style replay on INSEE 25k | bash + curl harness, scaled-down 50 s/engine |
| `04d601c` | build(make): root Makefile entry point | covers tests + benches lifecycle |
| `852f9db` | ci(check): fmt + clippy + test workflow | runs on every push + PR |
| `bdcd91c` | docs(ops): test automation plan | SLO targets table + scw plan |

#### Reste

| ID | Title | Effort | Output |
|---|---|---|---|
| **B-RUST-HARNESS** | Rust keep-alive artillery client (replace bash+curl) | 1 j | clean SLO p95 measurement vs matchID 200 ms target |
| **B-RUN-PAIR** | `scripts/bench/run-pair.sh` Surch + OS sequencer | 0.5 j | one JSON per engine per workload |
| **B-RSS-SAMPLE** | `scripts/bench/rss-sample.sh` (`pidstat → JSON`) | 0.5 j | RSS peak + steady metrics in the report schema |
| **B-TREC-COVID** | TREC-COVID NDCG@10 gate (171 k corpus, 50 queries) | 1 j | second BEIR baseline, denser qrels (~500/query) |
| **B-MMARCO-FR** | mMARCO-fr NDCG@10 gate (8.8 M corpus, FR) | 2 j | French BM25 + analyzer validation |
| **B-BENCH-REPORT** | Rust binary `bench_report` aggregator | 1 j | summary.md + regression detection vs baseline |
| **B-SCW** | Scaleway scw scripts (up / wait / rsync / run / down) | 1.5 j | `make bench-remote-scw` with hard cost caps |
| **B-S3-REPORTS** | publish JSON reports to Scaleway Object Storage | 0.5 j | history + regression diff in CI |

#### Attendu

- Branch `wp/b-test-auto` produces a clean Rust harness first so every
  later WP-A optim is measured under apples-to-apples SLO conditions.
- TREC-COVID NDCG@10 gate is the next correctness signal after
  SciFact; it is a stricter check (denser qrels) and runs in under
  10 min locally.
- All bench JSON outputs land under `target/bench-reports/<sha>/` and
  optionally upload to a Scaleway bucket; cost cap is enforced inside
  the scw scripts, not in the Makefile.

### WP-C — Opérationnalisation produit

#### Fait

| Commit | Title | Notes |
|---|---|---|
| `471d8cb` | feat(api): split `surch_version` / `opensearch_compat_version` | wire-compat string pinned at 2.17.1 |
| `bdcd91c` | docs(ops): packaging plan | 4 phases, sequencing, pitfalls |
| `da245bc` | build(docker): multi-stage Dockerfile, distroless | nonroot uid 65532, EXPOSE 7700 |
| `e7494e3` | ci(release): tag-triggered binary + docker release workflow | x86_64-gnu + aarch64-gnu via `cross`, multi-arch buildx push to ghcr.io |
| `2c4554a` | chart(helm): minimal Surch Helm chart | Deployment, Service, probes on `/`, distroless-friendly securityContext |
| `9233d8f` | build(make): docker-build + docker-smoke targets | smoke runs the image on host port 7711 |
| `e7f7b91` | ci(release): cosign keyless OIDC signing on OCI image | C-COSIGN — image signed by digest, verify command documented in README + packaging-plan |

#### Reste

| ID | Title | Effort | Output |
|---|---|---|---|
| **C-CARGO-DIST** | replace ad-hoc release workflow with `cargo-dist` | 1 j | macOS + musl targets, minisign signatures |
| **C-SBOM** | CycloneDX SBOM attached to each release | 0.5 j | supply-chain transparency |
| **C-METRICS** | `/_prometheus_metrics` endpoint | 1 j | scrapable counters + histograms (search latency, postings size, cache hit ratio) |
| **C-OTEL** | OpenTelemetry traces export via `opentelemetry-otlp` | 1 j | trace each search through scoring + hydration |
| **C-SNAPSHOT-API** | ES SLM-compatible REST surface, register / take / status / delete | 4-5 j | first end-to-end snapshot path |
| **C-SNAPSHOT-S3** | S3 backend (`aws-sdk-s3`), per-prefix layout, `format_version` | 3-4 j | snapshots actually leaving the box |
| **C-SNAPSHOT-RESTORE** | Restore path with write fencing + format validation | 4-5 j | non-negotiable take→wipe→restore CI test |
| **C-SLM-CRON** | Snapshot Lifecycle Management cron policies | 1 j | `tokio-cron-scheduler` |
| **C-OPERATOR** | kube-rs operator with `SurchCluster` + `SurchSnapshot` CRDs | 15-25 j | gated on a real cluster mode landing in WP-A first |
| **C-HELM-CR** | publish chart via `helm/chart-releaser-action` to `charts.surch.io` | 0.5 j | discoverable Helm chart |
| **C-CHANGELOG** | maintained `CHANGELOG.md` (Keep a Changelog format) | ongoing | release notes that match GitHub releases |

#### Attendu

- Branch `wp/c-ops` ships Phase A artefacts (binary, docker, helm,
  release CI) before any snapshot work starts. The whole Phase B
  (snapshots) lives on this branch.
- Snapshot format carries a `format_version: u32` from the first byte
  shipped; this is the only non-negotiable invariant in the plan.
- Kubernetes operator does not start before Surch has a real cluster
  mode in WP-A; a one-pod operator is `kubectl apply -f
  deployment.yaml` in disguise.

## Cross-WP coordination

- Any commit that touches an `engine path file + bench file + chart`
  trio should still be split: each side of the change goes to its WP
  branch.
- WP-B keeps the SciFact NDCG@10 gate green during every WP-A commit.
- WP-C ships the bench-result publication bucket so WP-B can write
  there.
- WP-A unblocks WP-C snapshots by stabilising the on-disk codec
  (block-128 FoR + skip list).
