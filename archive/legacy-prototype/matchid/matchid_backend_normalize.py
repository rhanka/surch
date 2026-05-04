#!/usr/bin/env python3

import argparse
import json
from pathlib import Path


def load_jsonl(path: Path):
    with path.open("r", encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def normalize_backend_capture(row):
    captures = row["captures"]
    preferred = captures.get("post") or captures.get("get")
    response = preferred.get("response", {})
    body = response.get("response", response)
    persons = body.get("persons", [])

    return {
        "case_id": row["case_id"],
        "http_status": preferred.get("http_status", 599),
        "response": {
            "hits": {
                "total": {
                    "value": body.get("total", 0),
                    "relation": "eq",
                },
                "hits": [
                    {
                        "_id": person.get("id"),
                        "_source": {
                            "UID": person.get("id"),
                            "NOM": person.get("name", {}).get("last"),
                            "PRENOM": (person.get("name", {}).get("first") or [None])[0],
                            "PRENOMS": " ".join(person.get("name", {}).get("first", [])),
                            "DATE_NAISSANCE": person.get("birth", {}).get("date"),
                            "COMMUNE_NAISSANCE": first_or_self(
                                person.get("birth", {}).get("location", {}).get("city")
                            ),
                            "PAYS_NAISSANCE": person.get("birth", {}).get("location", {}).get("country"),
                            "SOURCE": person.get("source"),
                        },
                    }
                    for person in persons
                ],
            }
        },
    }


def first_or_self(value):
    if isinstance(value, list):
        return value[0] if value else None
    return value


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--in", dest="input_path", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args(argv)

    rows = load_jsonl(Path(args.input_path))
    normalized = [normalize_backend_capture(row) for row in rows]

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with out_path.open("w", encoding="utf-8") as handle:
        for row in normalized:
            handle.write(json.dumps(row, ensure_ascii=True) + "\n")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
