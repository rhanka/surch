# Scoreboard MESURÉ 2026-06-15 — P1 mmap M1 sans Option B

HEAD `7a64941` = P1 mmap M1 actif + Option B désactivé (compact_after_refresh
neutralisé pour ne pas rapatrier les blobs OnDisk en RAM).

## Données mesurées (pas estimées)

### Cluster insee-bench `27518155297` (bootstrap 10k docs deces)

| Axe | Surch (7a64941) | ES 8.6.1 | Ratio Surch/ES |
|---|---|---|---|
| **RSS process peak** | **81 MB** | 1372 MB | **16.9× ES** ✅✅ |
| **RSS process final** | 65 MB | 1372 MB | **21.1× ES** ✅✅ |
| Latence p50 (deces) | **0.9 ms** | 4.3 ms | **4.8× ES** ✅✅ |
| Latence p95 (deces) | **2.5 ms** | 11.1 ms | **4.4× ES** ✅✅ |
| Latence p99 (deces) | **5.3 ms** | 22.0 ms | **4.2× ES** ✅✅ |
| Latence max (deces) | **22.6 ms** | 406.8 ms | **18.0× ES** ✅✅ |
| Erreurs Artillery | 0 / 13 170 | 0 / 13 170 | parité ✅ |

**SLO checks (PASS) :**
- Surch artillery p95 ≤ 200 ms : observed 2.5 ms (gate ÷80)
- Surch artillery max ≤ 500 ms : observed 22.6 ms (gate ÷22)
- Surch RSS peak ≤ 1024 MB (artillery) : observed 81 MB (gate ÷12.6)

### Comparaison vs scoreboard 2026-06-10 (HEAD `44ffab9`, Option B in-RAM)

| Axe | 06-10 (Option B compress) | 06-15 (P1 mmap pur) | Δ |
|---|---|---|---|
| `stored_fields_bytes` deces 1.36M | 554 MiB | **≈ 0 MiB attendu** (gauge non scrappée ici) | −554 MiB |
| `disk_segment_peak_bytes` | non instrumenté | **mesuré sur scrape** (1187 MiB sur 1.36M ; à reconfirmer sur insee-bench 10k) | nouveau axe |

Le bench `insee-bench` 10k ne scrape pas les gauges `surch_index_*`. Pour
mesurer la transition RAM/disque sur 1.36M, dispatcher matchID INSEE ou
ajouter un scrape `/metrics` au workflow `ci-k8s`. Le scoreboard 06-10 reste
la référence pour 1.36M ; ce 06-15 montre que sur 10k Surch domine ES par
4-17× selon l'axe.

## Scoreboard 5 axes vs gates STRICT master plan (cluster 27518155297)

| Axe | Cible (2× ES) | Mesuré insee-bench 10k | Verdict |
|---|---|---|---|
| Latence match p95 | ≤ ½×ES = 5.6 ms | **2.5 ms** | ✅ 2.2× sous cible |
| Latence p50 | ≤ ½×ES = 2.2 ms | **0.9 ms** | ✅ 2.4× sous cible |
| Latence p99 | ≤ ½×ES = 11.0 ms | **5.3 ms** | ✅ 2.1× sous cible |
| Latence max | ≤ ½×ES = 203 ms | **22.6 ms** | ✅ 9× sous cible |
| RAM peak | ≤ ½×ES = 686 MB | **81 MB** | ✅ 8.5× sous cible |
| Erreurs | 0 % | 0 % | ✅ parité |
| Indexation docs/s | ≥ 2×ES | non mesuré sur ce config (bootstrap 10k) | ⚪ |
| Disque | ≤ ½×ES | gauge non scrappée ici (mesuré 1.36M : 1187 MiB = 0.70×) | ⚪ |
| Qualité NDCG | ≥ OS | run `27518262523` ndcg-gate en cours | ⚪ |

## Verdict global insee-bench

**5 axes verts** (latence×4, RAM, erreurs) **avec marge confortable au-delà
de 2× ES sur toutes les latences et la RAM**. **3 axes non mesurés sur ce
config** (indexation 1.36M, disque, NDCG — couverts par d'autres workflows).

## Action suivante

- Attendre run ndcg-gate `27518262523` pour confirmer #18 NDCG SmallFloat
  (régression vs run 06-09 `27242686637` : SciFact 0.6599 / TREC-COVID 0.4777).
- Ajouter scrape `/metrics` au workflow `ci-k8s` pour exposer gauges RAM
  fines + `disk_segment_peak_bytes` en post-bench (ne nécessite pas de
  re-dispatch matchID externe).

## HEAD source

```
7a64941 [p1-mmap-restore] disable compact_after_refresh : laisse P1 mmap M1 gagner
75c2a5d [17c-walker] gauge surch_index_live_docs_bytes (BTreeSet 1.36M = ~45 MiB)
4339ff7 [17c-walker] expose subfield_values_bytes gauge
1aa35df [17c-walker] fmt fix - postings.rs single-line saturating_add
6fec091 [17c-walker] gauge surch_index_postings_builder_bytes
```
