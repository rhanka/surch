# Handover surch — 2026-05-24 (HEAD `2e4361e`)

Document de passation pour Codex. Contient : (1) les règles
durables à respecter, (2) l'état de chaque track au format de
reporting demandé, (3) les prochaines actions à conduire.

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
