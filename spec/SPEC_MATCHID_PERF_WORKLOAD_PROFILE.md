# SPEC - MatchID Performance Workload Profile

## Purpose

Freeze the first performance workload profile derived from MatchID's existing Artillery scenario.

## Source

- `matchID/matchID/packages/deces-backend/tests/performance/scenarios/test-backend-v1.yml`

## Extracted Baseline Characteristics

### Request Shape
- GET `/deces/api/v1/search?firstName=...&lastName=...&birthDate=...&fuzzy=false`
- POST `/deces/api/v1/search` with `firstName`, `lastName`, and `birthDate`

### Payload Source
- `clients_test.csv`
- semicolon-delimited
- random order

### Traffic Phases
- 30s at 2 arrivals/sec
- 30s at 2 arrivals/sec
- 30s at 5 arrivals/sec
- 30s at 10 arrivals/sec
- 30s at 20 arrivals/sec
- 60s at 50 arrivals/sec

### Existing Thresholds
- `maxErrorRate <= 1%`
- `max <= 500 ms`
- `p95 <= 200 ms`

## Use In Surch

This profile becomes the first reproducible external baseline to mirror in BR-10.

It is not sufficient for final sign-off by itself, but it is the minimal inherited workload that must be preserved in parity measurements.
