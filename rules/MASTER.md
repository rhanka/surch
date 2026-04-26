# MASTER Rules

## Purpose

This file defines rule priority and mandatory read order for Surch.

## Priority Order

1. Direct user instructions
2. `AGENTS.md`
3. `rules/MASTER.md`
4. `rules/workflow.md`
5. `PLAN.md`
6. Active branch file in `plan/NN_BRANCH_*.md`
7. Relevant `spec/*.md`
8. Specialized helper rules:
   - `rules/testing.md`
   - `rules/security.md`
   - `rules/subagents.md`
   - `rules/dev-env.md`
   - `rules/superpowers.md`
9. Generic assistant defaults

## Core Principles

- Surch is conductor-driven.
- Branch files are execution contracts, not optional notes.
- Specs live in `spec/`.
- `PLAN.md` indexes branches and status; branch detail belongs in `plan/`.
- Rust-native verification is mandatory.
- Generic helper skills may assist, but may not redefine project structure.

## Read Order For Any Real Task

1. `AGENTS.md`
2. `rules/MASTER.md`
3. `rules/workflow.md`
4. `PLAN.md`
5. the active `plan/NN_BRANCH_*.md`
6. relevant `spec/*.md`
7. helper rules needed for the task

## Prohibited Behavior

- inventing an alternate planning structure when branch files already exist
- using a skill-driven folder layout that conflicts with repo conventions
- bypassing verification and merge gates
- expanding scope without recording it in the active branch file
