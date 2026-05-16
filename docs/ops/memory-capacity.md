# Memory & capacity planning

Surch keeps every index fully in RAM, so sizing a cluster (notably the
matchID INSEE indexer at ~1.3 M docs) hinges on knowing how many bytes
the postings, prefix-postings side table, BM25 field stats, term
block-metas and stored `_source` payloads consume.

## Endpoints

- `GET /_prometheus_metrics` — per-index gauges (one label `index`):
  - `surch_index_postings_bytes`
  - `surch_index_prefix_postings_bytes`
  - `surch_index_stored_fields_bytes`
  - `surch_index_field_stats_bytes`
  - `surch_index_term_stats_bytes`
  - `surch_index_total_bytes` (sum of the above)
  - `surch_index_doc_count`
- `GET /_surch/stats[?index=<name>]` — JSON breakdown of the same
  numbers; the optional `?index=<name>` filter restricts the response
  to a single index. An unknown name returns `{"indices":{}, "total_bytes":0}`.

Gauges are refreshed at indexing time only (`_bulk`, `_doc`, delete,
snapshot import, mapping change). The search hot path is untouched.

The bytes counts are **approximations**: `sizeof + Vec/String
capacity` summed over `BTreeMap` entries, with the FST term
dictionary itself reported as zero (the per-term `Vec<Posting>` and
`Vec<BlockMeta>` dominate the RAM cost in practice). Use them for
capacity planning and trend analysis, not as a leak detector.

## Useful PromQL

```promql
# Total RAM per index, in GiB
surch_index_total_bytes / 1024 / 1024 / 1024

# Inverted-index share (postings + block metas + field stats)
( surch_index_postings_bytes
  + surch_index_term_stats_bytes
  + surch_index_field_stats_bytes )
  / surch_index_total_bytes

# Average bytes-per-doc, by index
surch_index_total_bytes / surch_index_doc_count
```

## Order-of-magnitude estimate — INSEE 1.3 M docs

Sampling a 25 k-doc BAN slice gives ~1.4 KB / doc of stored `_source`
and ~3.5 KB / doc total (postings + prefix + stored + stats). Linear
extrapolation:

| Corpus              | Docs       | Total RAM |
| ------------------- | ---------- | --------- |
| BAN Paris (sample)  | 25 000     | ~85 MB    |
| INSEE (target)      | 1 300 000  | ~4.5 GB   |

The estimate is an **upper bound** on the inverted-index side (long
field values + `index_prefixes` inflate the side table) and a
**lower bound** on the stored side if downstream callers attach
large derived fields. Verify on the real corpus once it is loaded;
the gauges will reflect the actual footprint after the first batch.
