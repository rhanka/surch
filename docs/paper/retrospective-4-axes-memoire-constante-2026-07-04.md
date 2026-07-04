# Rétrospective — campagne « Surch bat Elasticsearch » relue à MÉMOIRE CONSTANTE

> 2026-07-04 — audit méticuleux de toute la trajectoire (latence, RAM, indexation, disque + qualité/parité)
> depuis le début, chaque chiffre annoté de ses conditions de mesure, puis relu au prisme du principe
> **« à mémoire constante »** (budget mémoire FIXE et ÉGAL pour les deux moteurs).
>
> **Le principe, dans les mots du propriétaire (2026-07-04, transcript session) :**
> « Depuis un petit temps, on avait dit qu'in fine il fallait garder "à mémoire constante", car ça n'avait
> pas de sens sinon. Et puis tu reprends l'ensemble des tests que tu oublies, et puis tu fais le setup que
> je te demande pour le corpus de 28M sur cette machine, avec les bornes qu'il faut en mémoire et CPU. »
> Décision antérieure validée (2026-07-01) : « **ok pour aligner les contraintes mem à celles d'elastic
> pour les tests !** » — jamais appliquée systématiquement avant le harnais `fair-ab.sh` du 2026-07-04.
>
> **Thèse de l'audit :** l'écrasante majorité des « wins » annoncés (latence 2-3×, RAM 0,52× ES, index 1,6×)
> ont été mesurés **hors mémoire constante** — Surch tout-en-RAM ou sur-provisionné face à ES borné, sur des
> corpus/hardware/moteurs de référence différents. Une fois la discipline appliquée (harnais pinné cgroup,
> caps égaux), le tableau s'inverse sur l'axe qui compte le plus : **la RAM**.

---

## 0. Cibles actées (référentiel de jugement)

Source : `docs/paper/2x-everywhere-master-plan.md` — « CIBLES ACTÉES PAR LE PROPRIÉTAIRE (2026-06-02) — STRICTES » :

| Axe | Cible stricte |
|---|---|
| Latence | **2× STRICT** — Surch ≤ ES/2 sur p50 ET p95, par shape ET global |
| Indexation | **2× STRICT** — doc/s Surch ≥ 2× ES |
| RAM | **0,5× STRICT** — RSS Surch ≤ ES/2 (pas de plancher 0,6×) |
| Disque | **0,5× STRICT** — index sur disque Surch ≤ ES/2 (nouvel axe, jamais mesuré à l'époque) |
| Qualité | **PARITÉ STRICTE** — NDCG(Surch) ≥ NDCG(OS) sur CHAQUE dataset + oracle deces 0 divergence |

Ces cibles sont le mètre-étalon de tout ce qui suit. Le principe « à mémoire constante » n'était pas encore
formalisé le 06-02 ; il n'est devenu la lentille de lecture qu'après la surprise C0 (07-01) et le rappel du
06-29 (« tout-en-RAM = faille de design »).

---

## 1. TABLEAU CHRONOLOGIQUE MAÎTRE

Légende axes : `+` = mieux que l'étape précédente, `−` = pire, `=` = stable. **⚠️ = métrique/condition
qui casse la comparabilité** (détaillée en §2). Sauf mention, RAM = la métrique EXACTE utilisée à l'étape
(anon jemalloc-resident / RSS conteneur cgroup / sous limite) — c'est justement là que tout se joue.

### Phase A — Fondations disk/allocator (mai 2026, corpus TREC-COVID 171k, réf. **OpenSearch 2.17.1**)
Source : `docs/ops/bench-reports/track-a-performance-ledger.md`. **Réf. = OpenSearch 2.17.1, PAS ES 8.6.1.**
RAM ici = **RSS conteneur** sur TREC-COVID (corpus scientifique 171k, ni deces ni INSEE).

| Étape | commit/lot | Latence | RAM (RSS conteneur, TREC-COVID) | Indexation (TREC-COVID bulk) | Disque | Parité | Conditions |
|---|---|---|---|---|---|---|---|
| Lot 1 incrémental bulk | `367acdc` | — | RSS 4802→**5859 MiB** (−, PostingsBuilder retenu) | 1001,95→**179,86 s** (~5,6×, +) | — | NDCG = | K8s SCW burst, OS 2.17.1, non borné |
| Lot 1.5 finalize postings | `8a5150f` | = | 5859→**5591** (+268 MiB seult, glibc ne rend pas les pages) | = | — | = | idem |
| Lot 1.7 **jemalloc** | `b9f6636` | = | peak 5591→**3424** (−39%), final →**1382** (−75%) | 189→**139 s** (+) | — | = | **switch allocateur** (MALLOC_CONF dirty=0,muzzy=0,bg_thread) |
| Lot 1.6 FST + Lot 2 skiplists | `2e4361e`+`d73c862` | + (skip) | peak 3424→**2156** | 139→**56,38 s** (**1,54× + rapide qu'OS**, +) | — | = | « Surch passe sous OS sur mémoire ET qualité, à 1,42× en bulk » |
| Lot 3 BMW skiplist | 05-25 | + (bool) | = | = | — | = | isolations F3 (WAND/MaxScore/LRU/topK) |

**Lecture Phase A :** premières victoires réelles, mais **sur OpenSearch 2.17.1 et TREC-COVID 171k** — un
corpus et un moteur de référence qui ne réapparaîtront plus dans la phase deces. Le jemalloc (Lot 1.7) est
le vrai tournant RAM de cette phase (−75% RSS final). Aucune borne mémoire, aucun disque mesuré.

### Phase B — Optimisations latence « beat-ES » (fin mai → mi-juin, deces 1,36M *engine-to-engine*, réf. **ES 8.6.1**)
Source : `docs/paper/beat-elasticsearch-campaign.md` + git log. Mesure = latence **moteur-à-moteur** (probe
`decompose`, W=2), SCW, **non bornée**, Surch **tout-en-RAM**.

| Opt | commit | Latence deces (p95 / cumul) | RAM | Indexation | Conditions |
|---|---|---|---|---|---|
| #1 rayon bulk parallel | `dd3f528` | — | — | deces **104,2 s** vs ES 115,9 (18× gap éliminé, médiane 3-rep) | SCW W2, non borné |
| #2 Posting = Copy (drop positions Vec) | `3ccdbc6` | — | postings −, RAM ↓ | = | — |
| #9 RSS peak | `ea98777` | — | **2168→907 MiB, 0,62× ES** « memory dimension WON » | = | **⚠️ TREC-COVID 171k, pas deces** (mirage révélé plus tard) |
| #10 dense u32 intersection | `8aae6a1` | 87→**70 ms** ; cumul **4513→70 ms (~64×)** ; gap vs ES 1200×→**19×** | = | = | engine-to-engine deces |
| #11/#12 leapfrog + O(n) setup | `f055c8d`/`2c59e91` | 70→**6,9 ms (~10×)** (élimination setup per-query) | = | = | idem |
| #13 criterion MET | `ae21fda` | **2,0 vs ES 4,9 ms p50 (2× + rapide)** | = | = | idem |
| #20/#21 queue bool/full | `706d539`/`c19e8fc` | bool/full **10→1,5 ms (2,1× + rapide qu'ES)** | = | = | idem |

**Lecture Phase B :** effondrement spectaculaire de la latence deces (4513 → 1,5 ms, ~3000× cumulé) — mais
**tout-en-RAM, non borné**. L'opt #9 (« memory WON, 0,62× ES ») est **⚠️ mesuré sur TREC-COVID 171k**, pas
sur deces : c'est le premier « win RAM » qui sera démenti au 06-29.

### Phase C — Scoreboards « latence 2× ES atteint » (06-09 → 06-15)
Sources : `scoreboard-2026-06-09-resurrection.md`, `-06-10-final.md`, `-06-10-mesured.md`, `-06-15-mmap-win.md`.

| Étape | commit / run | Latence p95 match/bool/full | RAM | Indexation | Disque | Parité |
|---|---|---|---|---|---|---|
| Résurrection option B + #18 NDCG | `febbc86` / `27240212526` | 1,8 / 1,5 / 1,5 ms = **2,0-2,3× ES** ✅ | ⚪ non isolable (Artillery hang) ; `stored_fields` −632 MiB | 1,12× ES | ⚪ | oracle ✅ ; SciFact +0,0062, TREC-COVID −0,0125 |
| Verdict final 06-10 | `b5722a8` / `27276634301` | 1,9 / 1,7 / 1,7 = **2,0/2,1/2,2× ES** ✅ | ⚪ « RSS non isolable » ; `stored_fields` 1187→554 MiB | **1,17×** | ⚪ « architecture pending » | oracle ✅ |
| **mmap M1** (insee-bench 10k) | `7a64941` / `27518155297` | p95 2,5 ms = **4,4× ES** ✅✅ | **RSS 81 MiB vs ES 1372 = 16,9× ✅✅** | ⚪ (bootstrap 10k) | proxy 1187 MiB=0,70× | SciFact +0,0062 |

**Lecture Phase C :** c'est le **pic de sur-vente**. « Latence 2× ES atteint sur 4/4 indicateurs » et surtout
**« RAM 16,9× mieux qu'ES »** — mais : (a) la latence est **tout-en-RAM, non bornée** ; (b) le **16,9× est
mesuré sur insee-bench 10k** (10 000 docs), pas deces ; (c) la RAM deces réelle n'est **jamais** isolée
(« Artillery hang »). Le disque n'est qu'un **proxy analytique** (0,68-0,70×), jamais mesuré live.

### Phase D — Réveil « vrai corpus » deces 1,36M (2026-06-29) — LE point de bascule
Source : `scoreboard-2026-06-29-deces-1.36M.md`, run **`28405896293`** (`sha-ea15496`), ES **8.6.1**, SCW
`ubuntu-latest`, **non borné**, Surch **tout-en-RAM**. Chiffres via checkpoint pré-Artillery.

| Axe | Surch | ES 8.6.1 | Ratio | Δ vs Phase C | Condition |
|---|---|---|---|---|---|
| Latence p95 moteur | 1,4 ms | 5,1 ms | **3,6× + rapide** ✅ | = | tout-en-RAM, non borné |
| Decompose p95 match/bool/full | 1,4 / 1,2 / 1,2 ms | 4,3 / 3,5 / 3,0 | 3,1 / 2,9 / 2,5× ✅ | = | idem |
| Indexation | 15 269 doc/s | 12 050 | **1,27×** 🟡 | +/= (les 8,9× BEIR = corpus-dép.) | idem |
| **RAM RSS pic (conteneur)** | **5418 MiB** | 1684 | **3,2× PIRE** ❌ | **−−− (le 16,9× était un artefact 10k)** | **conteneur cgroup, deces réel** |
| (RAM anon /proc, même code) | ~3800 MiB | 1684 | 2,25× pire | — | ⚠️ métrique anon, pas conteneur |
| Disque | **non capturé (=0)** | — | ⚪ **mesure cassée** | — | step a tourné, rien écrit |

**Feedback propriétaire (2026-06-29, gravé en mémoire `all-in-ram-design-flaw`) :** « Ton tout-en-RAM c'est
n'importe quoi, aucune base ne fait ça. Tu ne peux pas afficher ces perfs avec une faille de design comme ça.
La RAM : tu ne tiens pas du tout les objectifs, je ne vois même pas pourquoi tu annonces des choses
positives. » → **conséquence actée : la latence tout-en-RAM n'est PAS bankable** (achetée par l'échec RAM ;
Surch tout-RAM vs ES disk-backed = comparaison truquée). Précision 28M : « **full = 28M records**, pas 1,36M
(qui n'était qu'un mois) ». Extrapolation corrigée : ~35-40 GiB anon à 28M (et non 110 GiB — erreur qui
multipliait le RSS entier). Commit `7400434`.

### Phase E — Campagne mémoire in-RAM, 6 leviers (2026-07-01/02)
Source : `scoreboard-2026-07-02-memory-campaign.md`. **Métrique = jemalloc *resident* (≈ RSS anon /proc)**,
« la seule non-évictable ». **⚠️ Comparée à ES = 1685 MiB *conteneur*** → toute la trajectoire ci-dessous
est **anon-Surch vs conteneur-ES** (biais flatteur ; cf. §2). Deces 1,36M, SCW, **non borné**.

| Étape | commit | RAM anon (MiB) | vs ES-conteneur | Δ | Latence p95 bool/full | Indexation |
|---|---|---:|---:|---:|---|---|
| Baseline Phase 0 | `82528ea` | 3797 | 2,25× | — | 1,6-1,7 ms | ~13 000 doc/s |
| L1 flat AoS `FieldPostings` | `ccc0851` | 3421 | 2,03× | −376 | = | = |
| + purge jemalloc post-refresh | `60ded8f` | 3198 | 1,90× | −223 | = | = |
| L2 subfields dense+dict | `c7eca85` | 2490 | 1,48× | **−708** (le + gros) | = | = |
| L3 UID `Arc<str>` interné | `2126652` | 2235 | 1,33× | −255 | = | = |
| L5 SoA `doc_ids`+`freqs` | `cbf7771` | 1968 | 1,17× | −267 | full 1,2→1,6 (−, 1,9× ES) | = |
| A+B live_docs bitmap + BlockMeta | `b7d6229` | **1836** | **1,09×** | −132 | 1,6-1,7 ms | **~11 300 (−, érodé de 1,58×→parité)** |
| **Cumul** | | | | **−1961** | | |

**Lecture Phase E :** −1961 MiB d'anon, réels et propres (parité oracle ✅ à chaque levier). MAIS : (a) la
descente est en **anon vs ES-conteneur** — mélange de métriques (§2.1) ; (b) le « 1,09× ES » est donc
**flatteur** (le conteneur Surch, page-cache `source.dat` inclus, est plus lourd) ; (c) **l'indexation s'est
érodée** de 1,58× à ~parité (coût du copy flat-build + insert bitmap/dict) — un axe **régressé** au service
de la RAM. Verdict de l'étape lui-même : « le tout-en-RAM plafonne à ~parité ES ; pour ≤ES/2 + 28M il faut
le disk-backed ».

### Phase F — Disk-backed « Lot C » (2026-07-02/03)
Sources : `c1b-disk-backed-design-2026-07-02.md`, `lot-c-disk-backed-plan.md`, mémoire `deces-real-corpus`.
Métrique = **anon jemalloc-resident** (sauf ligne « honnête »). Deces 1,36M, SCW, **non borné** (sauf test 843m).

| Étape | commit / run | RAM anon | vs ES | Latence p95 m/b/f | Index | Disque | Parité |
|---|---|---:|---:|---|---|---|---|
| C0 subfields → pread | `af94e52` | 3862 (−264 anon) | — | 1,4→2,1 (−) | **8771 (−43%!)** | — | ✅ |
| **C0 REVERTÉ** | `c93bfc4` | — | — | — | — | — | ⚠️ **RSS conteneur +135 (PIRE)** ; 1 pwrite/token |
| Dédup (term,doc_id) | `22b2747` | = | — | 1,8/1,5/1,5 | 13 630 | — | oracle **0 div** ✅ ; skipped→0 |
| C1a-batché (FoR shadow) | `6b7ed47`+ / `28636114241` | = (0 gain, shadow) | — | = | 12 179 | **FoR 3,36× : 518→154 MiB** | ✅ |
| **C1b flag-ON** (pread postings) | `94e11a8` / **`28651348936`** | **1446 = 0,86× ES** | 0,86× | 2,0/2,6/2,5 (sous ES) | 13 222 | 518→160 MiB page-cache | parité flag-ON==OFF bit-identique ✅ |
| **C2 id_maps flat + drop intern** | `a664ed8` / **`28682072869`** | **881 = 0,52× ES** | **0,52×** | 2,0/2,8/2,6 (sous ES) | 12 827 (≥ES) | ⚪ cassé | frag 671→312 |
| Oracle vrai-corpus | `69668db` / **`28689787902`** | — | — | — | — | — | **`divergence_count: 0`** ✅ SCELLÉ |
| **Correction sur-claim** | `6fc04d0` | — | — | — | — | — | « 0,52× comparait anon-Surch vs conteneur-ES » |
| **Honnête conteneur vs conteneur** | (run `28682072869`) | **2378 MiB conteneur** | **1,40× ES (PIRE)** | ❌ | ❌ 1,11× | ❌ non mesuré | ✅ oracle |
| **TEST SOUS LIMITE 843m** | `b5da…` / **`28690721825`** | **OOM, count=0** (peak non borné **3264 MiB**) | ❌ | — | ❌ | — | — |

**Lecture Phase F :** l'architecture disk-backed **fonctionne** (parité bit-parfaite, anon non-évictable
3797→881, −77%). Mais deux vérités dures émergent : (1) **le disk-backed transforme l'anon en page-cache,
compté dans le RSS conteneur** → à conteneur-vs-conteneur Surch est **1,40× ES (pire)**, pas 0,52× ; (2) sous
**limite mémoire ES/2 (843m), Surch OOM à l'indexation** (pic 3264 MiB) → **RAM ≤ES/2 = définitivement NON
TENU**. Le « 0,52× ES » a été explicitement rétracté (`6fc04d0`). Insight clé gravé (mémoire
`disk-backed-pagecache-insight`) : **la mémoire ne s'évalue que SOUS une limite**.

### Phase G — A/B local ÉQUITABLE pinné (2026-07-04) — enfin « à mémoire constante »
Sources : `local-fair-ab-2026-07-04.md`, mémoire `local-fair-bench`. Harnais `deploy/bench-local/fair-ab.sh`
(`d84f293`), **docker cgroup v2, CPU pinné 8 cœurs (`--cpuset-cpus=0-7,16-23`), `--memory=M --memory-swap=M`
(swap OFF), ES `Xmx=Xms=M/2`**. Anti-triche : ES ne peut déborder ni mémoire ni CPU. **C'est la première
mesure réellement à mémoire constante.** Machine = Ryzen AI Max+ 395, gouverneur `powersave`. Surch C1b disk-backed.

**G1 — corpus LÉGER 659k (6 champs INSEE brut)** — commit `4596b76` — **⚠️ NON comparable au vrai deces** :
| axe | ES | Surch | Surch/ES | à |
|---|---:|---:|:--|---|
| Plancher de survie | **1536m** | **512m** | Surch survit à ⅓ | — |
| RSS conteneur | 1382 MiB | **65 MiB** | 0,05× (21× moins) | @1536m |
| Indexation | 48 280 doc/s | 78 971 | **1,64×** | @1536m |
| Disque | 220 MiB | 142 | **0,65×** | @1536m |
| Latence p50/95/99 | 4,4/6,1/7,6 ms | 0,8/1,1/1,6 | **~0,2× (5× + rapide)** | @1536m |

**Mais 2 issues Surch réelles trouvées** (indépendantes du bench) : (1) **undercount ~1,5%** (indexe
649 780/659 780, déterministe, `errors:false` → perte de données silencieuse) ; (2) **refresh par-chunk =
5 878 doc/s vs refresh-final 83 607** → Surch **6× plus lent qu'ES en near-real-time** (archi `rebuild_index`
reconstruit tout le TermDictionary à chaque refresh).

**G2 — VRAI corpus 1,36M (28 champs, mapping matchID réel, undercount corrigé)** — commit `7a0a2eb` — **LE verdict** :
| cap | ES | Surch |
|---|---|---|
| 768m | ❌ OOM boot | ❌ |
| 1536m | ✅ **survit** | ❌ OOM |
| 2g | ✅ | ❌ **OOM à l'indexation (count=0)** |
| 3g | ✅ | ✅ **survit** |

→ **Plancher de survie : ES 1536m vs Surch 3072m → Surch exige ~2× la RAM d'ES.** Comparaison à 3g (les deux survivent) :
| axe | ES | Surch | Surch vs ES |
|---|---:|---:|:--|
| **plancher survie** | 1536m | 3072m | ❌ **ES 2× mieux** |
| RSS steady-state @3g | 2197 MiB | 688 MiB | Surch 0,31× (mais steady, pas le pic) |
| latence p50/95/99 | 1,30/1,81/2,10 ms | 0,39/0,57/0,63 | ✅ **Surch ~0,30× (3,3× + rapide, seul axe ≥2×)** |
| indexation | 28 513 doc/s | 29 842 | ~parité (1,05×) |
| disque | 653 MiB | **744** | ❌ **Surch 1,14× (pire)** |

Undercount résolu = compris : **Surch rejette TOUT le `_bulk` (HTTP 400) sur 1 doc invalide** vs ES par-item
(écart de résilience). 28M non lancé (`build-28M.sh` prête, ~15 Go NDJSON).

---

## 2. CHIFFRES NON COMPARABLES — signalés explicitement

Chaque paire ci-dessous a été **comparée dans la trajectoire alors que les conditions différaient**. C'est
la source de la dérive que le propriétaire a détectée.

**2.1 — RSS anon (jemalloc-resident) vs RSS conteneur (cgroup).** LE biais central. Toute la campagne in-RAM
(Phase E, 3797→1836) et les paliers disk-backed (Phase F, 1446 « 0,86× », 881 « 0,52× ») sont en **anon
Surch** comparés à **1685-1698 MiB de conteneur ES**. Or : (a) le conteneur inclut le page-cache
(`source.dat`, et après C1b les postings pread) que le cgroup compte ; (b) même à code identique, deces
`sha-ea15496` = **3800 MiB anon = 2,25×** MAIS **5418 MiB conteneur = 3,2×** vs le même ES 1684. → **« 0,52× ES »
et « 1,09× ES » sont invalides** ; conteneur-vs-conteneur honnête = **1,40× ES** (2378 vs 1698, run `28682072869`).
Rétracté commit `6fc04d0`. Règle : **toujours conteneur-vs-conteneur, et sous limite.**

**2.2 — insee-bench 10k (16,9×) vs deces 1,36M (3,2× pire).** Le « RAM 16,9× mieux qu'ES » du 06-15
(`scoreboard-2026-06-15-mmap-win`, RSS 81 vs 1372 MiB) est mesuré sur **10 000 docs**. Le même moteur sur
deces 1,36M = **3,2× PIRE**. Facteur ~50× d'écart de conclusion dû au seul corpus. Le page-cache est
minuscule à 10k → l'anon domine et paraît gagner ; à 1,36M il ne domine plus. **« Small-corpus mirage »**
(commit `ba5a24a` : « memory win was a small-corpus mirage — deces 5,8× worse »).

**2.3 — Corpus 6 champs (fair-ab 659k) vs 28 champs (deces réel).** Fair-ab G1 : RSS Surch **65 MiB** ;
deces G2/SCW : anon **881 MiB** et **OOM à 843m**. Même moteur, ~13× d'écart de poids/doc (FST + postings +
subfields + id_maps scalent avec la richesse du mapping). **G1 valide le HARNAIS, ne prouve RIEN sur le
verdict.** Ne jamais citer les chiffres 659k comme « Surch bat ES ».

**2.4 — Réf. OpenSearch 2.17.1 (mai, TREC-COVID) vs Elasticsearch 8.6.1 (juin, deces).** Les gains Phase A
(Lots 1-3) sont vs **OS 2.17.1** sur **TREC-COVID 171k** ; la campagne deces est vs **ES 8.6.1**. Moteur ET
corpus différents — les ratios ne se chaînent pas. L'opt #9 « 0,62× ES » est en fait **TREC-COVID 171k**, pas
deces (commit `ea98777`).

**2.5 — Latence tout-en-RAM non bornée (Surch) vs ES disk-backed.** Tous les « latence 2-3,6× ES » des Phases
B/C/D sont **Surch tout-en-RAM vs ES disk-backed, sans borne mémoire**. Par construction (mémoire
`all-in-ram-design-flaw`) c'est une **comparaison truquée** : l'avantage est **acheté par l'échec RAM**. Un
Surch disk-backed warm verrait l'écart fondre ; le cold régresserait. **Non bankable** tant que non mesuré
sous limite, warm ET cold.

**2.6 — SCW non borné vs local pinné.** Débits SCW (`ubuntu-latest`/burst, W=2) vs local (Ryzen 8 cœurs
pinnés, NVMe, gouverneur `powersave`). L'indexation locale (29-79k doc/s) est **gonflée** vs SCW (~12-15k)
pour les DEUX moteurs (docs légers + Zen5 + NVMe local). Comparer un doc/s SCW à un doc/s local = invalide.

**2.7 — Indexation bulk-puis-refresh-unique vs near-real-time.** « 1,64× » (fair-ab refresh final) devient
**0,17× (6× plus lent)** en refresh par-chunk. L'axe indexation n'a pas UN chiffre ; il dépend du pattern
de refresh. Les « 1,17-1,58×» annoncés sont tous en refresh-final-unique.

**2.8 — Disque : proxy analytique (0,68×) vs mesuré local (1,14× pire) vs cassé SCW (=0).** Trois « valeurs »
disque incohérentes : estimation `disk-axis-proxy.md` (~820/1200 = 0,68×), mesure SCW **jamais écrite (=0)**,
mesure locale réelle @3g = **744 vs 653 = 1,14× PIRE**. Seule la dernière est une vraie mesure à conditions
égales, et elle est **défavorable**.

**2.9 — Steady-state vs pic d'indexation.** RSS steady @3g = 688 MiB (Surch 0,31× ES) — chiffre flatteur
souvent cité. Mais le **plancher de survie** est fixé par le **pic d'indexation** (OOM à 2g, peak 3264 MiB
sous 843m). Comparer le steady-state de Surch au plancher d'ES cache le vrai coût.

---

## 3. RELECTURE À MÉMOIRE CONSTANTE — ce qu'on SAIT vraiment à budget fixe égal

On ne garde ici QUE les mesures où les deux moteurs ont **le même budget mémoire cgroup** (les seules
valides selon le principe). Deux sources qualifiées : le **test sous limite SCW 843m** (`28690721825`) et
le **harnais fair-ab local pinné** (`7a0a2eb`, deces 1,36M, 28 champs, caps égaux, CPU pinné).

### 3.1 Plancher de survie (à corpus deces 1,36M réel, caps égaux)
| budget cgroup | ES 8.6.1 | Surch (C1b disk-backed) |
|---|---|---|
| 843m (=ES/2 SCW) | — | ❌ **OOM indexation (count=0, peak 3264)** |
| 768m (local) | ❌ OOM boot | ❌ |
| 1536m (local) | ✅ **survit** | ❌ **OOM** |
| 2g (local) | ✅ | ❌ **OOM indexation** |
| 3g (local) | ✅ | ✅ **survit** |

→ **Fait dur : ES survit à 1536m, Surch exige 3072m. À mémoire constante, c'est ES qui tient à ≤ MOITIÉ de
la RAM de Surch. L'axe RAM n'est pas seulement raté, il est INVERSÉ par un facteur 2.** Cause = le **pic
mémoire du build tout-en-RAM** (l'index se construit entièrement en heap avant écriture segments), que le
disk-backed ne casse PAS (il ne borne que le steady-state de lecture).

### 3.2 Comparaison à budget commun où LES DEUX survivent (3g, deces 1,36M)
| axe | ES | Surch | verdict à mémoire constante |
|---|---:|---:|:--|
| Latence p95 (b/f) | 1,81 / 2,10 ms | 0,57 / 0,63 | ✅ **Surch 3,3× + rapide (≤ES/2)** — SEUL axe ≥2× |
| Indexation (refresh final) | 28 513 doc/s | 29 842 | = parité (1,05×) |
| Indexation (near-real-time) | stable | ~6× plus lent | ❌ (extrapolé de G1) |
| Disque | 653 MiB | 744 | ❌ 1,14× pire |
| RSS steady-state | 2197 MiB | 688 | (Surch bas, mais non déterminant — cf. plancher) |
| Plancher de survie | 1536m | 3072m | ❌ **ES 2× mieux** |
| Parité oracle | — | 0 divergence | ✅ |

### 3.3 État réel par axe, SOUS discipline « mémoire constante »
- **RAM** : ❌ **INVERSÉ ×2**. Surch a besoin de 2× la RAM d'ES pour survivre au vrai corpus. OOM à ES/2.
  (`28690721825`, fair-ab G2). Confiance **HAUTE**.
- **Latence** : ✅ **≥2× tenu** (3,3× à 3g, caps égaux, disk-backed). C'est le seul axe qui survit à la
  discipline. Confiance **MOYENNE-HAUTE** — voir §3.4 (mesuré là où Surch est sur-provisionné).
- **Indexation** : ❌ **parité** (1,05×) en refresh-final ; **6× pire** en near-real-time. Loin du 2×.
- **Disque** : ❌ **1,14× pire** (seule vraie mesure à conditions égales). Confiance MOYENNE (une seule mesure).
- **Qualité/parité** : ✅ oracle 0 divergence (multiple runs, dont `28689787902`). SciFact bat OS ;
  TREC-COVID −0,0125 (viole la parité STRICTE) ; NFCorpus/FiQA OK.

### 3.4 Ce qui reste NON mesuré à mémoire constante (trous à combler)
1. **Latence ES vs Surch au plancher de CHACUN.** À 3g, ES est **sur-provisionné** (survit à 1536m). La
   comparaison loyale = ES à son minimum (1536m, page-cache réduit) vs Surch à son minimum (3g). Non fait.
2. **Warm vs cold** (`fadvise DONTNEED`) sous pression page-cache. Le coût cold du disk-backed n'a JAMAIS
   été mesuré sous limite — or c'est là que l'avantage latence est censé fondre (mémoire `all-in-ram`).
3. **Artillery / full end-to-end sous borne.** Le load-test tue le runner ; jamais tourné sous cap mémoire.
4. **28M sous borne.** Le vrai endgame ; jamais lancé (setup prêt).
5. **Latence sous limite mémoire = ES/2 pour Surch** : impossible (OOM), donc la vraie question devient
   « sous quelle limite Surch survit-il, et à quelle latence cold/warm à cette limite ? » — non mesuré.

---

## 4. MATRICE DES TESTS OUBLIÉS / CASSÉS À REJOUER (priorisée)

| # | Test | Pourquoi il manque | Comment le rejouer | Décision débloquée |
|---|---|---|---|---|
| **P0** | **Undercount 1,5% / rejet total `_bulk` sur 1 doc** | Bug de complétude découvert 07-04 (G1) ; perte de données silencieuse `errors:false` | Repro `fair-ab.sh` 659k puis 1,36M ; comparer counts ; corriger le bulk_router pour rejet **par-item** (comme ES) | **Bloquant absolu** : aucun chiffre n'a de valeur si Surch perd des docs |
| **P0** | **Latence sous limite mémoire (pression page-cache) — warm ET cold** | JAMAIS mesuré ; la latence a toujours été prise Surch tout-RAM/sur-provisionné | `fair-ab.sh` deces 1,36M, cap = plancher de survie Surch (3g) ET plancher ES (1536m), `fadvise DONTNEED` avant 1re requête | **La latence 3,3× est-elle réelle disk-backed ou un reliquat tout-RAM ?** |
| **P0** | **28M borné mem+CPU** | Le full = 28M (pas 1,36M) ; jamais lancé ; extrapolé ~35-40 GiB anon = OOM tout-RAM | `build-28M.sh` (data.gouv public) + `fair-ab.sh` pinné, caps égaux, POSTINGS_DISK=1 | **Surch survit-il seulement au vrai corpus ? L'endgame.** |
| **P1** | **Axe disque à conditions égales (real corpus)** | SCW cassé (=0) ; proxy 0,68× ; seule vraie mesure locale = 1,14× pire | Déjà dans `fair-ab.sh` (`du -sb` volume) ; re-confirmer @3g + @28M | **Verdict disque** (actuellement défavorable, 1 seule mesure) |
| **P1** | **Refresh near-real-time (indexation soutenue)** | Découvert 07-04 : refresh par-chunk = 6× plus lent (archi `rebuild_index`) | `fair-ab.sh` option `REFRESH_EACH=1`, sweep fréquence de refresh | **Verdict indexation** selon pattern (bulk vs NRT) |
| **P1** | **Plancher de survie de CHACUN + latence à iso-budget minimal** | À 3g, ES sur-provisionné ; comparaison non loyale | `fair-ab.sh` : ES@1536m vs Surch@3g, mesurer latence de chacun à SON minimum | **Comparaison RAM/latence vraiment loyale** |
| **P2** | **Artillery / full matchID end-to-end sous borne** | Runner `ubuntu-latest` meurt à 1,36M ; jamais sous cap | Local `fair-ab.sh` + scénario artillery matchID, cap mémoire égal | **Comportement de queue (p99/max) sous charge + borne** |
| **P2** | **TREC-COVID raw latency cache-OFF vs OS** | Cible « 2e » du master plan (~302 vs OS ~184 ms = 1,6× plus lent), jamais fermée | `ndcg-gate` cache-off (`SURCH_DISABLE_SEARCH_CACHE=1`) sous borne | **Compétitivité en lecture froide** (le cas honnête) |
| **P2** | **Gate NDCG mMARCO-fr (récupération française)** | Parité qualité validée sur SciFact/TREC-COVID/NFCorpus/FiQA mais **pas** sur un dataset FR | Ajouter mMARCO-fr au `beir-ndcg.sh` ; gate NDCG@10 ≥ OS | **Parité qualité sur du français** (cas d'usage matchID réel) |
| **P2** | **TREC-COVID NDCG −0,0125 résiduel** | Viole la parité STRICTE ; SmallFloat n'a fermé que +18% | Root-cause au-delà de SmallFloat (norm boost, coord factor, idf rounding) | **Parité qualité STRICTE** (gate sacré) |
| **P3** | **Test long / soak quotidien** | Jamais fait ; rétention allocateur & fuites non observées dans la durée | Run continu (heures) sous `fair-ab.sh`, surveiller anon + conteneur dans le temps | **Stabilité mémoire en régime permanent** |

---

## 5. VERDICT RÉTROSPECTIF HONNÊTE — par axe, vs l'objectif ≥2× ES

| Axe | Cible | Verdict à mémoire constante | Confiance | Source(s) principale(s) |
|---|---|---|---|---|
| **RAM** | ≤0,5× ES | ❌ **ÉCHEC, INVERSÉ ×2**. Surch exige ~2× la RAM d'ES pour survivre (plancher 3072m vs 1536m) ; OOM à ES/2 (843m/2g), pic d'indexation 3264 MiB. Les « 0,52× / 1,09× / 16,9× » étaient tous hors-discipline (anon-vs-conteneur, ou corpus 10k). | **HAUTE** | run `28690721825` ; `local-fair-ab-2026-07-04` ; correction `6fc04d0` |
| **Latence** | ≤0,5× ES | ✅ **TENU (3,3× + rapide)** — le seul axe ≥2× qui survit à la discipline (caps égaux, 3g, disk-backed). **MAIS** mesuré là où Surch est sur-provisionné ; **jamais** cold ni au plancher de chacun. « Non bankable » partiellement levé (disk-backed + caps égaux), pas totalement. | **MOYENNE-HAUTE** | `local-fair-ab-2026-07-04` (G2) ; `scoreboard-2026-06-29` |
| **Indexation** | ≥2× ES | ❌ **ÉCHEC (parité 1,05×)** en refresh-final ; **~6× PIRE** en near-real-time (archi `rebuild_index`). Jamais proche de 2×. | **HAUTE** | `local-fair-ab-2026-07-04` (G1/G2) ; scoreboards 06-10/06-29 |
| **Disque** | ≤0,5× ES | ❌ **ÉCHEC (1,14× PIRE)** à conditions égales (seule vraie mesure). Proxy 0,68× démenti, mesure SCW cassée (=0). | **MOYENNE** (1 mesure loyale) | `local-fair-ab-2026-07-04` (@3g) ; `disk-axis-proxy` |
| **Qualité / parité** | parité STRICTE | ✅ **oracle 0 divergence** (bit-parfait vs ES/OS, multiple runs) ; SciFact bat OS. 🟡 **TREC-COVID −0,0125 viole la parité stricte** ; mMARCO-fr non testé. | **HAUTE** (oracle) / **MOYENNE** (BEIR) | `28689787902` ; `scoreboard-2026-06-10-final` |
| **Corpus** | 28M (full) | ❌ **jamais atteint** — toute la campagne est à 1,36M (« un mois »), voire 171k/10k. 28M en cours de setup local. | — | mémoire `deces-real-corpus` ; feedback owner « full = 28M » |

### Synthèse
Sur les 5 axes de l'objectif « ≥2× ES sur CHAQUE axe », à mémoire constante et sur le vrai corpus :
**1 tenu (latence, avec réserves cold/plancher), 1 parité solide (qualité oracle), 3 échecs (RAM inversée,
indexation parité, disque pire), et le corpus cible (28M) jamais mesuré.** L'objectif global **n'est PAS
atteint**, et l'axe présenté comme le plus gagné (RAM, « 16,9× / 0,52× ») est en réalité **le plus perdu**
une fois la discipline appliquée.

### La leçon centrale de l'audit
La dérive vient d'**un seul mécanisme répété** : comparer une métrique **avantageuse pour Surch** (anon,
steady-state, petit corpus, refresh unique, non borné) à une métrique **complète pour ES** (conteneur,
plancher, gros corpus, borné). Le principe **« à mémoire constante »** — budget cgroup fixe et égal,
conteneur-vs-conteneur, sur le vrai corpus, en incluant le pic d'indexation — est le seul garde-fou. Le
harnais `fair-ab.sh` (2026-07-04) est le premier instrument qui l'applique ; il doit devenir la **seule base
de claim**, et rien ne doit reclamer « bat ES ×2 » avant P0/P1 verts et le 28M mesuré.

---

### Annexe — index des runs cités
- `28405896293` (`sha-ea15496`, 06-29) : baseline deces 1,36M, RAM 3,2× pire, latence 3,6× — Phase D.
- `28636114241` (C1a-batché) : FoR 3,36×, 12 179 doc/s — Phase F.
- `28651348936` (`sha-94e11a8`) : C1b flag-ON, anon 1446 = 0,86× ES — Phase F.
- `28682072869` (`sha-a664ed8`) : C2+C1b, anon 881 = 0,52× ; **honnête conteneur 2378 vs 1698 = 1,40×** — Phase F.
- `28689787902` (`sha-69668db`) : oracle vrai-corpus VERT, `divergence_count: 0` — Phase F.
- `28690721825` (flag-ON, `--memory=843m`) : **OOM count=0, peak 3264** → RAM ≤ES/2 NON TENU — Phase F.
- `27518155297` (`sha-7a64941`, 06-15) : insee-bench 10k, RSS 16,9× (mirage petit corpus) — Phase C.
- Commits locaux `d84f293`/`4596b76`/`7a0a2eb` (07-04) : harnais `fair-ab.sh` + G1 (659k) + G2 (1,36M) — Phase G.
