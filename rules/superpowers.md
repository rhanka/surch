# Superpowers Framing Rules

## Goal

Ensure generic helper skills do not override Surch project governance.

## Priority

For Surch execution:
- repo rules and parity tickets win over generic skill workflow preferences
- skills are helpers, not project owners

## Hard Boundaries

No skill may:
- move specs out of `spec/`
- replace `PLAN.md`, `plan/00_AUTONOMOUS_PORTAGE_EXECUTION.md`, or the parity ledger with another planning layout
- bypass allowed and forbidden path boundaries
- bypass verification gates
- replace parity tickets with ad hoc notes

## Explicit Conflict Examples

If a skill suggests any of the following, Surch rules override it:
- saving specs under `docs/superpowers/specs/` instead of `spec/`
- using an hourly checkpoint system as the primary orchestration model
- forcing a documentation structure that conflicts with the portage ticket ledger
- delaying concrete execution after the next action is already known

## Allowed Use Of Skills

Skills may help with:
- brainstorming design tradeoffs
- writing plans
- debugging
- verification reminders

But their outputs must be normalized back into Surch repo structure.

## Conductor Rule

If a skill and the repo disagree, the conductor follows repo rules and documents the normalized result in the appropriate project file.
