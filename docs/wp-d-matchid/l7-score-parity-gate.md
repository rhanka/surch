# L7 score parity: ES 8.6.1 differential gate (specification)

Status: specified, not run. Every `_score` expectation committed with lot L7
is `lucene-source-derived` (re-derived from the Lucene 9.4.2 / ES 8.6.1 /
Jackson 2.13.3 / JDK 19 sources), none comes from an Elasticsearch run.
This gate is the step that turns them into ES-verified facts.

Track: D (matchID). Plan: `plan/mvp-matchid.md`, lot L7.

## 1. What the gate must prove

For every request of section 4, Surch and ES 8.6.1 return:

1. the same HTTP status;
2. the same `hits.total` (`value` and `relation`);
3. the same hit order (`_id` sequence) over the returned window;
4. per hit, the same `_score` **bits** (`f32::to_bits` of the parsed value)
   AND the same `_score` **JSON token text** (e.g. `1.0E-4`, `0.33333334`);
5. the same `max_score` bits and token text (`null` included);
6. stretch axis (reported, not gating yet): byte identity of the whole
   response once `took` is removed. Known gap: Surch serializes hit objects
   with sorted keys (`_id`, `_index`, `_score`, `_source`) where ES writes
   `_index`, `_id`, `_score`, `_source`; `_shards` / `timed_out` order also
   differs. Tracked as a shape gap, not a score gap.

## 2. Reference model implemented by L7 (source locations)

| Concern | Source (tag) | Surch location |
| --- | --- | --- |
| Default similarity is `LegacyBM25Similarity` (keeps the `(k1 + 1)` factor through the boost) | ES v8.6.1 `SimilarityService.java:48-50`, `SimilarityProviders.java:254-261`, `LegacyBM25Similarity.java:66-67` | `surch-search/src/scoring.rs` (`es_legacy_bm25_boost`) |
| `idf`, `avgFieldLength`, `LENGTH_TABLE`, norm cache, `BM25Scorer.score` | Lucene 9.4.2 `BM25Similarity.java:104-111, 123-129, 205-208, 229-252` | `scoring.rs` (`lucene_idf`, `lucene_avg_field_length`, `lucene_norm_inverse`, `lucene_bm25_score`, `Bm25TermScorer`) |
| No norms => encoded norm `1` | Lucene 9.4.2 `LeafSimScorer.java` | `search.rs` `term_bm25_score` |
| Double accumulation then `(float)` | `DisjunctionSumScorer.java:38-43`, `ConjunctionScorer.java:58-63`, `BlockMaxConjunctionScorer.java:206-211`, `WANDScorer.java:522-529`, `BooleanScorer.java:40,138,187` | `scoring.rs` `lucene_double_sum` |
| MUST + SHOULD: float addition | `ReqOptSumScorer.java:260-276` | `scoring.rs` `lucene_req_opt_sum` |
| Scorer selection (pure conjunction / pure disjunction / mix, FILTER-only = `0f`) | `Boolean2ScorerSupplier.java:95-260` | `search.rs` `lucene_score` (`Bool` arm) |
| Clause rewrites (single clause, FILTER-only => boost 0, duplicate SHOULD/MUST merged by boost sum) | `BooleanQuery.java:241-...` | `search.rs` `lucene_score`, `dedup_tokens`, `maxscore_match` |
| Boost threading | `BoostQuery.createWeight` (boost * incoming), ES `AbstractQueryBuilder.toQuery:114-121` | `search.rs` `lucene_score` (`boost` parameter) |
| Fuzzy rewrite, per-term boost, top-terms selection, blended df | `FuzzyQuery.java:60-62, 202-207`, `FuzzyTermsEnum.java:248-255`, `FuzzyAutomatonBuilder.java:50-51`, `TopTermsRewrite.java:64-175, 200-214`, `MultiTermQuery.java:163-202`, `BlendedTermQuery.rewrite` | `surch-search/src/fuzzy.rs`, `search.rs` `fuzzy_query_score` |
| `function_score` | ES `FunctionScoreQuery.java:223-246, 400-480`, `CombineFunction.java`, `WeightFactorFunction.java:45`, `FieldValueFactorFunction.java:67-199` | `search.rs` `combine_scoring_functions`, `combine_with_boost_mode` |
| `min_score` | ES `MinimumScoreCollector.java:45` (`float >=`) | `search.rs` `below_min_score` |
| Tie order | Lucene `HitQueue.lessThan` (equal score => lower doc id first), `TopScoreDocCollector` | `surch-search/src/topk.rs` `scored_pair_ordering` |
| `_score` text | ES `SearchHit.java:664-667`, `SearchHits.java:185-188`, `JsonXContentGenerator.java:248-254`, Jackson 2.13.3 `UTF8JsonGenerator.java:1035-1046` (`String.valueOf(float)`), JDK 19.0.1 `Float.toString` | `surch-search/src/java_float.rs`, `surch-api/src/es_json.rs` |

Sources were read from `raw.githubusercontent.com` at tags
`apache/lucene releases/lucene/9.4.2`, `elastic/elasticsearch v8.6.1`,
`FasterXML/jackson-core jackson-core-2.13.3`.

## 3. Environment (both engines)

- ES 8.6.1, single node, index `deces`, `number_of_shards: 1`,
  `number_of_replicas: 0`, mapping `tests/matchid_compat/deces/mapping_real.json`
  (the real `deces_index.yml`).
- Same corpus, bulk-loaded once, no update and no delete (Lucene counts
  deleted documents in `docFreq` / `docCount` until merged; Surch does not).
- ES: `POST /deces/_forcemerge?max_num_segments=1` then `_refresh`, so
  the Lucene doc id order is one stable sequence.
- Doc id alignment for ties: export the ES order with a `sort: ["_doc"]`
  scroll over `match_all`, and bulk-load Surch in exactly that order
  (internal id = load order). Without this step equal-score hits may be
  ordered differently even when every score matches.
- Corpus sizes: the 1 000-doc slice (`slice-1000.ndjson`) for the first
  run; then the 1.36 M smoke corpus.

## 4. Request list

Each entry is a full `_search` body (deces-backend shapes from
`packages/deces-backend/src/buildRequest.ts` + `queries.ts`, matchID main
`6732755`), run twice: as is, and with `size: 50`. `[x]` names reuse
`tests/matchid_compat/replays/deces_v1.json` entries.

### 4.1 deces-backend shapes

1. Simple full-text, one name (`buildSimpleMatch`, `min_score: 5`,
   `track_total_hits: true`): `bool.must[bool{must[match PRENOMS_NOM
   fuzziness auto], should[match PRENOM_NOM, match PRENOM_NOM fuzziness
   auto]}]`.
2. Simple full-text, two names (adds the `bool{minimum_should_match:1,
   should:[NOM/PRENOMS/PRENOM permutations]}` must clause).
3. Simple full-text, names + date `dd/mm/yyyy` (`function_score` over
   `bool{must:[namesQuery], should:[dateQuery]}`).
4. Simple full-text, date only (root `dateQuery`).
5. Simple full-text with no name and no date (`match_all`).
6. Id lookup `bool.must[match UID]` (`buildIndexMatch`).
7. Advanced search, every single field alone (`buildAdvancedMatch`,
   `function_score{bool{must:[...]}}`): `firstName`, `lastName`, `legalName`,
   `sex`, `birthDate` (exact / `yyyy` prefix / range), `birthCity`,
   `birthDepartment`, `birthCountry`, `deathDate`, `deathAge` (single /
   range), `birthGeoPoint` + distance (`geoPointQuery`).
8. Advanced search, fuzzy name (`nameQuery` with `fuzzy: true`:
   `boost: 2` / `boost: 0.5` bools, `firstNameQuery` `boost: 4` / `boost:
   2` matches, `fuzzyShouldTermQuery` on `.raw`).
9. Advanced search, exact name (`fuzzy: false`, `min_should`).
10. Adaptive block (`buildAdaptativeBlockMatch`, `track_total_hits: false`)
    with first+last name only, + legal name, + birthDate exact (fuzzy 1 +
    `prefix` 4 chars `bool{should, minimum_should_match:1}`), + birthDate
    range, + deathDate exact, + deathDate range.
11. Advanced block (`block.scope` + `block.minimum_match`, `block.should`).
12. Every request of `tests/matchid_compat/replays/deces_v1.json` (30
    entries, scroll pages included).

### 4.2 Unit shapes (one scoring rule each)

13. `match NOM "dupont"` (single term, text, norms).
14. `match NOM "jean pierre"` (SHOULD double sum).
15. `match NOM "jean jean jean"` (duplicate clauses merged, boost 3).
16. `match NOM {query: "jean pierre", operator: "and"}`.
17. `match NOM.raw "DUPONT"` (keyword sub-field: no norms, `freq = 1`).
18. `match AGE_DECES 75` (numeric: constant score 1).
19. `bool{must:[match NOM], should:[match PRENOM]}` (ReqOptSumScorer).
20. `bool{must:[match NOM], should:[match PRENOM], minimum_should_match: 1}`.
21. `bool{should:[match NOM, match PRENOM, match COMMUNE_NAISSANCE]}`.
22. `bool{boost: 2, should:[...]}` and `bool{boost: 0.5, should:[...]}`.
23. `bool{must:[match NOM], should:[prefix DATE_NAISSANCE "1950"]}`.
24. `bool{filter:[match NOM]}` (score `0.0`), `bool{}` (score `1.0`),
    `bool{must:[match_all], filter:[geo_distance]}`.
25. `match NOM {query: "dupond", fuzziness: "auto"}` and `fuzziness: 1`,
    `fuzziness: 2`, `prefix_length: 2`, `max_expansions: 3`, `boost: 2`.
26. `function_score{query: match NOM, boost: 2}` (no function) and with
    `weight`, `field_value_factor{log1p}`, `score_mode: avg`, `boost_mode:
    sum|replace|max`.
27. `min_score` equal to a hit score (kept), one ulp above (dropped).
28. A query whose scores reach `>= 1e7` or `< 1e-3` (e.g. `boost: 1e7`,
    `boost: 1e-6`) to pin the `E` notation of `Float.toString`.

## 5. Comparator specification

Implementation: extend the existing Rust driver
`crates/surch-demo/src/bin/b1_oracle.rs` with a `score` axis (no Python and
no new container image, per the owner rule; Node/TS is the only other
accepted stack).

Per request:

1. POST the body to both engines (`--url-surch`, `--url-es`), keep the
   raw response bytes.
2. Parse both responses with a parser that preserves the raw number token
   (`serde_json` with `arbitrary_precision`, or a token scan of the bytes):
   for every hit keep `(_id, raw _score token, f32 bits)`.
3. Compare status, `hits.total`, the `_id` sequence, then per position
   the `_score` token text and bits, then `max_score`.
4. On the first hit whose score differs, re-run the request on ES with
   `"explain": true` (or `GET /deces/_explain/{id}`) and on Surch with
   the same body; extract from the ES explanation tree the leaves `boost`,
   `idf` (`n`, `N`), `tf` (`freq`, `k1`, `b`, `dl`, `avgdl`) of every
   `weight(...)` node, recompute each term score with the L7 kernel
   (`surch_search::scoring`), and report the first leaf whose value Surch
   would compute differently (statistics gap: `N`, `n`, `avgdl`, `dl`;
   rewrite gap: missing/extra `weight(...)` term, fuzzy boost; combination
   gap: sum of leaves vs node value).
5. Stretch axis: byte compare after deleting the `"took":<n>,` token.

Outputs:

- `target/bench-reports/l7-score-gate.json` — machine envelope
  `surch.bench.l7_score_gate.v1`: per request `{name, status, total,
  order_ok, score_mismatches: [{pos, id, es_token, surch_token, es_bits,
  surch_bits, explain_leaf}], max_score_ok, bytes_identical}`.
- `target/bench-reports/l7-score-gate.md` — human summary: one row per
  request (pass/fail), the first mismatch per failing request with its
  explanation leaf, and totals.
- Exit code 0 iff every request passes axes 1-5 (axis 6 is reported only).

## 6. Residual risks known before the run

- `Math.log` (HotSpot intrinsic) vs Rust `f64::ln` (glibc) may differ by
  one `f64` ulp; visible only when the idf sits on an `f32` rounding
  boundary (~2^-29 per term).
- Boost products of three or more non-power-of-two factors: Lucene merges
  nested `BoostQuery` objects at rewrite time (`(a * b) * incoming`)
  where Surch multiplies top-down (`a * (b * incoming)`); bits can differ
  in the last place. All deces-backend boosts (2, 0.5, 4, 2) are powers
  of two, so the products are exact.
- `match_phrase` is scored bag-of-words (not a Lucene `PhraseQuery`); not
  emitted by deces-backend.
- A root `match_all` hit carries no `_score` in Surch (ES: `1.0`); the
  deces-backend wraps it in `bool.must`, which is scored `1.0`.
- `fuzzy` (term-level query) analyses its value and ANDs the tokens; ES
  takes one raw term. Not emitted by deces-backend (it uses `match` +
  `fuzziness`).
- A keyword value repeated inside one document: Lucene counts unique
  terms for `sumTotalTermFreq`; Surch counts values.
- Multi-shard indices: scores are per-shard statistics in ES; the gate
  assumes one shard.
- Index statistics of indices persisted before L7: keyword fields and
  sub-fields carry no `sumTotalTermFreq`/stats (avgdl falls back to 1.0;
  sub-fields score 0) until reindexed.
