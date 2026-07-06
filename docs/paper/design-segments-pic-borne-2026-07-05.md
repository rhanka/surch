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

## ✅ S2 MERGÉ + GATE DÉCISIF PASS (2026-07-06, sha 76674ed) — LA MALADIE A EST CASSÉE
- CI verte (dont test intégration parité multi==mono, scores BM25 identiques sur 7 requêtes).
- Oracle-local **0 divergence en multi-segment forcé** (budget 4 MiB / 10k docs).
- **Gate décisif : 1,36M docs riches @1536m avec budget 256M — SURVIT** (pré-S2 : OOM à 1536m ET 2g,
  plancher 3g) : count complet, **RSS 618 MiB**, 24 733 doc/s (≈ pré-S2), latence 0,36/0,50/0,78 ms
  (multi-segment ≈ mono). À mémoire constante 1536m : ES = 18 486 doc/s, RSS 1341 MiB, lat 1,64/2,92/3,46
  → **Surch y bat ES sur RAM (0,46×), débit (1,34×) et latence (~4-5×)**, disque 1,12× pire.
- Le plancher surch à 1,36M passe de 3g à **]1g, 1536m]** (sweep : @1g OOM à ~910k docs, @768m à ~300k,
  @512m à ~90k — docs-avant-mort ∝ cap). **= le plancher d'ES (1536m) : parité de survie à 1,36M.**
- Lecture du sweep : le pic du BUILDER est borné (mission S2 accomplie), mais le **résident linéaire
  par-corpus demeure** (~0,44 KB/doc : métadonnées des segments scellés — FST, directories, subfields,
  doc_len — + id_maps globales) = la **maladie B**. Extrapolé 28M ≈ ~12,6 GiB resident → confirme S5
  comme lift existentiel ; le smoke S3.5 (28M@16g) devrait passer SANS S5 (12,6 < 16g) — à mesurer.

## 🏆 S3 MERGÉ + S3.5 SMOKE PASS (2026-07-06, sha ac3f12a) — SURCH SERT LE FULL 28,9M (1re fois)
- S3 CI verte 1er coup (parité 3-voies mono/multi/mergé) ; oracle-local merges actifs 0 divergence.
- **SMOKE 28M@16g (budget 256M, fanin 8) : count COMPLET 28 917 511**, 22 388 doc/s (parité ES 22 719 —
  et NOS doc/s incluent les merges inline, per C6), **RSS 6,95 GiB vs ES 10,26 (0,68×)**, latence
  **0,38/0,49/0,57 ms vs ES 0,91/1,41/2,03 (~2,5-3,5×)**, disque 15,4 vs 11,6 GiB (1,33× — write-amp).
- Resident réel BIEN sous l'extrapolation maladie B (6,95 mesuré vs ~12,6 projeté) : colonnes locales
  doc_base + FoR page-cache évictable font mieux que prévu. Sweep plancher 28M (8g/4g) pour le verdict
  mémoire-constante complet ; S5 reste pertinent pour viser ≤4g (plancher ES).

## 📐 Dimensionnement S5 (ventilation mesurée 1,36M segments+merge @1536m, /_prometheus_metrics)
Plancher 28M mesuré : **@8g OOM à 21,9M docs (75%), @4g à 8,7M (30%)** → plancher Surch 28M = ]8g,16g]
vs ES ≤4g. Résiduel anon ~0,4 KB/doc. Ventilation à 1,36M : jemalloc allocated **555 MiB** = subfields 80
+ postings résiduel (directory/CSR/descriptors) 57 + FST 49 + roaring 13 + field_stats 9 + gauges état
(id_maps 20 + documents_overhead 31) + **~295 MiB NON GAUGÉS** (DenseIdMaps réelles, SourceBlob handles,
overlays, frag) — LE plus gros poste, ×21 ≈ ~6 GiB à 28M. Ordre S5 : (a) gauger/identifier le non-compté,
(b) disk-back par taille : le non-gaugé dominant + subfields + directory/CSR + FST (mmap/pread per-segment).
Roaring reste résident (hot path). Cible : plancher 28M ≤4g (= ES).

## ✅ S5a MERGÉ + MESURÉ (sha af65940) — gap identifié et 1re tranche livrée
Gap ~295 MiB = les **5 tableaux par-terme** (T≈10-15M termes distincts, edge_ngram/prefixes du mapping).
Disk-back de `segment_descriptors` : **allocated 555 → 447 MiB (−108)**, resident 675 → 567. Nouvelle
gauge `postings_directory_bytes` = **148 MiB** restants (offsets/block_offsets/block_directory/
block_dir_offsets). Comptabilité ≥90% : 148 dir + 80 subfields + 57 postings + 49 fst + 31 docs_ovh +
20 id_maps + 13 roaring + 9 stats = 407/447. **Projection S5b+c** (disk-back directory 148 + subfields 80
+ fst 49) : allocated ~170 MiB à 1,36M → **~3,6 GiB à 28M → plancher ≤4g = parité plancher ES au full**.

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

## 🔍 S5a — identification code du non-gaugé + 1er disk-back (`segment_descriptors`)
**Volet 1 (identifier)** : audit code (pas de run — analyse statique + calcul par structure).
- **SourceBlob écarté** : `compact_after_refresh` (state.rs) n'est PLUS appelée depuis
  `finalize_terms_for_refresh` (mmap M1 neutralise Option B) — les blobs `_source` restent `OnDisk
  {offset,len}` en régime permanent, `payload_len()==0`. Zéro octet anon. L'hypothèse « SourceBlob
  compressés résidents » du brief est donc **réfutée par le code actuel**.
- **DenseIdMaps déjà comptée en entier** : `AppState::index_state_memory_bytes` (state.rs) somme
  `reverse_uids`+`reverse_offsets`+FST `forward` (octets réels via `as_fst().as_bytes().len()`) +
  les 4 overlays dirty, et `dense.documents: Box<[Option<SourceBlob>]>` via
  `size_of::<Option<SourceBlob>>()` exact. `prefix_postings_bytes` est déjà gaugée aussi
  (`surch_index_prefix_postings_bytes`). Aucun de ces postes n'explique le gap.
- **Poste identifié (grep-certifié jamais sommé nulle part)** : dans `FieldPostings`
  (postings.rs), 5 tableaux CSR/annuaire par-TERME — `offsets`, `block_offsets`,
  `segment_descriptors` (16 o/terme après padding Rust), `block_directory`, `block_dir_offsets` —
  scalent avec **T = nombre de termes DISTINCTS**, pas avec le nombre de docs. Un champ
  `edge_ngram`/`autocomplete`/`index_prefixes` (le mapping matchID `deces_index.yml` en a, 28
  champs, norm/edge_ngram/.raw/dates) peut pousser T à plusieurs millions même à 1,36M docs.
  Recoupement : `fst_bytes` (49 MiB, COMPRESSÉ) suggère T de l'ordre de 10-15M si le ratio de
  compression FST est de quelques octets/terme sur cet automate peu partagé (28 champs
  hétérogènes) — × ~24-28 o/terme NON compressés pour le quintette ci-dessus ≈ 240-350 MiB,
  l'ordre de grandeur exact du gap ~295 MiB. **Tableau (à 1,36M, chiffré par structure) :**

  | Poste | Structure | Octet/terme (ou /doc) | Gaugé avant S5a | Contribution estimée |
  |---|---|---|---|---|
  | subfields | `SubfieldColumn` (dict+codes) | ~/doc | oui (`subfield_values_bytes`) | 80 MiB |
  | FST | `fst::Map` par champ (compressé) | ~qq o/terme | oui (`fst_bytes`) | 49 MiB |
  | roaring | bitmaps df>4096 | ~/doc chaud | oui (`roaring_bytes`) | 13 MiB |
  | field_stats | `doc_len_dense` 1 o/doc | /doc | oui (`field_stats_bytes`) | 9 MiB |
  | id_maps | FST uid→doc_id + reverse CSR + overlays | /doc | oui (`surch_state_id_maps_bytes`) | 20 MiB |
  | documents_overhead | slot `Option<SourceBlob>` + overlay | /doc | oui (`surch_state_documents_overhead_bytes`) | 31 MiB |
  | **postings_directory (NOUVEAU)** | `offsets`+`block_offsets`+`segment_descriptors`+`block_directory`+`block_dir_offsets` | **~24-28 o/terme + 12 o/bloc** | **NON — jamais sommé (grep confirmé)** | **~295 MiB (le gap entier)** |
  | **Total** | | | | **≈ 555 MiB** |

  Chiffres FST/subfields/roaring/etc. repris du scrape mesuré (commit précédent) ; la ligne
  `postings_directory` est une estimation par calcul de structure (T inconnu précisément sans
  re-mesure) — **la nouvelle gauge ci-dessous donnera le chiffre exact au prochain bench**.
- **Gauge ajoutée** : `surch_index_postings_directory_bytes` (+ `MemoryUsage::postings_directory_bytes`,
  `/_surch/stats` → `memory.postings_directory_bytes`) — somme exacte (`size_of`) des 5 tableaux,
  par segment, sur tout `DocumentIndex`. `total_accounted_bytes` en profite automatiquement (déjà
  `usage.total_bytes() + state_id_maps + state_documents_overhead`) : le prochain scrape 1,36M doit
  expliquer ≥90% de `surch_jemalloc_allocated_bytes`.

**Volet 2 (disk-back, poste dominant)** : `segment_descriptors` (le plus gros du quintette, 16
o/terme flat, **T-scaled indépendamment du nombre de postings** — contrairement à `block_directory`
qui scale avec le nombre de BLOCS) est maintenant disk-backé derrière le flag existant
`SURCH_POSTINGS_DISK` (pas de nouveau flag). `FieldPostings::segment_descriptors_directory:
Option<(u64 base_offset, u32 term_count)>` : quand `Some`, le tableau résident devient
`Box::default()` (0 octet) et chaque descripteur est un enregistrement 12 octets packés (u64+u32
LE, sans le padding Rust à 16) écrit une seule fois dans le `postings_segment` partagé (même
fichier pread que le FoR déjà disk-backed C1b), lu via `descriptor_at` (1 `pread` supplémentaire,
même compromis latence déjà accepté pour le payload FoR). Best-effort : flag off ou échec d'écriture
→ reste résident, comportement 100% inchangé. Couvre les DEUX producteurs
(`PostingsBuilder::build_with_disk_flag` et `FieldMergeAccumulator::finish`, S3 merge) et le
consommateur `merge_term_dictionaries` (lecture d'un descripteur SOURCE pendant un merge).
- Fichiers : `crates/surch-index/src/postings.rs` (struct `FieldPostings`, fn
  `persist_or_keep_descriptors`/`descriptor_at`/`encode_descriptor`/`decode_descriptor`,
  `TermDictionary::postings_directory_bytes`), `document_index.rs` (passthrough),
  `crates/surch-index/src/memory.rs` (`MemoryUsage::postings_directory_bytes`),
  `crates/surch-api/src/stats.rs` (gauge + `MemoryReport`).
- **Reste pour S5 complet** : `offsets`/`block_offsets`/`block_directory`/`block_dir_offsets`
  encore résidents (T-scalés eux aussi, mais nécessaires au hot-path term lookup dans les DEUX
  modes ou T+1-scalés à cadence fixe — refactor plus large, read-path partagé RAM/disk) ; FST par
  champ (49 MiB, déjà compressé — mmap/pread per-segment resterait le gain suivant) ; doc_len_dense
  / id_maps paginés (mentionnés par le design S5 mais hors scope de cette passe). Projection
  plancher 28M : le gain net dépend du ratio blocs/terme réel du corpus (voir piège ci-dessous),
  **à mesurer** — pas promis tant que non re-scrapé.
- **Piège chiffré (documenté dans le code)** : pour un terme mono-bloc (df ≤ 128, le cas
  dominant Zipfien), l'économie de 16 o (descriptor supprimé) est exactement compensée par le coût
  PRÉ-EXISTANT (C1b) de `block_directory`+`block_dir_offsets` (12+4 o) — un terme mono-bloc ne
  gagne donc RIEN en RAM nette ; le gain net vient des termes multi-blocs (df > 128, où
  `block_directory` coûte proportionnellement moins par terme) ET du fait que `segment_descriptors`
  est retiré INCONDITIONNELLEMENT (16 o/terme, quel que soit df) alors que l'ajout C1b scale avec
  les blocs. Tests : parité lecture flag on/off + white-box (RAM shrink direct sur les champs
  privés) dans `crates/surch-index/src/postings.rs` (`mod tests`) et
  `crates/surch-index/tests/postings.rs`.
- **Non exécuté** (contrainte session : jamais `cargo build/check/test/clippy/run`) : `cargo fmt
  --check` passe propre sur tout le workspace après l'implémentation (seul signal de validité
  syntaxique disponible sans compilateur) ; validation réelle (gauge coverage ≥90%, oracle 0
  divergence, plancher 28M) à faire via `ci-k8s`/bench-local au prochain passage.

## 🔧 S5b — spill des 4 tableaux par-terme restants (TABLE UNIFIÉE, fusion avec S5a)
Consensus **Codex GPT-5.5 @ xhigh + Opus 4.8 @ max** (2026-07-06, convergence forte, 0 divergence
de fond) sur le design, puis implémentation. Cible : la gauge `postings_directory_bytes` (148 MiB
résidents à 1,36M = `offsets` + `block_offsets` + `block_directory` + `block_dir_offsets`, ×21 ≈
3,1 GiB à 28M).

**Constats d'étude (grep exhaustif des consommateurs)** :
- `offsets` (CSR T+1) : en mode disque sa seule valeur utile est `df = offsets[i+1]−offsets[i]`
  (passé à `decode_postings_blocked`/`DiskPostingsCursor` par `segment_slice`/`disk_cursor`/
  `merge_term_dictionaries`) — les flats qu'il indexe sont vides.
- `block_offsets` (CSR T+1) : **MORT en mode disque** — ses seuls lecteurs
  (`lookup_block_metas`/`lookup_with_block_metas`) indexent `block_metas_flat` (vide flag on) et
  tous les call sites surch-api branchent sur `postings_disk_backed()` avant. Décision : plus
  jamais construit en mode disque (ni résident ni spillé).
- `block_directory`/`block_dir_offsets` : hot path réel = `disk_cursor()`, appelé UNE fois par
  terme résolu par les SEULES conjonctions mono-segment (`conjunction_hits_disk`,
  `fused_conjunction_scores_disk`). Le `match` mono-token passe par `decode_from_segment`
  (aucune lecture de directory) ; le multi-segment par `conjunction_hits_merged` (idem).

**Design retenu (fusion S5a+S5b)** : le spill S5a (12 o/terme) est REMPLACÉ par UNE table
unifiée `TermEntry` **28 o/terme packés LE** (`postings_offset u64`, `postings_len u32`,
`postings_count u32` = df, `block_dir_offset u64`, `block_dir_count u32`), écrite en DEUX
`append()` par champ (région block-directory packée 10 o/entrée, puis table TermEntry) après le
payload FoR — jamais un pwrite par terme. Accesseur unique `FieldPostings::term_entry(idx,
segment)` résident-ou-pread consommé par `segment_slice`/`disk_cursor`/`merge` ; le directory
d'un terme est lu par le cursor en UN SEUL pread groupé (`block_directory_entries`,
`block_dir_count`×10 o), jamais un pread par bloc. Sentinelles : `postings_len == 0` = pas de
couverture (offset 0 reste légitime) ; `block_dir_count == 0` avec `len > 0` = directory absent
→ fallback `open()` recompute — **corrige au passage un bug latent S5a/C1b** (directory vide
rendait le terme muet via `open_with_directory([])`). `doc_freq()` du cursor devient O(1)
(porte `postings_count`).
- **Coût pread par terme résolu** (page-cache chaud) : match mono-token = 1 TermEntry + 1
  payload (inchangé vs S5a : 1 descriptor + 1 payload) ; conjonction mono-segment = 1 TermEntry
  + 1 directory groupé + ~1/bloc touché (**+1 pread/terme vs S5a**, le prix incompressible de
  sortir le directory du résident) — ≤5 termes/requête ≈ +5 preads chauds, budget latence tenu
  d'après les deux panels (gate fair-ab p95 ≤ ~0,6 ms à vérifier).
- **Durcissement best-effort (fix Opus)** : sur échec d'append des MÉTADONNÉES, le segment
  reste VIVANT et les 5 tableaux restent résidents (S5a nullait `postings_segment` → en mode
  disque toutes les lectures seraient devenues silencieusement vides, les flats RAM étant déjà
  vides). Seul l'échec d'append du PAYLOAD FoR désactive le segment (comportement historique).
- **Écarté après consensus** : cache LRU RAM du directory (réintroduirait du résident à borner,
  le page cache EST le cache) ; inline du 1er BlockDirEntry dans TermEntry (complexité non
  prouvée, gate p95 d'abord) ; renumérotation des tables séparées S5a/S5b (2-3 preads/terme).
- Fichiers : `crates/surch-index/src/postings.rs` (struct `TermEntry`,
  `persist_or_keep_term_directory` + `TermDirectoryChannels`/`TermDirectoryTables`,
  `term_entry`/`block_directory_entries`, encodeurs 28 o/10 o, cursor `total_count`),
  `memory.rs`/`document_index.rs`/`stats.rs` (docs gauge : ~0 attendu flag on). Tests : parité
  cursor spillé vs leapfrog RAM (`disk_cursor_with_spilled_directory_matches_ram_leapfrog`),
  white-box « les 5 tableaux vides + gauge == 0 flag on »
  (`per_term_directory_moves_off_heap_when_disk_flag_is_on`), round-trips encodeurs ; le merge
  spillé est couvert par `merge_ram_and_disk_modes_produce_identical_reads` (document_index) et
  la parité end-to-end par `postings_disk_parity.rs` (surch-api, 300 docs = terme 3 blocs).
- **Attendu au prochain bench 1,36M flag on** : `postings_directory_bytes` ~0 ; allocated
  ~447−148 ≈ **~300 MiB** ; `disk_postings_bytes` +~150 MiB (payload + directory + TermEntry) ;
  latence p95 ≤ ~0,6 ms. Non exécuté localement (contrainte session : jamais cargo
  build/test/clippy) — `cargo fmt --check` propre ; CI + oracle-local + fair-ab à lancer.
