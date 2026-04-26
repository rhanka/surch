# SPEC - Surch Governance And Orchestration

## Status

- Date: 2026-04-25
- Author: Conductor
- Scope: governance docs, branch model, subagent contract, rules, environment policy
- Decision state: updated after review, ready for operational doc writing once approved

## Objective

Turn Surch into an execution-ready conductor repo for a 4-day MVP effort.

The governance layer must make it possible to:

- build from official OpenSearch specs instead of intuition
- decompose the MVP into explicit branches with stable scope and ownership
- run up to 4 subagents in parallel without cross-branch chaos
- keep development, tests, and end-to-end verification isolated
- enforce merge, testing, and security gates consistently
- preserve project-specific rules even when Superpowers skills are active

## Product Context

Surch is a 100% Rust search engine intended to reproduce the indexing and search surface of OpenSearch and Elasticsearch for the MVP scope, without analytics, while preserving the Lucene signature around fuzzy behavior and edit distance.

The MVP scope is:

- index management
- document CRUD and bulk indexing
- Query DSL core
- search endpoint compatibility
- fuzzy behavior up to edit distance 2
- storage and tests strong enough to support the next phases

## Requested Corrections From Review

This updated spec incorporates the following required corrections:

- specs belong in `spec/`, not in `docs/superpowers/specs/`
- `PLAN.md` must be built around the branch files themselves, in the Entropic style
- branch files must live in `plan/` and follow a numbered pattern such as `plan/01_BRANCH_...md`
- Superpowers must be explicitly framed as a complement to project rules, never as something that can overrule repo governance
- "drumbeat" must mean continuous forward motion and conductor continuity, not hourly scheduling

## Source References Used

The governance model is adapted from `entropic`, especially:

- `PLAN.md`
- `plan/BRANCH_TEMPLATE.md`
- `plan/SUBAGENT_PROMPT_TEMPLATE.md`
- `.cursor/rules/workflow.md`
- `.cursor/rules/testing.md`
- `.cursor/rules/subagents.md`
- `.cursor/rules/security.md`

The adaptation must stay Rust-first and Surch-specific.

## Core Design Decision

Adopt an Entropic-like orchestration model, but adapted for a Rust engine repo.

Keep:

- branch-centered execution
- worktree isolation for parallel streams
- explicit scope boundaries
- subagent launch packets
- feedback loops
- mandatory verification gates

Adapt:

- replace UI-heavy defaults with engine-oriented lots and checks
- use `cargo` and `make` as the primary command contract
- keep Docker optional for daily local dev, but available for isolated validation

Reject:

- hourly process theater
- rules that are heavier than the repo needs
- letting generic skill workflows break project-specific branch discipline

## Target Documentation Layout

The target operational documentation set is:

- `PLAN.md`
- `plan/BRANCH_TEMPLATE.md`
- `plan/SUBAGENT_PROMPT.md`
- `rules/workflow.md`
- `rules/testing.md`
- `rules/security.md`
- `rules/subagents.md`
- `rules/dev-env.md`
- `rules/superpowers.md`

## `PLAN.md` Role

`PLAN.md` must become the conductor index, not the place where all branch execution detail lives.

It must contain:

- vision and MVP contract
- spec harvesting policy
- compatibility surface summary
- branch catalog with one entry per numbered branch file
- dependency graph and wave ordering
- current status summary
- conductor execution policy
- release and rollback gates

It must reference the branch files rather than duplicating their full execution checklists.

## Branch File Model

Each concrete branch must have its own file in `plan/`.

Naming convention:

- `plan/01_BRANCH_spec-harvest-index-and-document-apis.md`
- `plan/02_BRANCH_spec-harvest-search-and-query-dsl.md`
- `plan/03_BRANCH_storage-wal-segments-and-docstore.md`
- `plan/04_BRANCH_indexer-mappings-analyzers-and-bulk-contract.md`
- `plan/05_BRANCH_search-query-execution-and-fuzzy.md`

Rules:

- one file per branch
- one branch ID per file
- one owner per branch
- branch file is the execution source of truth for that branch
- `PLAN.md` references branch files and their status

This mirrors the useful Entropic pattern while staying simpler.

## `plan/BRANCH_TEMPLATE.md` Role

This file must define the standard contract for any branch execution file.

Mandatory sections:

- Branch ID / Branch Name / Owner
- Objective
- Scope / Guardrails
- Spec Sources
- Allowed Paths
- Forbidden Paths
- Dependency Gates
- Environment Mapping
- Plan / Lots / Todo
- Feedback Loop
- Tests Required
- Security Checks
- Merge Checklist

The template must be leaner than Entropic and fit Rust domains such as storage, indexer, search, and API.

## `plan/SUBAGENT_PROMPT.md` Role

This file must define the reusable subagent launch prompt.

It must include:

- launch packet fields
- mandatory read order
- project rule priority
- allowed and forbidden scope handling
- environment and worktree rules
- validation rules
- reporting format
- escalation and stop conditions

It should be a real reusable launch contract, not a placeholder.

## Ruleset Model

### `rules/workflow.md`

Must define:

- branch naming
- worktree policy
- commit policy
- merge and PR policy
- orchestration mode
- when subagents may be launched
- how `PLAN.md` and `plan/NN_BRANCH_...md` interact

### `rules/testing.md`

Must define:

- Rust-adapted test pyramid
- standard commands
- `dev`, `test`, `e2e` isolation
- lot gates and final gates
- compatibility verification requirements

### `rules/security.md`

Must define:

- input validation expectations
- abuse controls for wildcard, regex, and expensive queries
- dependency and supply-chain checks
- security release gates

### `rules/subagents.md`

Must define:

- launch packet contract
- ownership boundaries
- reporting contract
- blocker handling
- maximum parallel subagents = 4

### `rules/dev-env.md`

Must define:

- local non-Docker dev mode
- Docker-backed isolated validation mode
- ports, data dirs, and environment naming
- `make` versus raw `cargo` guidance

### `rules/superpowers.md`

This file must explicitly prevent skill-driven disorder.

It must state:

- Superpowers skills are helpers, not project governance owners
- project files and repo rules have priority for Surch execution
- no skill may bypass `PLAN.md`, branch files, allowed-path boundaries, or validation gates
- no brainstorming or planning skill may force a documentation structure that conflicts with Surch repo rules
- if a skill suggests a conflicting process, Surch rules win

This rule is important because the repo must remain conductor-driven even when generic skills are loaded.

## Branching Model

The target branching model is:

- `main`: release only
- `develop`: integration branch
- `feature/BR-XX-<slug>`
- `bugfix/BR-XX-<slug>`
- `release/v0.1.0-mvp`

Each branch must have:

- one stable ID
- one owner
- one explicit scope
- declared dependencies
- path boundaries
- verification requirements
- one corresponding `plan/NN_BRANCH_...md` file

## Commit Policy

Commit convention remains explicit and strict.

Primary forms:

- `feat(storage): ...`
- `fix(search): ...`
- `test(api): ...`
- `docs(plan): ...`
- `security(core): ...`

Rules:

- atomic commits only
- selective staging only
- no amend unless explicitly requested
- no mixed mega-commits across branch scopes
- major user planning inputs are committed as governance milestones

## Worktree Policy

Worktrees are optional for conductor-only root documentation work.

They become mandatory when:

- more than one feature branch is active
- a subagent is launched
- port or data isolation is needed

Naming convention:

- `tmp/br-01-spec-harvest`
- `tmp/br-02-storage-wal`
- `tmp/br-03-indexer-pipeline`
- `tmp/br-04-search-dsl`

## Environment Model

Three environments are required:

- `dev`
- `test`
- `e2e`

Rules:

- `dev` may use raw local Rust tooling
- `test` must be isolated from `dev`
- `e2e` must be reproducible and containerized

Docker policy:

- optional for fast local coding
- recommended for branch-isolated validation
- mandatory for end-to-end reproducibility

## Command Contract

The repo should expose a stable `make` façade while keeping Rust-native commands explicit.

Target commands to standardize:

- `make dev`
- `make fmt`
- `make clippy`
- `make test`
- `make test-unit`
- `make test-integration`
- `make test-e2e`
- `make audit`
- `make ci`

Underlying commands to document:

- `cargo fmt`
- `cargo clippy --workspace --all-targets --all-features`
- `cargo test --workspace`
- `cargo audit`

## Testing Policy

The adapted test pyramid is:

- 70% unit
- 20% integration
- 10% end-to-end and compatibility verification

Priority surfaces:

- storage durability
- indexing and mapping behavior
- Query DSL parsing and execution
- fuzzy behavior and edit distance
- HTTP compatibility by endpoint
- invalid input and abuse-resistance checks

Pure engine branches must not be forced into irrelevant UI process.

## Security Policy

Minimum security requirements for the governance docs are:

- strict API input validation
- bounded payload sizes
- bounded wildcard and regex behavior
- dependency scanning
- explicit release security gate

## Subagent Policy

Subagents are launched only when branch decomposition is ready.

That means:

- branch files exist or are ready to be written
- dependencies are clear enough to avoid churn
- allowed and forbidden paths are known
- validation commands are known

The conductor remains responsible for integration and arbitration.

Maximum concurrent subagents: 4.

## Drumbeat Meaning

For Surch, "drumbeat" means:

- do not stall in abstract analysis
- keep conductor continuity across lots
- iterate until the execution frame is usable
- raise blockers immediately
- keep forward motion until the current governance slice is complete

It does not mean hourly checkpoint theater.

## Mandatory Versus Recommended

### Mandatory

- specs in `spec/`
- `PLAN.md` as conductor index
- numbered branch files in `plan/`
- branch template and subagent prompt
- rules files for workflow, testing, security, subagents, dev env, and superpowers framing
- spec-first execution
- one branch, one owner, one scope
- max 4 parallel subagents
- worktrees for parallel execution
- allowed and forbidden paths per branch
- atomic commit policy
- isolated `dev`, `test`, `e2e`
- verification before merge
- feedback loops for blockers, spec mismatches, security alerts, and scope exceptions

### Recommended

- `make` as the standard façade
- worktrees for medium solo branches too
- regular `cargo audit` and `cargo deny`
- compatibility matrices updated incrementally

### Not Mandatory

- hourly check-ins
- Docker for every local command
- UI-heavy UAT for pure engine work
- launching 4 subagents just for optics

## Expected Outcome Of Next Step

Once this updated spec is approved, the next writing step will:

- move governance into `PLAN.md` + `plan/NN_BRANCH_...md` structure
- create the branch template and subagent prompt files
- add the ruleset, including the Superpowers framing rule
- prepare the repo for controlled branch-by-branch MVP execution
