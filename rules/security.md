# Security Rules

## Goal

Define minimum security expectations for Surch MVP execution.

## Mandatory Controls

- validate all API inputs strictly
- reject malformed JSON and invalid field types
- bound request body size
- bound wildcard and regex execution behavior
- avoid logging secrets or raw credentials
- keep dependency checks in the validation loop

## Abuse Controls

The implementation must not leave these surfaces unbounded:

- request body size
- wildcard patterns
- regexp patterns
- nested bool depth
- query expansion counts
- deep pagination

## Supply Chain

Recommended checks:
- `cargo audit`
- `cargo deny` when configured

High-risk dependency issues must be documented before release branch merge.

## Branch-Level Security Review

Each implementation branch should ask:
- does this branch add new input parsing?
- does it add or expand attack surface?
- does it add an expensive query path?
- does it require new validation or size limits?

If yes, record the security check in the branch file.

## Release Gate

Release branch must verify:
- body size limits are in place or explicitly deferred with documented risk
- wildcard and regex controls are bounded
- invalid payload tests exist
- dependency review was run and summarized

## Escalation Conditions

Raise `security-alert` immediately for:
- unbounded regex execution
- unbounded wildcard execution
- missing validation on public API payloads
- accidental secret exposure in code or logs
