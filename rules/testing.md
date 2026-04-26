# Testing Rules

## Goal

Define a Rust-first test strategy for Surch.

## Test Pyramid

- 70% unit tests
- 20% integration tests
- 10% end-to-end or compatibility tests

## Priority Test Surfaces

- storage durability and replay
- mapping and analyzer behavior
- Query DSL parsing and execution
- fuzzy matching and edit distance behavior
- HTTP endpoint compatibility
- invalid input and abuse-resistance behavior

## Canonical Commands

Rust-native commands are canonical:

- `cargo fmt --all`
- `cargo clippy --workspace --all-targets --all-features`
- `cargo test --workspace`

Optional façade when targets exist:

- `make fmt`
- `make clippy`
- `make test`
- `make test-unit`
- `make test-integration`
- `make test-e2e`
- `make ci`

## Test Location Contract

- unit tests: inline `#[cfg(test)]` or module-local tests under crate source
- integration tests: `tests/integration/`
- compatibility tests: `tests/integration/api_compat/`
- security tests: `tests/integration/security/`

## Branch Gates

### Storage / Indexer / Search Branches
- run crate-focused unit tests first
- run affected integration tests second
- do not wait until release branch to discover broken module contracts

### API Branches
- run handler and serialization tests
- run compatibility tests against harvested request and response shapes

## Release Gates

- full workspace tests must pass
- compatibility smoke tests must pass on documented MVP endpoints
- fuzzy tests must cover edit distances 0, 1, 2 and rejection beyond limit

## Coverage Expectations

- core modules should target 80%+ logical coverage
- no branch may remove critical tests without explicit conductor approval

## Failure Handling

- do not hide flaky behavior with timeout inflation
- if a failure looks nondeterministic, document evidence in the active branch file
- if a failure changes compatibility semantics, raise `spec-mismatch`
