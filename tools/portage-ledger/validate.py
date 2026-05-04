#!/usr/bin/env python3
"""Validate Surch portage ledger tickets."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any


REQUIRED_FIELDS = [
    "id",
    "title",
    "owner",
    "priority",
    "upstream_ref",
    "parity_level",
    "dependencies",
    "allowed_paths",
    "forbidden_paths",
    "golden_tests_required",
    "gates",
    "status",
]

REQUIRED_UPSTREAM_FIELDS = ["repo", "commit", "files", "symbols"]
NON_EMPTY_LIST_FIELDS = ["allowed_paths", "golden_tests_required", "gates"]
VALID_OWNERS = {"StorageEngine", "Indexer", "SearchEngine", "APIServer", "Conductor"}
VALID_STATUSES = {"discovered", "triaged", "specced", "ready", "active", "pr", "validated", "done", "deferred"}


def load_ticket(path: Path) -> dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError(f"{path}: invalid JSON: {exc}") from exc

    if not isinstance(data, dict):
        raise ValueError(f"{path}: ticket root must be an object")
    return data


def require_string(ticket: dict[str, Any], path: Path, field: str) -> None:
    value = ticket.get(field)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{path}: {field} must be a non-empty string")


def require_list(ticket: dict[str, Any], path: Path, field: str, *, non_empty: bool) -> None:
    value = ticket.get(field)
    if not isinstance(value, list):
        raise ValueError(f"{path}: {field} must be a list")
    if non_empty and not value:
        raise ValueError(f"{path}: {field} must not be empty")
    if not all(isinstance(item, str) and item.strip() for item in value):
        raise ValueError(f"{path}: {field} must contain only non-empty strings")


def validate_ticket(path: Path) -> None:
    ticket = load_ticket(path)
    for field in REQUIRED_FIELDS:
        if field not in ticket:
            raise ValueError(f"{path}: missing required field {field}")

    for field in ["id", "title", "owner", "priority", "parity_level", "status"]:
        require_string(ticket, path, field)

    if ticket["owner"] not in VALID_OWNERS:
        raise ValueError(f"{path}: owner must be one of {sorted(VALID_OWNERS)}")
    if ticket["status"] not in VALID_STATUSES:
        raise ValueError(f"{path}: status must be one of {sorted(VALID_STATUSES)}")

    for field in ["dependencies", "allowed_paths", "forbidden_paths", "golden_tests_required", "gates"]:
        require_list(ticket, path, field, non_empty=field in NON_EMPTY_LIST_FIELDS)

    upstream = ticket["upstream_ref"]
    if not isinstance(upstream, dict):
        raise ValueError(f"{path}: upstream_ref must be an object")
    for field in REQUIRED_UPSTREAM_FIELDS:
        if field not in upstream:
            raise ValueError(f"{path}: upstream_ref missing required field {field}")
    for field in ["repo", "commit"]:
        value = upstream.get(field)
        if not isinstance(value, str) or not value.strip():
            raise ValueError(f"{path}: upstream_ref.{field} must be a non-empty string")
    for field in ["files", "symbols"]:
        value = upstream.get(field)
        if not isinstance(value, list) or not value:
            raise ValueError(f"{path}: upstream_ref.{field} must be a non-empty list")
        if not all(isinstance(item, str) and item.strip() for item in value):
            raise ValueError(f"{path}: upstream_ref.{field} must contain only non-empty strings")


def iter_ticket_files(path: Path) -> list[Path]:
    if path.is_file():
        return [path]
    return sorted(candidate for candidate in path.rglob("*.json") if candidate.is_file())


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", type=Path, help="Ticket file or directory containing .json tickets")
    args = parser.parse_args(argv)

    ticket_files = iter_ticket_files(args.path)
    if not ticket_files:
        print(f"{args.path}: no .json tickets found", file=sys.stderr)
        return 1

    try:
        for ticket_file in ticket_files:
            validate_ticket(ticket_file)
    except ValueError as exc:
        print(str(exc), file=sys.stderr)
        return 1

    print(f"validated {len(ticket_files)} ticket{'s' if len(ticket_files) != 1 else ''}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
