#!/usr/bin/env python3
"""Replay the matchID deces_v1 fixture against Elasticsearch 8.6.1.

The script writes a human-readable summary to
target/matchid-oracle/deces_v1/summary.md and exits non-zero when any
non-skipped replay request diverges on status, total hits, top id, or
critical response shape.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_INDEX = "deces"
DEFAULT_REPLAY = ROOT / "tests/matchid_compat/replays/deces_v1.json"
DEFAULT_MAPPING = ROOT / "tests/matchid_compat/deces/mapping.json"
DEFAULT_BULK = ROOT / "tests/matchid_compat/deces/slice-1000.ndjson"
DEFAULT_OUT_DIR = ROOT / "target/matchid-oracle/deces_v1"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Replay deces_v1 against a clean Elasticsearch 8.6.1 node."
    )
    parser.add_argument(
        "--elasticsearch-url",
        default=os.environ.get("ELASTICSEARCH_URL", "http://127.0.0.1:9200"),
        help="Elasticsearch 8.6.1 base URL; defaults to ELASTICSEARCH_URL or localhost.",
    )
    parser.add_argument("--index", default=DEFAULT_INDEX)
    parser.add_argument("--replay", type=Path, default=DEFAULT_REPLAY)
    parser.add_argument("--mapping", type=Path, default=DEFAULT_MAPPING)
    parser.add_argument("--bulk", type=Path, default=DEFAULT_BULK)
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR)
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Validate local inputs and print the planned replay without HTTP requests.",
    )
    return parser.parse_args()


def request(base: str, method: str, path: str, body: str | None = None) -> tuple[int, Any]:
    data = body.encode("utf-8") if body is not None else None
    headers = {"content-type": "application/json"}
    req = urllib.request.Request(base + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            raw = resp.read().decode("utf-8")
            return resp.status, json.loads(raw) if raw else None
    except urllib.error.HTTPError as err:
        raw = err.read().decode("utf-8")
        try:
            parsed = json.loads(raw) if raw else None
        except json.JSONDecodeError:
            parsed = {"raw": raw}
        return err.code, parsed


def body_for(entry: dict[str, Any]) -> str:
    request_data = entry["request"]
    has_body = "body" in request_data
    request_body = request_data.get("body")
    body_ndjson = request_data.get("body_ndjson")
    if has_body and body_ndjson is not None:
        raise RuntimeError(f"{entry['name']} declares both body and body_ndjson")
    if body_ndjson is not None:
        return "".join(json.dumps(line, separators=(",", ":")) + "\n" for line in body_ndjson)
    if not has_body or request_body is None:
        return ""
    return json.dumps(request_body, separators=(",", ":"))


def extract(entry: dict[str, Any], response: Any) -> tuple[int | None, str | None, list[str]]:
    if response is None:
        return None, None, ["empty response body"]

    path = entry["request"]["path"]
    errors: list[str] = []
    if path.endswith("_msearch"):
        responses = response.get("responses")
        if not isinstance(responses, list):
            return None, None, ["critical shape: _msearch response missing responses[]"]
        total = 0
        top_id = None
        for idx, sub in enumerate(responses):
            try:
                total += sub["hits"]["total"]["value"]
            except Exception:
                errors.append(f"critical shape: responses[{idx}] missing hits.total.value")
            if idx == 0:
                try:
                    top_id = sub["hits"]["hits"][0]["_id"]
                except Exception:
                    top_id = None
        return total, top_id, errors

    try:
        total = response["hits"]["total"]["value"]
    except Exception:
        return None, None, ["critical shape: response missing hits.total.value"]

    top_id = None
    if entry.get("expected", {}).get("hits.hits[0]._id") is not None:
        try:
            top_id = response["hits"]["hits"][0]["_id"]
        except Exception:
            errors.append("critical shape: response missing hits.hits[0]._id")

    if "?scroll=" in path or "&scroll=" in path:
        if not response.get("_scroll_id"):
            errors.append("critical shape: scroll response missing non-empty _scroll_id")

    return total, top_id, errors


def write_summary(
    out_dir: Path,
    base: str,
    rows: list[list[str]],
    failures: list[str],
) -> Path:
    out_dir.mkdir(parents=True, exist_ok=True)
    summary = out_dir / "summary.md"
    with summary.open("w", encoding="utf-8") as out:
        out.write("# deces_v1 Elasticsearch Oracle Summary\n\n")
        out.write(f"Elasticsearch URL: `{base}`\n\n")
        out.write("| request | verdict | status | hits.total.value | top id | notes |\n")
        out.write("|---|---:|---:|---:|---|---|\n")
        for row in rows:
            out.write("| " + " | ".join(cell.replace("|", "\\|") for cell in row) + " |\n")
        out.write("\n")
        if failures:
            out.write(f"Verdict: FAIL ({len(failures)} mismatch(es))\n")
        else:
            out.write("Verdict: PASS\n")
    return summary


def load_inputs(args: argparse.Namespace) -> dict[str, Any]:
    for path in [args.replay, args.mapping, args.bulk]:
        if not path.exists():
            raise RuntimeError(f"missing required input: {path}")
    return json.loads(args.replay.read_text())


def dry_run(args: argparse.Namespace, manifest: dict[str, Any]) -> int:
    requests = manifest["requests"]
    skipped = sum(1 for entry in requests if entry.get("skip"))
    print("deces_v1 Elasticsearch oracle dry-run")
    print(f"replay: {args.replay}")
    print(f"mapping: {args.mapping}")
    print(f"bulk: {args.bulk}")
    print(f"out_dir: {args.out_dir}")
    print(f"requests: {len(requests)} total, {skipped} skipped")
    print("HTTP requests: disabled")
    return 0


def run(args: argparse.Namespace, manifest: dict[str, Any]) -> int:
    base = args.elasticsearch_url.rstrip("/")

    request(base, "DELETE", f"/{args.index}")
    status, body = request(base, "PUT", f"/{args.index}", args.mapping.read_text())
    if status not in (200, 201):
        raise RuntimeError(f"create index failed: status={status} body={body}")
    status, body = request(base, "POST", "/_bulk", args.bulk.read_text())
    if status != 200 or (body or {}).get("errors"):
        raise RuntimeError(f"bulk load failed: status={status} body={body}")
    status, body = request(base, "POST", f"/{args.index}/_refresh", "")
    if status != 200:
        raise RuntimeError(f"refresh failed: status={status} body={body}")

    rows: list[list[str]] = []
    failures: list[str] = []
    for entry in manifest["requests"]:
        if entry.get("skip"):
            rows.append([entry["name"], "SKIP", entry["skip"], "", "", ""])
            continue
        status, response = request(
            base,
            entry["request"]["method"],
            entry["request"]["path"],
            body_for(entry),
        )
        total, top_id, shape_errors = extract(entry, response)
        expected = entry["expected"]
        mismatches = []
        if status != 200:
            mismatches.append(f"status expected 200 got {status}")
        if total != expected["hits.total.value"]:
            mismatches.append(
                f"hits.total.value expected {expected['hits.total.value']} got {total}"
            )
        expected_top = expected.get("hits.hits[0]._id")
        if expected_top is not None and top_id != expected_top:
            mismatches.append(f"top id expected {expected_top} got {top_id}")
        mismatches.extend(shape_errors)
        verdict = "PASS" if not mismatches else "FAIL"
        rows.append(
            [
                entry["name"],
                verdict,
                str(status),
                str(total),
                str(top_id),
                "; ".join(mismatches),
            ]
        )
        if mismatches:
            failures.append(entry["name"])

    summary = write_summary(args.out_dir, base, rows, failures)
    print(f"Wrote {summary}")
    if failures:
        print("Failing requests: " + ", ".join(failures), file=sys.stderr)
        return 1
    return 0


def main() -> int:
    args = parse_args()
    manifest = load_inputs(args)
    if args.dry_run:
        return dry_run(args, manifest)
    return run(args, manifest)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(1)
