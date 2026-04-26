# Workflow Rules

## Goal

Provide a lean but strict execution model for Surch branch work.

## Branch Model

- `main`: release-only
- `develop`: integration branch
- `feature/BR-XX-<slug>`: one scoped feature stream
- `bugfix/BR-XX-<slug>`: one scoped fix stream
- `release/v0.1.0-mvp`: hardening and release validation

## Branch File Contract

- Every real branch must have a corresponding `plan/NN_BRANCH_*.md` file.
- The branch file is the execution source of truth for that branch.
- `PLAN.md` references branch files and tracks status and dependencies.
- Do not duplicate full lot checklists in `PLAN.md`.

## Worktree Policy

- Worktree is optional for root-level conductor-only documentation work.
- Worktree is mandatory when:
  - more than one feature branch is active
  - a subagent is launched
  - isolated ports or data dirs are needed

Naming convention:
- `tmp/br-01-spec-harvest`
- `tmp/br-03-storage-wal`
- `tmp/br-05-search-fuzzy`

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

- Allowed and forbidden paths must be declared in every branch file.
- Conditional scope exceptions must be recorded in the branch file before touching sensitive paths.
- If the required change crosses branch boundaries, stop and escalate through the feedback loop.

## Verification Before Merge

Minimum merge gate for implementation branches:
- `cargo fmt --all`
- `cargo clippy --workspace --all-targets --all-features`
- scoped unit tests
- scoped integration tests
- branch checklist complete
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
- PR title and body must align with the branch file
- do not merge without branch checklist completion and verification evidence

## Release Policy

- only `release/v0.1.0-mvp` may merge to `main` for MVP release
- hardening fixes land on release branch, then back to `develop`
- never bypass release checklist
