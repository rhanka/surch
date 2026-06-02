# PLAN MAÎTRE — Garantir Surch >= 2x ES/OS sur chaque axe

Architecte en chef : synthèse des 4 plans du panel. Désaccords tranchés, hypothèses validées contre le code réel (`run_leapfrog` execution.rs:273, `advance_to` postings.rs:525, `BlockMeta.max_term_freq` présent, profil release sans target-cpu, aucune SIMD/roaring actuellement).

---

## 1. SCOREBOARD (Surch vs ES/OS, statut, cible 2x reformulée)

Mesures de référence W2 (charge équitable, la seule base de décision). Ratio = facteur d'avance Surch ; rouge = pas atteint.

| Axe | Métrique (W2) | Surch | Réf. | Ratio | Statut | Cible "2x" opérationnelle |
|---|---|---|---|---|---|---|
| Latence p50 global | ms | 1.0 | ES 2.5 | 2.5x | **VERT** | Surch <= ES/2 (mediane 5 rep). Gate non-régression: < 1.25ms |
| Latence match p95 (1 terme) | ms | 1.6 | ES 4.2 | 2.6x | **VERT** | Surch <= ES/2. Gate non-régr.: < 2.1ms |
| Latence **bool** p95 (conj. 2 noms) | ms | 9.3 | ES 3.5 | **0.38x (2.7x plus lent)** | **ROUGE — CIBLE** | Surch <= ES/2 = **<= 1.75ms**. Parité = <= 3.5ms |
| Latence **full** p95 (function_score) | ms | 9.4 | ES 3.2 | **0.34x (2.9x plus lent)** | **ROUGE — CIBLE** | Surch <= ES/2 = **<= 1.6ms**. Parité = <= 3.2ms |
| Mémoire RSS (TREC-COVID 171k) | MiB | 964 | OS 1462 | 0.66x | **JAUNE (gagné, pas 2x)** | Reformulé: Surch RSS <= OS/2 = <= 731 MiB. Réaliste: 0.5–0.6x. Gate: jamais > OS |
| Indexation bulk (deces 1.36M) | s | ~104 | ES ~116 | 1.12x | **JAUNE (gagné, pas 2x)** | docs/s(Surch) >= 2x docs/s(ES). À prouver 3-rep |
| Indexation bulk (TREC-COVID) | s | 56 | OS 87 | 1.55x | **JAUNE** | idem |
| Qualité NDCG@10 SciFact | — | 0.6576 | OS 0.6537 | +0.6% | **VERT (parité)** | **PAS un "2x"** (NDCG borné [0,1]). Cible = `NDCG(Surch) >= NDCG(OS) − 0.01` |
| Qualité NDCG@10 TREC-COVID | — | 0.4750 | OS 0.4902 | −3.1% | **JAUNE (parité-tol)** | idem, floor = 0.465 |
| Latence TREC-COVID cache-OFF p50 | ms | ~302 | OS ~184 | 0.61x (1.6x plus lent) | **ROUGE (2e cible)** | Surch <= OS/2. Réaliste sous contraintes: parité (voir caveats) |
| Parité oracles b1/b2 deces | divergences | 0 | — | — | **VERT — SACRÉ** | Toujours 0. Bloquant absolu |

**Reformulation des axes non-ratio (à faire acter par le propriétaire) :**
- **Qualité NDCG : "2x" est mathématiquement impossible** (doubler 0.66 = 1.32 > 1.0). L'axe qualité n'est PAS une dimension de vitesse. Définition retenue : **"jamais inférieur à OS, tolérance 0.01"** (gate de non-régression, déjà tenu). Les 3 plans concordent : c'est une exception de définition, pas un objectif atteignable.
- **Mémoire : "2x moins de RAM" (0.5x) est plausible mais borné par ce qu'OS choisit d'allouer** (positions, normes, fielddata). Cible affichée 0.5x, plancher honnête documenté 0.6x.

---

## 2. PLAN TECHNIQUE bool/full (approches classées, désaccords tranchés)

**Root-cause validée par le code** : `run_leapfrog` (execution.rs:273) fait, par candidat du driver dense : (a) `advance_to` scalaire/galloping par follower, (b) `out.insert` dans un **BTreeMap** alors que le driver sort déjà trié ascendant, (c) un **`binary_search_by_key` par follower par hit** (ligne 330) pour re-trouver une freq que le curseur connaissait. Le coût dominant est le **walk d'intersection sur 2 termes très denses** (df_rare grand), pas le scoring.

### Désaccords du panel — tranchés

| Désaccord | Position panel | **Tranche (architecte)** |
|---|---|---|
| Crate `roaring` externe (plans 2,4) vs module maison (plan 1) | partagé | **Module roaring maison.** Raisons : (1) parité SACRÉE — un module testé isolément contre le walk donne une garantie bit-identique auditable ; une crate externe = boîte noire à valider quand même. (2) On ne matérialise QUE les termes haut-df (centaines de termes), pas tout le dico → pas besoin de la compression sophistiquée de `roaring-rs`. (3) `_mm_and_si128` SSE2 + `u64::count_ones` suffisent, zéro dépendance, contrôle total du seuil. (4) Évite le risque de dispatch SSE2/AVX2 caché de la crate. |
| `df` réel MARIE/MARTIN | 300k×80k vs 5%×1% | Ordre de grandeur ~5% (MARIE ~68k) inter ~1% (MARTIN ~14k) sur 1.36M. Peu importe le chiffre exact : **les deux sont >> seuil roaring** → cas dense couvert. À profiler en Phase 0. |
| SSE2 scan u32 (A2/B) comme levier principal | plan 2 le pousse | **Rétrogradé en réserve.** Le galloping branchless est DÉJÀ en place (postings.rs:556-567) et #18 a prouvé neutre. Le scan SSE2 n'aide que les paires MOYENNES non couvertes par roaring → gate par décompose, pas en tête. |
| AVX2 runtime-dispatch | plan 1 (A5) bonus | **Bonus, jamais dépendance du 2x.** Le runner Scaleway peut ne pas exposer AVX2 ; cargo-dist = binaire générique. Le 2x DOIT tenir en SSE2 seul. |
| Pré-jointure offline des paires fréquentes | plan 4 (D) | **REJETÉ comme solution.** Overfit matchID sévère, ne généralise pas. Acceptable uniquement comme borne-sup théorique de démonstration, jamais en benchmark. |

### Approches retenues, classées par ROI

**RANG 1 — A1 : Roaring/hybrid bitmap maison sur termes haut-df** *(LE gap-closer)*
- **Mécanisme** : en plus du canal `doc_ids[]`, pour chaque terme `df > seuil` (~2048), partitionner l'espace doc_id en chunks de 65536 (high 16 bits). Par chunk : si densité > ~6% → bitmap dense 8 KiB ; sinon array trié u16 (low bits). Intersection par chunk : bitmap∩bitmap = 1024× `u64 AND` (ou 256× `_mm_and_si128` SSE2) + `count_ones`/extraction ; bitmap∩array = test de bit O(1) ; array∩array = galloping. Câblé dans `conjunction_leapfrog`/`run_leapfrog` comme chemin haut-df, **fallback walk pour bas-df**. C'est l'algorithme exact de Lucene (RoaringDocIdSet/BitSetConjunction) — la source des ~2x mesurés côté ES.
- **Gain attendu** : bool/full p95 W2 **9.3 → 3–4.5ms** (parité ES, idéalement < 2ms). Le AND traite 64 doc_ids/instruction (256 en SSE2) vs 1/itération scalaire ; facteur 30–60x sur la boucle brute, ramené à ~2–2.5x net p95 après scoring+collecte top-K.
- **Effort** : **L**. **Risque** : complexité code roaring (mitigé : module testé isolément AVANT câblage, comparé au walk sur 100 paires aléatoires). Coût mémoire borné : ~12 MB sur deces (centaines de termes), à récupérer en dérivant `doc_ids[]` du roaring.
- **Fit-build SSE2** : **PARFAIT.** `u64 AND` scalaire OU `_mm_and_si128` (SSE2 baseline inconditionnel) ; `count_ones` = intrinsèque LLVM sûr ; zéro nightly, zéro target-cpu, cargo-dist OK.

**RANG 0 (prérequis) — A3 : Nettoyer `run_leapfrog`** *(quick-win parité-sûr, à faire AVANT A1 pour isoler le vrai coût)*
- **Mécanisme** : (1) remplacer le `BTreeMap` (execution.rs:277) par un `Vec<(u32,f64)>` — le driver sort déjà ascendant, append O(1) amorti, zéro arbre rouge-noir. (2) Supprimer le `binary_search_by_key` par follower par hit (ligne 330) : faire retourner par `advance_to` l'index d'atterrissage, lire `postings[idx].freq` en O(1) (fused scorer généralisé à la voie bool — full~=bool prouve qu'il manque sur cette voie).
- **Gain attendu** : 10–20% (bool 9.3 → ~8ms). Pas un gap-closer, mais **nettoie le bruit pour isoler l'intersection avant A1**.
- **Effort** : **S**. **Risque** : très faible, refactoring iso-résultat, parité bit-identique. **Garder même si dans le bruit** (forme correcte, comme le fused #19).
- **Fit-build** : pur Rust stable, zéro SIMD.

**RANG 2 — A4 : Block-Max conjunction (BMW-AND) + early-termination top-K** *(serrage, gated par décompose)*
- **Mécanisme** : réutiliser `BlockMeta.max_term_freq` (déjà présent et testé) pour calculer un block-max-score par terme ; borne sup du score d'un doc = somme des block-max. Si < K-ième meilleur score collecté (heap min top-K), sauter le bloc sans scorer. Top-K en binary heap au lieu de tout collecter+trier.
- **Gain attendu** : sur deces **modeste (10–30%, idf plats)** ; sur TREC-COVID **1.5–2x (idf contrastés)** — c'est là qu'il paie le plus. À mesurer par corpus.
- **Effort** : **M**. **Risque** : parité — la borne sup DOIT être conservatrice (jamais couper un doc du top-K) ; tie-break doc_id à tester contre oracle. Bug d'arrondi float = top-K incomplet silencieux → gate oracle obligatoire.
- **Fit-build** : pur Rust stable, réutilise l'existant.

**RANG 3 — A5 : Runtime-dispatch AVX2 (`_mm256_and_si256`) + fallback SSE2** *(bonus uniquement)*
- **Mécanisme** : `is_x86_feature_detected!("avx2")` une fois au démarrage → fn-pointer AVX2 (256 bits) ou SSE2 (128). Appliqué au AND bitmap d'A1.
- **Gain** : +1.5–2x sur la boucle AND **si AVX2 dispo sur le hardware**. **Le 2x ne doit JAMAIS en dépendre.**
- **Effort** : M. **Risque** : double chemin à maintenir bit-identique. **Fit-build** : c'est LE pattern autorisé par cargo-dist (`#[target_feature]` + détection, stable depuis 1.27).

**RANG 4 — A2 : SSE2 scan u32 du canal doc_id** *(réserve, gated)*
- Seulement si le décompose post-A1 prouve que les paires MOYENNES (non couvertes roaring) pèsent encore au p95. Sinon **NE PAS L'ÉCRIRE** (anti-erreur #18 : le galloping branchless est déjà là et neutre).

---

## 3. AUTRES AXES

| Axe | Statut | Action |
|---|---|---|
| **p50 global** | VERT 2.5x | Aucune action. Gate non-régression : A1/A3 ne touchent pas la voie mono-terme. Surveiller que le roaring n'alourdit pas le warmup/cache chaud. |
| **match p95** | VERT 2.6x | Aucune action. Gate non-régression seul. |
| **RSS mémoire** | JAUNE 0.66x | Pour viser 0.5x : (1) roaring matérialisé **uniquement** haut-df (sinon régression) ; (2) **récupérer le +57 MiB** du canal doc_id en dérivant `doc_ids[]` du roaring quand il existe (pas de duplication — c'est le point clé de tension inter-axes) ; (3) éventuel bitpacking FoR des postings bas-df en RAM. Plancher honnête 0.5–0.6x. |
| **Indexation bulk** | JAUNE 1.12–1.55x | Mesurer docs/s 3-rep vs ES/OS, **confirmer >=2x**. Construire le roaring **en parallèle du build postings** (même rayon loop, O(n) cache-friendly) pour ne pas régresser l'indexation > +10%. |
| **Qualité NDCG** | VERT parité | A1/A4 ne changent PAS les scores (accélération de l'intersection, pas du scoring) → parité garantie par construction. Gate ndcg-gate bit-stable bloquant. |
| **TREC-COVID cache-OFF p50** | ROUGE 1.6x lent | 2e cible, distincte de bool/full. Bottleneck = decode/copy + hydration postings longs (#6/7/8 neutres). Leviers : (1) mesurer le zero-copy borrow déjà mergé (f66519b, mesure cache-off PENDING) ; (2) **A4 BMW-AND paie ici** (idf contrastés) ; (3) compression _source (zstd/LZ4) si hydration domine. **Honnête : 2x ambitieux, parité réaliste sans Elias-Fano/memmap.** |

---

## 4. MÉTHODOLOGIE D'ÉVALUATION (harness concret)

Corrige la racine meta : 3 nuls de suite (#17/18/19) = on optimisait APRÈS au lieu de root-causer AVANT, et 1 run bruité ne tranchait pas.

**1. Décompose-AVANT-optimiser (règle d'or, bloquante à la conception).** Toute expérience commence par un décompose qui ISOLE la couche coûteuse avant d'écrire du code : 3 shapes `match`/`bool`/`full` sur les MÊMES paires de noms (déjà outillé) + compteurs instrumentés `iterations_driver / hits`. **Seuil d'attribution : on n'optimise une couche QUE si le décompose lui attribue > 30% du coût.** Critère mécanique : `iterations_driver/hits > 50` → goulot = walk (roaring/SIMD) ; `< 10` → goulot = scoring (fused) ; entre → dominant d'abord. A2 et A4-sur-deces sont **explicitement gated par ce décompose, sinon on ne les écrit pas.**

**2. Contrôle du bruit : N=5 reps minimum** (pas 3 — ±1ms sur un delta de 3ms = ±33%). Reporter **médiane + Q1/Q3 (IQR)** par percentile. **Critère de signal** : gain réel SSI `Q1(traitement) > Q3(contrôle)` (les IC ne se chevauchent pas). Avec ±1ms de bruit, un gain < 15% est indistinguable → **on exige des leviers à effet >= 30% isolé** (d'où A1 priorisé). Interdiction de conclure sur 1 run.

**3. Charge équitable : WORKERS=2 obligatoire** pour tout go/no-go (équitable sur 2-vCPU). W4 (sur-souscrit) en stress secondaire labellisé "oversubscribed" — il mélange tail-CPU et tail-queue (ES halve 6.8→3.5 en W2 = c'était du queueing). **Comparaison valide = ratio Surch/ES dans le MÊME job K8s, même nœud, même chronologie.**

**4. Isolation : un levier = une expérience.** A3 d'abord → mesure → PUIS A1 sur base nettoyée. Jamais bundler A1+A4 dans le même run de décision. Microbenchmark isolé (criterion/Instant sur corpus synthétique) AVANT K8s — si le microbench ne montre pas >= 30% sur la fonction ciblée, STOP avant K8s.

**5. Définition opérationnelle de "2x" par métrique :**
- Latence (p50, p95 par shape) : `Surch_median <= ES/OS_median / 2` sur 5 reps W2, par shape ET global.
- Mémoire : `RSS_peak(Surch) <= RSS_peak(OS) / 2`, même corpus, même warmup, régime stationnaire.
- Indexation : `docs/s(Surch) >= 2 × docs/s(ES/OS)`, médiane 3 reps.
- **Qualité NDCG : EXCEPTION** — `NDCG(Surch) >= NDCG(OS) − 0.01` (jamais régressé). À acter par le propriétaire.

**6. Gates de non-régression (SACRÉS, bloquants pour tout merge) :**
- **PARITÉ** : oracles b1/b2 deces = **0 divergence** vs ES 8.6.1. NON-NÉGOCIABLE. A1/A3/A4 produisent un set d'intersection identique par construction.
- **QUALITÉ** : ndcg-gate BEIR — SciFact >= 0.647, TREC-COVID >= 0.465.
- **MÉMOIRE** : RSS TREC-COVID <= 1000 MiB (marge +40 sur 964) — le roaring ne doit pas exploser le RSS.
- **RÉGRESSION CROISÉE** : p50 W2 < 1.3ms ET match p95 W2 < 2.1ms (axes gagnés). **Tableau de bord multi-axes à CHAQUE run**, pas seulement l'axe visé.
- **`cargo fmt --check`** local autorisé.

**7. Tout sur cluster ci-k8s Scaleway.** Jamais de gros workload local. Chaque job = image `sha-<HEAD>`, reporter run_id/artifact/image/node. Décisions sur artefacts CI 5-rep, jamais sur impression locale.

---

## 5. SYSTÈME DE DÉCISION (machine go/no-go)

**Gate par expérience — 4 verdicts chiffrés :**
- **ADOPTER** : gain médian bool/full p95 W2 **>= 20%** (et IC disjoints) **ET** tous gates verts (parité 0-div, NDCG stable, RSS sous plafond, p50/match non régressés) → commit + scoreboard mis à jour.
- **ITÉRER** : gain réel mais < cible-2x, gates verts, hypothèse chiffrée sur le coût résiduel (re-décompose) → garder le code, formuler le levier suivant.
- **REJETER** : gain dans le bruit (< 15%, IC chevauchants) OU gate cassé → **REVERT** (pas de "au cas où"). Exception de garde : seulement si forme architecturalement correcte + parité-safe + coût-complexité nul (comme le fused #19).
- **PARKER** : gain réel mais sur axe déjà vert, ou coût ingé > bénéfice résiduel → documenter dans le ledger, déprioriser.

**Critères d'abandon précoce (savoir s'arrêter, c'est une GARANTIE pas un acharnement) :**
- Décompose montre couche ciblée < 20% du coût → **abandon avant code** (économise 1-2j dev).
- Microbench isolé < 15% sur la fonction → **abandon avant K8s**.
- **2 expériences consécutives REJETÉES sur la même couche → rotation de couche obligatoire** (re-décompose).
- Levier "L" dépasse 2× son estimation sans franchir le seuil-signal → STOP + re-décompose.
- Si APRÈS A1 (qui attribuait > 50% à l'intersection) bool/full reste > ES → escalade propriétaire : soit re-décompose obligatoire, soit acter que le 2x est infaisable en SSE2 (investir AVX2-dispatch ou acter 3-de-4-axes). **Si bool/full ne baisse pas après 3 expériences indépendantes à confiance >= 0.7 → conclure plafond physique SSE2 et documenter honnêtement.**

**Scoreboard vivant = source unique de vérité**, mis à jour après CHAQUE run K8s (même neutre). Colonnes : p50, match p95, bool p95, full p95, RSS, index docs/s, NDCG BEIR/TREC, oracle. Lignes : Surch/ES/OS/ratio/verdict. **Une expérience n'est "réussie" que si elle fait passer SA case >= 2x SANS faire repasser une autre case sous 2x.**

**Anti-optimisation-aveugle (3 règles strictes) :**
1. Aucune ligne de code perf sans : (a) décompose attribuant > 30% à la couche, (b) hypothèse chiffrée du gain, (c) plan 5-rep W2 + gates. Les 3 manquent → expérience refusée à la conception.
2. Toute PR perf porte une ligne : *"Décompose avant : X% du coût dans la couche ciblée, mesuré par [méthode]"* + verdict microbench. Pas de merge sans.
3. Expériences neutres DOCUMENTÉES dans le ledger avec leur réfutation causale (éviter de réinventer — leçon #11 réinventé par #18a).

---

## 6. SCÉNARIO ORDONNÉ (phases, critère de sortie mesurable, par ROI/effort/risque)

**PHASE 0 — Baseline propre + décompose instrumenté** *(critère sortie : scoreboard 5-rep W2 figé avec IQR)*
Lancer 5 reps W2 sur main (sha actuel) : décompose match/bool/full + p50 + RSS TREC-COVID + NDCG + index docs/s. Établir le bruit `(max−min)/2` par métrique. Ajouter compteurs `iterations_driver/hits` dans `run_leapfrog`, profiler 100 requêtes sur les paires lentes (MARIE/MARTIN), profiler les df réels.
**Sortie** : baseline reproductible avec IQR par métrique + ratio iterations/hits publié (confirme walk dominant). Si ratio < 20 → re-examiner (scoring dominant, contredirait le brief).

**PHASE 1 — A3 nettoyage** *(effort S, parité-sûr ; critère sortie : oracle 0-div + bool p95 mesuré)*
Vec trié à la place du BTreeMap (execution.rs:277), suppression du binary_search par hit via index retourné par `advance_to`.
**Sortie** : oracle b1/b2 0-divergence, p50/match non régressés, bool p95 5-rep W2 (~8ms attendu). On GARDE même dans le bruit (forme correcte) ; surtout on a NETTOYÉ pour isoler l'intersection.

**PHASE 2 — A1 roaring/hybrid maison** *(effort L, LE go/no-go central ; critère sortie : oracle 0-div + bool/full p95 <= 3.5ms)*
Module roaring testé isolément d'abord (comparé au walk sur 100 paires aléatoires, invariant `bitmap == set(doc_ids)`). Chunks 65536, bitmap dense si df_chunk > seuil sinon array u16, AND `_mm_and_si128` SSE2. Matérialisé UNIQUEMENT df > seuil. Câblé en chemin haut-df, fallback walk bas-df.
**Sortie** : oracle b1/b2 0-divergence, RSS sous plafond (1000 MiB), bool/full p95 5-rep W2 **<= 3.5ms (parité ES) idéalement < 2ms (2x)**. C'est le go/no-go central.

**PHASE 3 — Serrage A4 + A5** *(si A1 atteint parité mais pas 2x ; critère sortie : bool/full p95 <= 1.75ms)*
A4 BMW-AND (réutilise `max_term_freq`) + heap top-K ; A5 dispatch AVX2 avec fallback SSE2. **Le 2x doit tenir en SSE2 seul** ; AVX2 = bonus. A4 validé aussi sur TREC-COVID (idf contrastés).
**Sortie** : bool/full p95 5-rep W2 **<= 1.75ms (>= 2x ES 3.5)**, parité+NDCG verts, RSS/p50/match non régressés.

**PHASE 4 — Fermer axes restants + verrouiller** *(critère sortie : scoreboard complet vert OU caveat tranché)*
Mesurer index docs/s 3-rep (confirmer >= 2x) ; serrer RSS vers 0.5x (dériver doc_ids du roaring, récupérer +57 MiB) ; mesurer TREC-COVID cache-off post-zero-copy + A4 ; acter avec le propriétaire l'exception "NDCG = parité". 
**Sortie** : chaque case latence/mémoire/indexation >= 2x, qualité >= parité, sur deces ET TREC-COVID, prouvé CI 5-rep, gates verts. "On garantit" atteint OU caveat documenté et tranché.

---

## 7. CAVEATS HONNÊTES (garantissable vs à reformuler)

1. **bool/full à 2x PLUS RAPIDE qu'ES en SSE2-seul : PLAUSIBLE, NON GARANTI a priori.** Le roaring AND est l'algorithme exact de Lucene → il devrait ramener Surch à **parité** (même technique). Pour passer 2x plus rapide il faut l'avantage structurel déjà acquis (postings en RAM, pas de decode FoR par requête, zéro positions stockées) qui explique les 2.5x sur p50/match — la conjonction devrait en hériter. **MAIS** : ES bénéficie aussi du page-cache chaud, et la limite SSE2 = 4×u32/instruction (vs 8 en AVX2). Analyse de coût : si le walk = 60% des 9.3ms = 5.6ms et roaring le divise par 4 → ~1.4ms walk + ~3.7ms reste = ~5ms total, encore 1.4x ES ; le 2x exige que roaring divise plus fort OU que A4/A5 serrent le reste. **Verdict : parité très probable, 2x probable mais à PROUVER, pas à promettre avant Phase 2-3.**

2. **AVX2 (A5) potentiellement absent sur le runner Scaleway 2-vCPU.** Le 2x mesuré ne doit JAMAIS dépendre d'A5 — sinon non portable (cargo-dist générique, risque SIGILL). **Le 2x DOIT tenir en SSE2-baseline (A1+A4).** Si atteint seulement avec AVX2 → violation de "partout".

3. **Qualité NDCG "2x" : structurellement impossible** (borné [0,1]). À redéfinir explicitement comme "jamais inférieur à OS, tolérance 0.01". **Sans cet accord du propriétaire, "2x sur CHAQUE axe" est littéralement infaisable sur la qualité.** Si le propriétaire veut "2x plus de docs pertinents dans le top-10", c'est du ranking (LTR/expansion), hors-scope de la campagne latence.

4. **Mémoire 0.5x (2x moins de RAM) : plausible mais borné par les choix d'OS** (positions, normes, FST, fielddata qu'on ne contrôle pas). Surch part avantagé (pas de positions) → 0.66x acquis. **Cible réaliste 0.5–0.6x ; le 0.5x strict n'est pas garanti, à documenter selon la mesure** après récupération du +57 MiB.

5. **TREC-COVID cache-off : 2x très difficile** sous contraintes. Postings très longs = stall mémoire L2/L3 que le SIMD ne compense pas (orthogonal au débit vectoriel). Sans Elias-Fano ou block-FoR decode on-disk, le 1.6x pourrait ne pas franchir 2x. **Réaliste : parité, via zero-copy + A4 block-skip.**

6. **Overfit deces.** Les idf plats de deces rendent A4 peu payant ; sur-optimiser deces pourrait masquer une faiblesse TREC-COVID. **Gate latence+qualité PAR CORPUS obligatoire.** Le seuil roaring df doit être profilé sur la distribution réelle (sinon zone grise des termes juste sous le seuil + gaspillage RAM sur termes rares BEIR).

7. **Artefacts runner 2-vCPU.** Les absolus (9.3 vs 3.5ms) sont runner-bound, pas hardware-représentatifs. **Seule la comparaison RELATIVE (ratio même runner/même run) est valide** ; un 2x sur le runner peut devenir <2x ou >2x sur vrai hardware (NUMA/LLC). Toujours W2 pour les verdicts.

**BOTTOM LINE.** ">= 2x partout" est **CRÉDIBLE** sur les axes latence mono-terme/p50 (déjà acquis) et indexation, **ATTEIGNABLE-À-PROUVER** sur bool/full via roaring maison + nettoyage A3 + serrage A4 (parité quasi-certaine, 2x probable), **NÉGOCIABLE** sur mémoire (0.5–0.6x), **STRUCTURELLEMENT INAPPLICABLE** sur la qualité (à redéfinir comme parité) et **DIFFICILE** sur TREC-COVID cache-off (parité réaliste). La voie : roaring/hybrid bitmap haut-df 100% SSE2-baseline (algorithme de Lucene), précédé du nettoyage BTreeMap/binary_search, piloté par décompose-avant-optimiser + 5-rep-médiane-W2 qui transforme chaque "on espère" en go/no-go gaté. Les deux exceptions à faire acter par le propriétaire dès maintenant : **qualité = parité (pas 2x)** et **mémoire = cible 0.5x, plancher 0.6x**.
