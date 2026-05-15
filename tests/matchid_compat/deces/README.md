# INSEE deces slice fixture (B2)

Gap reference: `B2` in `docs/wp-d-matchid/gap-analysis.md` and
`docs/wp-d-matchid/SPEC.md`.

This directory provides the frozen INSEE-shaped fixture that the
matchID compatibility replay (gap `B1`) loads into Surch.

## Files

- `mapping.json` — ES-7.x style `mappings` body derived from the
  `deces_index.yml` excerpt in
  `docs/wp-d-matchid/incoming/2026-05-15-deces-backend-dsl-inventory.md`
  (§2.12). Trimmed to mapping primitives Surch already supports
  (`text`, `keyword`, `integer`). The richer matchID mapping
  (`edge_ngram` analyzer, custom `normalizer`, `index_prefixes`,
  `geo_point`, `date{format}`, multi-fields like `NOM.raw` /
  `PRENOMS.first`) will be added when gaps `A1`, `A2`, `A6`, `A7`, and
  `A13` land.
- `slice-1000.ndjson` — 1000 synthetic INSEE-shaped documents in
  `_bulk` NDJSON format (alternating action + source lines, stable
  `_id` values `deces_00001`..`deces_01000`).

## Provenance

The slice is **synthetic** (no real INSEE record is committed).
Documents are generated from a deterministic AWK seed (`srand(20260515)`)
over public-domain field-value pools:

- `NOM` / `PRENOMS` — common French surnames and given names from
  public INSEE name tables.
- `COMMUNE_NAISSANCE` / `COMMUNE_DECES` — French commune names from
  public INSEE-Code-Officiel-Geographique.
- `CODE_INSEE_NAISSANCE` — INSEE municipality codes paired with the
  commune.
- `DATE_NAISSANCE` / `DATE_DECES` — `yyyyMMdd` strings; year of birth
  in `[1900, 1960]`, year of death in `[birth + 30, min(birth + 95,
  2025)]`. Stored as `keyword` until gap `A7` lands.
- `SEXE` — `M` or `F`.
- `SOURCE` — `INSEE` or `INSEE-OFFICIEL`.
- `SOURCE_LINE` — 1-based row index.

Because the data is synthetic, no PII is present and there is no
INSEE Open License redistribution constraint. The seed is fixed so
the slice is byte-stable.

## Licence

Files in this directory are released under the same licence as the
rest of Surch (`Apache-2.0` — see repository root). When the slice
is later derived from a real INSEE `deces-YYYY-mMM.txt.gz` export,
the README must be updated to reference the **INSEE Open Licence
2.0** (Etalab) and the source file path.

## Size budget

Slice size target: < 1 MB uncompressed. Current size:
~270 kB (1000 documents). If a future expansion crosses 1 MB,
either compress with `gzip` (the loader can be extended) or shrink
to 100 documents and re-record expected hit ids in `replays/`.

## Regenerating the slice

The generator is intentionally a single AWK script (no Python by
project rule):

```bash
awk -f tools/gen_deces_slice.awk </dev/null \
    > tests/matchid_compat/deces/slice-1000.ndjson
```

The exact AWK source used to produce the current committed slice is
preserved under `tools/gen_deces_slice.awk` next to the fixture.
Changing the seed, the field-value pools, the document count, or
the AWK locale produces a different byte stream — any regeneration
must be accompanied by an update of the expected ids in
`tests/matchid_compat/replays/deces_v1.json`.

## Future expansion

When matchID publishes the promised 10k-row sample from
`deces-2020-m01.txt.gz` (see SPEC §4.2 and DSL inventory §7), this
slice will be replaced or extended. The mapping will then need:

- `NOM.raw` keyword sub-field with the custom `norm` normalizer
  (gap `A13`).
- `PRENOMS.first` text sub-field with the same analyzer.
- `DATE_NAISSANCE` / `DATE_DECES` typed as `date` with
  `format: yyyyMMdd` (gap `A7`).
- `GEOPOINT_NAISSANCE` typed as `geo_point` (gap `A2`).
- `PRENOMS` enriched with the `index_prefixes` mapping option
  (gap `A6`).
