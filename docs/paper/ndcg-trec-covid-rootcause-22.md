# #22 — Root-cause TREC-COVID NDCG@10 −0.0152 (Surch 0.4750 vs OS 0.4902)

Date : 2026-06-08
État : analyse, pas encore de fix code (gate avec parité oracle b1/b2 + ndcg-gate SciFact ≥ 0.65)

## Observation banque (`docs/ops/bench-reports/track-a-performance-ledger.md`, ledger F2)

- Surch NDCG@10 TREC-COVID = **0.4750**
- OpenSearch NDCG@10 TREC-COVID = **0.4902**
- Δ = **−0.0152** (Surch trail OS de 3.1 %)
- **Recall@10 = 0.0132 IDENTIQUE** des deux côtés
- SciFact : Surch 0.6576 vs OS 0.6537 → +0.6 % (parité dépassée)

Conclusion mécanique : **le set des top-10 est identique, l'ordre interne diffère**. Le levier
n'est PAS dans la sélection des candidats (postings/intersection) — il est dans le **scoring BM25**.

## Hypothèse principale : quantization `doc_len` Lucene SmallFloat absente côté Surch

Lucene `BM25Similarity` quantize chaque `doc_len` sur 1 byte via `SmallFloat.intToByte4`
au moment de l'indexation, puis dé-quantize via `byte4ToInt` au scoring. C'est une
**perte d'information délibérée** : ~256 buckets de doc_len au lieu d'une valeur exacte.

Surch stocke aujourd'hui `doc_len_dense: Vec<u64>` (entiers exacts, `document_index.rs:104`)
et calcule au scoring (`scoring.rs:133`) :

```rust
let length_norm = doc_len as f64 / self.avg_doc_len;
```

avec `doc_len` exact. Sur un corpus à longueurs très variées (TREC-COVID = abstracts
académiques, ~50–500 tokens), cela produit un `length_norm` finement granulaire qui
discrimine des docs que Lucene regroupe dans le même bucket. Le `tf_norm` résultant
diffère légèrement → l'ordre du top-10 diffère → NDCG@10 diffère.

Sur SciFact (corpus court, lengths ~80–200 mots), l'effet est invisible — d'où la
parité spontanée (+0.6 %) sur ce dataset.

## Test de l'hypothèse (avant d'écrire le fix)

1. **Microbench scoring isolé** : extraire 50 paires `(doc_len, term_freq)` de TREC-COVID,
   calculer Surch_score vs Lucene_score_quantizé. Confirmer un Δ ≠ 0 sur l'ordre Top-K.
2. **Microbench reproduit en Java** : appliquer la formule Lucene exacte
   (`BM25Similarity.score()` + `SmallFloat`) sur le même input, vérifier bit-identité.
3. **Si confirmé** : écrire le fix.

## Fix proposé — `surch_search::small_float`

```rust
// Lucene-compatible 1-byte doc_len quantization. Bit-identical to
// org.apache.lucene.util.SmallFloat.intToByte4 / byte4ToInt.
pub fn int_to_byte4(value: u32) -> u8 { /* 4-bit mantissa + 4-bit exponent */ }
pub fn byte4_to_int(byte: u8) -> u32 { /* inverse, lossy */ }
```

Modifications :
- `FieldLengthStats::doc_len_dense: Vec<u64>` → `Vec<u8>` (norms quantizés).
- `record_doc_len(doc_id, raw_len)` → `dense[idx] = int_to_byte4(raw_len)`.
- `doc_len(doc_id)` → `byte4_to_int(dense[idx])`.
- Hot path scoring inchangé (lit `u64` reconstitué).

Bonus mémoire : 1 byte/doc au lieu de 8 → sur deces 1.36 M docs × ~6 champs analysés
indexés = **65 MiB** économisés sur `field_stats_bytes` (qui pèse 126 MiB aujourd'hui,
soit ~52 % de réduction). Petit gain mais gratuit.

## Garde-fous obligatoires

1. **ndcg-gate SciFact ≥ 0.65** : doit rester verte (actuellement 0.6576).
2. **Parité oracle b1/b2 deces** : 0-divergence sacrée. Le scoring change → vérifier
   que le tie-break par `doc_id` couvre les anciens regroupements.
3. **TREC-COVID NDCG@10 ≥ 0.4902** : cible STRICT du master plan (master-plan ligne 30,
   "désormais un point à CORRIGER, pas à tolérer").

## Effort / risque

- Effort : **M** (~1-2 jours code + 1 jour cluster validation 3-rep).
- Risque parité oracle : **moyen** (modifie scoring kernel → peut shifter le Top-K
  deces si des paires actuellement départagées par `doc_len` exact tombent dans le
  même bucket Lucene).
- Mitigation : ajouter test bit-identité contre une reference Lucene (jar embarqué
  en harness CI, ou vecteurs de test commités).

## Verdict scope

À programmer après les leviers RAM (#15/#17c/mmap) qui pèsent **>> 100×** sur le
scoreboard global. NDCG −0.0152 violé la parité stricte mais n'est pas un effet
exponentiel ; les axes RAM/disque sont en mode rouge.
