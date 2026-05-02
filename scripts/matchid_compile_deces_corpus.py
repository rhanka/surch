#!/usr/bin/env python3

import argparse
import json
from pathlib import Path


def load_jsonl(path: Path):
    with path.open("r", encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def date_transform_mask(date_string: str) -> str:
    parts = date_string.split("/")
    if len(parts) != 3:
        return date_string
    day, month, year = parts
    return f"{year}{month}{day}"


def compile_request(seed_row):
    request = seed_row["matchid_request"]
    should_clauses = []

    if request.get("firstName"):
        should_clauses.append({"match": {"PRENOM": request["firstName"]}})
    if request.get("lastName"):
        should_clauses.append({"match": {"NOM": request["lastName"]}})

    must_clauses = []
    if should_clauses:
        minimum_should_match = 2 if request.get("firstName") and request.get("lastName") else 1
        must_clauses.append(
            {
                "bool": {
                    "should": should_clauses,
                    "minimum_should_match": minimum_should_match,
                }
            }
        )

    if request.get("birthDate"):
        must_clauses.append(
            {"match": {"DATE_NAISSANCE.raw": date_transform_mask(request["birthDate"])}}
        )
    if request.get("birthCity"):
        must_clauses.append({"match": {"COMMUNE_NAISSANCE.raw": request["birthCity"]}})
    if request.get("birthCountry"):
        must_clauses.append({"match": {"PAYS_NAISSANCE.raw": request["birthCountry"]}})

    return {
        "case_id": seed_row["case_id"],
        "family": seed_row.get("family", "representative-seed"),
        "request": {
            "method": "POST",
            "path": "/deces/_search",
            "json": {
                "size": 20,
                "from": 0,
                "sort": [{"_score": "desc"}],
                "query": {"bool": {"must": must_clauses}},
            },
        },
        "assertion": {
            "top_k": 20,
            "compare_total": True,
            "compare_ids": True,
            "compare_source_fields": [
                "UID",
                "NOM",
                "PRENOM",
                "DATE_NAISSANCE",
                "COMMUNE_NAISSANCE",
                "PAYS_NAISSANCE",
            ],
        },
        "notes": seed_row.get("notes", "Compiled from MatchID request seed"),
    }


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--seed", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args(argv)

    rows = load_jsonl(Path(args.seed))
    compiled = [compile_request(row) for row in rows]

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with out_path.open("w", encoding="utf-8") as handle:
        for row in compiled:
            handle.write(json.dumps(row, ensure_ascii=True) + "\n")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
