# MatchID Compatibility Fixtures

This directory is reserved for sanitized MatchID Elasticsearch/OpenSearch replay fixtures.
Do not commit production secrets, customer data, raw identifiers, or unredacted payloads.

## Required Fixture Shape

A useful MatchID replay needs:

- index creation body, including mappings/settings actually used by MatchID;
- representative bulk NDJSON payloads with stable synthetic IDs;
- `_search`, `_msearch`, `_count`, and `_mget` requests from real traffic classes;
- expected Elasticsearch/OpenSearch responses with volatile fields normalized;
- a short note describing which MatchID workflow each request represents.

## Redaction Rules

- Replace names, emails, phone numbers, addresses, tokens, and organization identifiers.
- Preserve field names, field types, analyzer-relevant token shapes, cardinality, nullability,
  arrays, nested/object structure, and sort/filter distributions.
- Preserve query operators and request options exactly unless they contain sensitive values.
- Use stable synthetic IDs so top-hit and ordering assertions remain meaningful.

## Go/No-Go Use

These fixtures are the compatibility gate for MatchID shadow UAT. Surch can be considered for
shadow-read testing only when every critical replay either passes or has a documented, accepted
delta.
