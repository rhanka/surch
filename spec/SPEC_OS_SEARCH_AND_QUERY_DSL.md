# SPEC - OpenSearch Search And Query DSL

## Purpose

Capture the MVP-compatible search request grammar and Query DSL behavior Surch must reproduce.

## Sources

- OpenSearch Search API
- OpenSearch Query DSL main documentation
- clause-level docs for `match`, `match_phrase`, `multi_match`, `term`, `terms`, `range`, `exists`, `bool`, `prefix`, `wildcard`, `regexp`, `fuzzy`

## MVP Request Shape

```json
{
  "query": { "<clause>": { } },
  "from": 0,
  "size": 10,
  "sort": ["_score"],
  "track_total_hits": true
}
```

Optional top-level fields in MVP:
- `query`
- `from`
- `size`
- `sort`
- `track_total_hits`

## Clause Contract Summary

| Clause | Required Fields | Important Optional Fields | Key Defaults | MVP Support |
|---|---|---|---|---|
| `match` | target field and `query` | `operator`, `analyzer`, `fuzziness`, `prefix_length`, `max_expansions` | `operator=or` | MUST |
| `match_phrase` | target field and `query` | `slop`, `analyzer` | `slop=0` | MUST |
| `multi_match` | `query` | `fields`, `type`, `operator`, `fuzziness` | `type=best_fields` | MUST |
| `term` | target field and exact `value` | `boost`, `case_insensitive` | exact semantics | MUST |
| `terms` | target field and list of values | `boost` | OR semantics | MUST |
| `range` | target field and one or more of `gt`, `gte`, `lt`, `lte` | `format`, `time_zone` | none | MUST |
| `exists` | `field` | `boost` | none | MUST |
| `bool` | none, but useful clauses must exist | `must`, `filter`, `should`, `must_not`, `minimum_should_match` | MSM depends on composition | MUST |
| `prefix` | target field and `value` | `case_insensitive` | false | MUST |
| `wildcard` | target field and `value` | `case_insensitive` | false | MUST |
| `regexp` | target field and `value` | `flags`, `max_determinized_states` | engine-bounded | SHOULD |
| `fuzzy` | target field and `value` | `fuzziness`, `prefix_length`, `max_expansions`, `transpositions` | `fuzziness=AUTO`, `transpositions=true` | MUST |

## Fuzzy Rules For MVP

- Accept `AUTO`, `0`, `1`, or `2`
- Treat edit distance above `2` as out of MVP support
- `transpositions` defaults to `true`
- `prefix_length` defaults to `0`
- `max_expansions` defaults to `50`

`AUTO` interpretation:
- input length `0-2` -> distance `0`
- input length `3-5` -> distance `1`
- input length `>5` -> distance `2`

## Search Response Shape

Critical MVP response fields:
- `took`
- `timed_out`
- `_shards`
- `hits.total`
- `hits.max_score`
- `hits.hits`
- each hit should expose `_index`, `_id`, `_score`, `_source`

## Validation Expectations

- reject malformed JSON with clear error
- reject invalid types for `from`, `size`, `track_total_hits`, and clause-specific fields
- reject `range` queries without any bound
- reject unsupported or contradictory clause combinations when the MVP does not implement them

## Required Integration Scenarios

1. `match` simple success
2. `match_phrase` with `slop`
3. `multi_match` basic success
4. `term` exact success
5. `terms` list success
6. `range` numeric bounds success
7. `exists` success
8. `bool` with `must`, `filter`, `should`, `must_not`
9. `prefix` success
10. `wildcard` success with bounded pattern
11. `regexp` success or explicit bounded deferral
12. `fuzzy` success with edit distance up to `2`
13. invalid `range` returns validation error
14. invalid top-level types return validation error
15. malformed JSON returns parse error

## Anti-Abuse Notes

- wildcard and regexp support must be bounded
- deep bool nesting must be bounded
- pagination must be bounded
- `track_total_hits` must not create accidental unbounded work in MVP
