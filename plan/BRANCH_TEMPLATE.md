# Branch: BR-XX - <Title>

## Objective
- <One or two sentences describing the branch goal>

## Scope / Guardrails
- Scope limited to <areas>
- Follow `rules/MASTER.md`, `rules/workflow.md`, and the relevant helper rules
- Keep changes minimal and within the declared branch scope
- All new text in English

## Spec Sources
- Required:
  - `spec/<primary-spec>.md`
- Optional:
  - `spec/<supporting-spec>.md`

## Allowed Paths
- `<path-or-glob-1>`
- `<path-or-glob-2>`

## Forbidden Paths
- `PLAN.md`
- `rules/**`
- `plan/BRANCH_TEMPLATE.md`
- `plan/SUBAGENT_PROMPT.md`
- unrelated crate paths outside branch scope

## Conditional Paths
- `spec/**` only when branch findings change documented behavior or syntax understanding
- `surch-core/src/common/**` only if required by branch contract and recorded in feedback loop

## Dependency Gates
- [ ] Upstream dependencies reviewed
- [ ] Required specs confirmed
- [ ] Environment mapping confirmed

## Environment Mapping
- Worktree: `tmp/<branch-slug>`
- Local data path: `data/test/<branch-slug>/`
- API port if needed: `92XX`
- Mode:
  - [ ] local Rust only
  - [ ] local Rust + isolated data
  - [ ] containerized e2e needed

## Plan / Lots / Todo

- [ ] **Lot 0 - Read and confirm contract**
  - [ ] Read active specs
  - [ ] Read current implementation files in scope
  - [ ] Confirm allowed and forbidden paths
  - [ ] Confirm validation commands

- [ ] **Lot 1 - First implementation slice**
  - [ ] <Task>
  - [ ] <Task>
  - [ ] Lot gate:
    - [ ] `cargo fmt --all`
    - [ ] `cargo clippy --workspace --all-targets --all-features`
    - [ ] <Scoped unit test command>

- [ ] **Lot 2 - Second implementation slice**
  - [ ] <Task>
  - [ ] <Task>
  - [ ] Lot gate:
    - [ ] <Scoped integration test command>

- [ ] **Lot N - Final validation**
  - [ ] `cargo fmt --all`
  - [ ] `cargo clippy --workspace --all-targets --all-features`
  - [ ] `cargo test --workspace`
  - [ ] Branch-specific compatibility checks complete

## Feedback Loop
- Use these statuses when needed:
  - `blocked`
  - `attention`
  - `spec-mismatch`
  - `security-alert`
  - `clarification`

## Tests Required
- Unit:
  - <files or modules>
- Integration:
  - <files or suites>
- Compatibility:
  - <endpoint or DSL checks>

## Security Checks
- [ ] Input validation reviewed for this branch scope
- [ ] No new unbounded expensive path introduced
- [ ] Dependency impact reviewed if new crates are added

## Merge Checklist
- [ ] Objective met
- [ ] Scope respected
- [ ] No unresolved blockers
- [ ] Tests passed
- [ ] Verification evidence recorded
