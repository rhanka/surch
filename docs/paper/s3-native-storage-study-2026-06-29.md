# Étude — Stockage S3 natif pour Surch (style Quickwit / Iceberg / DuckDB)

> 2026-06-29 — étude en arrière-plan, **double consensus** (4 propositions de design diverses
> → 2 panels de juges indépendants A et B → synthèse Opus). Objectif : un stockage S3 **natif**
> (compute découplé, cluster stateless, S3 source de vérité), meilleur que le snapshot/restore ES,
> tout en gardant `snapshot_es` pour la parité. Méthode : agents free-text (pas de schéma, pour
> éviter le bug StructuredOutput). 9 agents, ~431k tokens.

Vérifications confirmées (les faits porteurs des deux panels sont exacts) :
- `repository.rs:748` `S3Repository::compare_and_set` = `read_etag` → compare en mémoire → `put_object` **inconditionnel** (l.766). Aucun `if_match`. CAS racy confirmé.
- `state.rs:7` `flate2::{Compress, Decompress}`, `Compression::fast()` (DEFLATE niveau 1), pas zstd ; on-disk reverté.
- `Cargo.toml` : `aws-sdk-s3`/`aws-config`/`flate2` présents, **pas** d'`object_store`.
- `SegmentManifest { files: Vec<SegmentFile { name, bytes }> }` confirmé ; **0** occurrence de `tantivy` ; `source_store` = `posix_fallocate`+`pwrite`/`pread` (pas de vrai mmap au read malgré « mmap M1 »).

---

# Surch — Stockage S3 natif : recommandation d'architecture finale

## 1. Convergence et divergence des deux panels

### Consensus fort (A et B alignés)

1. **Même cible architecturale** : splits immuables sur S3 + footer/hotcache en RAM + range-GET ciblés + pointeur `_manifest` basculé par CAS + nœuds stateless + cache multi-niveaux + compaction façon Iceberg. Les trois propositions convergent à ~85 % ; le débat ne porte pas sur le modèle mais sur l'exactitude et l'ancrage.
2. **Le vrai premier chantier n'est PAS le format de split** : c'est le **CAS S3 natif atomique**. Les deux panels ont indépendamment trouvé que `compare_and_set` actuel est *racy* (TOCTOU read-then-write, last-write-wins → perte de commit silencieuse à deux writers). Tout le discours « commit atomique multi-writer sans service externe » en dépend.
3. **L'erreur zstd est factuelle** : le `_source` est en DEFLATE/flate2, en RAM, et la compression on-disk a été revertée. Les props 2 et 3 la sur-vendent.
4. **Pas de Tantivy** → le cadrage `tantivy-object-store` / risque `Directory` sync-async de la prop 2 est hors-sujet pour Surch.
5. **La latence 2,5 ms est all-in-RAM** et non reproductible à froid sur S3 ; le succès du projet = **taux de hit du cache**, mesuré chaud vs froid séparément en CI cluster.
6. **Recommandation hybride identique** : épine dorsale = prop 1 (layout + double schéma CAS), carte du code = prop 3 (ancrages réels + bloom footer), contrat de latence = prop 2 (table de régimes).

### Divergences honnêtes (à trancher)

| Point | Panel A | Panel B | Tranche |
|---|---|---|---|
| **Classement #1** | Sous-juge coût→**P3** (bloom + ancrage), sous-juge fit→**P1** (CAS atomique) | Pragmatisme→**P3**, red-team→**P1** | Faux conflit : P1 = correction de la primitive porteuse, P3 = carte d'exécution. L'hybride les fusionne. |
| **`object_store` dès le lot 1 ?** | Plutôt oui (get_ranges, PutMode) | Non — `aws-sdk-s3` suffit pour Range + If-Match natif | **Panel B gagne** : pas de nouvelle dép au lot critique. `object_store` arrive au lot read-path pour le coalescing. |
| **Bloom filter footer** | Levier coût décisif (élimine GET postings) | Bonne idée mais faux-positifs/maintenance non traités | **Inclus**, mais cadré : filtre statique par split immuable, faux-positif = un GET évitable, jamais une faute de correction. |

La seule divergence réelle qui change le code est *quand* introduire `object_store` : je tranche **après** le lot CAS, pas avant.

---

## 2. Architecture recommandée : hybride « Surch-split », pas une proposition seule

**Modèle retenu : splits immuables type Quickwit + pointeur de manifeste versionné type Iceberg (1 seul niveau, pas la hiérarchie à 4 niveaux de la prop 1), commit par conditional-write S3 natif.** On rejette la hiérarchie `_root → snapshots → manifests → segments` de la prop 1 pour la v1 : elle ajoute 2 round-trips/requête à froid, à l'opposé de l'objectif « minimiser les GET ». Le time-travel reviendra en v3 si le besoin existe.

### Layout S3

```
s3://{bucket}/surch/{index}/
  _manifest                         # SEUL objet mutable, basculé par CAS If-Match. JSON ~1-4 KB:
                                    # {version, splits:[{uuid, doc_count, min/max_seq, footer_len}]}
  splits/{uuid}/
    footer.bin                      # hotcache: offsets de tous les blocs + stats min/max + BLOOM termes. 50-100 KB
    fst.bin                         # dictionnaire FST (intégrable au footer si petit)
    postings.bin                    # postings (réutilise surch-codec, skip-lists par bloc)
    source.zst                      # _source — voir caveat: aujourd'hui DEFLATE, pas zstd
    meta.json                       # checksums, doc_count
```

Chaque `splits/{uuid}/` est **content-addressed et immuable** : jamais réécrit, seulement créé puis GC. Idempotent à l'upload (un retry ne duplique rien).

### Flux WRITE (commit atomique)

1. Build local du split (pipeline actuel inchangé : `SegmentManifest` + `source.dat`).
2. `footer.bin` = offsets de chaque bloc + stats min/max + **bloom des termes** du split.
3. PUT multipart de `splits/{uuid}/*` (footer écrit en dernier).
4. **Commit = CAS sur `_manifest`** : GET `_manifest`+ETag → ajoute l'UUID → PUT `If-Match: <etag>`. Sur **412** (un autre writer a gagné) : rebase (relire le gagnant, re-merger) + retry avec backoff+jitter.
5. Schéma de repli pour les stores S3-compatibles sans `If-Match` (MinIO ancien, etc.) : fichiers de version immuables `_manifest.v{N}` créés en `If-None-Match: *` ; la version courante = le plus grand N. Sonde de capacité au démarrage.

### Flux READ (range-GET + hotcache + cache local)

1. GET `_manifest` (cache RAM, TTL/ETag 30 s) → liste des splits.
2. **Pruning** par stats min/max (élimine des splits sans les lire) puis par **bloom** (élimine les splits qui ne contiennent pas le terme, *sans GET postings*).
3. Par split retenu : footer depuis le cache RAM L0 ; miss → 1 range-GET `footer.bin`.
4. `get_ranges(postings.bin, [ranges])` (vectored IO, coalescing) ; chaque bloc cherché d'abord en cache NVMe L1 ; miss → range-GET S3 → écrit en L1.
5. Scoring BM25 (réutilise `PostingsBlockSkipIter`/`BlockMeta`).
6. `_source` lu en range-GET **uniquement pour les top-K hits**.

### Cache (le cœur du projet)

- **L0 RAM** : footers + bloom + FST + `_manifest`/manifests. ~100 KB/split → 1000 splits ≈ **100 MB**. C'est l'index in-RAM actuel **recyclé en cache**, plus en stockage primaire.
- **L1 NVMe** : block cache LRU des postings/`_source` déjà lus, indexé `(uuid, offset, len)`. **Réutilise l'infra `source_store`** (`posix_fallocate` + `pread`/`pwrite`).
- **L2 S3** : source de vérité, lu à froid uniquement.

### Coordination cluster (N nœuds stateless)

- Searchers sans état, **rendezvous hashing** `score = hash(uuid | node)` → un split tend vers les mêmes nœuds (localité de cache L1) ; ajout/retrait = secondes, **1/N des splits re-mappés**, aucune donnée déplacée.
- Découverte de version : poll ETag de `_manifest` (TTL 30 s) ou S3-event/gossip si fraîcheur sub-minute requise.
- **Writers v1 = single-writer-par-index** (recommandé). Multi-writer OCC (CAS+rebase) en v2 ; au-delà de ~5-10 indexers concurrents → metastore PostgreSQL/DynamoDB.

---

## 3. Parité ES + migration (additif, pas de réécriture)

- **`snapshot_es` reste intact** pour la parité snapshot/restore ES et l'import/export inter-clusters. Le natif est un **chemin parallèle** sur un préfixe distinct du même bucket.
- **Le natif réutilise l'existant, sans rip-and-replace** :
  - `aws-sdk-s3` **conservé** (on rejette le remplacement par `object_store` de la prop 2 : il toucherait `S3Repository` qui marche). `object_store` viendra **à côté**, au lot read-path, seulement pour `get_ranges`.
  - `SnapshotRepository` (trait sync, ponté `block_in_place`+`block_on`) **étendu** d'un `get_range(key, off, len)` (impl par défaut = get+slice) ; nouveau `SplitRepository` = nouvelle impl, sans toucher `FsRepository`/`S3Repository`.
  - `SegmentManifest { files: Vec<SegmentFile{name,bytes}> }` = **unité d'upload directe**.
  - `source_store`/`source.dat` = **cache de blocs L1** du natif.
  - Index in-RAM (`Arc<RwLock<MemoryStore>>`) = **cache L0**.
- **Angle mort de parité à assumer** : ES garantit le GET realtime par `_id` depuis le translog avant refresh et `refresh=wait_for`. Le natif (fraîcheur 30 s-2 min) casse ce contrat → conserver le chemin in-RAM/translog pour les requêtes NRT, ou réserver le natif au batch (cas BAN/décès, quasi-statique).

---

## 4. Efficience et scalabilité chiffrées

**Coût S3 par requête** (GET $0,40/M ; LIST $0,005/1000 = **12,5× plus cher** ; egress $0,09/GB) :
- À froid sans cache : ~3 GET/split. 1 M req/j × 10 splits × 3 = 30 M GET = **~$12/j**.
- Footer en RAM (hit 95 %) → **~$0,60/j**. + bloom éliminant 80 % des GET postings sur termes rares (Zipf) → **~$0,15-0,25/j**. Le bloom est le meilleur ratio effort/gain du corpus (crate `fastbloom`, stable, ~50-100 KB/split).
- **Egress = $0 si compute co-localisé** (S3↔EC2 même région). Le poste dominant est le **nombre de requêtes**, pas l'octet → footer unique + `get_ranges` coalescing.

**Latence (contrat d'acceptation CI, table prop 2)** :

| Régime | Round-trips | Latence | vs 2,5 ms actuel |
|---|---|---|---|
| Fully warm (footers RAM + blocs L1) | 0 S3 | **2-10 ms** | comparable |
| Semi-warm (footers RAM, blocs froids) | 1 batch `get_ranges` | **30-80 ms** | dégradé, acceptable batch |
| Cold (footer+blocs S3 Standard) | 3-4 séquentiels | **60-200 ms** | régression assumée |
| S3 Express One Zone (option chaude) | idem | **50-100 ms** (TTFB p50 3-5 ms) | mitigation latence |

Les 2,5 ms ne tiennent qu'en **fully-warm**. Succès = hit-rate L0/L1.

**RAM** : ~100 MB footers + 256-512 MB postings chauds L2 = **~350-700 MB/nœud**, vs 81 MB all-RAM aujourd'hui. **Nuance honnête** : sur le bench 10k actuel, 81 MB all-RAM est *moins cher* ; le modèle S3 gagne quand **l'index ≫ RAM** (RAM ∝ working set, pas ∝ index total) et quand on veut N nœuds. Sur petit corpus mono-nœud, l'in-RAM actuel reste supérieur — le natif vise la scalabilité, pas le micro-bench.

**N nœuds** : stateless, scale-out en secondes, lecture directe S3, pas de leader, pas de migration de données.

---

## 5. Plan incrémental (étapes mesurables, 1ère étape bas-risque)

**Lot 0 — CAS S3 natif atomique (LA première étape, ~1-2 sem, additif, signature inchangée).**
Ne PAS commencer par le format split. Réimplémenter `S3Repository::compare_and_set` via `PutObject` `If-Match: <etag>` / `If-None-Match: *` (aws-sdk-s3, **pas de nouvelle dép**), renvoyer `CasConflict` sur 412 ; sonde de capacité + repli read-then-PUT *seulement* si le backend n'a pas `If-Match`, avec warning explicite.
**Validé comment (ci-k8s)** : N writers se disputent `index-0` ; assert exactement un gagnant par génération, **longueur d'historique == nombre de commits réussis** (zéro commit perdu). C'est le test que le code actuel **échoue** aujourd'hui.

**Lot A — Split writer + reader mono-nœud (~3-4 sem).**
`SplitWriter` (à partir de `SegmentManifest` + `source.dat`) → `splits/{uuid}/{footer,fst,postings,source}` + bloom ; CAS `_manifest` (réutilise `write_manifest` de `service.rs`). `SplitReader` : manifest→footer→range-GET (`GetObject` header `Range`). Footer LRU RAM (<50 lignes). **Validé** : un nœud indexe vers S3 et sert depuis S3, RSS cible ~200 MB ; nb GET/requête mesuré.

**Lot B — Read-path optimisé (~2-3 sem).** Adopter `object_store` (feature `aws`, **à côté** d'aws-sdk-s3) pour `get_ranges` coalescing ; block cache NVMe L1 sur l'infra `source_store`. **Validé** : p95 chaud vs froid séparés, GET/requête, cold vs hot en CI.

**Lot C — Découplage executor↔MemoryStore (LE gros lot réel, ~4-6 sem).** Rendre l'exécuteur de requête capable de lire des **blocs de postings paresseusement** (range-GET + cache) via les skip-lists `PostingsBlockSkipIter`/`BlockMeta` **déjà présentes**, au lieu de `PostingsList` pleinement décodées depuis le `MemoryStore` RAM. C'est ici qu'est l'effort, pas dans le format de fichier — les deux panels insistent sur ce point sous-estimé.

**Lot D — Cluster stateless (~2-3 sem).** N searchers, rendezvous hashing, poll ETag `_manifest` (TTL 30 s), démarrage échelonné. **Validé** : ajout/retrait nœud, localité cache.

**Lot E — Janitor (~3-4 sem).** Compaction (N petits splits → 1, commit CAS remplaçant N par 1) + GC avec fenêtre de rétention/version-fence + time-travel/rollback.

---

## 6. Caveats honnêtes

1. **Cold-start irréductible** : 1ʳᵉ requête sur split froid = 60-200 ms (S3 Standard), jusqu'à ~500 ms si `_source` froid sur splits multiples. On ne battra jamais 2,5 ms à froid. Mitigation : prewarm des footers au démarrage, S3 Express pour splits chauds.
2. **`_source` top-K = angle mort** : K=10 hits sur 10 splits distincts = 10 range-GET = 300-800 ms potentiels. **Aucune** proposition ne le couvre → prévoir un cache `_source` par doc-id chaud.
3. **CAS racy = bug latent** dont *toutes* les propositions héritent si on ne fait pas le Lot 0 d'abord. Non négociable.
4. **Erreur zstd** : le `_source` est **DEFLATE en RAM**, on-disk reverté. `source.zst` est un objectif, pas l'état actuel — le réemploi est partiel (streamer le blob compressé vers S3, pas réécrire le codec).
5. **Coût LIST** (OpenSearch #22106 : milliers de LIST/s peuvent exploser) : **toujours bootstrapper depuis `_manifest`**, jamais depuis un listing de bucket. LIST = 12,5× le prix d'un GET.
6. **GC vs requêtes en vol** : un `get_range` sur un split GC'd → 404 (facturé) → requête échouée. Rétention garantie > p99 durée requête, ou version-fence (les searchers annoncent leur snapshot courant).
7. **Thundering herd au scale-out** : N nœuds démarrant ensemble = N×100 GET simultanés. Démarrage échelonné + cache manifeste partagé.
8. **Fraîcheur 30 s-2 min** : régression vs ~ms après bulk ; casse le GET realtime ES. Acceptable batch, pas pour NRT.
9. **Ce qui NE scalera PAS** : le `_manifest` unique en CAS au-delà de ~5-10 writers concurrents (thundering herd sur le CAS, débit d'indexation sérialisé) → passer à un metastore PostgreSQL/DynamoDB. v1 = single-writer-par-index.

**En une phrase** : adopter le modèle splits-immuables + manifeste-CAS (épine P1, layout plat), carte d'exécution P3 (`SegmentManifest`/`SnapshotRepository`/`source_store` + bloom footer), contrat de latence P2 ; **commencer par réparer le CAS S3 (Lot 0) puis découpler l'executor de la RAM (Lot C)** — les deux vrais lots critiques — en gardant `aws-sdk-s3` et `snapshot_es` intacts pour la parité.

Fichiers de greffe : `crates/surch-api/src/snapshot_es/repository.rs` (CAS l.748 à upgrader, trait l.61), `crates/surch-api/src/snapshot_es/service.rs` (`write_manifest`/`ROOT_MANIFEST_KEY`), `crates/surch-index/src/segment_manifest.rs` (enveloppe split), `crates/surch-api/src/state.rs` (MemoryStore→cache L0, source_store DEFLATE→cache L1), `crates/surch-search/` (executor à découpler), `Cargo.toml:68` (aws-sdk-s3 ; ajouter `object_store` au Lot B).
