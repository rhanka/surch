# SPEC - MatchID Elastic Parity Corpus Format

## Purpose

Define the frozen corpus and capture formats used to compare Elasticsearch and Surch in the MatchID context.

## Core Principle

The corpus stores normalized OpenSearch-like requests derived from MatchID usage.

It does not need to store MatchID UI payloads directly if those payloads are already compiled into canonical Elasticsearch requests.

## Corpus File Format

The corpus file is JSONL, one case per line.

Each line must follow this shape:

```json
{
  "case_id": "golden-term-published",
  "family": "golden",
  "request": {
    "method": "POST",
    "path": "/books/_search",
    "json": {
      "query": { "term": { "status": "published" } },
      "size": 10
    }
  },
  "assertion": {
    "top_k": 10,
    "compare_total": true,
    "compare_ids": true,
    "compare_source_fields": ["title", "status", "year"]
  },
  "notes": "Derived from MatchID usage pattern X"
}
```

## Required Fields

- `case_id`: unique stable identifier
- `family`: corpus family such as `golden`, `representative`, or `adversarial`
- `request.method`: currently `GET` or `POST`
- `request.path`: OpenSearch-compatible path
- `request.json`: canonical JSON request body
- `assertion.top_k`: maximum hit count considered for comparison

## Optional Assertion Fields

- `compare_total`: default `true`
- `compare_ids`: default `true`
- `compare_source_fields`: source-field subset to compare

## Capture File Format

Replay output is JSONL, one line per case:

```json
{
  "case_id": "golden-term-published",
  "http_status": 200,
  "response": {
    "hits": {
      "total": { "value": 1, "relation": "eq" },
      "hits": [
        {
          "_id": "1",
          "_score": 1.0,
          "_source": {
            "title": "Rust Search",
            "status": "published",
            "year": 2024
          }
        }
      ]
    }
  }
}
```

## Comparison Rules

Default zero-gap comparison checks:

- same HTTP status
- same `hits.total.value` when `compare_total=true`
- same ordered `_id` sequence for the first `top_k` hits when `compare_ids=true`
- same selected `_source` fields for compared hits when `compare_source_fields` is set

## Diff Categories

The comparator must classify differences as:

- `status_mismatch`
- `total_mismatch`
- `hit_count_mismatch`
- `id_order_mismatch`
- `source_field_mismatch`
- `missing_case`

## Exit Rule

The comparator exits with:

- `0` when no diffs exist
- `1` when at least one diff exists

This allows the harness to act as a release gate.
