# #19 — Axe disque : estimation analytique en attendant la mesure live

Date : 2026-06-10
État : note technique, en attendant la dispatch K8s ad hoc.

## Contexte

Le master plan exige `disque(Surch) ≤ ½ × disque(OS)`. La mesure live demande
un workflow K8s qui boot Surch, indexe deces 1.36 M, puis dumpe :
- côté Surch : `du -sb /tmp/surch-data` (ou taille snapshot via `_snapshot/<repo>/_create`)
- côté ES : `GET _cat/indices?bytes=b` ou `du -sb` sur le PVC

Cette mesure n'a jamais été faite. En attendant, on peut **estimer
analytiquement** la taille du segment on-disk Surch en s'appuyant sur les
gauges déjà émises.

## Estimation analytique

État RAM côté Surch (run perf-W2 27067004820 sha-319f19a) :

| Composant | RAM | Si écrit en FoR sur disque | Ratio |
|---|---|---|---|
| `_source` blobs (post-option B compressed) | 350-400 MiB (estimation post compact) | identique (déjà deflate) | 1.0× |
| `postings_bytes` (per-term `Posting` + `doc_id` channel) | 753 MiB | ~225 MiB (FoR ~3×) | 0.30× |
| `block_metas_bytes` | 119 MiB | ~119 MiB (varints, ~1×) | 1.0× |
| `field_stats_bytes` (post #18 SmallFloat u8) | 60 MiB | ~60 MiB | 1.0× |
| `fst_bytes` (term dict serialisé) | 35 MiB | identique (déjà serialized) | 1.0× |
| `roaring_bytes` (high-df bitmaps) | 34 MiB | ~34 MiB | 1.0× |
| **TOTAL disque estimé** | — | **~820 MiB** | — |

Sur OpenSearch 2.17.1 deces 1.36 M : ~1.2 GiB on-disk mesuré au `_cat/indices`
(à confirmer ; chiffre provient de la roadmap matchID-replacement-readiness).

**Ratio estimé Surch / OS ≈ 820 / 1200 = 0.68× = pas encore 0.5× STRICT.**

Gap restant : **−205 MiB sur disque Surch** pour atteindre la cible 600 MiB
(= ½ × 1200). Sources d'économie :
1. Posting compression FoR plus agressive (delta encoding + bit-packing au lieu
   du Posting fixed-size struct actuel) → encore 50-100 MiB.
2. Dictionnaire FST partagé entre champs (heritage matchID où PRENOM et NOM
   sont des champs séparés mais partagent beaucoup d'unigrammes) → 10-15 MiB.
3. Block_metas en bytes-packed au lieu de struct → 50-80 MiB.

## Plan pour dispatch live

Une fois Artillery hang traité (track concurrent bulk-search stall) :
1. Ajouter un step au surch-eval-perf.yml après "Indexation timed" :
   ```yaml
   - name: "Disk size (surch)"
     if: matrix.engine == 'surch'
     run: |
       docker exec surch-eval du -sb /tmp/surch-data | tee surch-eval/ci/reports/disk-surch.txt
   ```
2. Pareil côté ES via `_cat/indices?bytes=b&format=json`.
3. Calcul ratio dans `write_summary.sh`.

## Rappel honnête

La mesure analytique 820 MiB est une **estimation à ±10 %**. La mesure live
fait foi. Pas de claim disque dans le scoreboard tant que pas mesuré.
