# Development Environment Rules

## Goal

Define how Surch uses local Rust tooling and optional containers without environment confusion.

## Environment Names

- `dev`: interactive local development
- `test`: isolated validation environment
- `e2e`: reproducible full-stack validation

## Default Policy

- local Rust coding may run without Docker
- integration isolation should avoid sharing `dev` data
- end-to-end validation should be containerized

## Data Isolation

Recommended data directories:
- `data/dev/`
- `data/test/<branch-slug>/`
- `tmp/e2e/<branch-slug>/`

No test campaign should reuse `dev` data.

## Port Isolation

Recommended convention:
- root dev API: `9200`
- branch-local isolated API: `92XX` where `XX` maps to branch ID

Example:
- BR-03 -> `9203`
- BR-06 -> `9206`

If another service is already using the chosen port, declare a new mapping in the branch file before running.

## Command Policy

Use raw `cargo` commands when moving fast locally.

Use `make` targets when:
- the target exists and is stable
- the branch requires a repeatable shared command contract
- conductor wants consistent validation across agents

## Docker Policy

Docker is:
- optional for local coding
- recommended for isolated branch validation
- mandatory for reproducible `e2e`

Future expected files, when introduced:
- `docker-compose.dev.yml`
- `docker-compose.test.yml`
- `docker-compose.e2e.yml`

## Branch Worktrees

For parallel work, each active branch should use its own worktree and isolated data path.
