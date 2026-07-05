# Design — segments immuables à pic ET resident bornés (battre ES à mémoire constante)

> 2026-07-05 — consensus **Codex GPT-5.5 @ xhigh + Opus 4.8 @ max** (Fable 5 en arbitre au reset
> quota — 1 divergence à trancher, voir §Divergence). Répond au verdict 28M mesuré
> (`local-fair-ab-2026-07-04.md`) : ES indexe le full 28,9M sous 4 GiB, Surch OOM jusqu'à 16 GiB.

## Diagnostic racine — DEUX maladies (Opus, ancré code)
- **Maladie A — pic transitoire du build** : `PostingsBuilder` (BTreeMap<field,BTreeMap<term,Vec<Posting>>>,
  postings.rs:415) accumule TOUT le corpus, puis `build_with_disk_flag` (postings.rs:503) matérialise
  FST+FoR+roaring+directory en une passe. `rebuild_index` re-tokenise tout à chaque update/delete.
  Mesuré : ~1,4M docs/GiB avant OOM ; pic 3264 MiB à 1,36M.
- **Maladie B — resident O(corpus) même postings-sur-disque** : après C1b/C2, allocated live = 532 MiB
  à 1,36M, TOUT linéaire (FST par champ, block_directory, offsets, roaring, doc_len_dense, SubfieldColumn,
  id_maps). Extrapolation ×21 → **~10-11 GiB resident à 28M avec postings déjà sur disque**. Des segments
  qui ne bornent que le build transforment l'OOM en steady ~10 GiB : **il faut AUSSI disk-backer les
  métadonnées per-segment** (le vrai lift 28M, ce que fait Lucene).

## Architecture retenue (convergence des 2 panels)
**Segments immuables Lucene-like** : `DocumentIndex` détient `Vec<Arc<Segment>>` scellés + `active_builder`.
- **Scellement** : `build_with_disk_flag` actuel EST déjà le scelleur d'un segment (FST+FoR+directory) —
  le découpage se fait au-dessus : flush quand `postings_builder.memory_bytes() ≥ BUDGET_FLUSH`
  (~128-256 MiB) OU au `_refresh`. Brique Q6 retenue : `fst::MapBuilder` → `Writer` fichier (streaming).
- **Read-path multi-segment** : curseurs leapfrog/roaring PER-SEGMENT, arène par requête per-segment
  (existe déjà), top-K fusionné. **Stats BM25 GLOBALES agrégées** (doc_count/avg_doc_len/df sommés sur
  les S segments, idf global injecté dans Bm25TermScorer) — **non négociable pour la parité oracle**.
- **Merge tiered STREAMING** (paliers ×10, fan-in ~4-10) : k-way merge des FST (Map::stream), concat FoR
  avec remap, fusion doc_len/subfields, application des tombstones. Streaming obligatoire (sinon on
  recrée la maladie A au merge). S final borné ~10-30 segments.
- **Delete/update = tombstones** (`live_docs` per-segment) + réclamation au merge — suppression du
  `rebuild_index` complet.
- **Refresh NRT = sceller le buffer actif SEUL** (O(buffer), pas O(corpus)) → NRT passe de 5,9k à ~débit
  bulk (≥ES). **L'axe de gain le plus net et le plus sûr.**
- **S5 (le lift 28M)** : disk-back per-segment des métadonnées — FST mmap/pread, block_directory,
  doc_len_dense, id_maps paginés ; working-set resident borné ; roaring peut rester resident (hot path
  bool/full) = l'arbitrage RAM/latence.
- Tout derrière **flag `SURCH_SEGMENTS`** (off = moteur mono-segment actuel bit-identique).

## ⚖️ Divergence à arbitrer (Fable 5, au reset quota)
**doc_ids : LOCAUX per-segment + doc_base (Opus) vs GLOBAUX (Codex, défaut).** L'argument Opus est
ancré : `doc_len_dense`/`SubfieldColumn.codes` sont dimensionnés au max doc_id global
(document_index.rs:250,455) → avec des IDs globaux, un segment tardif allouerait des colonnes de 28M
entrées (maladie B aggravée) ; locaux = colonnes de seg_size + meilleure compression FoR (deltas petits).
Coût : remap au merge + table de routage `doc_base[]`. Position par défaut en attendant Fable : **locaux**.

## Plan d'exécution (chaque étape commit-able, gatée `deploy/bench-local/fair-ab.sh`)
| # | Périmètre | Risque | Gate | Réversibilité |
|---|---|---|---|---|
| **S0** | **Bulk PAR-ITEM** (fix perte de données) : `parse_bulk_ndjson` (bulk.rs:461) tout-ou-rien → résilient par-item (l'infra de réponse partielle existe, bulk.rs:185-214) | Faible | count complet 659k + oracle | revert 1 commit |
| S1 | `Vec<Segment>` + read-path fusion (idf global), **1 segment** au départ (parité comportementale) | Moyen | oracle 0 div ; latence ≤ actuel | flag off |
| S2 | Flush par budget + doc_ids locaux/doc_base + MapBuilder→Writer | Moyen | pic RSS borné à 1,36M ; count ; oracle | flag off |
| S3 | Merge tiered streaming + tombstones | Élevé | S borné ; oracle post-merge ; disque récupéré | merge off (dégradé, pas cassé) |
| S4 | Refresh NRT (buffer seul) + delete/update tombstone | Moyen | **NRT ≥ ES** (REFRESH_EACH=1) ; oracle CRUD | fallback rebuild flag |
| S5 | Disk-back métadonnées per-segment (maladie B) | Élevé | **survie 28M @4g puis 2g** ; warm<ES ; cold<2×warm | par structure |
| S6 | 28M end-to-end : sweep mémoire, warm+cold, disque | — | plancher de survie vs ES | — |

## Cibles honnêtes par axe (28,9M, mémoire constante) — consensus des 2 panels
- **RAM** : **survie ≤ plancher ES** (inverse le verdict actuel) ; ≤ES/2 non promis au 1er jet (dépend S5).
- **Latence** : **< ES conservable ; ~2× warm few-segments** ; le 3,3× mono-segment est abandonné
  honnêtement ; cold à surveiller (WILLNEED + merge agressif).
- **Indexation bulk** : parité (write-amp merge ~0,8-1,0×) — pas 2×.
- **Indexation NRT** : **≥ ES** — gain net (aujourd'hui 6× pire).
- **Disque** : ~1,1-1,4× — pas ≤0,5× sans compression `_source`.
- **Parité** : 0 divergence préservé SSI idf global — gate sacré à chaque étape.

**Le pari** : pas « ≥2× partout » (démenti par la mesure) mais **un moteur qui EXISTE à 28M sous budget
ES** — survie ≤ES, latence < ES, NRT gagné franchement — au lieu d'un moteur qui meurt à l'échelle.
