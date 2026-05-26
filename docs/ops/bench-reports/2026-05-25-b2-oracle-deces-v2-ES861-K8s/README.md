# Track D — matchID B2 oracle (deces_v2 edge_ngram) vs Elasticsearch 8.6.1

First run of the new `b2-oracle-gate` K8s job: it bootstraps the `deces`
index from `mapping_v2.json` (multi-field `.raw` keyword+normalizer and
`.autocomplete` edge_ngram, with `settings.analysis`) on BOTH Surch and
Elasticsearch 8.6.1, then replays `deces_v2.json` (8 requests) against both
and diffs the responses live (`b1_oracle` binary reused as-is).

- First run GHA `26427933905` @ `f62894b`: **7 / 8 at parity, 1 divergence**.
- Divergence fixed (`eeefcaf`); re-run GHA `26428660584` @ `eeefcaf`:
  **8 / 8 at parity, 0 divergence — A1/A13 CERTIFIED at parity with
  Elasticsearch 8.6.1.**
- ES accepts the deces_v2 mapping (edge_ngram tokenizer + custom analyzers)
  and Surch indexes/serves it identically.

## Certification (re-run after fix)

| Metric | first run (`f62894b`) | re-run (`eeefcaf`) |
|--------|----------------------:|-------------------:|
| total requests | 8 | 8 |
| at parity | 7 | **8** |
| divergences | 1 | **0** |
| skipped | 0 | 0 |

The fix (`eeefcaf`, see below) made the `_source` scan sub-field-aware, so
the `bool` combining a derived sub-field `match` with a `term` now matches
ES. All 8 deces_v2 requests — standalone autocomplete, `.raw` normalizer,
bool, baseline `norm`, accent folding, sort-on-`.raw` — are bit-identical to
Elasticsearch 8.6.1.

## Result

| Metric | Value |
|--------|------:|
| total requests | 8 |
| at parity | 7 |
| divergences | 1 |
| skipped | 0 |

The 7 parity requests include: `autocomplete_nom_prefix_2/3`,
`autocomplete_prenoms_prefix`, `autocomplete_nom_accented_prefix`,
`raw_nom_exact_normalized`, `match_nom_norm_baseline`,
`autocomplete_nom_sorted_by_raw`. So standalone edge_ngram autocomplete,
the keyword `.raw` normalizer, the `norm` baseline, accent folding, and
sort-on-`.raw` all match ES 8.6.1.

## The one divergence (diagnosed)

```
request: bool_autocomplete_nom_and_sexe
  bool.must = [ match NOM.autocomplete=dup , term SEXE=M ]
  ES    hits.total.value = 11
  Surch hits.total.value = 0
```

**Root cause** (`crates/surch-api/src/search.rs`): a top-level `match` is
served from postings (`match_documents_for_index` line 2126 →
`posting_candidate_ids`, no source re-filter), so standalone
`match NOM.autocomplete=dup` matches the indexed ngrams correctly (hence
parity). But a `bool` re-filters its postings candidates through
`query_matches` (line 2116), which evaluates each clause against `_source`
(line 5099+). The derived sub-field `NOM.autocomplete` does not exist in
`_source` (it is index-only), so `field_tokens_for_source` (which does
`field_text(source, "NOM.autocomplete")`) returns no tokens → the clause
fails → the whole bool yields 0.

The slice genuinely contains the data (SEXE is `M`/`F`, 4860 `M`; many
`DUP*` names incl. DUPONT×9), so the query is valid and ES is right.

**Fix direction**: make the source-scan path sub-field-aware — when a
`match`/`term`/`match_phrase` targets a declared sub-field (`parent.sub`),
analyze the PARENT `_source` value with the sub-field's own chain (mirroring
the index-time `subfield_terms` fan-out: keyword+normalizer, custom
edge_ngram, or text analyzer), and tokenize the query with the sub-field's
`search_analyzer`. Tracked as A1/A13 inc.4b follow-up.

## Sources

- GHA run `26427933905` (ci-k8s `b2-oracle-gate`), image
  `sha-f62894b…` / `bench-sha-f62894b…`.
- `b2-oracle.json` envelope (in the `b2-oracle-driver` log, between the
  `BEGIN/END_SURCH_K8S_B2_ORACLE` markers).
- Mapping `tests/matchid_compat/deces/mapping_v2.json`; replay
  `tests/matchid_compat/replays/deces_v2.json`.
