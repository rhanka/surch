# Workflow Rules

## Goal

Provide a lean but strict execution model for Surch branch work.

## Branch Model

- `main`: release-only
- `develop`: integration branch
- `feature/<ticket-id>-<slug>`: one scoped parity feature stream
- `bugfix/<ticket-id>-<slug>`: one scoped fix stream
- `release/vX.Y.Z-*`: hardening and release validation

## Ticket Contract

- Every implementation branch must have a parity ticket.
- The ticket is the execution source of truth for that branch.
- `PLAN.md` tracks phase order and source documents.
- `plan/00_AUTONOMOUS_PORTAGE_EXECUTION.md` defines the required ticket schema.
- Do not start work without upstream references, allowed paths, forbidden paths, golden tests, and gates.

## Worktree Policy

- Worktree is optional for root-level conductor-only documentation work.
- Worktree is mandatory when:
  - more than one feature branch is active
  - a subagent is launched
  - isolated ports or data dirs are needed

Naming convention:
- `tmp/lucene-store-datainput-001`
- `tmp/os-api-bulk-001`
- `tmp/lucene-search-fuzzyquery-001`

## Commit Policy

Allowed commit forms:
- `feat(storage): ...`
- `feat(indexer): ...`
- `feat(search): ...`
- `api(search): ...`
- `fix(storage): ...`
- `test(api): ...`
- `docs(plan): ...`
- `security(core): ...`

Rules:
- one logical change per commit
- selective staging only
- never `git add .` for subagent branch work
- do not amend unless explicitly requested
- major user planning inputs should be committed as governance milestones

## Scope Control

- Allowed and forbidden paths must be declared in every parity ticket.
- Conditional scope exceptions must be recorded in the ticket before touching sensitive paths.
- If the required change crosses branch boundaries, stop and escalate through the feedback loop.

## Verification Before Merge

Minimum merge gate for implementation branches:
- `cargo fmt --all`
- `cargo clippy --workspace --all-targets --all-features`
- scoped unit tests
- scoped integration tests
- golden parity tests for the ticket
- ticket checklist complete
- no unresolved blocker items

## Orchestration Model

- conductor is the only integrator
- subagents work on orthogonal branches only
- maximum 4 active subagents
- one subagent owns one branch at a time

## Drumbeat

Drumbeat means continued execution until the current slice is usable.

It does not imply time-based ceremony.

Expected behavior:
- move lot by lot
- keep feedback loop current
- do not stop after partial analysis if the next concrete action is known

## Pull Request Policy

- one capability or one branch objective per PR
- PR title and body must align with the parity ticket
- do not merge without ticket checklist completion and verification evidence

## Release Policy

- only release branches may merge to `main`
- hardening fixes land on release branch, then back to `develop`
- never bypass release checklist
