# Persistent data directory (`SURCH_DATA_DIR`)

Plan: `plan/mvp-matchid.md` lot L6, step 1. Design:
`docs/paper/persistence-iceberg-architecture.md` (segments + atomic
manifest). This page is the operator view of what is implemented.

## Why

Without a data directory Surch keeps no state across a restart: the
postings, `_source` and sub-field spill files are `TMPDIR` tempfiles
deleted when the process exits, and mappings, settings and aliases live
only in memory. For matchID (29.3 M documents, production node 4.5 GiB /
1 core) a pod restart means hours of re-indexing. With `SURCH_DATA_DIR`
set, every index is committed to disk at each `_refresh` and reopened at
startup without re-analysing a single document.

## Enabling

| Variable | Effect |
|---|---|
| `SURCH_DATA_DIR=/path` | Persist under `/path`, reopen everything at start. Unset (default): historical in-memory engine, byte-for-byte unchanged. |
| `SURCH_DATA_VERIFY=size` | At start, check data files by existence + length only (skip the blake3 re-hash of `.pst`, `.sub`, `source.dat`). Default: full blake3 verification. Framed metadata files are always hash-checked. |

At start the server takes an exclusive lock (`LOCK`, `flock`) on the
directory: a second process on the same directory refuses to start. It
then points every spill tempfile (postings segments, sub-field columns,
merge scratch, the live `_source` store) at `<data>/tmp`, the same
filesystem as the index directories, so sealing a segment is a hard
link, never a copy.

Linux only (the target image is distroless glibc). The mount must
support hard links and `rename(2)`; a local volume (PVC, hostPath, NVMe)
is the expected backend. Network filesystems without POSIX rename
semantics are not supported.

## On-disk layout

```
<SURCH_DATA_DIR>/
├── LOCK                         exclusive process lock
├── ALIASES                      framed "ALIA": every alias -> {index: definition}
├── tmp/                         spill tempfiles of the running process (wiped at start)
└── indices/
    └── i-<escaped index name>/  one directory per index ([a-z0-9._-] kept, other bytes %XX)
        ├── MANIFEST             framed "MANI": the committed generation (atomically replaced)
        ├── source.dat           append-only _source store (raw / zstd per doc / D1 zstd blocks)
        ├── meta-<id>.meta       framed "META": mapping, settings, counters, D1 block directory,
        │                        index_prefixes side postings
        ├── ids-<id>.ids         framed "IDMP": uid -> doc_id FST, doc_id -> uid, packed _source slots
        ├── seg-<id>.seg         framed "SEGM": one sealed segment's resident state
        ├── seg-<id>.pst         that segment's FoR postings file (existing encoding, linked as-is)
        └── seg-<id>.sub         that segment's spilled sub-field columns (existing encoding)
```

`<id>` is a per-index counter that is never reused; a file is written
once and never modified. `source.dat` is the only file that grows: it is
append-only and the manifest records its valid prefix length.

### Framed files

Every file holding state that used to be resident-only starts with a
52-byte header:

```
magic "SRCH" (4) | kind (4, e.g. "MANI") | format version u32 LE |
payload length u64 LE | blake3-256 of the payload (32)
```

The reader checks magic, kind, version and exact length before decoding,
hashes the payload while decoding, and refuses a trailing byte. Format
versions: `MANI`/`META`/`IDMP`/`ALIA` = 1 (`surch-api::persist`),
`SEGM` = 1 (`surch-index::document_index::SEGMENT_FORMAT_VERSION`).

### What each file holds

- `SEGM`: doc range (`doc_base`, `doc_count`), live-docs bitset,
  per-field BM25 length stats (Lucene SmallFloat norms, 1 byte/doc),
  sub-field columns (resident dict + codes, or the spill descriptors into
  `.sub`), and the resident part of the term dictionary: per-field FST
  bytes, CSR / descriptor channels (RAM-postings mode or disk-mode
  fallbacks), roaring side tables of high-`df` terms (container-level
  encoding), the P2 page digests and codec counters.
- `.pst`: unchanged C1/D2 layout — FoR blocked payloads, packed block
  directories (10 B/entry) and `TermEntry` tables (28 B/term). With
  `SURCH_POSTINGS_DISK=1` the reopened index reads postings from it
  through the same `pread` + P2-verified path as before the restart.
- `.sub`: unchanged S5c layout (dict offsets, dict bytes, codes).
- `source.dat`: unchanged `mmap M1` / tranche 2b / D1 encodings; the D1
  block directory (8 B/block) lives in `META`.
- `IDMP`: the dense id maps of lot C2 (`forward` FST, `reverse_uids`,
  `reverse_offsets`, packed `documents` slots).

## When state is committed

| Event | Committed |
|---|---|
| `PUT /<index>` (create) | empty index + its aliases |
| `POST /<index>/_refresh` | full new generation of that index + aliases |
| `PUT /<index>/_mapping`, mapping merge | the index is sealed (as a refresh) and committed |
| `POST /_aliases`, alias add/remove | `ALIASES`, one atomic rename for the whole batch |
| `DELETE /<index>`, `remove_index` alias action | the index directory is removed |

Documents written after the last `_refresh` are **not** durable: a crash
loses them (there is no translog; design §9 "Append-only WAL: skipped").
The bulk-then-refresh loading pattern (matchID) is fully covered.
`_bulk?refresh=true` and `_snapshot/.../_restore` do not go through
`_refresh` today: send an explicit `POST /<index>/_refresh` after them
to make the result durable.

A commit only writes what changed: segments already on disk are
recognised by identity and never rewritten; `ids-*.ids` is rewritten
only when the id maps changed; `meta-*.meta` is small except for the D1
block directory. The id-maps rewrite is O(corpus) per refresh that
changed them — the same order as the in-memory FST merge the refresh
already performs.

## Crash safety

Commit order for a new generation (under the index write lock):

1. `source.dat`: in-progress D1 block sealed, file trimmed to its
   written length and `fsync`ed; reachable as `<index>/source.dat`
   (hard link on first commit, same inode afterwards).
2. New segment files: `.pst` / `.sub` trimmed and linked, `fsync`ed;
   `.seg` written and `fsync`ed.
3. `ids-*.ids` (if changed) and `meta-*.meta`, `fsync`ed.
4. `fsync` of the index directory.
5. `MANIFEST.tmp.<pid>.<nonce>` written + `fsync`, `rename` over
   `MANIFEST`, `fsync` of the directory — **the commit point**.
6. Files no longer referenced are deleted (best effort).

Argument: before step 5 completes, `MANIFEST` is the previous generation
and still names only files that existed and were durable when it was
committed — nothing in steps 1-4 modifies or deletes an existing file
(new names only; `source.dat` only grows past the committed prefix).
After step 5, every file the new manifest names was `fsync`ed in steps
1-4. `rename(2)` makes the switch atomic, so a reader of the directory
sees one generation or the other, never a mix. Index deletion is a single
rename out of the `i-*` namespace, then the recursive removal.

At start (`AppState::open_with_data_dir`):

1. `tmp/` is emptied, half-deleted `.deleted-*` directories and
   `ALIASES.tmp.*` are removed.
2. An index directory without `MANIFEST` was never committed (crash
   before its first rename): removed.
3. For each `MANIFEST`: files it does not reference (generation orphans,
   `MANIFEST.tmp.*`) are deleted; `.pst` / `.sub` are checked (length,
   blake3) and `source.dat` per committed chunk (blake3 of each grown
   range); `source.dat` is truncated back to the committed length.
4. `META`, `IDMP`, `SEGM` are decoded with their checksums; id maps and
   block directories are bounds-checked against `source.dat`.
5. `ALIASES` is loaded; entries pointing at missing indices are dropped.

**Fail closed**: any missing file, short file, checksum mismatch,
structural inconsistency or format-version mismatch aborts the start
with an error naming the file, e.g.

```
surch-api: cannot open SURCH_DATA_DIR /data: /data/indices/i-deces/seg-00000003.pst:
truncated or resized file (expected 41213 bytes, found 20606)
```

Operator action: restore the directory from a backup (or the S3
snapshot once L6 step 2 lands) or move it aside and re-index. Never edit
files in place.

## Behaviour after a restart

- No document is re-analysed: the reopened term dictionaries are the
  persisted ones (`index_terms_build_count` stays 0, asserted by the
  test suite), postings / `_source` / spilled sub-fields are read
  through the same `pread` paths, so RSS after reopen stays at the
  resident structures (FSTs, id maps, norms, roaring, digests).
- Search responses are byte-identical to the ones served before the
  restart (scores, sort values, aggregations, order), `took` excepted.
- The reopened index is in the state a `_refresh` leaves behind: a
  later write takes the existing post-refresh `rebuild_index()` path
  (re-analysis of the live documents), exactly as it would without a
  restart.
- Per-index test/ops overrides (`set_postings_disk_enabled`, flush
  budget, merge fan-in) are not persisted; the reopened segments keep the
  layout they were built with and the next rebuild follows the
  environment flags.

## Not covered yet (L6 step 2+)

- S3 snapshot of sealed segment files (`_snapshot` still stores
  documents for re-ingest).
- Index templates and component templates are not persisted.
- No translog: writes since the last `_refresh` are lost on crash.
- Incremental writes after a reopen (or after any refresh) rebuild the
  index in memory; an append path on top of sealed segments is needed
  for a write-heavy workload.
- Id maps are rewritten whole when they change (append-only layout to
  make refreshes O(new docs)).
- Proof at 29 M under the production caps (time to first query, RSS).

## Measurement (200 k synthetic documents, release build)

Local run, 2026-11-15, release `surch-api` binary (jemalloc), one
process per phase, production-like flags `SURCH_SOURCE_COMPRESS=1` (D1
blocks), `SURCH_POSTINGS_DISK=1`, `SURCH_FLUSH_BUDGET_BYTES=268435456`,
`SURCH_MERGE_FANIN=8`, `SURCH_MERGE_MAX_DOCS=7000000`,
`SURCH_DENSIFY_BUDGET_DOCS=1000000`. Corpus: 200 000 deces-shaped
documents (generated, matchID v2 mapping with `raw`/`autocomplete`
multi-fields), 20 `_bulk` chunks of 10 000, one `_refresh`. The restart
follows a `kill -9` of the server (no graceful shutdown). Single run,
no repetition: treat the build-time delta as indicative.

| Metric | In-memory (no data dir) | `SURCH_DATA_DIR` |
|---|---|---|
| Build: 20 bulks + `_refresh` | 3 356 ms | 3 606 ms (+7 %, includes the commit) |
| Persisted size (`du`, apparent / allocated) | — | 47.3 MB / 45.2 MiB, 8 files |
| ↳ `seg-*.pst` / `seg-*.seg` / `seg-*.sub` | — | 24.4 MB / 6.8 MB / 4.2 MB |
| ↳ `source.dat` / `ids-*.ids` / `meta-*.meta` | — | 7.9 MB / 4.0 MB / 24 KB |
| Restart (`exec` → first `_search` answered), full blake3 verification | (re-index: 3.4 s + client time) | 46 ms |
| RSS after build (after `_refresh` purge) / peak | 57.2 MiB / 382 MiB | 57.3 MiB / 391 MiB |
| RSS right after reopen / peak | — | 30.8 MiB / 30.8 MiB |
| RSS after the 6-query battery (aggs, sort on `NOM.raw`, fuzzy) | — | before restart 291 MiB, after reopen 268 MiB |
| Responses (6 queries, `took` excepted) | identical to persisted | identical before / after restart |

The reopen reads no posting and analyses no document: resident memory
right after reopen is the FSTs, id maps, norms and roaring side tables
only, and it grows with the queries the same way it does on a freshly
built index. Not measured here: the 29 M production scale (L6 proof
item), repeated runs, a cold page cache.
