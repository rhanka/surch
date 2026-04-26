# Surch Governance Documentation Design

## Status

- Date: 2026-04-25
- Author: Conductor
- Scope: governance, branch orchestration, subagent contract, dev/test environment rules
- Decision state: approved for writing into repo operational docs

## Objective

Turn the current Surch repo from a rough bootstrap into an execution-ready conductor repo for a 4-day MVP effort.

The documentation set must make it possible to:

- drive work from official OpenSearch specs instead of intuition
- split the MVP into explicit branches with stable ownership and dependencies
- launch up to 4 subagents in parallel without scope drift
- keep development, testing, and end-to-end runs isolated
- enforce repeatable merge, testing, and security gates
- commit major user inputs and planning milestones as traceable project decisions

## Product Context

Surch is a 100% Rust search engine intended to reproduce OpenSearch and Elasticsearch indexing and search syntax for the MVP scope, while carrying the Lucene signature behavior around fuzzy search and edit distance.

The MVP target is not analytics completeness. The MVP target is:

- index management
- document CRUD and bulk indexing
- search API compatibility for core Query DSL
- fuzzy behavior up to edit distance 2
- durable enough storage and test coverage to serve as a foundation for later expansion

## Problems In Current State

The current repo has an initial `PLAN.md` and `AGENTS.md`, but the governance layer is incomplete.

Main gaps:

- `PLAN.md` is roadmap-heavy but not orchestration-ready
- no explicit branch contract template exists for each feature stream
- no standard subagent launch prompt exists
- no adapted workflow rules exist for Rust branch execution, commits, testing, or environment isolation
- no spec harvesting policy is written even though syntax compatibility is a primary goal
- no clear distinction exists between optional process and mandatory gates

## Design Goals

The new doc set must be:

- strict enough to support parallel execution
- light enough for a Rust engine repo, not a SaaS product monorepo
- spec-first
- branch-first
- verification-first
- explicit about scope boundaries
- explicit about what is mandatory versus recommended

## Non-Goals

This design does not try to:

- recreate the full Entropic process stack line-by-line
- impose hourly scheduling as the primary orchestration mechanism
- force Docker for every local coding action
- require UI/UAT-heavy workflows for pure storage or search branches
- define final implementation details for every MVP branch

## Source References Used

The governance shape is adapted from the following reference assets in `entropic`:

- `PLAN.md`
- `plan/BRANCH_TEMPLATE.md`
- `plan/SUBAGENT_PROMPT_TEMPLATE.md`
- `.cursor/rules/workflow.md`
- `.cursor/rules/testing.md`
- `.cursor/rules/subagents.md`
- `.cursor/rules/security.md`

The result must be adapted to a Rust-first workspace rather than copied verbatim.

## Core Design Decision

Adopt an "Adapted Rust Lean" governance model.

That means:

- preserve the good parts of Entropic: branch contracts, worktree isolation, scope boundaries, subagent launch packets, feedback loops, and mandatory gates
- translate those rules into Rust-native execution with `cargo` and `make`
- avoid overfitting the repo to UI-heavy, multi-surface workflows that do not match Surch's current structure

## Documentation Set To Produce

### 1. `PLAN.md`

This becomes the conductor source of truth.

It must contain:

- vision and MVP contract
- spec harvesting policy
- compatibility surface matrix
- branch catalog with IDs, owners, dependencies, deliverables, and status
- wave plan with maximum parallelism of 4
- continuous conductor cadence and lot gates
- release, hardening, and rollback gates

It must stop being only a roadmap and become an execution board.

### 2. `plan/BRANCH_TEMPLATE.md`

This becomes the operational contract for any real branch.

It must contain:

- branch ID, name, owner
- objective
- spec sources to read first
- allowed paths
- forbidden paths
- dependency list
- environment mapping
- lots and checklists
- feedback loop area
- testing requirements
- security checks
- merge checklist

It must be lighter than the Entropic template and adapted to Rust modules such as storage, indexer, search, and API.

### 3. `plan/SUBAGENT_PROMPT.md`

This becomes the standard launch prompt for subagents.

It must include:

- required launch packet
- mandatory read order
- execution rules
- scope boundaries
- environment rules
- test and verification expectations
- reporting format
- stop conditions and escalation rules

This file is intended for actual repeated use, not as a vague placeholder.

### 4. `rules/workflow.md`

This defines:

- branch naming
- when worktrees are mandatory
- commit policy
- PR and merge policy
- mono-branch versus multi-branch orchestration
- when subagents may be launched

### 5. `rules/testing.md`

This defines:

- Rust-adapted test pyramid
- standard `cargo` and `make` commands
- `dev`, `test`, and `e2e` isolation
- required gates before merge and before release
- compatibility testing expectations against OpenSearch syntax

### 6. `rules/security.md`

This defines:

- input validation rules
- anti-abuse limits for payloads, regex, wildcard, and expensive queries
- dependency and supply-chain checks
- release security gate requirements

### 7. `rules/subagents.md`

This defines:

- subagent contract
- launch packet fields
- ownership boundaries
- reporting expectations
- escalation rules
- maximum parallel agent count

### 8. `rules/dev-env.md`

This defines:

- Docker and non-Docker development modes
- port allocation
- data directory isolation
- environment naming
- how `make` wraps `cargo` and containers

## Branching Model

The target branching model is:

- `main`: release-only
- `develop`: integration branch
- `feature/BR-XX-<slug>`: normal feature streams
- `bugfix/BR-XX-<slug>`: targeted fixes
- `release/v0.1.0-mvp`: stabilization branch before MVP release

Each branch must have:

- one stable branch ID
- one owner
- one explicit scope
- declared dependencies
- path boundaries
- branch-level validation requirements

## Commit Policy

Commits must follow a conventional format adapted to Surch scopes.

Primary pattern:

- `feat(storage): ...`
- `fix(search): ...`
- `test(api): ...`
- `docs(plan): ...`
- `security(core): ...`

Required behavior:

- atomic commits only
- selective staging only
- no commit amend unless explicitly requested
- no giant mixed commits when branch work is parallelized
- commit major user planning inputs as traceable governance milestones

## Worktree Policy

Worktrees are not mandatory for the initial conductor-only documentation pass.

They become mandatory when:

- more than one feature branch is active
- a subagent is launched
- isolated environment or port ownership is required

Naming convention:

- `tmp/br-01-storage-wal`
- `tmp/br-02-indexer-pipeline`
- `tmp/br-03-search-dsl`
- `tmp/br-04-api-compat`

## Environment Model

Three environments are required:

- `dev`: interactive development
- `test`: unit and integration validation
- `e2e`: full-stack reproducible validation

Rules:

- `dev` may run without Docker for fast local Rust work
- `test` should be isolated from `dev` data and ports
- `e2e` must be reproducible and containerized

The governance docs must state clearly that Docker is:

- optional for day-to-day local coding
- recommended for isolated branch validation
- mandatory for end-to-end and reproducible integration stacks

## Command Contract

The repo should expose a stable command façade through `make`, with `cargo` underneath.

Target commands to standardize in docs:

- `make dev`
- `make fmt`
- `make clippy`
- `make test`
- `make test-unit`
- `make test-integration`
- `make test-e2e`
- `make audit`
- `make ci`

The rules must also document the underlying Rust-native commands:

- `cargo fmt`
- `cargo clippy --workspace --all-targets --all-features`
- `cargo test --workspace`
- `cargo audit`

## Testing Policy

The adapted pyramid is:

- 70% unit
- 20% integration
- 10% end-to-end and compatibility validation

Priority test surfaces for MVP:

- storage durability behavior
- indexing and mapping behavior
- Query DSL parsing and execution
- fuzzy matching and edit-distance behavior
- HTTP endpoint compatibility
- invalid input and abuse-resistance checks

Pure engine branches must not be forced through UI-heavy validation patterns that do not apply.

## Security Policy

The governance docs must enforce minimal MVP security requirements:

- strict input validation at API boundaries
- bounded payload sizes
- bounded wildcard and regex behavior
- dependency vulnerability scanning
- documented security gate before MVP release

The release gate must reject avoidable high-risk issues in the MVP path.

## Subagent Policy

Subagents are not launched for optics.

They may be launched only when:

- branch boundaries are defined
- dependencies are clear enough to avoid rework churn
- launch packets are complete

The conductor remains responsible for:

- wave planning
- branch decomposition
- integration
- arbitration on blockers and scope conflicts

Maximum concurrent subagents: 4.

## Continuous Conductor Cadence

The design rejects hourly scheduling as the main concept.

Instead, the docs must define continuous conductor cadence:

- keep advancing without stalling in analysis
- operate lot by lot
- trigger feedback loops as soon as blockers or spec mismatches appear
- summarize state after meaningful progress or integration changes

This preserves "drumbeat" as forward motion and orchestration continuity, not as a time grid.

## Mandatory Versus Recommended Rules

### Mandatory

- `PLAN.md` as conductor source of truth
- branch template and subagent prompt files
- spec-first workflow
- one branch, one owner, one scope
- max 4 parallel subagents
- worktrees for parallel execution
- allowed and forbidden paths per branch
- atomic commits and explicit commit convention
- isolated `dev`, `test`, `e2e` environments
- verification before merge
- feedback loop for blockers, spec mismatches, security alerts, and scope exceptions

### Recommended

- `make` as the standard façade
- worktree use even for medium solo branches
- regular `cargo audit` and `cargo deny`
- compatibility matrices updated incrementally
- Docker for reproducible branch validation beyond local coding

### Explicitly Not Mandatory

- hourly check-in schedules
- Docker for every local command
- UI/UAT-heavy process for branches that are purely engine internal
- launching four subagents when the work is not yet decomposed enough

## Expected Outcome Of Next Step

After this design is approved and written:

- `PLAN.md` will be rewritten into conductor-grade form
- `plan/BRANCH_TEMPLATE.md` will be created
- `plan/SUBAGENT_PROMPT.md` will be created
- `rules/workflow.md`, `rules/testing.md`, `rules/security.md`, `rules/subagents.md`, and `rules/dev-env.md` will be added

These documents together will turn the repo into an execution-ready conductor workspace for the 4-day MVP push.

## Self-Review Notes

- No hourly orchestration requirement retained
- No placeholder sections left intentionally vague
- Entropic references used as pattern source, not copied blindly
- Design remains focused on repo governance, not implementation details of engine code
