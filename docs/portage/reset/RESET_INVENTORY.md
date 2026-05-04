# Reset Inventory

Date: 2026-05-04

## Purpose

This inventory records the cleanup that moved Surch from the old prototype roadmap to a blank OpenSearch + Lucene Rust portage workspace.

## Preserved State

- Before-reset status: `docs/portage/reset/git-status-before-reset.txt`
- Before-reset diff stat: `docs/portage/reset/git-diff-stat-before-reset.txt`
- Before-reset worktree list: `docs/portage/reset/git-worktrees-before-reset.txt`

Archive branches created:

- `archive/prototype-before-portage-2026-05-04`
- `archive-prototype-before-portage-2026-05-04`

The flat branch exists because the first non-escalated attempt could not write the nested ref under sandboxed `.git`; the nested branch was created after escalation and is the canonical archive branch.

The original dirty-worktree patch remains recoverable from the archive branches and Git history. It was removed from the active tree during the language-policy cleanup because it embedded disallowed historical script references.

## Removed Worktree Checkouts

The old branch checkouts were removed from `.worktrees/`. Their Git branches remain available under `feature/BR-*`.

Patches saved before removal:

- `docs/portage/reset/worktree-br-01-spec-harvest.patch`
- `docs/portage/reset/worktree-br-02-spec-harvest.patch`
- `docs/portage/reset/worktree-br-03-storage-wal.patch`
- `docs/portage/reset/worktree-br-04-indexer.patch`
- `docs/portage/reset/worktree-br-05-search-fuzzy.patch`
- `docs/portage/reset/worktree-br-06-api-index-doc.patch`

The BR-05 and BR-06 patches are empty because those worktrees had no local diff.

## Archived Prototype

The active prototype implementation was moved out of the workspace:

- `archive/legacy-prototype/surch-core/`
- `archive/legacy-prototype/surch-api/`
- `archive/legacy-prototype/devops/`

The new active workspace uses `crates/*`.

The archived MatchID prototype remains recoverable from the archive branches and Git history. It was removed from the active tree during the language-policy cleanup.

## Removed Runtime Artifacts

The following local generated directories were removed:

- `target/`
- `data/`
- `.worktrees/runtime-surch/`
- Interpreter cache directories under archived MatchID scripts/tests
- Historical script archive under `archive/legacy-prototype/matchid/`

## Governance Decisions

The old roadmap files under `plan/01_BRANCH_*.md` and old `spec/SPEC_*.md` were removed from the active plan/spec directories.

The active source documents are:

- `PLAN.md`
- `plan/00_AUTONOMOUS_PORTAGE_EXECUTION.md`
- `spec/SPEC_INTEGRAL_OPENSEARCH_LUCENE_PORTAGE.md`
- `docs/portage/REFERENCES.md`

Historical old references remain in archive branches and Git history. The active tree keeps only the reset inventories and the BR worktree patches that satisfy the current language policy.
