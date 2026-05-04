# OpenSearch + Lucene Rust Port Design

Date: 2026-05-04

## Design Decision

Surch will be rebuilt around upstream-traceable crates instead of extending the current scan-based prototype. The prototype can remain as a migration facade, but it is not the architecture of record.

## Architecture

The workspace splits into `surch-types`, `surch-analysis`, `surch-codec`, `surch-store`, `surch-index`, `surch-search`, and `surch-api`. The Lucene-like crates form the engine substrate; OpenSearch compatibility sits at the API and request/response layers.

Before those crates are created, the current prototype must be archived out of the active workspace. The cleanup step records the dirty worktree, creates an archive branch, removes stale old-plan worktrees after conductor confirmation, archives prototype crates and MatchID-specific harnesses, and rewrites governance references.

## Data Flow

OpenSearch REST requests are parsed into typed request models, validated, and dispatched locally. Indexing flows through mappings and analyzers into Lucene-style segments. Search requests parse Query DSL into a Lucene-style query tree, rewrite through term dictionaries and automata, execute via scorers and collectors, and render OpenSearch-compatible JSON.

## Testing

Every feature starts with a golden oracle:

- Lucene golden fixtures for binary formats, analyzers, query behavior, scoring, and fuzzy automata.
- OpenSearch REST replay fixtures for API status, JSON shape, errors, totals, ordering, and score tolerance.

## Error Handling

All public API errors render OpenSearch-compatible envelopes. Unsupported features must fail explicitly with compatible status and reason rather than silently degrade.

## Security

Bounded input parsing is part of the contract. JSON/NDJSON body size, bool nesting, wildcard length, regexp determinization, fuzzy expansion, pagination, and sorting all require explicit limits and negative tests.

## Scope Control

The first release is a single-node compatible engine. Distributed cluster behavior, plugins, snapshots, remote store, vector search, and advanced ingest are deferred to explicit P2+ tickets.
