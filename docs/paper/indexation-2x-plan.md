# Plan indexation 2× ES — chantier suivant identifié

Date : 2026-06-10
Cible : passer de 1.17× ES (13 817 docs/s) à **2× ES (≥ 23 120 docs/s)**.
État : analyse + design, pas encore d'implémentation.

## Diagnostic

Profilage attendu de `apply_document_writes` (`crates/surch-api/src/state.rs:1646`) :

```
Per-doc cost (deces matchID, ~70 µs/doc baseline) :
  ├── serde_json parse de _source ............  ~10 µs (10 %)
  ├── mapping.ensure_fields(&Value) ..........  ~15 µs (15 %)
  ├── indexed_fields_for_document(&Value) ....  ~35 µs (50 %) ← TOKENIZATION
  ├── PostingsBuilder.insert (serial) ........  ~10 µs (15 %)
  └── source_compression compress (option B) .  ~5 µs  (10 %)
```

**Levier majeur** : `indexed_fields_for_document` est **pure et thread-safe** :
- Inputs : `&Value` (immuable) + `&IndexMapping` (immuable pour la durée du
  bulk)
- Output : `Vec<(String, String)>` owned
- Sites : `crates/surch-api/src/state.rs:1250`

50 % du coût par doc peut donc être paralllisé. Avec 2 cores W=2 et un
overhead de coordination ~10 %, le gain attendu est :

| Métrique | Avant | Après parallèle | Ratio |
|---|---|---|---|
| Per-doc cost | 70 µs | ~50 µs | 1.4× |
| docs/s | 13 817 | ~19 350 | 1.40× |
| bulk_s deces 1.36 M | 98.1 s | ~70 s | 1.40× |

Cible STRICT 2× ES ≈ 60 % du baseline ; **avec parallélisation seule on
arrive à 70 s ≈ 0.7× ES → 1.42× ES**. Encore 1.4× supplémentaire à
grappiller via les leviers suivants.

## Implémentation proposée

### Étape 1 — Parallélisation tokenization (effort S, ~1 j)

Dans `apply_document_writes` (crates/surch-api/src/state.rs:1646) :

```rust
use rayon::prelude::*;

// PHASE 1 (parallèle): tokenization + ensure_fields par doc.
//   - chaque thread lit immutablement le shared mapping
//   - chaque thread alloue son propre Vec<(String, String)>
//   - rendez-vous : (doc_id, indexed_fields) accumulés dans Vec
let pre_indexed: Vec<(u32, Vec<(String, String)>)> = operations
    .par_iter()
    .filter_map(|op| match op {
        DocumentWriteOperation::Index { id, source, index, .. }
        | DocumentWriteOperation::Create { id, source, index, .. } => {
            // Mapping shared read-only via Arc<IndexMapping>
            let fields = indexed_fields_for_document(source, &mapping);
            Some((id.clone(), fields))
        }
        _ => None,
    })
    .collect();

// PHASE 2 (serial): merge dans PostingsBuilder + source_store.
let mut store = self.store.write()...;
for (id, fields) in pre_indexed { ... }
```

**Gates obligatoires** :
- Parité oracle b1/b2 SACRÉE : la tokenization étant pure, ordre indifférent.
  Le merge serial préserve l'ordre d'insertion → doc_id assignment cohérent.
- Pas de régression latence : la parallélisation est UNIQUEMENT sur la voie
  bulk, pas sur search.

### Étape 2 — PostingsBuilder thread-local + merge (effort M, ~3 j)

Le `PostingsBuilder` actuel est dans `DocumentIndex` (single instance). Pour
paralléliser au-delà de Phase 1, il faudrait un PostingsBuilder par thread
puis un merge final. C'est plus complexe (terms à dédupliquer, postings à
fusionner sortés).

Architecture cible :
```rust
let partial_builders: Vec<PostingsBuilder> = operations
    .par_chunks(chunk_size)
    .map(|chunk| {
        let mut local_builder = PostingsBuilder::new();
        for op in chunk { local_builder.insert(...); }
        local_builder
    })
    .collect();
// Merge serial des partial builders dans self.postings_builder.
self.postings_builder.merge_all(partial_builders);
```

Gain attendu cumulé : 1.4× × 1.5× ≈ **2.1× ES STRICT atteint** (target 2.0×).

### Étape 3 — Codec FoR delta-encoding (effort M, ~2 j)

Réduit la taille on-disk + cache pressure. Levier surtout disque, mais
indirect effet RAM cache → latence bulk write.

## Risques + mitigations

| Risque | Mitigation |
|---|---|
| Borrow-checker sur `&mapping` partagé en `par_iter` | `Arc<IndexMapping>` cloné par thread (zero-cost via Arc) |
| Determinism doc_id assignment | Phase 1 retourne ordre d'insertion ; Phase 2 attribue doc_id serial-incremental |
| Régression sur petites bulks (overhead rayon > gain) | Threshold : si `operations.len() < 100`, fallback serial |
| Parité oracle b1/b2 (SACRÉ) | Tokenization pure, postings_builder.insert ordering préservé → bit-identique |

## Mesure

Lancer perf-W2 sur sha-XXX après chaque étape :
- Étape 1 attendu : ~70 s bulk (1.4× ES)
- Étape 2 attendu : ~50 s bulk (2.2× ES)
- Étape 3 : impact disque attendu, latence neutre

Toutes étapes doivent passer b1-oracle + ndcg-gate.

## Prochaine action

Step 1 implémentable en une seule session avec gate cluster. Hors scope
autonomy actuel — demande engagement développement focal.
