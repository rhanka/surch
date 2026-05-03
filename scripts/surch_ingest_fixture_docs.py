#!/usr/bin/env python3

import argparse
import json
import urllib.error
import urllib.request
from pathlib import Path


def load_jsonl(path: Path):
    with path.open("r", encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def request(method: str, url: str, body=None):
    data = None
    headers = {"Accept": "application/json"}
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"

    req = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=30) as response:
            payload = response.read().decode("utf-8")
            return response.status, json.loads(payload) if payload else {}
    except urllib.error.HTTPError as exc:
        payload = exc.read().decode("utf-8")
        return exc.code, json.loads(payload) if payload else {}


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--docs", required=True)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--index", default="deces")
    args = parser.parse_args(argv)

    base_url = args.base_url.rstrip("/")
    docs = load_jsonl(Path(args.docs))

    create_status, _ = request("PUT", f"{base_url}/{args.index}", {})
    if create_status not in (200, 201):
        return 1

    for row in docs:
        status, _ = request(
            "PUT",
            f"{base_url}/{args.index}/_doc/{row['_id']}",
            row["document"],
        )
        if status not in (200, 201):
            return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
