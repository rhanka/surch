# Snapshot plan — cloning the ES/OS live-snapshot architecture

Date: 2026-05-16. Last checked against the repo: 2026-05-18.
Status: partial implementation shipped. This doc now tracks the
remaining work for `C-SNAPSHOT-S3`, `C-SNAPSHOT-RESTORE` hardening
and `C-SLM-CRON` follow-ups on top of what already exists in `main`.

It complements two existing docs:

- `docs/ops/snapshot-raw.md` — describes `C-SNAPSHOT-RAW`, the
  single-tarball matchID export route already in `main`
  (`/_surch/snapshot/{export,import}`).
- `docs/ops/packaging-plan.md` — section 4 "Snapshots compatible
  with the Elasticsearch SLM surface" sketched the destination at
  a high level. The current doc is the detailed roadmap.

This is the only place where the three snapshot work-packages share
a single decision tree. It is no longer a pure design note: the
sections below distinguish what is already shipped from what remains
open.

## 0. Status checkpoint — repo state on 2026-05-18

### Already shipped on `main`

- Legacy raw export/import still exists under
  `/_surch/snapshot/{export,import}` in
  `crates/surch-api/src/snapshot.rs`.
- ES-parity `_snapshot` routes are live in
  `crates/surch-api/src/lib.rs` and
  `crates/surch-api/src/snapshot_es/routes.rs`:
  `PUT/GET/DELETE /_snapshot/{repo}`,
  `PUT/GET/DELETE /_snapshot/{repo}/{snap}`,
  `POST /_snapshot/{repo}/{snap}/_restore`.
- The repository SPI is implemented for both filesystem and S3 in
  `crates/surch-api/src/snapshot_es/repository.rs`.
- The on-repository layout, manifest handling and tarball-backed
  restore logic are implemented in
  `crates/surch-api/src/snapshot_es/service.rs`.
- SLM REST + scheduler are already wired in
  `crates/surch-api/src/slm/` and spawned from
  `crates/surch-api/src/main.rs`.

### Still open / intentionally incomplete

- Repository registration and SLM policies are in-memory only; no
  `surch.toml` persistence yet.
- `_snapshot` take/restore is still tarball-per-index, not the
  incremental chunk graph sketched for later phases.
- Snapshot takes are synchronous; `/_snapshot/{repo}/{snap}/_status`
  is not exposed yet.
- S3 is covered at registration/config-validation level, but the real
  MinIO/S3 end-to-end `take -> wipe -> restore` path is still missing.
- SLM retention fields are accepted and stored, but trimming is not
  enforced yet.
- Concurrent-write fencing remains operator-driven; restore still
  refuses to overwrite an existing index instead of providing a richer
  close/reopen lifecycle.

### Next Track C steps

1. Add persistent repo/policy storage so `_snapshot` and `_slm`
   survive process restarts.
2. Land the MinIO/S3 e2e path and cite its CI run ids in Track C
   reporting.
3. Finish SLM retention and decide whether `_status` stays deferred or
   ships with any future async take path.

---

## 1. ES / OS architecture overview

### 1.1 Why ES snapshots are "live"

Elasticsearch and OpenSearch take snapshots without quiescing
writes. Three primitives make this possible:

1. **Lucene segments are immutable.** Once a segment is written
   it never changes; only `IndexWriter` flushes produce new
   segments, and merges produce *new* ones that supersede old
   ones — they never mutate bytes in place.
2. **`IndexCommit` snapshot deletion policy.** When the snapshot
   starts, ES calls `SnapshotDeletionPolicy.snapshot()` on the
   Lucene `IndexWriter`. This *pins* the current `IndexCommit`
   (the set of segments that constitute the index at that
   instant) — the merge thread is free to keep running and to
   produce new segments, but the pinned ones are not garbage
   collected until the snapshot releases them. (Reference:
   `org.apache.lucene.index.IndexCommit`.)
3. **Per-segment incremental upload.** ES walks the pinned
   `IndexCommit`, and for each segment file it checks the
   repository: if a file with the same physical name + length +
   checksum is already present in the repository, the upload is
   skipped (deduplicated by content). Only *new* segments since
   the previous snapshot reach the network. This is what makes
   the 50th snapshot of a 1 TB index take seconds.

The combination — immutable segments + pinned commit + dedup
upload — means the operator never has to pause writes. A snapshot
is logically "the state of the index at `t0`", physically "the
file set referenced by the pinned `IndexCommit`".

### 1.2 Repository SPI

ES abstracts storage behind the `Repository` SPI. Concrete
implementations live in separate plugins:

- `FsRepository` (shared filesystem / NFS)
- `S3Repository` (`repository-s3` plugin)
- `GcsRepository`, `AzureRepository`, `HdfsRepository`,
  `URLRepository` (read-only HTTP).

Every implementation is a key/value store with four operations
(`put_object`, `get_object`, `list_objects_by_prefix`,
`delete_object`) plus atomic-CAS for the root `index-N` file (see
1.4). The rest of the snapshot machinery is repository-agnostic.

### 1.3 REST surface

The subset that real clients (Kibana, Curator, Elastic agent,
matchID) actually call:

```text
PUT    /_snapshot/{repo}                       register a repository
DELETE /_snapshot/{repo}                       remove a repository
GET    /_snapshot/{repo}                       repository metadata
PUT    /_snapshot/{repo}/{snap}                take a snapshot
GET    /_snapshot/{repo}/{snap}                metadata + state
GET    /_snapshot/{repo}/{snap}/_status        in-flight progress
DELETE /_snapshot/{repo}/{snap}                drop a snapshot
POST   /_snapshot/{repo}/{snap}/_restore       restore into a cluster

PUT    /_slm/policy/{id}                       schedule (cron)
GET    /_slm/policy/{id}                       policy + execution history
POST   /_slm/policy/{id}/_execute              fire one job now
DELETE /_slm/policy/{id}                       drop the policy
GET    /_slm/policy/{id}/_executions           runs history (ES 7.x+)
```

SLM policies carry a `name` pattern (`<deces-{now/d}>`), a
`repository`, an `indices` selector, a `schedule` (cron), and a
`retention` block (`expire_after`, `min_count`, `max_count`).

### 1.4 On-repository layout (BlobStoreRepository)

Excerpt of the bucket layout used by ES 7.17 / OS 2.x:

```text
{prefix}/
  index-{N}                       root manifest, CAS-protected
  index.latest                    pointer to current generation N
  snap-{uuid}.dat                 per-snapshot metadata
  meta-{uuid}.dat                 cluster-level metadata
  indices/{index-uuid}/
    meta-{snap-uuid}.dat          mapping + settings + aliases
    0/                            per-shard subtree
      __{segment-file-uuid}        deduped segment payloads
      snap-{snap-uuid}.dat         per-shard snapshot manifest
```

The `index-{N}` root file is the source of truth: it lists the
active snapshots, their state, and the active repositories. It is
updated by **generation bump** (`index-N+1` is written, then
`index.latest` atomically points to `N+1`) — the only place where
the repository SPI must offer compare-and-set semantics.

### 1.5 Restore

Restore is conceptually a snapshot replayed in reverse:

1. Read the root `index-{N}` manifest, locate the snapshot UUID.
2. Read `snap-{uuid}.dat` to learn the list of indices and
   shard files.
3. For each shard file `__{file-uuid}` referenced by the manifest,
   pull bytes from the repository into the target node's data
   directory.
4. Hand the result to `IndexWriter.open()`. The index is live.

Restore is heavier than snapshot because the bytes must come back
in full (no dedup), and because write fencing is required: the
target index must not exist or must be closed before restore
starts (otherwise concurrent writes would race the rebuild). ES
refuses to restore over an open index by default.

---

## 2. Surch context

Surch is single-node, in-memory today. The "index on disk" is the
`DocumentIndex` struct in `crates/surch-index/src/` plus the
mapping + settings + alias state held in `AppState`
(`crates/surch-api/src/state.rs`). There are no segment files
because there is no on-disk format yet — `wp/a-optim` is the
work-package that stabilises the per-block FoR + skip-list codec.

What already ships in `main`:

- `C-SNAPSHOT-RAW`: `POST /_surch/snapshot/export?index=<name>`
  and `POST /_surch/snapshot/import?index=<name>` — a single
  tarball carrying `manifest.json` + `mapping.json` +
  `settings.json` + `aliases.json` + `documents.ndjson`. Code in
  `crates/surch-api/src/snapshot.rs`. Format version pinned at
  `surch_snapshot_format_version: 1`.
- `C-SNAPSHOT-API` / `C-SNAPSHOT-RESTORE` MVP: ES-parity REST
  endpoints backed by the repository SPI, root manifest, and
  tarball-based restore logic in `crates/surch-api/src/snapshot_es/`.
- `C-SNAPSHOT-S3` MVP: `type: "s3"` repository registration and
  config validation, including credential redaction in
  `GET /_snapshot/{repo}` responses.
- `C-SLM-CRON` MVP: `_slm/policy/*`, `_slm/status`, a background
  scheduler, and execution history in `crates/surch-api/src/slm/`.

What Surch still does **not** have and must add to close the gap with
the full ES / OS surface:

- Persistent repository and SLM registries. Today they are rebuilt in
  memory on each process start.
- Incremental snapshots. A full re-export is cheap at the matchID
  25 M-record scale (~3 GiB gzipped) but does not scale past it.
- Restore hardening / write fencing. Restore refuses to overwrite, but
  does not pause concurrent writes to *other* indices, nor does it
  offer a "close index" step.
- Async status reporting (`/_status`) and retention enforcement.

The plan below incrementally lifts Surch from "single-tarball" to
"ES-compatible live snapshot" in three phases, each shippable on
its own.

---

## 3. Roadmap

### Phase S1 — repository abstraction + full-tarball write

Status on 2026-05-18: mostly shipped on `main` for `fs`, with S3
registration in place and restore already working for the tarball
payload.

**Goal:** make a snapshot leave the box. Same content as
`C-SNAPSHOT-RAW`, but written through a pluggable repository SPI,
and addressable by `(repo, snap_name)` afterwards.

**Trait:**

```text
trait SnapshotRepository: Send + Sync {
    async fn put_object(&self, key: &str, bytes: Bytes) -> Result<()>;
    async fn get_object(&self, key: &str) -> Result<Bytes>;
    async fn list_objects(&self, prefix: &str) -> Result<Vec<String>>;
    async fn delete_object(&self, key: &str) -> Result<()>;
    async fn compare_and_set(
        &self,
        key: &str,
        expected_etag: Option<&str>,
        bytes: Bytes,
    ) -> Result<String /* new etag */>;
}
```

Two implementations:

- `FsRepository { root: PathBuf }` — straight file I/O, CAS via
  `rename(tmp, dst)` after `fcntl(F_OFD_SETLK)` advisory lock.
  Unit-test target.
- `S3Repository { client: aws_sdk_s3::Client, bucket: String,
  prefix: String }` — `PutObject` + `GetObject` + `ListObjectsV2`
  + `DeleteObject` + `PutObject(IfNoneMatch | IfMatch)` for CAS.

**REST surface (subset):**

```text
PUT    /_snapshot/{repo}                      register
DELETE /_snapshot/{repo}                      unregister
GET    /_snapshot/{repo}                      list
PUT    /_snapshot/{repo}/{snap}               take (full tarball)
GET    /_snapshot/{repo}/{snap}               metadata
DELETE /_snapshot/{repo}/{snap}               drop
POST   /_snapshot/{repo}/{snap}/_restore      restore
```

**Repository state:** at boot, `AppState` loads registered repos
from a small config file (`surch.toml` `[[snapshot.repository]]`
section). `PUT /_snapshot/{repo}` appends an entry to the same
file (atomic rename) and constructs the `SnapshotRepository`
trait object.

**Snapshot payload:** the *same* tarball the `C-SNAPSHOT-RAW`
route already produces — bumped to `surch_snapshot_format_version:
2` with a `repository` field added to the manifest. The take
handler reuses `build_tarball()` from `snapshot.rs` and pushes the
bytes via `repo.put_object("snap-{uuid}.dat", bytes)`. The root
manifest `index-{N}` is updated via `compare_and_set` (S3
`IfMatch` ETag).

**Restore:** `POST .../{snap}/_restore` pulls the tarball back,
reuses `parse_tarball()` + `state.create_index()` + bulk
re-ingest. Write fencing: the target index must be absent (same
as `C-SNAPSHOT-RAW`); when index aliasing under cluster mode
lands, we add a `closed` flag to the state.

Remaining work from S1: persistent registration, real S3
take/restore e2e, and stronger write fencing around restore.

### Phase S2 — incremental snapshots via generation IDs

**Goal:** snapshot N+1 only ships the postings written since
snapshot N. Surch has no Lucene segments to dedup, so we mint a
Surch-native equivalent.

**Mechanism:**

- Every `index_document` / `delete_document` / bulk mutation bumps
  a monotonic `generation: u64` on the target index. The
  `DocumentIndex` keeps `(doc_id, source, generation)` and the
  postings keep a side-table `posting_generation: BTreeMap<TermId,
  u64>` updated to `max(current, new_generation)` on every
  `index_term`.
- A snapshot manifest records `min_generation` and `max_generation`.
  `max_generation` of snapshot N becomes `min_generation` of N+1.
  Bytes shipped for N+1 are the documents and posting deltas with
  `generation > snap_N.max_generation`.
- Repository layout for incremental snapshots:

  ```text
  {prefix}/
    index-{N}                                   root manifest
    snap-{uuid}.json                            per-snapshot metadata
    indices/{index-name}/
      meta-{snap-uuid}.json                     mapping + settings
      docs-{snap-uuid}.ndjson.gz                doc deltas
      postings-{snap-uuid}.cbor.zst             posting deltas
  ```

- Restore walks the snapshot DAG from oldest to newest and applies
  each delta to the rebuilt `DocumentIndex`. The full-tarball
  format from S1 stays as the "base" entry of the chain
  (`min_generation = 0`).

**Tradeoffs:** Surch's postings are not byte-stable across
versions (FoR layout under `wp/a-optim` may evolve), so we ship
the *logical* delta (term → list of (doc_id, freq, positions))
rather than the *physical* segment bytes. Restore reconstructs
the index from logical state — slower than ES segment copy, but
codec-independent.

**Effort:** 10-15 j. Depends on `wp/a-optim` per-block stats
landing first so that the posting iterator can emit deltas
cheaply.

### Phase S3 — SLM-equivalent scheduling

Status on 2026-05-18: the REST surface, scheduler loop and execution
history are already present; retention and persistence are the
missing pieces.

**Goal:** match the ES SLM surface so Kibana / Curator clients
work unchanged.

**Components:**

- `tokio-cron-scheduler` background task spawned at boot,
  parameterised by the SLM policies in `surch.toml` and the
  REST-driven ones in `AppState`.
- REST endpoints `PUT /_slm/policy/{id}`, `GET /_slm/policy/{id}`,
  `POST /_slm/policy/{id}/_execute`, `DELETE /_slm/policy/{id}`,
  `GET /_slm/policy/{id}/_executions`.
- Policy payload mirrors ES: `name` (snapshot name pattern with
  `{now/d}` placeholder), `schedule` (cron expression),
  `repository`, `config.indices`, `retention { expire_after,
  min_count, max_count }`.
- Per-policy execution log persisted under
  `{repo}/_slm/{policy}/executions.json` — bounded ring of the
  last 100 runs (timestamp, snap UUID, state, error).
- Retention pass runs after each successful take: list snapshots
  for the policy, drop those that exceed `expire_after` AND would
  not break `min_count`, then drop the tail beyond `max_count`.

**Effort:** 4-6 j. Cron parsing is `cron` crate; the heavy
lifting is the retention math + the bounded execution log.

---

## 4. Decisions — validated 2026-05-16

User decision: **Surch ships the same snapshot API surface as
Elasticsearch / OpenSearch — same routes, same wire shapes, same
repository layout — because Surch will be offered as a PaaS where
existing ES tooling (Curator, the `_snapshot` UI in Kibana, the
client SDKs) must keep working without code changes**. Surch does
not invent a "simpler native flavour" alongside the ES one.

The plan below replaces the earlier "Surch-native" wording.
Recommendations are now binding decisions:

1. **S3 SDK.** `aws-sdk-s3` 1.x — official, covers SigV4 / STS /
   IMDS, gets MinIO / R2 / GCS interop for free.
2. **`C-SNAPSHOT-RAW` lifecycle.** Keep `/_surch/snapshot/*` for
   the two minor versions currently shipped because matchID's
   `deces-backend` boot path already relies on the body-stream
   shape (intake §2.12). Mark deprecated in
   `docs/ops/snapshot-raw.md` as soon as the ES-compatible
   `_snapshot` routes land, drop the `_surch/*` route in `0.4.0`
   after one matchID release cycle. **The ES-compatible API is
   the only long-term shape.**
3. **Repository format = ES `BlobStoreRepository`, bit-compatible.**
   Same `index-N` generation file, same `meta-{uuid}.dat` per
   snapshot, same `snap-{uuid}.dat` per snapshot, same
   `indices/{uuid}/meta-…` per snapshotted index, same chunked
   segment-blob layout. The reason is non-negotiable: PaaS
   customers must be able to point their existing ES Curator /
   Kibana / `elasticsearch-py` `client.snapshot` calls at Surch
   and have them work. Diverging from the on-disk format would
   force every downstream tool to recompile. The Lucene-segment
   abstraction is faked at the blob layer: Surch's in-memory
   `DocumentIndex` is serialised into a tar-of-blobs that
   matches what a Lucene shard would have produced, with a
   single synthetic `_n.cfs` carrying the postings + doc store +
   mapping. Restore reverses the same packing.
4. **Test strategy.** Three layers:
   - unit tests with a `MockRepository` (in-memory `BTreeMap`)
     covering happy path + CAS conflicts + missing key + listing
     pagination,
   - integration tests with `FsRepository` against a `tempfile::tempdir`
     (no network, no Docker, runs under `cargo test --workspace`),
   - e2e test under `docker compose` with `minio/minio` —
     `take → wipe → restore → grep doc count` matches. This is
     the non-negotiable test from the packaging plan ("blocks the
     PR that introduces snapshots").
5. **Integrity envelope.** Today's `C-SNAPSHOT-RAW` tarballs are
   not signed (the binary itself is, via minisign — see
   `docs/ops/packaging-plan.md` §1). For S3-shipped snapshots:
   add `sha256` per object in the manifest (cheap, lets restore
   refuse a tampered blob), and reuse the release-time minisign
   key to sign the root `index-{N}` manifest. End users verify
   via `minisign -Vm index-7 -p surch.pub` before restoring.
   Recommendation: ship sha256 from S1, minisign signing from S2
   (needs the key to be reachable at runtime, which is a separate
   ops decision).

---

## 5. Test strategy in detail

| Layer | Tool | What it checks |
|---|---|---|
| unit | `MockRepository` (`BTreeMap<String, Bytes>`) | tarball round-trip, manifest CAS conflict, missing key, list pagination, retention math |
| integration | `FsRepository` + `tempfile::tempdir` | full take→restore cycle, multi-snapshot retention, format-version refusal |
| e2e | `docker compose` + `minio/minio` | real S3 wire (SigV4, ListObjectsV2 truncation, IfMatch CAS), large body (1 GiB), restore-into-fresh-node |
| regression | `cargo bench --bench snapshot` | take + restore latency on the SciFact corpus, fail if 2× the baseline |

The `MockRepository` lives in `crates/surch-api/src/snapshot/`
under a `#[cfg(test)]` module so production code never depends
on it. The MinIO compose file goes under `tests/e2e/snapshot/`
and is gated by a `e2e-snapshot` feature so day-to-day `cargo
test --workspace` does not require Docker.

The non-negotiable from `docs/ops/packaging-plan.md` line 197
("an end-to-end CI test take → wipe → restore → cat indices in
the same PR that ships restore") is the integration-layer test:
it must pass under `cargo test --workspace -p surch-api` with no
network, no Docker, no opt-in feature flag.

---

## 6. Open questions

- **Cluster mode interaction.** Phase S2 incremental needs a
  cluster-wide `generation` counter. Single-node today, no
  problem; once `wp/a-optim` or a future WP-D ships replication,
  the generation must be Raft-replicated or it desyncs across
  nodes. Decide before S2 lands: "snapshot is per-node" (simple,
  matches the in-memory model) or "snapshot is per-cluster"
  (needs Raft consensus).
- **Schema-evolution policy.** `format_version` is a hard refusal
  today. ES has a "best-effort upgrade" path (mapping rewrite on
  restore). Do we want that, or do we keep strict refusal? The
  matchID use case (single-tenant, single-version) does not need
  it; multi-tenant SaaS does.
- **Concurrent snapshots of the same index.** ES allows them; the
  pinned `IndexCommit` makes it safe. Surch's in-memory state is
  mutable; the simplest answer is "one snapshot at a time per
  index", enforced by an `RwLock<SnapshotState>` flag. Confirm
  the constraint before S1 ships.
- **GC of orphan blobs.** When a snapshot delete races a take, S3
  blobs can become unreferenced. ES has a periodic
  `repository-s3 cleanup` task. Same is needed here; backlog
  item for after S3.
- **Auth surface.** `PUT /_snapshot/{repo}` exposes S3 creds in
  the request body (or refers to env vars). The packaging plan
  routes them via `aws-config` (env, IMDS, profile). Confirm:
  Surch reads creds *only* from the environment at boot, never
  from the REST body. Spelled out as a hard rule before S1.

---

## 7. References

External:

- Elasticsearch 7.17 snapshot/restore reference —
  <https://www.elastic.co/guide/en/elasticsearch/reference/7.17/snapshot-restore.html>
- OpenSearch snapshot API —
  <https://opensearch.org/docs/latest/tuning-your-cluster/availability-and-recovery/snapshots/index/>
- Elasticsearch 7.17 repository registration —
  <https://www.elastic.co/guide/en/elasticsearch/reference/7.17/snapshots-register-repository.html>
- Elasticsearch 7.17 SLM —
  <https://www.elastic.co/guide/en/elasticsearch/reference/7.17/snapshot-lifecycle-management.html>
- Lucene `IndexCommit` —
  <https://lucene.apache.org/core/9_0_0/core/org/apache/lucene/index/IndexCommit.html>
- Lucene `SnapshotDeletionPolicy` —
  <https://lucene.apache.org/core/9_0_0/core/org/apache/lucene/index/SnapshotDeletionPolicy.html>
- Elasticsearch source `BlobStoreRepository` —
  <https://github.com/elastic/elasticsearch/blob/v7.17.0/server/src/main/java/org/elasticsearch/repositories/blobstore/BlobStoreRepository.java>
- AWS SDK for Rust (`aws-sdk-s3`) —
  <https://github.com/awslabs/aws-sdk-rust>
- `tokio-cron-scheduler` —
  <https://github.com/mvniekerk/tokio-cron-scheduler>

Internal:

- `docs/ops/snapshot-raw.md` — the matchID single-tarball route
  already in `main`.
- `docs/ops/packaging-plan.md` §4 — high-level packaging
  decisions on snapshots (the source of the C-SNAPSHOT-* work
  package IDs).
- `docs/ops/workpackages.md` lines 211-214 — `C-SNAPSHOT-API`,
  `C-SNAPSHOT-S3`, `C-SNAPSHOT-RESTORE`, `C-SLM-CRON`
  definitions and effort budgets.
- `crates/surch-api/src/snapshot.rs` — current `C-SNAPSHOT-RAW`
  implementation that Phase S1 builds on.
