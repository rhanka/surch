#!/usr/bin/env python3

import argparse
import json
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


def load_jsonl(path: Path):
    with path.open("r", encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def build_post_request(base_url: str, payload: dict):
    body_bytes = json.dumps(payload).encode("utf-8")
    return urllib.request.Request(
        base_url.rstrip("/") + "/deces/api/v1/search",
        data=body_bytes,
        method="POST",
        headers={
            "Content-Type": "application/json",
            "Accept": "application/json",
            "User-Agent": "Mozilla/5.0 (compatible; surch-parity-harness)",
        },
    )


def build_get_request(base_url: str, payload: dict):
    normalized_payload = {}
    for key, value in payload.items():
        if isinstance(value, bool):
            normalized_payload[key] = "true" if value else "false"
        else:
            normalized_payload[key] = value

    query = urllib.parse.urlencode(normalized_payload)
    return urllib.request.Request(
        base_url.rstrip("/") + "/deces/api/v1/search?" + query,
        method="GET",
        headers={
            "Accept": "application/json",
            "User-Agent": "Mozilla/5.0 (compatible; surch-parity-harness)",
        },
    )


def capture_case(base_url: str, case: dict):
    payload = case["matchid_request"]
    captures = {}

    for mode, builder in [("get", build_get_request), ("post", build_post_request)]:
        request = builder(base_url, payload)
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                captures[mode] = {
                    "http_status": response.status,
                    "response": json.loads(response.read().decode("utf-8")),
                }
        except urllib.error.HTTPError as exc:
            body = exc.read().decode("utf-8")
            try:
                response = json.loads(body)
            except json.JSONDecodeError:
                response = {"raw": body}

            captures[mode] = {
                "http_status": exc.code,
                "response": response,
            }

    return {
        "case_id": case["case_id"],
        "matchid_request": payload,
        "captures": captures,
    }


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--seed", required=True)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--limit", type=int)
    args = parser.parse_args(argv)

    rows = load_jsonl(Path(args.seed))
    if args.limit is not None:
        rows = rows[: args.limit]

    captures = [capture_case(args.base_url, row) for row in rows]
    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with out_path.open("w", encoding="utf-8") as handle:
        for row in captures:
            handle.write(json.dumps(row, ensure_ascii=True) + "\n")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
