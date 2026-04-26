# SPEC - Security And Testing Baseline

## Purpose

Provide a practical baseline for MVP security controls and the test strategy that must support them.

## Security Control Matrix

| Threat | Control | Area | Priority |
|---|---|---|---|
| oversized request body | request size cap | API | P0 |
| malformed JSON | strict parsing and validation | API | P0 |
| deep nested query abuse | query depth cap | API, search | P0 |
| wildcard abuse | bounded patterns and expansion controls | API, search | P0 |
| regex abuse | bounded regex support | API, search | P0 |
| pagination abuse | cap `from` and `size` | API, search | P1 |
| brute-force request rate | rate limiting foundation | API | P1 |
| unsafe logs | redact secrets and avoid verbose payload leaks | API | P0 |
| storage corruption | replay and corruption tests | storage | P1 |
| vulnerable dependencies | dependency review | whole repo | P0 |

## Test Pyramid

- 70% unit
- 20% integration
- 10% end-to-end or compatibility

## Minimum Test Families

### Unit
- storage replay and metadata
- analyzer behavior
- Query DSL clause semantics
- fuzzy edit-distance behavior
- request parsing helpers

### Integration
- index/document API compatibility
- search API compatibility
- cross-module indexing then search flow
- invalid input and error-shape tests

### Release Smoke
- create index
- index document
- get document
- search simple term or match
- search fuzzy distance <= 2

## Recommended Validation Commands

- `cargo fmt --all`
- `cargo clippy --workspace --all-targets --all-features`
- `cargo test --workspace`
- `cargo audit` when available

## Release Security Checklist

- input validation reviewed
- expensive query paths reviewed
- no obvious secret leakage in logs
- dependency review run
- compatibility tests pass on core endpoints
