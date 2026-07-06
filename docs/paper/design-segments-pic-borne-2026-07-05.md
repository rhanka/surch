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

## ⚖️ ARBITRAGE FABLE 5 (rendu 2026-07-06) — doc_ids GLOBAUX STABLES, jamais renumérotés au merge
Fable tranche CONTRE la renumérotation Lucene-style : (1) les id_maps sont GLOBALES (FST uid→doc_id
immuable, state.rs:497) → renuméroter un tier de 300k déclencherait une reconstruction 28M (O(corpus)
par merge) ; (2) l'invariant « doc_id jamais réutilisé » est porteur de correction
(`deleted_since_dense`) ; (3) garder les ids rend le merge **quasi I/O-bound** : segments ADJACENTS
(runs consécutifs de doc_base, style LogMergePolicy) → concat déjà triée → **copie VERBATIM des blobs
FoR avec fixup du seul premier varint**, roaring=union, re-encode seulement les termes à tombstone ;
(4) coût des trous marginal (~R×70 B/slot ; INSEE append-only R≈0) ; (5) id_maps/source_store : zéro
changement. Stats du merge recalculées depuis les postings (JAMAIS depuis doc_len SmallFloat quantisé
→ dérive avg_doc_len → mort de l'oracle). Renumérotation reléguée à une « compaction d'époque »
explicite et rare (aussi la soupape de l'espace u32 — cf. C7). Les colonnes per-segment restent
indexées par `doc_id − doc_base` (compatible : plages contiguës).

## 🔴 Challenges Fable (C1-C7) — intégrés au plan
- **C1 — `densify()` est O(corpus) à CHAQUE refresh** (state.rs:1164, FST 28M + CSR + documents[]
  reconstruits, pic transitoire ~1-1,5 GiB) : le gate S4 « NRT ≥ ES » est intenable sans **densify par
  budget d'overlay** ; la vraie solution (id_maps per-segment paginées) est du travail S5 → S5 AVANT S4.
- **C2 — le vrai tueur NRT est `terms_finalized`** (state.rs:1034-1042) : le prochain append après
  refresh fait un rebuild COMPLET qui **reset le multi-segment en mono-segment**. S4 = SUPPRIMER ce
  flag au profit du seal S2 (un delete de code, pas une construction).
- **C3 — `prefix_postings` hors Segment = pic NON borné** (BTreeMap global, document_index.rs:153).
  **CONFIRMÉ actif dans nos benchs** : le mapping deces a `index_prefixes` sur DATE_NAISSANCE/DATE_DECES.
  Fix : router les préfixes comme champ synthétique `champ._index_prefix` dans le PostingsBuilder normal
  (budget/flush/merge gratuits). À faire avant S6.
- **C4 — concurrence merge** : S3 = merge SYNCHRONE inline (simple, gateable) ; background + carry-over
  deletes + generation-stamp seulement en S4 (quand les tombstones existent).
- **C5 — gate oracle post-CRUD à redéfinir** : df/doc_count incluent les tombstones jusqu'au merge (comme
  Lucene) pendant qu'ES merge à son rythme → divergence légitime. Gate CRUD = parité après force-merge
  des DEUX côtés (ou parité d'ensembles + tolérance score).
- **C6 — nos doc/s n'incluent AUCUN coût de merge** (ceux d'ES si) : fair-ab doit mesurer le wall-clock
  jusqu'à QUIESCENCE des merges + le PIC disque (2× le tier avant suppression des inputs).
- **C7 — contrat update change en S4** : update = tombstone + NOUVEL id (jamais-réutilisé) → à ~25k
  updates/s soutenu, u32 épuisé en ~2 jours → la compaction d'époque n'est pas cosmétique.

## 📋 ORDRE AMENDÉ (Fable) : S3 → S3.5 → S5 → S4 → S6
- **S3** : merge tiered inline sur runs adjacents (copie verbatim + fixup varint). Sans merge, 28M à
  budget 256 MiB = 100+ segments → fan-out FST × S tue la latence.
- **S3.5 (nouveau, ~1h)** : smoke 28M@16g dès S3 — mesure la courbe resident réelle vs l'extrapolation
  ~10-11 GiB (maladie B) et dimensionne S5. Ne pas attendre S6 pour toucher 28M.
- **S5 avant S4** : le disk-back des métadonnées est le lift existentiel (« un moteur qui existe à
  28M ») ; le corpus INSEE append-only n'a besoin ni de NRT ni de tombstones pour le gate survie 28M@4g ;
  un gate NRT fait avant S5 serait invalidé par S5.
- **S4 absorbe** : suppression `terms_finalized` (C2), tombstones + update=nouvel-id (C7), densify
  budgeté (C1), merge background + carry-over (C4), gate oracle CRUD redéfini (C5).
- **Transverse avant S6** : C3 (prefix synthétique) + C6 (fair-ab quiescence + pic disque).

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
