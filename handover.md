# Handover surch — 2026-05-25 (HEAD `bec6d75`)

Document de passation pour Codex. Contient : (1) les règles
durables à respecter, (2) l'état de chaque track, (3) les prochaines
actions. **Les §2/§3 plus bas datent de `2e4361e` — le bloc
"Avancement loop 2026-05-25" ci-dessous est l'état à jour et prime.**

---

## Avancement loop 2026-05-25 (autonome, prime sur §2/§3)

Session autonome `/loop` pendant absence user. État à `fa9ae0d`.
**Loop arrêté à la frontière des décisions (voir "Décisions
attendues" en fin de bloc).**

### Bilan du loop (commits `2e4361e` → `fa9ae0d`)
- Lot 3 + A10 validés K8s & publiés (Lot 3 latence-neutre honnête ;
  A10 parité b1-oracle 30/30).
- 2 fixes infra Track E : `bench_report` SLO RSS Surch-only
  (`e37a864`) + wait-loop tolère SIGTERM sidecar reason=Error
  exit=143 (`97e81f3`) → insee-bench teardown fiable.
- Objectif F : F1 méthodo (`docs/paper/methodology.md`), **F2
  complet** (bulk+RSS+qualité 3-rep `2026-05-25-F2-ndcg-3rep-K8s` +
  latence 3-rep `2026-05-25-F2-insee-3rep-K8s`), **F5 premier draft
  d'article** (`docs/paper/draft.md`), F3 investigué (blocage
  documenté).
- Multi-rep paper-ready : bulk TREC-COVID Surch médiane `70.96 s`
  (non-recouvrant vs OS `109.73 s`), RSS `2168 MiB ±0.5%`, latence
  Surch `1.5/4.1/8.4/40.6 ms` (2.7–3.1x < OS), NDCG bit-stable.
- Ménage worktrees + branches temp (`.claude` 1.3G→32K).

### Décisions attendues (le loop est bloqué là-dessus)
1. **F3 — replays historiques** : les SHAs historiques
   (71ceb275/5081cc7/3157afb/e38bf91) n'ont pas la surface
   CI/Docker. Les isoler = greffer le harness moderne sur du code
   ancien (risque de non-compilation rustc 1.91.1), gros effort ROI
   incertain. **Investir, OU publier l'article sur les seuls lots
   récents** (déjà isolés + multi-rep) en citant les historiques
   comme "delivered, mesurés cumulativement" ?
2. **F4 — charges additionnelles** : ajouter un harness de latence
   search **grand corpus** (artillery TREC-COVID-scale) prouverait le
   régime de bénéfice de Lot 3 (aujourd'hui non mesurable) ; +
   BEIR multi-dataset + sweep de taille. Priorité ?
3. **Priorité Track D vs F** : enchaîner Phase 4 matchID (A1/A13,
   A7, A2, A5, A12 consommant `.raw`/`.norm` d'A10) OU concentrer sur
   l'article (F3/F4/F5) ?

### (Ancien état au lancement du loop, conservé pour trace)

### Fait (depuis `2e4361e`)
- **Lot 3** (Track A, MaxScore block-leapfrog via skip lists Lot 2) :
  mergé, correctness prouvée (ranking bit-stable, `ci` vert), mais
  **latence-neutre sur INSEE 10k** (posting lists trop courtes) →
  gardé, pas revendiqué comme gain. Rapport
  `docs/ops/bench-reports/2026-05-25-lot3-bmw-skiplist-K8s/`.
- **A10** (Track D Phase 4, write-time fan-out `.raw`/`.norm`) :
  mergé, parité matchID préservée (b1-oracle 30/30, 0 divergence).
  Rapport `docs/ops/bench-reports/2026-05-25-b1-oracle-A10-ES861-K8s/`.
  Consommation query-side (sort/agg sur `.raw`) déférée à A1/A12.
- **Objectif F** ouvert : `plan/wp-f-perf-paper.md` (gap analysis +
  verdict faisabilité) ; **F1** = `docs/paper/methodology.md` livré.
- **2 fixes infra Track E** : `bench_report` SLO RSS gate Surch-only
  (`e37a864`) ; wait-loop ne traite plus SIGTERM sidecar
  (reason=Error exit=143) comme erreur terminale (`97e81f3`). Les
  deux débloquent insee-bench (teardown vert).
- **Lot 1.6 / Lot 2** isolés et publiés (bulk parity crossed + skip
  lists search `p95 -13% / p99 -18%`).
- Ménage : worktrees agents + branches temp supprimés (`.claude`
  1.3G→32K).

### Reste % par track (à jour)
- **A** ~2% : Lots 1→3 livrés. Reste Lot 4 (replays historiques,
  bloqué) + (F-gap-4) harness latence grand corpus pour prouver Lot 3.
- **D** Phase 4 active : A10 fait. Reste A1/A13, A7, A2, A5, A6/A13,
  A12 (consomme `.raw`/`.norm`), B2.
- **F** ~75% : F1 fait. Reste F2 (multi-rep médiane+IQR), F3
  (débloquer replays historiques), F4 (charges + harness latence
  grand corpus), F5 (draft).
- **B / C / E** : clos (E : 2 fixes wait-loop/SLO ce cycle).

### Prochaines actions (ordre, toutes autonomes sauf mention)
1. **F2** — multi-rep (≥3) médiane+IQR des lots récents
   (ndcg-gate bulk+RSS, insee-bench latence) pour passer du
   single-run au verdict final. EN COURS de démarrage.
2. **F4 / harness latence grand corpus** — ajouter un artillery
   TREC-COVID-scale pour mesurer le régime où Lot 3 aide.
3. **F3** — débloquer Lot 4 (refs replay aux SHAs historiques +
   surface CI/K8s) ; gros, possible blocage technique.
4. **Track D A1/A12** — consommer `.raw`/`.norm` (A10) dans
   sort/agg ; **décision user souhaitable** sur la priorité D vs F.

### Décisions user — TRANCHÉES le 2026-05-25
1. **Priorité = Track D Phase 4** (matchID), pas Objectif F.
2. **Isolation MaxScore grand corpus : NON** — citer comme livré,
   bénéfice grand corpus, contribution individuelle non chiffrée.
3. **Anciennes optimisations (replays historiques / F3) : OUI** — greffer
   l'outillage benchmark actuel sur les vieux SHAs et les chiffrer une à une
   (backlog, après la priorité D).
4. **BEIR multi-datasets (NFCorpus/FiQA) : OUI** — réécrire le téléchargeur
   de corpus en shell (pas de Python) puis ajouter les jeux (backlog F).

Ordre d'exécution : D d'abord (A1/A13), puis F3 + BEIR en backlog F.

### EN ATTENTE — décision user : rapport perf matchID end-to-end (2026-05-26)
Le user veut une publication perf matchID via le **vrai** test artillery du
CI/CD matchID (`deces-backend make test-perf-v1`) sur son corpus défini, avec
**dataprep**, idéalement sur une **branche matchID où Surch remplace Elastic
en end-to-end** (flip `ELASTIC_URL` → Surch, cf. `docs/wp-d-matchid/swap-guide.md`).
matchID est checkouté en `/home/antoinefa/src/matchID` (monorepo
`matchID/packages/{deces-backend,deces-dataprep}` ; ~8 copies dupliquées
ailleurs). deces-backend restaure le corpus depuis un **snapshot ES** d'un
bucket (`fichier-des-personnes-decedees-elasticsearch`). **4 questions posées
au user (corpus/snapshot accessible ? peuplement Surch = restore snapshot vs
dataprep ? local docker-compose vs CI ? quel repo/branche ?)** — bloqué tant
que non répondu. NE PAS toucher l'environnement matchID avant.

### F3 (isolation perf) — EN COURS sur branche `perf-isolation` (jamais mergée main)
Décision user : isoler via toggles de mesure sur une branche isolée (pas de
flag en prod). 1er PoC : `SURCH_DISABLE_MAXSCORE` (toggle WAND/MaxScore, lu une
fois, défaut activé). Branche `perf-isolation` poussée (`6e1846d`) ; run
trec-covid-latency maxscore-OFF en cours → comparer à la médiane 3-rep
maxscore-ON (`2026-05-25-F4-trec-covid-latency-3rep-K8s`) pour le delta WAND.

### Track D Phase 4 — avancement (2026-05-26)
- **A1/A13 (autocomplete edge_ngram multi-field) : CERTIFIÉ parité ES 8.6.1**
  (b2-oracle 8/8, 0 divergence ; b1-oracle deces_v1 reste 30/30). Gate
  `b2-oracle-gate` opérationnel.
- **A7 (dates runtime) : fait** (range conscient des dates + date-math
  now±N, e2e). matchID garde DATE_NAISSANCE en keyword (placeholders INSEE).
- **A2 (geo) : fait** (geo_bounding_box + geo_polygon, e2e ; geo_distance
  préexistant).
- **A5 (scoring) : decay fait** — exp/linear decay livrés + validés (unifiés
  en `ScoringFunction::Decay` + `DecayKind`, tests e2e). Restent 2 items
  décision-gated : **random_score** (parité bit-à-bit ES infaisable, RNG
  différent ; matchID ne l'utilise pas → défaut hors-scope) et **script_score**
  (= moteur de script, décision scope).
- Reste aussi A6 (keyword-prefix side-table, optionnel) + A12 composite/
  histogram (partiellement couvert).

### Bilan Track D Phase 4 (axes query)
A1/A13 (CERTIFIÉ ES 8.6.1), A7 (dates+date-math), A2 (geo bbox+polygon),
A5-decay (gauss/exp/linear) : **faits et validés CI**, régression-safe
(b1-oracle 30/30). Le reste (random_score, script_score, A6) est
décision/jugement-gated ou optionnel.

### Décision scope ouverte (A5)
- **script_score** : nécessite un moteur d'évaluation d'expressions (mini
  painless). Sous-système conséquent, peu utilisé par matchID. Construire un
  évaluateur minimal OU déclarer hors-scope ? (exp/linear decay + random_score
  sont eux contenus et seront faits sans décision.)

### (historique) Décisions initialement en attente
- Priorité après F2 : approfondir Track D Phase 4 (A1/A12…) ou
  Objectif F (F3 replays historiques) ? → tranché : D.
- **Isolation Lot 3 (MaxScore) sur grand corpus** : le harness F4
  `trec-covid-latency` permet enfin de mesurer le régime de Lot 3
  (longues listes), mais il n'existe AUCUN toggle runtime de MaxScore
  (câblé en dur dans `search.rs:1793/1803`). Pour produire un contrôle
  « sans Lot 3 » il faut soit (a) ajouter un flag de mesure
  (env `SURCH_DISABLE_MAXSCORE`) — va à l'encontre de la règle
  « pas de feature flags », mais isole proprement depuis HEAD ; soit
  (b) porter le harness F4 (job + `--query-mode trec` + `--rss-peak-mb`)
  sur le SHA parent de `e293cfc` (plumbing style F3, lourd, risque de
  build avec le toolchain actuel). **Décision : (a) flag de mesure
  temporaire, (b) port historique, ou (c) renoncer à l'isolation
  large-corpus de Lot 3 et le citer comme « livré, neutre sur INSEE,
  bénéfice grand-corpus non isolé » ?** Le loop avance par défaut sur
  le multi-rep F4 (sans décision requise) en attendant.
- **BEIR multi-datasets (F-gap-4, généralité qualité)** : ajouter
  NFCorpus/FiQA pour élargir la preuve qualité. BLOQUÉ par 2 frictions :
  (1) le job `deploy/k8s/jobs/00-init-corpora.yaml` qui provisionne les
  corpus est en **Python** — l'étendre violerait la règle no-Python ;
  (2) effort : re-download ~GiB dans le PVC `surch-corpus-beir`, +
  scripts NDCG par dataset, + seuils SLO `bench_report`, + câblage
  ci-k8s. **Décision : investir (réécrire l'init en shell + ajouter les
  datasets) ou rester sur SciFact+TREC-COVID pour le premier article ?**
- **État Objectif F au {2026-05-25}** : la story « lots récents » est
  complète et rigoureuse — bulk (F2 3-rep), RSS (F2 3-rep), latence
  INSEE (F2 3-rep), latence grand corpus (F4 3-rep + équivalence
  in-artefact), qualité (NDCG stable), parité matchID (A10+A12). Restent
  pour un article « complet » : F3 (historiques, bloqué), isolation
  Lot 3 (décision ci-dessus), F5 figures, généralité BEIR (décision).

---

## 1. Règles durables (NON négociables)

### Communication
- **Français** pour parler au user. Commits/messages techniques en
  anglais conventionnel (`[wp-x] type(scope): subject`).
- **Format de reporting obligatoire** à chaque jalon, en tableaux :
  - **Fait** : ce qui a été livré (hash + effet concret).
  - **À faire** : *par track*, avec **finalité** + **reste %** +
    **action concrète**.
  - **Attendus** : soit le next step automatique, soit une décision
    user (posée via l'outil `AskUserQuestion`, avec contexte +
    option recommandée en premier).
- **Pas de jargon interne** dans les rapports : pas de « promo »
  (dire « publier le rapport sous `docs/ops/bench-reports/…/` »),
  pas de « ledger » (dire « mettre à jour le tableau de bord
  performance `docs/ops/bench-reports/track-a-performance-ledger.md` »),
  pas de « Lot N » seul sans dire ce qu'il fait.
- **Pas d'état des lieux fainéant** : toujours le format
  Fait/À faire(tous tracks)/Attendus, jamais un simple « j'attends ».

### Git / commits
- Branche unique **`main`**. Push régulier après gates verts.
- **STRICT — aucune attribution Claude/Anthropic** : pas de
  `Co-Authored-By: Claude`, pas de « Generated with Claude Code »,
  pas de 🤖. Signature unique `antoinefa <fabien.antoine@m4x.org>`.
  Vérifier après chaque commit :
  `git log -1 --format="%B" | grep -iE "claude|anthropic|🤖|co-authored"`
  → doit être vide.

### Validation — PAS DE TESTS LOCAUX
- Le user a interdit les tests/builds lourds locaux (ils faisaient
  planter la session) : **ne pas** lancer `cargo test`,
  `cargo clippy --workspace`, `cargo build` lourd.
- **Seul autorisé en local** : `cargo fmt --all -- --check` (et
  `cargo fmt --all`), `cargo update -p <crate>` (résolution
  lockfile, métadonnée only), inspection git/grep/read.
- **Toute validation passe par le remote** :
  - Le workflow `ci` (cargo test workspace) se déclenche
    automatiquement sur push → c'est le juge compile + tests.
  - `ci-k8s` (dispatch manuel) pour les preuves perf K8s.
- Boucle de validation Rust :
  ```bash
  gh workflow run docker-build.yml -R rhanka/surch -r main   # ~7 min
  gh workflow run ci-k8s.yml -R rhanka/surch -r main -f job=ndcg-gate  # ~22-30 min
  ```
  Jobs `ci-k8s` : `ndcg-gate` (SciFact+TREC-COVID NDCG/Recall+RSS),
  `insee-bench` (latence artillery), `b1-oracle-gate` (matchID
  parité ES 8.6.1), `00-init-corpora`.

### Autonomie
- Mode autonome maximal : ne PAS demander validation pour
  `cargo fmt`, `git add/commit/push`, édits, dispatch `gh workflow`.
- Parler aux jalons (commit poussé) avec le format de reporting.
- Décisions de scope réelles → `AskUserQuestion` (jamais en prose).

### Code
- **Pas de Python** dans la codebase (tout Rust ; bash pour les
  scripts ops).
- Commentaires : « why » seulement, pas « what ».

### Auth gh
- Si `gh auth status` est cassé, restaurer depuis le cache git
  (un `git push` récent a marché) AVANT de demander au user :
  ```bash
  printf 'protocol=https\nhost=github.com\n\n' | git credential fill \
    | awk -F= '/^password=/{print $2}' | gh auth login -h github.com --with-token
  ```

---

## 2. État des tracks (format de reporting)

### Fait (séquence de cette série de sessions, du plus récent au plus ancien)

| Hash | Track | Effet |
|------|-------|-------|
| `2e4361e` | A | **Lot 1.6** — FST term-dictionary build différé hors bulk (`add_documents_with_mapping_deferred`, `materialize_terms` lazy, `ensure_terms_ready` sur 7 entrées read). Validation K8s EN COURS. |
| `d73c862` | A | **Lot 2** — skip lists sur postings FoR + leapfrog AND (`surch-codec` + `surch-search`). Validation K8s EN COURS. |
| `1baf5a5` | C | Sync PLAN.md Track C → 0%. |
| `75a7b35` | C | **Lot 4** — `scripts/verify-release.sh` tag-driven fail-closed + `docs/ops/release-verification.md`. |
| `99d9f33` / `b9f6636` | A | **Lot 1.7 jemalloc** — RSS pic `-39%`, RSS final `-75%`, bulk `-26%`. Rapport `2026-05-24-ndcg-gate-lot1.7-jemalloc-K8s/`. |
| `8a5150f` / `7f27cc5` | A | **Lot 1.5** — `refresh_index` libère le PostingsBuilder. Gain modeste (glibc) avant jemalloc. |
| `367acdc` / `8571bb9` | A | **Lot 1** — append incrémental bulk : TREC-COVID `1002 s → 180 s`. |
| `04fde72` | E | Wait-loop `ci-k8s` tolère `exit=143` (SIGTERM sidecars). |
| `137b352` / `4df387f` / `975eea4` / `7da0718` | B/E | Chaîne RSS sampler K8s + fermeture Track B + Track E. |

### À faire (par track : finalité + reste % + action)

| WP | Finalité | Reste % | Action concrète |
|----|----------|---------|-----------------|
| **A — perf/optim** | Gains search+index mesurables sans régression qualité (NDCG@10/Recall@10), prouvés par run K8s paired Surch vs ES/OS, avec ligne dans le tableau de bord performance. | ~8% | (0) **EN COURS** : valider Lot 1.6 + Lot 2 (run ci-k8s `26373579876`). (1) **Lot 3** — next Block-Max WAND step sur les skip lists de Lot 2 (dépend de Lot 2). (2) **Lot 4** — replays historiques A-replay-1/2/3, BLOQUÉ (les SHAs anciens n'ont pas `docker-build.yml`/`ci-k8s.yml`, cf. `plan/perf-replay-wp-a-algo-ledger.md`). |
| **B — test-auto** | Chaîne benchmark rejouable Surch vs ES/OS, paired RSS + verdict SLO explicite. | 0% | clos. Réagir seulement si un rapport régresse. |
| **C — ops/snapshots** | Release + snapshot vérifiables bout-à-bout (signing + SBOM + restore). | 0% | clos. `scripts/verify-release.sh` s'exécutera réellement quand un tag `v0.1.0` sera émis. |
| **D — matchID** | Parité matchID prouvée vs Elasticsearch 8.6.1 sur corpus deces_v2 INSEE étendu (multi-field, dates, geo, edge_ngram, sort/agg/composite). | B1 0% / Phase 4 backlog inactif | Phase 4 = 8 lots / ~28 leaves dans `plan/wp-d-matchid-phase4.md`. Premier débloquant : **A10 write-time fan-out** (`.raw`/`.norm`), seul refactor indexation de la phase, les 7 suivants en dépendent. ~500 LoC. |
| **E — infra K8s** | `ci-k8s` cible heavy fiable avec diagnostics préservés (Job conditions, pod describe, live metrics, RSS, summary, bench JSON) reconstruits après terminaison driver. | 0% | clos. Si un nouveau exit code casse le wait-loop, ajouter exception dans `.github/workflows/ci-k8s.yml` (bloc awk ~L178-235). |

### Attendus

| # | Type | Détail | Next step |
|---|------|--------|-----------|
| 1 | Run K8s en cours | `ci-k8s ndcg-gate` run `26373579876` sur `2e4361e` valide Lot 1.6 (bulk) + Lot 2 (search) en un seul run. `ci` workspace déjà vert (run `26373423517`) → A+B compilent + tests OK ensemble. | À la fin : télécharger l'artefact, comparer bulk TREC-COVID + RSS vs baseline jemalloc (`2026-05-24-ndcg-gate-lot1.7-jemalloc-K8s/`), publier `docs/ops/bench-reports/2026-05-24-ndcg-gate-lot1.6-K8s/`, mettre à jour le tableau de bord performance + `plan/wp-a-perf-followups.md` Lots 1.6/2 + `PLAN.md`. |
| 2 | Décision user | Prochaine attaque après A+B validés : Lot 3 (Block-Max WAND v2) ou pivot Track D Phase 4 A10 ? | Poser via `AskUserQuestion` quand le run sera vert. |
| 3 | Ménage worktrees | 3 worktrees d'agents restent `locked` (cf. §4). | `git worktree remove --force` une fois les branches mergées confirmées. |

### Vérifications (état au handover)

| Item | État |
|------|------|
| HEAD `origin/main` | `2e4361e` |
| `ci` workspace (A+B) | run `26373423517` SUCCESS |
| docker-build (A+B) | run `26373429382` SUCCESS |
| ci-k8s ndcg-gate (A+B perf) | run `26373579876` IN PROGRESS |
| Attribution AI commits | scan vide sur toute la série |
| Tracks clos | B, C, E (3/5) ; A actif ; D dormant |

---

## 3. Prochaines actions à conduire (ordre)

### Action 0 (immédiate) — finaliser la validation Lot 1.6 + Lot 2

Le run `26373579876` valide les deux lots. Quand il finit :

```bash
gh run download 26373579876 -R rhanka/surch -D /tmp/ndcg-ab
# Lire /tmp/ndcg-ab/k8s-bench-ndcg-gate-*/ndcg-gate.summary.md (NDCG + bulk_ms)
#   et les rss-ndcg-*.json (peak/final)
```
Attendu :
- **NDCG@10 / Recall@10 inchangés** (0.6576/0.8100 SciFact ;
  0.4750/0.0132 TREC-COVID). Si ça bouge → régression, investiguer.
- **Bulk TREC-COVID Surch** : doit baisser sous `139 s` (baseline
  jemalloc) grâce à Lot 1.6 (un seul `terms.build()` par refresh au
  lieu d'un par chunk).
- **Latence search** : Lot 2 (skip lists) n'est pas finement mesuré
  par `ndcg-gate` ; pour le voir il faudra un `insee-bench` replay
  (latence artillery) — à considérer comme sous-action.

Puis publier `docs/ops/bench-reports/2026-05-24-ndcg-gate-lot1.6-K8s/`
(README + summary + bench.json + rss + job.yaml), mettre à jour :
- `docs/ops/bench-reports/track-a-performance-ledger.md` (lignes Bulk
  + RSS + une éventuelle ligne search/skip-list).
- `plan/wp-a-perf-followups.md` : cocher Lot 1.6 + Lot 2.
- `PLAN.md` Track A reste % (→ ~5%).

### Action 1 — mesurer Lot 2 (skip lists) sur la latence search

`ndcg-gate` ne stresse pas la latence search. Dispatcher un
`insee-bench` sur `2e4361e` pour capter p50/p95/p99 search et
comparer à la baseline `2026-05-21-A-replay-current-main-61a13f-insee-K8s/`.
Le gain skip-list (leapfrog AND) doit apparaître sur les requêtes
multi-terme.

### Action 2 — Lot 3 (next Block-Max WAND step)

Décrit dans `plan/wp-a-perf-followups.md`. S'appuie sur les skip
lists de Lot 2 pour étendre le skipping cross-terme en OR-match
top-K et `multi_match`. Tests + K8s + tableau de bord.

### Action 3 (à arbitrer avec le user via AskUserQuestion)

Quand Track A perf sera à un palier, proposer :
- Track D Phase 4 A10 (write-time fan-out) — débloque parité
  matchID étendue. Gros chantier indexation.
- Track A Lot 4 (replays historiques) — nécessite d'abord de
  débloquer la surface workflow aux anciens SHAs.

---

## 4. Pièges connus / notes

- **Subagents parallèles** : la dernière session a dispatché 3
  agents en worktree isolé (Lot 1.6, Lot 2, Track C Lot 4). **Stream
  A (Lot 1.6) a crashé avant de committer** — le travail était dans
  le worktree non commité ; je l'ai récupéré (`git -C <worktree>
  diff`, inspection, fmt, commit, cherry-pick). **Leçon** : après un
  dispatch parallèle, toujours vérifier `git worktree list` +
  l'état non-commité de chaque worktree, ne pas se fier uniquement
  à la notif de complétion.
- **Worktrees à nettoyer** (locked, branches mergées) :
  ```bash
  git worktree remove --force .claude/worktrees/agent-a3c5eb804be485a41
  git worktree remove --force .claude/worktrees/agent-a507d5ac301cfd108
  git worktree remove --force .claude/worktrees/agent-ab7337df5f93d7b46
  git push origin --delete worktree-agent-a3c5eb804be485a41 worktree-agent-a507d5ac301cfd108
  # (la branche ab7337df n'a jamais été poussée)
  ```
  Vérifier d'abord que `2e4361e` et `d73c862` contiennent bien tout
  le travail avant de supprimer.
- **Cargo.lock** : Lot 1.7 a ajouté `tikv-jemallocator` +
  `tikv-jemalloc-sys`. `build-essential` est requis dans le builder
  Dockerfile pour compiler jemalloc.
- **jemalloc** : allocator global Surch sur Linux uniquement
  (`#[cfg(target_os = "linux")]`), tuné via
  `MALLOC_CONF=background_thread:true,dirty_decay_ms:0,muzzy_decay_ms:0`
  dans le Dockerfile runtime. Parité avec ES/OS (qui utilisent
  jemalloc par défaut sur Linux).
- **Wait-loop `ci-k8s`** historiquement fragile (sidecar exits,
  OOM, timeouts). `exit=143` (SIGTERM) toléré ; `137` (OOM), `1`,
  `2` restent fatals.
- **Dependabot demo** : 4 alertes Svelte closes (`91b1057`). Si
  réapparition : `cd demo && npm audit fix && npm run check &&
  npm run build`.

---

## 5. Fichiers de référence

| Fichier | Rôle |
|---------|------|
| `PLAN.md` | Vue racine A→E + Conductor Iteration Contract. |
| `plan/wp-a-perf-followups.md` | **Forward queue Track A active** (Lots 1→1.7 livrés, Lot 2 livré, Lots 3/4 ouverts). |
| `plan/wp-a-optim.md` | Historique des lots Track A livrés (clos). |
| `plan/perf-replay-wp-a-algo-ledger.md` | Replays historiques A-replay-1/2/3 (bloqués). |
| `plan/wp-b-test-auto.md` | Track B (clos). |
| `plan/wp-c-ops.md` | Track C (clos, Lot 4 inclus). |
| `plan/wp-d-matchid.md` + `plan/wp-d-matchid-phase4.md` | Track D (B1 clos, Phase 4 backlog). |
| `plan/main-infra.md` | Track E (clos). |
| `docs/ops/bench-reports/track-a-performance-ledger.md` | **Tableau de bord performance** — maj à chaque preuve perf. |
| `docs/ops/release-verification.md` | Mode d'emploi `verify-release.sh`. |

**Dernière action en vol au handover** : run ci-k8s `26373579876`
(validation Lot 1.6 + Lot 2). Reprendre par l'Action 0 dès qu'il
finit.
