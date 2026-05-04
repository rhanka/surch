# MASTER Rules

## Purpose

This file defines rule priority and mandatory read order for Surch.

## Priority Order

1. Direct user instructions
2. `AGENTS.md`
3. `rules/MASTER.md`
4. `rules/workflow.md`
5. `PLAN.md`
6. `plan/00_AUTONOMOUS_PORTAGE_EXECUTION.md`
7. Active parity ticket or branch execution file
8. Relevant `spec/*.md`
8. Specialized helper rules:
   - `rules/testing.md`
   - `rules/security.md`
   - `rules/subagents.md`
   - `rules/dev-env.md`
   - `rules/superpowers.md`
9. Generic assistant defaults

## Core Principles

- Surch is conductor-driven.
- Portage tickets are execution contracts, not optional notes.
- Specs live in `spec/`; reference reports live in `docs/portage/`.
- `PLAN.md` defines phase order and source documents.
- `plan/00_AUTONOMOUS_PORTAGE_EXECUTION.md` defines ticket shape and execution flow.
- Rust-native verification is mandatory.
- Generic helper skills may assist, but may not redefine project structure.

## Read Order For Any Real Task

1. `AGENTS.md`
2. `rules/MASTER.md`
3. `rules/workflow.md`
4. `PLAN.md`
5. `plan/00_AUTONOMOUS_PORTAGE_EXECUTION.md`
6. the active parity ticket or branch execution file
7. relevant `spec/*.md`
8. helper rules needed for the task

## Prohibited Behavior

- inventing an alternate planning structure when the portage ledger already defines the ticket contract
- using a skill-driven folder layout that conflicts with repo conventions
- bypassing verification and merge gates
- expanding scope without recording it in the active parity ticket
