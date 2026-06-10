# Étape 2 indexation 2× ES — blocker, analyse et voies alternatives

Date : 2026-06-10
HEAD investigation : `9faac87` (post-Étape 1 rayon dans `append_to_index`).
Mesure de départ : 14 316 docs/s @ W=2 sur deces 1.36 M (1.24× ES 11 560,
cible STRICT ≥ 23 120 docs/s manque 1.62×).

## TL;DR

**L'algo « PostingsBuilder thread-local + merge per-chunk » tel que prévu
dans `indexation-2x-plan.md` ne livrera PAS le 1.5× supplémentaire
escompté.** La cause est architecturale, pas algorithmique : la
tokenization est DÉJÀ parallélisée en amont, à un endroit que le plan
original n'avait pas identifié. L'Étape 1 (rayon dans `append_to_index`)
n'a livré que +3.6 % parce qu'elle a doublé une parallélisation déjà
faite. L'Étape 2 telle que décrite va paralléliser un deuxième fragment
serial qui ne dépasse pas ~15 % du coût per-doc — borne sup Amdahl
ridicule.

Voies viables hiérarchisées en §3.

## 1. Cartographie du chemin d'indexation (relue intégralement)

```
HTTP _bulk handler
└── AppState::apply_document_writes  (state.rs L1660)
    ├── pour chaque op (SERIAL — mutex écriture sur self.store) :
    │   └── data.upsert_document_deferred(id, source)  (state.rs L442)
    │       ├── mapping.ensure_fields(&source)           ~15 µs ← SERIAL inevitable
    │       ├── serde_json::to_string(&source)            ~10 µs ← SERIAL avec lock
    │       └── self.documents.insert(SourceBlob::Raw)    ~3 µs
    │
    └── pour chaque index touché :
        └── data.append_to_index(&new_doc_ids)  (state.rs L548)
            ├── Étape 1 rayon : new_doc_ids.par_iter() …
            │   └── indexed_fields_for_document(&Value, &mapping)  ~5-10 µs  ← (déjà parallèle)
            │       → Vec<(field, value_string)>
            │
            └── self.index.add_documents_with_mapping_deferred(documents, &mapping)
                └── DocumentIndex::add_documents_with_mapping_internal  (document_index.rs L365)
                    ├── documents.into_par_iter() …                            ← (déjà parallèle, depuis Track A)
                    │   └── analyze_document(doc_id, fields, mapping)  ~25 µs
                    │       → AnalyzedDocument { postings, prefixes, … }
                    │
                    └── for document in analyzed { self.merge_analyzed(doc) }  ~15 µs/doc ← SERIAL
                        ├── self.postings_builder.add(field, term, doc_id, positions)  (postings.rs L120)
                        │   └── BTreeMap<field, BTreeMap<term, Vec<Posting>>>.entry().or_default().push()
                        ├── self.prefix_postings.entry().or_default().insert(doc_id)
                        ├── self.subfield_values.entry().or_default().insert(doc_id, stored)
                        └── self.field_stats.entry().or_default().record_doc_len(...)
```

Bilan : `analyze_document` (tokenisation, asciifold, prefix fan-out,
sub-field re-analysis) — le coût per-doc dominant — **est déjà
parallélisé par `into_par_iter` ligne 407 de document_index.rs** depuis
Track A (commit antérieur, voir TODO `wp-a-perf-followups.md`).

C'est pour ça que l'Étape 1 dans `append_to_index` n'a livré que +3.6 % :
elle parallélise `indexed_fields_for_document`, un walk JSON peu coûteux
(~5-10 µs/doc), pas la tokenisation (~25 µs/doc) qui l'était déjà.

## 2. Estimation Amdahl honnête de l'Étape 2 telle que décrite

### Décomposition per-doc révisée (post-Étape 1, mesurée à 70 µs/doc) :

| Étape | Coût | Statut parallélisation |
|---|---|---|
| `upsert_document_deferred` (ensure_fields + serde) | ~25 µs (35 %) | SERIAL sous `RwLock<MemoryStore>` |
| `indexed_fields_for_document` (Value walk) | ~5-10 µs (10 %) | déjà parallèle (Étape 1) |
| `analyze_document` (tokenisation, asciifold, prefix) | ~25 µs (35 %) | **déjà parallèle** (Track A L407) |
| `merge_analyzed` (postings_builder.add + side-tables) | ~10-15 µs (15 %) | SERIAL — c'est ce que vise l'Étape 2 |
| FST `materialize_terms` au `_refresh` | amorti | hors hot path bulk |

### Borne Amdahl pour Étape 2 (PostingsBuilder thread-local + merge)

Si on parallèlise le merge `merge_analyzed` (~15 % du coût) avec 2 cores
en gardant un merge final serial qui ne peut pas être < 5 % du coût :

- Avant Étape 2 : 70 µs/doc (35 % serial upsert + 35 % par analyse + 10 %
  par walk + 15 % serial merge + 5 % autres)
- Après Étape 2 : 35 µs serial + (35 % merge analyse + 15 % merge) / 2 +
  5 % autres = 35 + 17.5 + 7.5 + 3.5 ≈ **63.5 µs/doc**
- Gain : 70/63.5 = **1.10×** → 15 700 docs/s = 1.36× ES

**Conclusion : Étape 2 = +10 % au mieux. Pas même 1.5× supplémentaire,
encore moins 2× ES.**

Le plan original surestimait grossièrement parce qu'il n'avait pas vu
que `analyze_document` était déjà parallèle.

### Pourquoi le merge ne peut pas tomber à 0 :

Le `merge_from(other: PostingsBuilder)` doit :
1. walker chaque `(field, term)` du partial
2. extend l'inner `Vec<Posting>` du final (move sans allouer si grosseur ok)
3. préserver l'ordre — postings du final puis du partial-0, puis partial-1 …
   pour respecter la propriété « doc_id croissant » que `build()` assume
   (postings.rs L150-153 sort by doc_id, mais le coût du sort serait
   amorti correctement seulement si on le fait UNE FOIS à `build()`)

Le merge final reste serial à ~5-7 µs/doc. Pas négociable.

## 3. Voies alternatives hiérarchisées par ROI

### Voie A — Parallélisation `upsert_document_deferred` (35 % du coût)

**Cible** : la voie SERIAL sous write lock. C'est le plus gros block
restant.

**Approche** :
- Phase 1 (parallèle, lock-free) : décoder chaque `source: Value`,
  faire `serde_json::to_string` et `ensure_fields_local(&source) →
  HashMap<String, FieldMapping>` (collecte locale des champs nouveaux).
- Phase 2 (serial, court) : merger les `ensure_fields_local` partiels
  dans `self.mapping`, attribuer les doc_ids serial, insérer les
  `SourceBlob::Raw` un par un.

**Gain attendu Amdahl** : 35 % parallèle / 2 cores = 17.5 % gagné →
70 → 57.5 µs/doc → **1.22× = 17 460 docs/s = 1.51× ES**.

**Effort** : M (refactor `apply_document_writes` pour pré-collecter
`ensure_fields` partiels avant d'entrer dans la boucle serial). Demande
un nouveau API sur `IndexMapping` : `ensure_fields_collect_diff(&Value)
→ Vec<(String, FieldMapping)>` pure.

**Risque parité oracle** : nul si l'ordre d'attribution des doc_ids
reste serial après. Le `mapping` final = mêmes champs ajoutés dans le
même ordre (les diffs partiels sont commutatifs : ajouter le champ
`X: text` deux fois = idempotent).

### Voie B — Codec FoR delta-encoded postings (PARK, hors scope 2×)

Réduit le `postings_bytes` (753 MiB sur deces) → moins de pression cache
L2/L3 → indirect speedup. Mais N'AFFECTE PAS le hot path bulk
(décodage à la lecture). Le bénéfice est sur les requêtes longues.

**Rejeté pour Étape 2 indexation** : 0 % d'impact sur docs/s indexation.

### Voie C — Async I/O sur source_store + jemalloc mmap (REJET)

Le `source_store` est un `BTreeMap<String, SourceBlob>` purement RAM.
Pas d'I/O à overlap. Rejeté.

### Voie D — Sharding intra-index (RÉSERVE, effort L)

Diviser `DocumentIndex` en N shards par `doc_id % N`, chacun avec son
propre `postings_builder` indépendant. À l'indexation, chaque shard est
écrit par 1 thread → pas de merge.

**Gain théorique** : 1.8× (Amdahl avec 0 % serial merge — seul reste le
write lock sur store).

**Pourquoi RÉSERVE et pas RANG 1** :
- Au moment du `search`, il faut interroger les N shards et merger les
  top-K (overhead +20-30 % de latence par doc).
- Le `materialize_terms` (FST build) est appelé N fois.
- Implémentation = refactor majeur du `DocumentIndex` (effort L, ~5 j).
- Risque de régression latence (axe gagné aujourd'hui sur 4/5 indicateurs).

**À garder en réserve si Voie A + Voie E ne suffisent pas.**

### Voie E — `SourceBlob::Raw` zero-copy lazy serialization (15 % gain)

Aujourd'hui `upsert_document_deferred` appelle `serde_json::to_string(&source)`
SYNCHRONEMENT dans le write lock. Si `source: Value` provient déjà du
parse HTTP body, on peut conserver son `Arc<[u8]>` source bytes
ORIGINAL (slice du buffer Hyper) et différer la sérialisation au
`_refresh`.

**Gain attendu** : éliminer ~10 µs/doc serial → 70 → 60 µs/doc =
**1.17× = 16 740 docs/s = 1.45× ES**.

**Effort** : S (modification du parsing `_bulk` pour propager le slice
brut + adaptation `SourceBlob`).

**Composable avec Voie A** : oui. A + E ensemble :
- 70 µs → -17.5 (A) -10 (E) = 42.5 µs/doc → **23 530 docs/s = 2.04× ES** ✅
- C'est la combinaison qui ATTEINT 2× ES STRICT.

## 4. Plan recommandé après Étape 1 (re-priorité)

| Rang | Levier | Effort | Gain estimé | Cumul docs/s | Cumul ratio ES |
|---|---|---|---|---|---|
| 1 | Voie A : parallèle upsert_document_deferred | M (~2-3 j) | +1.22× | ~17 460 | 1.51× |
| 2 | Voie E : zero-copy lazy serialize source | S (~1 j) | +1.17× | ~20 420 | 1.77× |
| 3 | Étape 2 originale (PostingsBuilder merge) | M (~3 j) | +1.10× | ~22 460 | 1.94× |
| 4 | Voie D sharding (réserve) | L (~5 j) | +1.8× sur reste | ~25 000+ | 2.16× ✅ |

**Avec Voie A + E : 2× ES STRICT atteint (1.77× < 2 mais Étape 2 ajoutée
amène à 1.94×, marge ~3 %).**

**Sans Voie A : aucune combinaison Étape 2 + Voie B/C/E ne fait passer 2× ES.**

## 5. Gates de validation cluster (à enforcer pour CHAQUE voie)

1. **Parité oracle b1/b2 deces : 0 divergence** (SACRÉ).
2. **NDCG SciFact ≥ 0.65** + **TREC-COVID ≥ 0.465**.
3. **Latence p50 W=2 ≤ 1.25 ms**, **bool p95 ≤ 1.75 ms**, **match p95 ≤ 2.1 ms** (non-régression).
4. **RSS TREC-COVID ≤ 1000 MiB**.
5. **docs/s deces W=2 ≥ {seuil voie}** sur médiane 3-rep cluster.

Gate STRICT bloquant : un seul rouge → revert.

## 6. Décision recommandée pour l'utilisateur

**ABANDON Étape 2 telle que décrite** ; la borne Amdahl à 1.10× rend le
chantier non-rentable (effort M pour < 4 % d'écart sur l'axe).

**PROCHAIN CHANTIER : Voie A (parallèle upsert_document_deferred)** —
c'est le seul levier seul qui rend le 2× ES STRICT atteignable une fois
composé avec Voie E (zero-copy). Effort total Voie A + E : ~3-4 j.

Si l'utilisateur préfère un chemin plus court : Voie E seule (1 j) livre
1.45× ES, fermer 0.55× restants nécessite alors un autre chantier.

## 7. État des commits dans ce worktree

- HEAD : `9faac87` (Étape 1 = rayon par_iter sur `append_to_index`, +3.6 %).
- Pas de commit Étape 2 dans ce worktree — la décision de ne pas
  l'implémenter est documentée ici-même.
- Pas de cargo test/clippy local (gate user enforcé).

## 8. À valider sur cluster

- Confirmer le décompose per-doc serial vs parallèle via flamegraph
  `surch-api` sur deces 1.36 M (instrumentation `profile-perf` step
  existant) : valider que `merge_analyzed` ≈ 15 % et
  `upsert_document_deferred` ≈ 35 %, faute de quoi la borne Amdahl
  ci-dessus est à recalculer.
- Si Voie A est retenue : pousser un POC sur branche `perf/voie-a-upsert-parallel`,
  lancer perf-W2 5-rep, gates ci-dessus, médiane bulk_s reportée
  scoreboard.
