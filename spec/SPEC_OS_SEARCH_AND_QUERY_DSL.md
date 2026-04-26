# SPEC - OpenSearch Search And Query DSL

## Purpose

Define the Search API request body and Query DSL subset that Surch MVP must accept, validate, and execute with OpenSearch-compatible semantics where explicitly listed below.

This document is intentionally narrow: it locks the MVP request contract and records explicit exclusions so later implementation branches do not need to infer behavior from broad OpenSearch documentation.

## Sources

- OpenSearch Search API
- OpenSearch Query DSL main documentation
- Clause-level docs for `match`, `match_phrase`, `multi_match`, `term`, `terms`, `range`, `exists`, `bool`, `prefix`, `wildcard`, `fuzzy`
- `regexp` docs reviewed only to decide MVP deferral policy

## MVP Scope

In scope:
- Search request body fields: `query`, `from`, `size`, `sort`, `track_total_hits`
- Query clauses: `match`, `match_phrase`, `multi_match`, `term`, `terms`, `range`, `exists`, `bool`, `prefix`, `wildcard`, `fuzzy`
- Validation rules, defaults, and bounded anti-abuse expectations

Out of scope for MVP:
- Aggregations, highlighting, explain, suggest, collapse, rescore, post_filter, search_after, pit, scroll
- Query-string syntax
- Nested queries, geo queries, span queries, script queries, function score
- Numeric `track_total_hits` values
- `regexp` execution support

## Top-Level Request Contract

### Accepted request body

```json
{
  "query": { "<clause>": { } },
  "from": 0,
  "size": 10,
  "sort": ["_score"],
  "track_total_hits": true
}
```

All top-level fields are optional.

### Top-level field rules

| Field | Type | Default | MVP Rules |
|---|---|---|---|
| `query` | object | implicit match-all behavior when absent | Exactly one root clause object. Empty object is invalid. |
| `from` | integer | `0` | Must be `>= 0`. |
| `size` | integer | `10` | Must be `>= 0` and subject to server-side maximum. |
| `sort` | array | `["_score"]` | MVP accepts `_score`, `{"_score":"asc|desc"}`, or single-field sort objects. Unsupported sort forms must be rejected. |
| `track_total_hits` | boolean | `true` | Only boolean supported in MVP. Numeric form is rejected as unsupported. |

### Unknown field policy

- Unknown top-level fields must be rejected with a validation error.
- Unknown clause parameters must be rejected with a validation error.
- Later branches may relax this only by explicit spec update, not by implementation choice.

## Clause Support Matrix

| Clause | MVP Status | Notes |
|---|---|---|
| `match` | MUST | Main analyzed full-text query |
| `match_phrase` | MUST | Phrase query with `slop` |
| `multi_match` | MUST | Limited to `best_fields` semantics |
| `term` | MUST | Exact value query |
| `terms` | MUST | Exact OR over explicit list |
| `range` | MUST | One or more bound operators required |
| `exists` | MUST | Field presence query |
| `bool` | MUST | `must`, `filter`, `should`, `must_not`, `minimum_should_match` |
| `prefix` | MUST | Prefix query with bounded execution |
| `wildcard` | MUST | Wildcard query with bounded execution |
| `fuzzy` | MUST | Edit distance bounded to `<= 2` |
| `regexp` | OUT OF MVP | Reject with explicit unsupported-clause error |

## Clause Grammar And Semantics

### `match`

Accepted forms:

```json
{ "match": { "title": "rust" } }
```

```json
{
  "match": {
    "title": {
      "query": "rust",
      "operator": "or",
      "analyzer": "standard",
      "fuzziness": "AUTO",
      "prefix_length": 0,
      "max_expansions": 50
    }
  }
}
```

Rules:
- Target field name is required and must map to either a scalar value or an option object.
- Scalar shorthand is equivalent to `{ "query": <scalar> }`.
- `query` is required in object form.
- `operator` accepts only `or` or `and`; default is `or`.
- `fuzziness`, `prefix_length`, and `max_expansions` follow the fuzzy rules defined later in this spec.

### `match_phrase`

```json
{
  "match_phrase": {
    "title": {
      "query": "search engine",
      "slop": 0,
      "analyzer": "standard"
    }
  }
}
```

Rules:
- Field name is required.
- Scalar shorthand is allowed and means `{ "query": <scalar> }`.
- `slop` default is `0`.
- `slop` must be an integer `>= 0`.

### `multi_match`

```json
{
  "multi_match": {
    "query": "rust",
    "fields": ["title", "body"],
    "type": "best_fields",
    "operator": "or",
    "fuzziness": "AUTO"
  }
}
```

Rules:
- `query` is required.
- `fields` is required and must be a non-empty array of field names.
- `type` defaults to `best_fields`.
- Only `best_fields` is supported in MVP; any other `type` is rejected.
- `operator` accepts only `or` or `and`; default is `or`.
- `fuzziness` follows the fuzzy rules in this spec.

### `term`

Accepted forms:

```json
{ "term": { "status": "published" } }
```

```json
{
  "term": {
    "status": {
      "value": "published",
      "boost": 1.0,
      "case_insensitive": false
    }
  }
}
```

Rules:
- Field name is required.
- Scalar shorthand is equivalent to `{ "value": <scalar> }`.
- `value` is required in object form.
- `case_insensitive` defaults to `false`.
- If case-insensitive term execution is not implemented in the consuming branch, requests using `case_insensitive: true` must be rejected rather than silently ignored.

### `terms`

```json
{ "terms": { "status": ["published", "draft"] } }
```

Rules:
- Field name is required.
- Value must be a non-empty array.
- Semantics are logical OR across listed values.

### `range`

```json
{
  "range": {
    "price": {
      "gte": 10,
      "lt": 100
    }
  }
}
```

Rules:
- Field name is required.
- At least one of `gt`, `gte`, `lt`, `lte` is required.
- `format` and `time_zone` are accepted only when the mapped field type supports them.
- Contradictory bounds must be rejected with validation error.

### `exists`

```json
{ "exists": { "field": "title" } }
```

Rules:
- `field` is required.
- No alternate shorthand form.

### `bool`

```json
{
  "bool": {
    "must": [{ "match": { "title": "rust" } }],
    "filter": [{ "term": { "status": "published" } }],
    "should": [{ "prefix": { "title": "sur" } }],
    "must_not": [{ "exists": { "field": "deleted_at" } }],
    "minimum_should_match": 0
  }
}
```

Rules:
- At least one of `must`, `filter`, `should`, or `must_not` must be present.
- `must`, `filter`, `should`, and `must_not` must each be arrays of clause objects.
- Default `minimum_should_match` behavior:
  - `1` when the bool query contains only `should`
  - `0` when the bool query also contains `must` or `filter`
- `minimum_should_match` must be an integer `>= 0` in MVP.
- Deep bool nesting must be bounded by the implementation branch.

### `prefix`

Accepted forms:

```json
{ "prefix": { "sku": "sur" } }
```

```json
{
  "prefix": {
    "sku": {
      "value": "sur",
      "case_insensitive": false
    }
  }
}
```

Rules:
- Scalar shorthand is equivalent to `{ "value": <scalar> }`.
- `value` is required in object form.
- `case_insensitive` defaults to `false`.

### `wildcard`

Accepted forms:

```json
{ "wildcard": { "sku": "sur*" } }
```

```json
{
  "wildcard": {
    "sku": {
      "value": "sur*",
      "case_insensitive": false
    }
  }
}
```

Rules:
- Scalar shorthand is equivalent to `{ "value": <scalar> }`.
- `value` is required in object form.
- `case_insensitive` defaults to `false`.
- Execution must be bounded. Leading-wildcard allowance is an implementation decision only if it is explicitly guarded by cost controls; otherwise reject it.

### `fuzzy`

```json
{
  "fuzzy": {
    "title": {
      "value": "surch",
      "fuzziness": "AUTO",
      "prefix_length": 0,
      "max_expansions": 50,
      "transpositions": true
    }
  }
}
```

Rules:
- Field name is required.
- `value` is required.
- `fuzziness` defaults to `AUTO`.
- `prefix_length` defaults to `0`.
- `max_expansions` defaults to `50`.
- `transpositions` defaults to `true`.

## Fuzzy Rules For MVP

Accepted `fuzziness` values:
- `AUTO`
- `0`
- `1`
- `2`

Rejected `fuzziness` values:
- Any edit distance above `2`
- Non-numeric strings other than `AUTO`
- OpenSearch variants such as `AUTO:low,high`

`AUTO` interpretation:
- input length `0-2` -> distance `0`
- input length `3-5` -> distance `1`
- input length `>5` -> distance `2`

Semantic note:
- Surch MVP fuzzy behavior must use Damerau-Levenshtein distance semantics with transpositions enabled by default.

## Validation Contract

The parser or request validator must distinguish three classes of failure:
- malformed JSON -> parse error
- wrong type or structurally invalid query -> validation error
- syntactically valid but unsupported MVP feature -> unsupported error

Validation requirements:
- Reject malformed JSON with a clear parse error.
- Reject wrong types for `from`, `size`, `sort`, `track_total_hits`, and clause-specific fields.
- Reject empty `query` objects.
- Reject `range` queries with no bounds.
- Reject `terms` queries with an empty list.
- Reject `bool` queries with no populated clause list.
- Reject `multi_match.type` values other than `best_fields`.
- Reject `track_total_hits` numeric values as unsupported in MVP.
- Reject `regexp` queries as unsupported in MVP.
- Reject unknown top-level fields and unknown clause parameters.

## Search Response Minimum Contract

Critical response fields for MVP compatibility:
- `took`
- `timed_out`
- `_shards`
- `hits.total`
- `hits.max_score`
- `hits.hits`
- Each hit exposes `_index`, `_id`, `_score`, `_source`

This branch does not define full response-body compatibility outside those fields.

## Required Compatibility Scenarios

Positive cases:
1. request without `query` returns match-all behavior
2. `match` with scalar shorthand
3. `match` with `operator` and `fuzziness`
4. `match_phrase` with `slop`
5. `multi_match` with `fields` and `best_fields`
6. `term` with scalar shorthand
7. `terms` with non-empty array
8. `range` with numeric bounds
9. `exists` success
10. `bool` with `must`, `filter`, `should`, and `must_not`
11. `prefix` with scalar shorthand
12. `wildcard` with bounded trailing wildcard pattern
13. `fuzzy` with `AUTO`
14. `fuzzy` with explicit edit distance `2`

Negative cases:
15. malformed JSON returns parse error
16. invalid top-level type returns validation error
17. unknown top-level field returns validation error
18. empty `query` object returns validation error
19. `range` without any bound returns validation error
20. `terms` with empty array returns validation error
21. `bool` without any populated clause array returns validation error
22. unsupported `multi_match.type` returns unsupported error
23. numeric `track_total_hits` returns unsupported error
24. `regexp` query returns unsupported error
25. `fuzziness: 3` returns unsupported error

## Anti-Abuse And Cost Controls

The consuming implementation branch must enforce bounded work for:
- maximum `size`
- maximum bool nesting depth
- maximum wildcard pattern cost
- maximum fuzzy expansions
- `track_total_hits` computation cost

This branch does not lock the exact numeric ceilings because governance and runtime limits were not available inside the allowed-path scope. Those ceilings must be defined before API implementation is declared production-ready.
