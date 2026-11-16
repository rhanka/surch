# Segment snapshots (format v2) and the monthly reload

Plan: `plan/mvp-matchid.md` lots L4 (monthly reload) and L6 step 2
("snapshot = copy of sealed segment files"). Persistence layout:
`docs/ops/persistence.md`. Code: `crates/surch-api/src/snapshot_es/`
(`segments.rs` format, `service.rs` take / restore / delete, `rename.rs`
index selection and renaming, `repository.rs` `fs` / `s3` backends),
`crates/surch-api/src/state.rs` (`pin_index_for_snapshot`,
`install_restored_index`).

## What changes for matchID

The monthly reload builds the new `deces` index offline (a Surch node with
`SURCH_DATA_DIR`), snapshots it to the bucket, and the production Job
(`matchID/deploy/k8s/overlays/prod/elasticsearch-restore.job.yaml`)
restores it under `deces-<snapshot>` and swaps the `deces` alias. With v2
the restore copies the index files and opens them: no document is
re-analysed (the v1 path re-ingested 29.3 M documents, hours on one core).

## Format choice

| Node taking the snapshot | Format written | Restore |
|---|---|---|
| `SURCH_DATA_DIR` set | `segments-v2` (index files as is) | requires `SURCH_DATA_DIR` on the restoring node; fails closed otherwise (`snapshot_restore_exception ... requires SURCH_DATA_DIR`) |
| `SURCH_DATA_DIR` unset | `logical-v1` (documents) | re-ingestion, any node |

The create response says which one was written, so a builder started
without a data directory is visible at once instead of producing an
artefact whose restore takes hours:

```json
{"snapshot": {"snapshot": "snap-2026-11", "state": "SUCCESS", ...,
  "surch": {"format": "segments-v2", "files_uploaded": 9, "files_reused": 0,
            "bytes_uploaded": 47312345, "bytes_reused": 0}}}
```

`logical-v1` adds `"reason": "SURCH_DATA_DIR is not set on this node: ..."`.
Snapshots written before L6.2 are `logical-v1` and stay restorable; the v1
restore now also keeps `settings.analysis` (it used to be dropped),
honours rename / `index_settings`, and commits the result (`_refresh`), so
it survives a restart.

## Repository layout

On top of the ES-shaped v1 layout (`index-0`, `meta-<uuid>.dat`,
`snap-<uuid>.dat`, `indices/<index uuid>/meta-<snap uuid>.dat`):

```
blobs/<blake3 hex>     one object per distinct content, immutable
```

A blob is the exact bytes of one file of the index directory (`MANIFEST`,
`meta-*.meta`, `ids-*.ids`, `seg-*.seg`, `seg-*.pst`, `seg-*.sub`) or one
committed chunk of `source.dat` (the bytes one `_refresh` appended). The
per-index metadata (`format_version: 2`) lists them:

```json
"segments": {
  "format": "surch-segments-v2",
  "files":  [{"name": "MANIFEST", "len": 1234, "blake3": "…"}, …],
  "source": [{"start": 0, "end": 7912345, "blake3": "…"}],
  "source_len": 7912345, "doc_count": 200000
}
```

- **Dedup.** The key is the content hash: a file or chunk already in the
  repository (from any earlier snapshot of any index) is not uploaded
  again (`HEAD` then skip). Snapshotting an unchanged index uploads one
  new `MANIFEST` (a few hundred bytes). After new documents and a
  `_refresh`, the old `source.dat` chunks are reused.
- **Deletion.** `DELETE /_snapshot/<repo>/<snap>` first commits the root
  manifest without the snapshot, then deletes every blob no remaining
  snapshot references. If a remaining snapshot's metadata cannot be read,
  nothing is deleted.

## Snapshot take (v2)

1. Under the index write lock: the index is committed (the ES "flush":
   writes since the last `_refresh` are sealed), then the committed
   generation is pinned — its `MANIFEST` and every file it references are
   hard linked into `<data>/tmp/snapshot-*/`. The lock is released.
2. Upload from the pin, without any lock: writes, refreshes and the
   garbage collection of superseded generations go on; the pinned inodes
   are immutable (`source.dat` only grows past the recorded prefix).
3. Each data file (`.pst`, `.sub`) and `source.dat` chunk is re-hashed and
   must match the blake3 its `MANIFEST` records — a corrupt local file
   fails the snapshot instead of being propagated.
4. Metadata, summary, then the root manifest (CAS) — the snapshot is
   visible only once complete. The pin directory is removed (and swept at
   the next start if the process dies).

## Restore (v2)

For each selected index (after `indices` filter and renaming):

1. Download every blob into `<data>/tmp/restore-*/i-<target>/`, hashing
   while writing; length and blake3 must match the snapshot metadata.
2. Rewrite the snapshot `MANIFEST` under the target name, open the index
   in the staging directory (framed files hash-checked, structure
   validated), apply `index_settings`, commit a new generation there.
3. Under the store write lock: conflicts re-checked, one `rename(2)` into
   `indices/` — the commit point — reopen, add the snapshot aliases
   (`include_aliases`, default `true`) and commit `ALIASES`.

Failure at any step before 3 removes the staging directory; the live
indices and aliases are untouched. A crash leaves only `tmp/` content,
wiped at the next start. A crash after the rename leaves a complete,
committed index (the Job's "target index already has N documents; reusing
it" branch). The restored index keeps the original `creation_date` (ES
8.6.1 `RestoreService.restoreToCreateNewIndex` copies the snapshot
settings and only replaces the index UUID).

### `_restore` request parity (ES 8.6.1)

| Field | Behaviour |
|---|---|
| `indices` | names, `*` wildcards, `-` exclusions, `_all` (`SnapshotUtils.filterIndices`); a missing name is `index_not_found_exception` (404) unless `ignore_unavailable` |
| `rename_pattern` / `rename_replacement` | Java `String.replaceAll` (`$1`, `${name}`, `\$`); pattern compiled by the Rust `regex` crate (no look-around / back-references: refused with an error); two indices renamed onto one name refused |
| `index_settings` | merged into the restored settings; `index.number_of_shards`, `index.version.created`, `index.uuid`, `index.creation_date`, `index.history.uuid` refused (`cannot modify setting [...] on restore`) |
| `ignore_index_settings` | removed; the above plus `number_of_replicas` / `auto_expand_replicas` refused |
| `include_aliases` | default `true` |
| `include_global_state` | accepted; Surch snapshots carry no global state (templates are not persisted yet) |
| `partial` | accepted, no effect (single shard per index) |
| `?wait_for_completion=true` | answers when every index is live: `{"snapshot": {"snapshot", "indices", "shards": {"total", "failed", "successful"}}}` |
| `?wait_for_completion=false` | validates, answers `{"accepted": true}`, restores in the background; the index appears atomically |
| absent | Surch keeps its historical synchronous behaviour (ES default is `false`) |

Errors follow ES: `snapshot_restore_exception` (HTTP 500) with
`[repo:snap/uuid] reason` for an existing target (`cannot restore index
[x] because an open index with same name already exists in the cluster
...`), an alias of the same name, an invalid replacement, a v2 snapshot on
a node without data directory; `snapshot_exception` (500) for an
integrity failure (`downloaded N bytes with blake3 …, snapshot records …`).

## S3 repository settings

`PUT /_snapshot/<repo>` with `"type": "s3"`:

| Setting | Surch |
|---|---|
| `bucket` | required |
| `region` | SigV4 region (default `us-east-1`) |
| `endpoint` | host (`s3.fr-par.scw.cloud`, ES style) or full URL |
| `protocol` | `https` (default) / `http`, used when `endpoint` is a bare host |
| `path_style_access` | honoured; when absent Surch keeps path style for any custom endpoint (historical default; ES default is `false`) |
| `client` | named client whose credentials come from the environment (below) |
| `readonly` | refuses snapshot creation (`repository_exception` 500, `[repo] cannot create snapshot in a readonly repository`, ES `SnapshotsService.createSnapshot`) and deletion; listing and restore allowed |
| `buffer_size` | above it, multipart upload in parts of this size (default `100mb`, minimum `5mb`, ES semantics) |
| `base_path` | key prefix inside the bucket |

`GET /_snapshot/<repo>` echoes the declared settings as strings, never
credentials. With a custom endpoint the SDK only adds request checksums
when the S3 API requires them (several S3-compatible stores reject the AWS
CRC trailers); blob integrity is enforced end to end by blake3.

### Credentials (no ES keystore)

ES reads `s3.client.<name>.access_key` / `secret_key` from its keystore.
Surch reads, first match wins:

1. `SURCH_S3_CLIENT_<NAME>_ACCESS_KEY`, `SURCH_S3_CLIENT_<NAME>_SECRET_KEY`,
   optional `SURCH_S3_CLIENT_<NAME>_SESSION_TOKEN` — `<NAME>` is the
   repository `client` (default `default`) upper-cased, non-alphanumerics
   as `_`: `SURCH_S3_CLIENT_DEFAULT_ACCESS_KEY`;
2. `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, optional
   `AWS_SESSION_TOKEN`;
3. legacy `access_key` / `secret_key` in the repository settings (Surch
   only; ES 8 rejects them).

They are read when the repository is registered. For matchID the secrets
go on the **Surch server pod** (where ES had its keystore), not on the
curl Job:

```yaml
env:
  - name: SURCH_DATA_DIR
    value: /data
  - name: SURCH_S3_CLIENT_DEFAULT_ACCESS_KEY
    valueFrom: {secretKeyRef: {name: elasticsearch-restore-secrets, key: STORAGE_ACCESS_KEY}}
  - name: SURCH_S3_CLIENT_DEFAULT_SECRET_KEY
    valueFrom: {secretKeyRef: {name: elasticsearch-restore-secrets, key: STORAGE_SECRET_KEY}}
```

## Health, settings, marker

- `GET /_cluster/health/<idx>?wait_for_status=green&timeout=600s`: an
  existing index (or alias) is green at once. A missing one is waited for
  until `timeout` (default `30s`), then `status: red`, `timed_out: true`,
  HTTP 408 — ES 8.6.1 `TransportClusterHealthAction` (missing index =
  RED, wait never satisfied) and `ClusterHealthResponse.status()`
  (`REQUEST_TIMEOUT` when timed out). The previous Surch answer was 404.
  `index.number_of_replicas` is echoed per index.
- `PUT /<idx>/_settings` (`{"index": {"number_of_replicas": 0}}`) stores
  and acknowledges dynamic settings (committed under `SURCH_DATA_DIR`);
  static ones are refused like ES (`Can't update non dynamic settings`).
  `GET /<idx>/_settings` echoes them.
- `GET /<idx>/_doc/<id>` exists (the Job reads its marker with it; before
  L6.2 the route answered 405 and the marker always read as absent).
- **Gap: the marker write is not durable.** `PUT
  /.matchid-restore-state/_doc/deces` is in memory until a `_refresh`
  commits it (Surch has no translog). After a pod restart without a
  refresh the marker is gone and the next Job run takes its "no marker;
  recording and skipping restore" branch. Recommended Job change
  (matchID): `curl -fsS -XPOST "$ES_URL/$RESTORE_MARKER_INDEX/_refresh"`
  right after `write_marker`. Covered by
  `tests/snapshot_v2.rs::deces_job_restores_segment_snapshot_without_reindexing`.

## Proof

- `crates/surch-api/tests/snapshot_v2.rs`: the prod Job replayed request
  by request on a fresh data directory (readonly repo, exact `_restore`
  body, `_count`, `_settings`, health green, alias swap, marker, `_cat`
  prune), byte-identical searches through the alias (deces v1 replay set +
  12 queries, `took` and the concrete index name excepted), zero term
  dictionary built, restart; blake3 dedup and blob GC across three
  snapshots; corrupted blob mid-restore (live alias untouched, staging
  removed) and crash leftovers swept at start; ES validation errors; v1
  restore with rename / analysis / creation date; v2 without data dir.
- `crates/surch-api/tests/snapshot_v2_s3_mock.rs`: the same snapshot /
  restore through `S3Repository` against an in-process S3 mock (path
  style, SigV4 with the env credentials, dedup on the wire, readonly
  refused before any request, 12 MiB multipart in 5 MiB parts).
- Measurement (200 k documents): table below.

## Measurement (200 k synthetic documents, release build, `fs` repository)

Local run, 2026-11-15, release `surch-api`, same corpus, mapping
(`tests/matchid_compat/deces/mapping_v2.json`) and production-like flags
as the L6 measurement (`docs/ops/persistence.md`). Restore = the matchID
Job body into a fresh `SURCH_DATA_DIR`, readonly repository, timed from
the `_restore?wait_for_completion=true` request to the first `_search`
answered. Replay: `ROOT=<scratch dir> scripts/bench/l62-snapshot-200k.sh
<release surch-api> tests/matchid_compat/deces/mapping_v2.json` (bash +
awk + curl; `ROOT` is wiped). Single run.

| Metric | v1 logical (documents) | v2 segments |
|---|---|---|
| Snapshot take | 1 142 ms | 53 ms (8 blobs uploaded, 0 reused) |
| Repository size | 6.7 MB, 5 objects (gzip NDJSON) | 47.8 MB, 12 objects (index files as is) |
| Restore → first query | **aborted after 29 min at 18 676 / 200 000 documents** | **786 ms** (restore answered at 776 ms) |
| Restored documents | — | 200 000 |
| Restoring node RSS after restore | — | 35.3 MiB (peak 35.3 MiB) |
| Responses (6 queries, source vs restored) | — | identical |

The v1 restore re-ingests one document at a time through
`AppState::index_document`, which refreshes the memory gauges after every
document (cost grows with the index: super-linear, extrapolated to hours
at 200 k and out of reach at 29.3 M). v2 trades a larger artefact (the
index structures, not compressed documents: 7.1x the v1 tarball here) for
a restore bounded by the copy + blake3 of the files. Not measured: S3
transfer time (only the mock ran), the 29.3 M scale, a cold page cache,
repeated runs.

## Not covered

- MinIO / Scaleway wire: only the in-process mock ran here
  (`SURCH_MINIO_E2E=1` + Docker for MinIO, not run).
- 29.3 M scale: blob sizes (a single `.pst` may pass 5 GiB, hence
  multipart) and restore time to first query are not measured at scale.
- Uploads and downloads are sequential (one blob at a time).
- Dedup probes each blob with `HeadObject`: on AWS a key that does not
  exist answers 403 instead of 404 when the credentials lack
  `s3:ListBucket`, which fails the take — the builder's credentials need
  list + read + write on the bucket (restore needs read only).
- The v1 restore path (re-ingestion, one document at a time) is
  unchanged in speed; only its fidelity was fixed.
- Index / component templates and `include_global_state` content are not
  snapshotted (templates are not persisted, `docs/ops/persistence.md`).
- The restore opens the index under the store write lock (seconds at
  29 M: requests wait meanwhile).
