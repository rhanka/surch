#!/usr/bin/env python3

import argparse
import json
from pathlib import Path


def load_jsonl(path: Path):
    with path.open("r", encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def build_docs(rows):
    docs = []
    seen = set()
    for row in rows:
        for hit in row.get("response", {}).get("hits", {}).get("hits", []):
            source = hit.get("_source", {})
            doc_id = hit.get("_id")
            if not doc_id or doc_id in seen:
                continue
            seen.add(doc_id)
            docs.append(
                {
                    "_id": doc_id,
                    "document": {
                        "UID": source.get("UID"),
                        "NOM": source.get("NOM"),
                        "PRENOM": source.get("PRENOM"),
                        "PRENOMS": source.get("PRENOMS"),
                        "DATE_NAISSANCE": source.get("DATE_NAISSANCE"),
                        "DATE_NAISSANCE.raw": source.get("DATE_NAISSANCE"),
                        "COMMUNE_NAISSANCE": source.get("COMMUNE_NAISSANCE"),
                        "COMMUNE_NAISSANCE.raw": source.get("COMMUNE_NAISSANCE"),
                        "PAYS_NAISSANCE": source.get("PAYS_NAISSANCE"),
                        "PAYS_NAISSANCE.raw": source.get("PAYS_NAISSANCE"),
                        "SOURCE": source.get("SOURCE"),
                    },
                }
            )
    return docs


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args(argv)

    rows = load_jsonl(Path(args.baseline))
    docs = build_docs(rows)

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with out_path.open("w", encoding="utf-8") as handle:
        for row in docs:
            handle.write(json.dumps(row, ensure_ascii=True) + "\n")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
