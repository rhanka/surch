# Raw-index snapshot (`/_surch/snapshot/{export,import}`)

Status: implemented, `surch_snapshot_format_version = 1`.

## Why a Surch-specific route

matchID's `deces-backend` already ships its own bundle between hosts:
a tarball of the raw on-disk index, re-ingested on container boot.
The Surch equivalent must be a single self-contained artifact the
operator can `scp` from one node to another, simpler than the full
Elasticsearch `_snapshot` + `_slm` repository surface (which the
packaging plan tracks separately as a Phase B item with an S3
backend). See `docs/wp-d-matchid/SPEC.md` — "Out-of-scope reminders"
for the matchID-side framing.

## Routes

| Method | Path                                  | Body in           | Body out             |
|--------|---------------------------------------|-------------------|----------------------|
| `POST` | `/_surch/snapshot/export?index=<name>`| —                 | `application/gzip`   |
| `POST` | `/_surch/snapshot/import?index=<name>`| `application/gzip`| `application/json`   |

`export` returns 404 when the index does not exist, 400 on an invalid
name. `import` returns 400 when:

- the index already exists (caller `DELETE`s first if they want to
  overwrite — closer to ES semantics than silent clobber);
- the body is not a valid gzipped tar;
- the body exceeds the 1 GiB hard cap (`SNAPSHOT_IMPORT_BODY_LIMIT_BYTES`);
- the archive declares an unsupported `format_version`;
- the archive ships an entry whose path is not in the whitelist (see
  "Security" below).

## On-the-wire format

`surch_snapshot_format_version: 1`. Tarball, gzipped, flat layout (no
directories). Five entries, all required:

```
manifest.json
mapping.json
settings.json
aliases.json
documents.ndjson
```

### `manifest.json`

```json
{
  "format_version": 1,
  "surch_version":  "0.1.0",
  "index":          "deces",
  "doc_count":      25000000
}
```

`format_version` is checked on import — unknown versions are rejected
with `snapshot_import_exception`. The packaging-plan pitfall list
calls this out explicitly: every snapshot manifest carries the format
version so a future codec change cannot silently corrupt an old
restore.

### `mapping.json`

Identical to what `GET /<index>/_mapping` returns for this index:
`{ "properties": { ... } }`. Re-hydrated on import via the same
`IndexMapping::from_properties_value` path the `PUT /<index>`
handler uses, so an export immediately followed by an import is a
round-trip in the strict sense (same field types, same analyzers).

### `settings.json`

Whatever JSON value lives under the index's `settings` slot today.
Surch keeps a single-node static settings shape, but we ship the
whole value so future fields ride along automatically.

### `aliases.json`

Map `alias_name -> alias_definition`. Re-applied on import via the
state's `create_index` call. Aliases pointing at the *source* index
are not rewritten — they will resolve to whatever name the caller
chose under `?index=<new_name>`.

### `documents.ndjson`

One JSON object per line, no trailing comma, blank lines tolerated:

```jsonl
{"_id": "doc-1", "_source": {"title": "alpha", "category": "science"}}
{"_id": "doc-2", "_source": {"title": "beta",  "category": "fiction"}}
```

The `_id` field is the public document id (what the bulk API and
`_doc/{id}` use); the `_source` field is the original document body
as it would come out of `GET /<index>/_doc/{id}`. The internal `u32`
doc id is intentionally not preserved — it is rebuilt on import in
ingestion order, which keeps the snapshot portable across Surch
versions (a future codec change to the doc-id allocator must not
break old snapshots).

## Security

- **Body cap.** `axum`'s `DefaultBodyLimit` rejects bodies larger
  than 1 GiB before the handler runs. The matchID INSEE corpus is
  shipped through `_bulk`, not through this route — anything past
  this cap is a misuse.
- **Path traversal.** The import handler whitelists the five entry
  names above. Any other entry path — including `../etc/passwd`
  shipped by a hostile archiver that didn't go through the Rust
  `tar` crate — is rejected with
  `snapshot_import_exception: unexpected tar entry`.
- **Existing-index refusal.** `import` refuses to overwrite. The
  caller deletes the target index first if that is the intent. This
  matches the ES `restore` default and rules out silent data loss
  by typo on `?index=<name>`.
- **No directory write.** The handler never materialises the
  archive on disk: it parses straight from the in-memory `Bytes`
  body, so there is no temp-file race and no `/tmp` cleanup
  contract to honour.

## Example — `curl`

Export an existing `deces` index and gzip-stream the artifact:

```bash
curl -X POST -o deces.tar.gz \
  http://localhost:7700/_surch/snapshot/export?index=deces
```

Import the same archive on a fresh node under a different name:

```bash
curl -X POST --data-binary @deces.tar.gz \
  -H 'Content-Type: application/gzip' \
  http://localhost:7700/_surch/snapshot/import?index=deces_restored
```

Response:

```json
{
  "acknowledged": true,
  "documents": 25000000,
  "format_version": 1,
  "index": "deces_restored",
  "source_index": "deces"
}
```

## Test coverage

`crates/surch-api/tests/snapshot_raw.rs` exercises five scenarios:

- 100-doc end-to-end export → delete → import → same `hits.total.value`
  on a `match` query (and a 50/50 split on a keyword field, to confirm
  the mapping round-trip);
- export 404 on an unknown index;
- import 400 when the target name is already taken;
- import 400 on a body that is not a gzipped tar;
- import 400 when the archive ships an entry that is not in the
  whitelist (the path-traversal defence).

CI runs the suite under `cargo test --workspace`.

## Out of scope

- S3 / repository registration. The packaging plan tracks this as a
  Phase B item (`PUT /_snapshot/{repo}`, `PUT /_snapshot/{repo}/{name}`,
  cron-driven SLM). The raw-index route documented here is the
  single-tarball half of that plan, scoped to the matchID milestone.
- Incremental / segment-level export. Surch is in-memory single-node
  today; a full re-export is cheap (~25M docs at INSEE scale is
  ~3 GiB gzipped). Incremental shapes ride on the S3 work above.
- Schema evolution. `format_version: 1` is the only one understood.
  When the on-disk codec stabilises (currently in flight under
  wp/a-optim with per-block stats), v2 will switch from
  documents.ndjson to a raw segment dump for a 2-3× restore speedup.
