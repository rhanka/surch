# deces_v1 OpenSearch Oracle Gate

This runbook turns the committed `deces_v1` replay into an explicit
OpenSearch / Elasticsearch 7.x gate. It is intentionally human-readable:
the run produces `summary.md` and exits non-zero on any mismatch, so the
user does not need to inspect raw JSON.

## Inputs

- Replay: `tests/matchid_compat/replays/deces_v1.json`
- Mapping: `tests/matchid_compat/deces/mapping.json`
- Bulk slice: `tests/matchid_compat/deces/slice-1000.ndjson`
- Index: `deces`

## Required Comparison

For every non-skipped replay request, compare:

- HTTP `status`
- `hits.total.value`
- `hits.hits[0]._id`
- critical shape:
  - normal search responses must contain `hits.total.value`
  - normal search responses with an expected top hit must contain
    `hits.hits[0]._id`
  - `_msearch` responses must contain `responses[]`; compare the sum of
    each sub-response `hits.total.value` and the first sub-response top id
  - scroll-opening responses with `?scroll=` must contain a non-empty
    `_scroll_id`

Volatile fields such as `took`, shard counts, and scores are not part of
this first gate. The gate is actionably scoped to status, totals, top-hit
identity, and response shape.

## External Gate

Prerequisites:

- A clean OpenSearch or Elasticsearch 7.x node is running.
- `OPENSEARCH_URL` points at that node, for example
  `http://127.0.0.1:9200`.
- Python 3 is available.

Run from the repository root:

```sh
OPENSEARCH_URL="${OPENSEARCH_URL:-http://127.0.0.1:9200}" python3 - <<'PY'
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(".")
INDEX = "deces"
REPLAY = ROOT / "tests/matchid_compat/replays/deces_v1.json"
MAPPING = ROOT / "tests/matchid_compat/deces/mapping.json"
BULK = ROOT / "tests/matchid_compat/deces/slice-1000.ndjson"
OUT_DIR = ROOT / "target/matchid-oracle/deces_v1"
BASE = os.environ.get("OPENSEARCH_URL", "http://127.0.0.1:9200").rstrip("/")


def request(method, path, body=None):
    data = None
    headers = {"content-type": "application/json"}
    if body is not None:
        data = body.encode("utf-8")
    req = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
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


def body_for(entry):
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


def extract(entry, response):
    if response is None:
        return None, None, ["empty response body"]

    path = entry["request"]["path"]
    errors = []
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


def main():
    manifest = json.loads(REPLAY.read_text())
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    request("DELETE", f"/{INDEX}")
    status, body = request("PUT", f"/{INDEX}", MAPPING.read_text())
    if status not in (200, 201):
        raise RuntimeError(f"create index failed: status={status} body={body}")
    status, body = request("POST", "/_bulk", BULK.read_text())
    if status != 200 or (body or {}).get("errors"):
        raise RuntimeError(f"bulk load failed: status={status} body={body}")
    status, body = request("POST", f"/{INDEX}/_refresh", "")
    if status != 200:
        raise RuntimeError(f"refresh failed: status={status} body={body}")

    rows = []
    failures = []
    for entry in manifest["requests"]:
        if entry.get("skip"):
            rows.append([entry["name"], "SKIP", entry["skip"], "", "", ""])
            continue
        status, response = request(
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
        rows.append([
            entry["name"],
            verdict,
            str(status),
            str(total),
            str(top_id),
            "; ".join(mismatches),
        ])
        if mismatches:
            failures.append(entry["name"])

    summary = OUT_DIR / "summary.md"
    with summary.open("w", encoding="utf-8") as out:
        out.write("# deces_v1 OpenSearch Oracle Summary\n\n")
        out.write(f"OpenSearch URL: `{BASE}`\n\n")
        out.write("| request | verdict | status | hits.total.value | top id | notes |\n")
        out.write("|---|---:|---:|---:|---|---|\n")
        for row in rows:
            out.write("| " + " | ".join(cell.replace("|", "\\|") for cell in row) + " |\n")
        out.write("\n")
        if failures:
            out.write(f"Verdict: FAIL ({len(failures)} mismatch(es))\n")
        else:
            out.write("Verdict: PASS\n")

    print(f"Wrote {summary}")
    if failures:
        print("Failing requests: " + ", ".join(failures), file=sys.stderr)
        return 1
    return 0


raise SystemExit(main())
PY
```

Expected human artifact:

```text
target/matchid-oracle/deces_v1/summary.md
```

The committed replay remains the source of request bodies. The generated
`summary.md` is the review surface: it shows one row per request and a
PASS/FAIL verdict, including the exact mismatch when status,
`hits.total.value`, top id, or critical shape diverges.

## Replay Request Coverage

- `adv_nom_prenoms_date`
- `adv_nom_commune_sexe`
- `adv_nom_prenoms_commune`
- `adv_fuzzy_nom_with_match`
- `adv_fuzzy_auto_prenoms_sexe`
- `adv_bool_should_msm`
- `adv_match_filter_commune`
- `adv_bool_must_not_sexe`
- `block_msearch_a`
- `block_msearch_b`
- `block_msearch_c`
- `block_msearch_d`
- `block_msearch_e`
- `ft_multi_match_nom_prenoms`
- `ft_match_simple_nom`
- `ft_multi_match_from_size`
- `ft_match_min_score`
- `sort_date_naissance_asc`
- `sort_nom_desc`
- `sort_match_date_deces_desc`
- `fs_match_wrap`
- `fs_bool_wrap`
- `prefix_nom`
- `prefix_prenoms`
- `prefix_date_naissance_short`
- `scroll_full`
- `scroll_filtered_size_500`
- `scroll_match_all_size_1000`
- `range_date_naissance_window`
- `range_date_naissance_lte_open`
