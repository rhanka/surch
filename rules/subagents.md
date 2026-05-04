# Subagent Rules

## Goal

Provide a repeatable contract for subagent launches in Surch.

## Maximum Parallelism

- maximum 4 active subagents
- one subagent owns one branch at a time

## Launch Preconditions

Do not launch a subagent until:
- the target parity ticket exists
- the target branch name is derived from the ticket ID
- the ticket cites exact upstream references
- dependencies are explicit
- allowed and forbidden paths are explicit
- golden tests are explicit
- validation commands are explicit

## Mandatory Launch Packet

- ticket ID and branch name
- worktree path
- owner role (`#1` StorageEngine, `#2` Indexer, `#3` SearchEngine, `#4` APIServer)
- exact scope
- upstream references
- allowed paths
- forbidden paths
- dependency gates
- golden test oracle
- environment mapping
- validation commands
- expected output
- stop conditions

## Read Order For Subagents

1. `AGENTS.md`
2. `rules/MASTER.md`
3. `rules/workflow.md`
4. `plan/00_AUTONOMOUS_PORTAGE_EXECUTION.md`
5. active parity ticket
6. relevant `spec/*.md`
7. relevant helper rules

## Reporting Contract

Subagent reports must include:
- changed files
- commands run
- outcomes
- golden parity result
- active feedback items
- risks
- scope exceptions used or `none`

## Stop Conditions

Stop and escalate if:
- scope needs to cross forbidden paths
- a dependency gate is unresolved
- ticket and spec disagree
- environment ownership is unclear
- a command would be destructive or ambiguous

## Integration Rule

Subagents do not integrate other branches.

Only conductor integrates and resolves cross-branch arbitration.
