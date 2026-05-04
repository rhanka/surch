#!/usr/bin/env python3

import argparse
import csv
import json
from pathlib import Path


def build_rows(csv_path: Path, limit: int):
    with csv_path.open("r", encoding="utf-8") as handle:
        reader = csv.DictReader(handle, delimiter=";")
        for index, row in enumerate(reader, start=1):
            if index > limit:
                break
            yield {
                "case_id": f"clients-test-{index:04d}",
                "family": "representative-seed",
                "matchid_request": {
                    "firstName": row["Prenom"],
                    "lastName": row["Nom"],
                    "birthDate": row["Date"],
                    "birthCountry": row["Pays"],
                    "birthCity": row["Lieu"],
                    "fuzzy": False,
                },
                "notes": "Seeded from MatchID clients_test.csv performance-compatible fields",
            }


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--csv", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--limit", type=int, default=50)
    args = parser.parse_args(argv)

    rows = list(build_rows(Path(args.csv), args.limit))
    output_path = Path(args.out)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with output_path.open("w", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row, ensure_ascii=True) + "\n")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
