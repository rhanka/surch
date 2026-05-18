# Superpowers Compatibility Notes

Canonical active guidance is now `AGENTS.md`.

Superpowers are execution helpers for local work:

- brainstorming and design clarification
- TDD and systematic debugging
- implementation plans for bounded features
- verification before completion

They do not replace Surch persistent tracking:

- `PLAN.md` is the global status source
- `plan/*.md` files are branch/lane execution trackers
- user-facing reporting follows `AGENTS.md`

If a Superpowers workflow suggests a different persistent layout, the
agent should execute the useful local method and then normalize the
result back into `AGENTS.md`, `PLAN.md`, and the relevant `plan/*.md`.
