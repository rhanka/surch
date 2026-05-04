# Surch Portage Plan

Date: 2026-05-04

## Objective

Rebuild Surch from a clean planning baseline as a Rust port of OpenSearch plus Lucene, with function-level traceability to upstream source, compatibility tests, and autonomous subagent execution.

The current Rust prototype remains only as migration context. It is not the target architecture.

## Upstream References

- OpenSearch clone: `/tmp/surch-portage-references/opensearch`
- OpenSearch commit: `fead3a928236b61f9c759c61e738b541a148ab9f`
- Lucene clone: `/tmp/surch-portage-references/lucene`
- Lucene commit: `7691b7ef9cfe3b87178646f4f32b3854afa0a567`
- Graphify reports copied under `docs/portage/graphify/`
- Full graph outputs remain in `/tmp/surch-portage-graph-corpora/*/.graphify/`

## Target Architecture

The target workspace is split by porting boundary, not by prototype convenience:

```text
crates/
  surch-types/        # OpenSearch JSON/API types, documents, errors
  surch-analysis/     # Lucene analyzer/token stream model
  surch-codec/        # Lucene codec utilities, binary formats, checksums
  surch-store/        # Directory, translog, manifests, segments, docstore
  surch-index/        # mappings, indexing chain, postings, term dictionary
  surch-search/       # Lucene query model, scorers, collectors, fuzzy automata
  surch-api/          # Axum REST API compatible with OpenSearch
```

Existing `surch-core` and `surch-api` prototype crates must be archived out of the active workspace before new implementation work starts. New feature work lands in `crates/*`.

## Execution Rule

Every feature ticket must be traceable to upstream:

```text
Epic -> Capability -> Feature -> Function/API -> Golden scenario
```

No ticket is ready unless it includes:

- upstream repository, commit, file, class, and method or REST spec
- owner subagent
- dependencies
- allowed paths and forbidden paths
- failing golden test against Surch and passing oracle against upstream
- unit, integration, security, and parity gates

## Phase Order

0. Clean restart housekeeping: archive current prototype state, remove obsolete branch plans, remove stale worktrees, clear runtime/build artifacts, and leave only governance plus upstream references.
1. Reference harness and blank workspace reset.
2. Lucene store, codec utilities, segment metadata.
3. Lucene postings, term dictionary, writer, reader, stored fields, doc values.
4. Lucene analysis, search model, BM25, collectors, automata, fuzzy query.
5. OpenSearch REST contracts, response rendering, errors, mappings/settings.
6. OpenSearch document, update, delete, get, bulk, refresh, translog semantics.
7. OpenSearch Query DSL, search responses, sorting, pagination, fuzzy signature.
8. P1/P2 compatibility, admin endpoints, security, performance, release parity.

## Clean Restart Housekeeping

Before implementation starts, the conductor must make the repository intentionally blank for the new port while preserving recoverability:

- record `git status --short`, `git diff --stat`, and `git worktree list` in `docs/portage/reset/RESET_INVENTORY.md`
- create an archive branch named `archive/prototype-before-portage-2026-05-04` from the current `develop` tip
- save a patch of uncommitted user/prototype changes to `docs/portage/reset/prototype-dirty-worktree.patch`
- remove obsolete `plan/01_BRANCH_*.md`, old `spec/SPEC_*.md`, and old branch prompt/template files
- remove stale `.worktrees/br-*` only after confirming they have no unique commits not reachable from named branches
- move or delete prototype implementation crates from the active workspace before creating `crates/*`
- remove runtime/build artifacts from version consideration: `target/`, `data/`, `.worktrees/runtime-surch/`, generated captures, and local caches
- rewrite README claims so the repo states "portage in progress" until parity gates prove compatibility
- update `rules/MASTER.md` and related governance docs so they point at the new `PLAN.md`, `plan/00_AUTONOMOUS_PORTAGE_EXECUTION.md`, and `spec/SPEC_INTEGRAL_OPENSEARCH_LUCENE_PORTAGE.md`
- require a clean `git status --short` except for the intentionally staged reset commit before spawning feature subagents

## Subagent Allocation

- `#1 StorageEngine`: `surch-codec`, `surch-store`, Lucene segment/commit/translog foundations.
- `#2 Indexer`: `surch-analysis`, `surch-index`, mappings, field types, indexing chain.
- `#3 SearchEngine`: `surch-search`, Query DSL execution, BM25, fuzzy automata, collectors.
- `#4 APIServer`: `surch-types`, `surch-api`, REST compatibility, request/response rendering.

Maximum active work in parallel: four subagents, one feature per subagent.

## Gates

Each branch must pass:

- `cargo fmt --all`
- `cargo clippy --workspace --all-targets --all-features`
- targeted unit tests
- targeted integration tests
- targeted golden parity tests
- unsafe scan with documented exception for any `unsafe`
- dependency/security scan once `cargo-audit` or `cargo-deny` is configured

PRs target `develop`. `main` is release-only.

## Source Documents

- Portage spec: `spec/SPEC_INTEGRAL_OPENSEARCH_LUCENE_PORTAGE.md`
- Autonomous execution plan: `plan/00_AUTONOMOUS_PORTAGE_EXECUTION.md`
- Reference inventory: `docs/portage/REFERENCES.md`
- Superpowers design: `docs/superpowers/specs/2026-05-04-opensearch-lucene-rust-port-design.md`
- Superpowers plan: `docs/superpowers/plans/2026-05-04-opensearch-lucene-rust-portage.md`
