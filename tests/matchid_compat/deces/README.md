# INSEE deces slice fixture (B2)

Gap reference: `B2` in `docs/wp-d-matchid/gap-analysis.md` and
`docs/wp-d-matchid/SPEC.md`.

This directory provides the frozen INSEE-shaped fixtures that the
matchID compatibility replay (gap `B1`) and the INSEE 10k slice
acceptance test (gap `B2`) load into Surch.

## Files

- `mapping.json` — Elasticsearch-style `mappings` body derived from the
  `deces_index.yml` excerpt in
  `docs/wp-d-matchid/incoming/2026-05-15-deces-backend-dsl-inventory.md`
  (§2.12). Trimmed to mapping primitives Surch already supports
  (`text` with `index_prefixes`, `keyword`, `integer`). The richer
  matchID mapping (`edge_ngram` analyzer, custom `normalizer`,
  `geo_point`, `date{format}`, multi-fields like `NOM.raw` /
  `PRENOMS.first`) will be added when gaps `A1`, `A2`, `A7`, and
  `A13` finish landing.
- `slice-1000.ndjson` — 1 000 **synthetic** INSEE-shaped documents in
  `_bulk` NDJSON format (alternating action + source lines, stable
  `_id` values `deces_00001`..`deces_01000`). Drives the deterministic
  `deces_v1.json` replay — its `hits.total.value` and
  `hits.hits[0]._id` expectations are pinned to this byte stream.
- `slice-10000.ndjson.gz` — **real INSEE Open Licence 2.0 extract**,
  10 000 documents (first 10 000 rows of `Deces_2024.csv` from
  the INSEE "Fichier des personnes décédées" dataset). Loaded by
  `crates/surch-api/tests/matchid_insee_slice.rs` as the B2 v1
  acceptance fixture. Stored gzipped (gzip `-9 -n`, ~357 kB) to keep
  the checked-in artefact under 500 kB; the decoded NDJSON is
  ~1.8 MB / 20 000 lines.

## Provenance

### Synthetic slice (`slice-1000.ndjson`)

Documents are generated from a deterministic AWK seed (`srand(20260515)`)
over public-domain field-value pools — see
`tools/gen_deces_slice.awk`. No real INSEE record is present in this
file; it carries no PII and no Open Licence redistribution
constraint. It is byte-stable and remains the source of truth for the
B1 replay. The Elasticsearch 8.6.1 oracle gate now cross-checks this
same fixture for Track D B1 closure; a future `deces_v2` replay can
move frozen expectations onto the real INSEE slice when that scope is
opened.

### Real INSEE slice (`slice-10000.ndjson.gz`)

- **Source**: INSEE — "Fichier des personnes décédées",
  <https://www.insee.fr/fr/information/4769950> (page produit).
  Specifically the yearly extract `Deces_2024.zip` → `Deces_2024.csv`
  (CSV, semicolon-separated, ~60 MB unzipped, ~660 k rows).
- **Capture date**: 2026-05-15 (cached at
  `target/insee/Deces_2024.csv` by the wp-b session, byte-identical
  to the INSEE-published zip from early 2025).
- **Slice rule**: first 10 000 rows of the CSV in source order
  (skipping the header row). No shuffling, no sampling — this keeps
  the slice cheap to reproduce from the upstream CSV.
- **Licence**: INSEE Open Licence 2.0 (Etalab) — see
  <https://www.etalab.gouv.fr/licence-ouverte-open-licence>.
  Attribution: "Institut national de la statistique et des études
  économiques (INSEE) — Fichier des personnes décédées 2024".
- **Checksum** (gzip `-9 -n`, byte-stable):
  `sha256: 1f71d52c554900fbfb055be75ddff4fc04bb891cbce8725295f5fd7e68eace02`
  (10 000 docs / 20 000 NDJSON lines / 357 138 bytes compressed).

### Shape mapping (INSEE CSV → matchID NDJSON)

| INSEE CSV column | matchID NDJSON field         | Notes                                          |
|------------------|------------------------------|------------------------------------------------|
| `nomprenom`      | `NOM`, `PRENOMS`             | Split on `*`, trailing `/` stripped.           |
| `sexe`           | `SEXE`                       | INSEE `1`→`M`, `2`→`F` (matchID convention).   |
| `datenaiss`      | `DATE_NAISSANCE`             | `yyyyMMdd` keyword (A7 will widen to `date`).  |
| `lieunaiss`      | `CODE_INSEE_NAISSANCE`       | INSEE 5-char commune code.                     |
| `commnaiss`      | `COMMUNE_NAISSANCE`          | Verbatim INSEE commune name.                   |
| `paysnaiss`      | (dropped)                    | Not in `mapping.json` v0 — added when A2 (`geo_point`) lands. |
| `datedeces`      | `DATE_DECES`                 | `yyyyMMdd` keyword.                            |
| `lieudeces`      | `COMMUNE_DECES`              | **INSEE commune code** (no COG name lookup table is bundled — the matchID `COMMUNE_DECES` text value will be backfilled from `commune_2024.csv` once the wp-c COG loader lands). |
| `actedeces`      | (dropped)                    | Not in `mapping.json` v0.                      |
| (synthesised)    | `SOURCE`                     | Constant `"INSEE"`.                            |
| (synthesised)    | `SOURCE_LINE`                | 1-based row index (matches the future B2 v2 cross-replay manifest). |

The `_id` namespace is `ins_NNNNNNN` (7-digit zero-padded counter)
to keep this fixture disjoint from the synthetic `deces_NNNNN`
namespace used by `slice-1000.ndjson` — the B1 replay can therefore
target either slice without id collisions.

## Regenerating the fixtures

### Synthetic slice

```bash
awk -f tools/gen_deces_slice.awk </dev/null \
    > tests/matchid_compat/deces/slice-1000.ndjson
```

The AWK source is `tools/gen_deces_slice.awk`. Changing the seed,
field-value pools, document count, or AWK locale produces a different
byte stream; any regeneration must be accompanied by an update of the
expected ids in `tests/matchid_compat/replays/deces_v1.json`.

### Real INSEE slice

```bash
# 1. Download the INSEE extract (one-shot; ~18 MB zip → ~60 MB CSV).
#    The URL is the INSEE product page; the year and the actual
#    download link rotate, so this is intentionally manual.
#    See https://www.insee.fr/fr/information/4769950
#
#    The expected file is `target/insee/Deces_2024.csv`.

# 2. Build the 10k matchID-shaped slice.
tools/fetch-insee-slice.sh
#   → tests/matchid_compat/deces/slice-10000.ndjson.gz
```

`tools/fetch-insee-slice.sh` is the B2 v1 fetcher: it reads the cached
CSV, applies the shape mapping above, takes the first `LIMIT=10000`
rows, and writes a byte-stable `gzip -9 -n` artefact. The script does
**not** auto-download the INSEE zip (URLs rotate); the CSV must be
pre-populated under `target/insee/`.

## Size budget

- `slice-1000.ndjson` (synthetic, plaintext): ~270 kB / 1 000 docs.
- `slice-10000.ndjson.gz` (real, gzip `-9 -n`): ~357 kB / 10 000 docs.
  Decoded: ~1.8 MB.

Both stay well under the 1 MB checked-in budget. A future B2 v2 (50k+
docs, full month of INSEE deaths) will need either Git LFS or an
out-of-tree fetch (GitHub release asset) — the
`tools/fetch-insee-slice.sh` `LIMIT` parameter already supports
larger slices for ad-hoc benchmarking.

## Licence

The Surch sources in this directory (`mapping.json`, `README.md`,
`slice-1000.ndjson`, the AWK generator, the fetcher) are released
under the same licence as the rest of Surch (`Apache-2.0` — see
repository root).

`slice-10000.ndjson.gz` is a derivative of the INSEE "Fichier des
personnes décédées 2024" published under the **INSEE Open Licence 2.0
(Etalab)**. Attribution is required when redistributing the slice;
see the "Provenance — Real INSEE slice" section above.

## Future expansion

When matchID publishes the promised 10k-row sample from
`deces-2020-m01.txt.gz` (see SPEC §4.2 and DSL inventory §7), this
directory will host a `slice-matchid-deces-2020-m01.ndjson.gz`
fixture next to the current INSEE 2024 slice. The mapping will then
need:

- `NOM.raw` keyword sub-field with the custom `norm` normalizer
  (gap `A13`).
- `PRENOMS.first` text sub-field with the same analyzer.
- `DATE_NAISSANCE` / `DATE_DECES` typed as `date` with
  `format: yyyyMMdd` (gap `A7`).
- `GEOPOINT_NAISSANCE` typed as `geo_point` (gap `A2`).
- `PRENOMS` enriched with `index_prefixes` already shipped in A6
  phase 2 (visible in the current `mapping.json`).
