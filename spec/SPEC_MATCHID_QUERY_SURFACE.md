# SPEC - MatchID Query Surface

## Purpose

Document the MatchID-side request surface that matters for Elasticsearch parity work in Surch.

## Primary Source Files

- `matchID/matchID/packages/deces-backend/src/models/requestInput.ts`
- `matchID/matchID/packages/deces-backend/src/fieldsWithQueries.ts`
- `matchID/matchID/packages/deces-backend/tests/performance/scenarios/test-backend-v1.yml`
- `matchID/matchID/packages/deces-backend/tests/clients_test.csv`

## High-Priority Query Families

### Family A - Performance Scenario Seed

Seen in `test-backend-v1.yml`:
- `firstName`
- `lastName`
- `birthDate`
- `fuzzy=false`

This is the first workload that should be frozen for replay because it already exists in MatchID perf tooling.

### Family B - Core Identity Lookup

Seen in `RequestInput` and `fieldsWithQueries`:
- `firstName`
- `lastName`
- `legalName`
- `birthDate`
- `birthCity`
- `birthCountry`
- `birthLocationCode`
- `source`

These define the main relevance-critical search surface.

### Family C - Location And Death Filters

- `deathDate`
- `deathCity`
- `deathCountry`
- `deathLocationCode`
- `deathDepartment`
- `deathAge`

### Family D - Navigation And Result Shape

- `size`
- `page`
- `sort`
- `scroll`
- `scrollId`
- `aggs`
- `aggsSize`

These matter for parity only if the corresponding MatchID flows are in the acceptance scope.

## Priority Recommendation For BR-09

Freeze corpus in this order:
1. `firstName + lastName + birthDate + fuzzy=false`
2. `firstName + lastName + birthDate + fuzzy=auto`
3. location variations
4. source filters
5. paging and sorting

## Corpus Source Recommendation

Immediate seed source:
- `matchID/matchID/packages/deces-backend/tests/clients_test.csv`

Immediate perf reference source:
- `matchID/matchID/packages/deces-backend/tests/performance/scenarios/test-backend-v1.yml`

These are not enough for final sign-off, but they are enough to bootstrap a frozen representative seed corpus.
