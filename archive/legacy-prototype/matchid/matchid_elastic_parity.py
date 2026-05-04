#!/usr/bin/env python3

import argparse
import json
import subprocess
import sys
import urllib.request
from pathlib import Path


def load_jsonl(path: Path):
    with path.open("r", encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def dump_jsonl(path: Path, rows):
    with path.open("w", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row, ensure_ascii=True) + "\n")


def normalize_response(case, capture):
    assertion = case.get("assertion", {})
    top_k = assertion.get("top_k", 10)
    compare_total = assertion.get("compare_total", True)
    compare_ids = assertion.get("compare_ids", True)
    compare_source_fields = assertion.get("compare_source_fields", [])

    response = capture.get("response", {})
    hits = response.get("hits", {})
    total = hits.get("total", {})
    hit_rows = hits.get("hits", [])

    normalized_hits = []
    for hit in hit_rows[:top_k]:
        normalized_hit = {}
        if compare_ids:
            normalized_hit["_id"] = hit.get("_id")
        if compare_source_fields:
            source = hit.get("_source", {})
            normalized_hit["_source"] = {
                field: source.get(field) for field in compare_source_fields
            }
        normalized_hits.append(normalized_hit)

    normalized = {
        "http_status": capture.get("http_status"),
        "hits": normalized_hits,
    }
    if compare_total:
        normalized["total"] = total.get("value")

    return normalized


def compare_case(case, baseline, candidate):
    baseline_norm = normalize_response(case, baseline)
    candidate_norm = normalize_response(case, candidate)

    diffs = []
    if baseline_norm["http_status"] != candidate_norm["http_status"]:
        diffs.append("status_mismatch")
    if baseline_norm.get("total") != candidate_norm.get("total"):
        diffs.append("total_mismatch")

    baseline_hits = baseline_norm["hits"]
    candidate_hits = candidate_norm["hits"]

    if len(baseline_hits) != len(candidate_hits):
        diffs.append("hit_count_mismatch")

    baseline_ids = [hit.get("_id") for hit in baseline_hits]
    candidate_ids = [hit.get("_id") for hit in candidate_hits]
    if baseline_ids != candidate_ids:
        diffs.append("id_order_mismatch")

    for baseline_hit, candidate_hit in zip(baseline_hits, candidate_hits):
        if baseline_hit.get("_source") != candidate_hit.get("_source"):
            diffs.append("source_field_mismatch")
            break

    return diffs, baseline_norm, candidate_norm


def replay_case(base_url: str, case, docker_container: str | None = None):
    request_def = case["request"]
    body_json = json.dumps(request_def.get("json", {}))

    if docker_container:
        return replay_case_via_docker(docker_container, request_def, body_json, case["case_id"])

    body_bytes = body_json.encode("utf-8")
    request = urllib.request.Request(
        base_url.rstrip("/") + request_def["path"],
        data=body_bytes,
        method=request_def.get("method", "POST"),
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return {
                "case_id": case["case_id"],
                "http_status": response.status,
                "response": json.loads(response.read().decode("utf-8")),
            }
    except Exception as exc:
        return {
            "case_id": case["case_id"],
            "http_status": 599,
            "response": {"error": str(exc)},
        }


def replay_case_via_docker(container: str, request_def: dict, body_json: str, case_id: str):
    command = [
        "docker",
        "exec",
        container,
        "curl",
        "-s",
        "-X",
        request_def.get("method", "POST"),
        "http://localhost:9200" + request_def["path"],
        "-H",
        "Content-Type: application/json",
        "-d",
        body_json,
        "-w",
        "\n%{http_code}",
    ]

    result = subprocess.run(command, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        return {
            "case_id": case_id,
            "http_status": 599,
            "response": {"error": result.stderr.strip() or result.stdout.strip()},
        }

    lines = result.stdout.splitlines()
    if not lines:
        return {
            "case_id": case_id,
            "http_status": 599,
            "response": {"error": "empty docker exec response"},
        }

    status_line = lines[-1]
    response_body = "\n".join(lines[:-1])
    return {
        "case_id": case_id,
        "http_status": int(status_line),
        "response": json.loads(response_body) if response_body else {},
    }


def command_replay(args):
    corpus = load_jsonl(Path(args.corpus))
    captures = [
        replay_case(args.base_url, case, docker_container=args.docker_container)
        for case in corpus
    ]
    dump_jsonl(Path(args.out), captures)
    return 0


def command_compare(args):
    corpus = {row["case_id"]: row for row in load_jsonl(Path(args.corpus))}
    baseline = {row["case_id"]: row for row in load_jsonl(Path(args.baseline))}
    candidate = {row["case_id"]: row for row in load_jsonl(Path(args.candidate))}

    report = []
    exit_code = 0

    for case_id, case in corpus.items():
        if case_id not in baseline or case_id not in candidate:
            report.append({"case_id": case_id, "diffs": ["missing_case"]})
            exit_code = 1
            continue

        diffs, baseline_norm, candidate_norm = compare_case(
            case, baseline[case_id], candidate[case_id]
        )
        if diffs:
            exit_code = 1
            report.append(
                {
                    "case_id": case_id,
                    "diffs": diffs,
                    "baseline": baseline_norm,
                    "candidate": candidate_norm,
                }
            )

    if args.report:
        Path(args.report).write_text(json.dumps(report, indent=2), encoding="utf-8")
    else:
        sys.stdout.write(json.dumps(report, indent=2) + "\n")

    return exit_code


def build_parser():
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    replay = subparsers.add_parser("replay")
    replay.add_argument("--corpus", required=True)
    replay.add_argument("--base-url", required=True)
    replay.add_argument("--docker-container")
    replay.add_argument("--out", required=True)
    replay.set_defaults(func=command_replay)

    compare = subparsers.add_parser("compare")
    compare.add_argument("--corpus", required=True)
    compare.add_argument("--baseline", required=True)
    compare.add_argument("--candidate", required=True)
    compare.add_argument("--report")
    compare.set_defaults(func=command_compare)

    return parser


def main(argv=None):
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
