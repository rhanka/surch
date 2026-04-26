# Subagent Rules

## Goal

Provide a repeatable contract for subagent launches in Surch.

## Maximum Parallelism

- maximum 4 active subagents
- one subagent owns one branch at a time

## Launch Preconditions

Do not launch a subagent until:
- the target branch exists in `PLAN.md`
- the target branch file exists in `plan/`
- dependencies are explicit
- allowed and forbidden paths are explicit
- validation commands are explicit

## Mandatory Launch Packet

- branch ID and branch name
- worktree path
- owner role (`#1` StorageEngine, `#2` Indexer, `#3` SearchEngine, `#4` APIServer)
- exact scope
- allowed paths
- forbidden paths
- dependency gates
- environment mapping
- validation commands
- expected output
- stop conditions

## Read Order For Subagents

1. `AGENTS.md`
2. `rules/MASTER.md`
3. `rules/workflow.md`
4. active `plan/NN_BRANCH_*.md`
5. relevant `spec/*.md`
6. relevant helper rules

## Reporting Contract

Subagent reports must include:
- changed files
- commands run
- outcomes
- active feedback items
- risks
- scope exceptions used or `none`

## Stop Conditions

Stop and escalate if:
- scope needs to cross forbidden paths
- a dependency gate is unresolved
- branch file and spec disagree
- environment ownership is unclear
- a command would be destructive or ambiguous

## Integration Rule

Subagents do not integrate other branches.

Only conductor integrates and resolves cross-branch arbitration.
