# Handover surch — 2026-05-24

Document de passation pour Codex. Reprend les instructions
durables, l'état réel des tracks, et les actions concrètes à
prendre en charge en priorité.

## 1. Règles durables (à respecter pendant toute la session)

### Communication
- **Toujours en français.** Le user est francophone.
- Format de rapport obligatoire après chaque jalon :
  `Fait` / `Reste à faire` / `Attendu de ma part` / `Vérifications`
  / `Décision / risque`. Utiliser des **tableaux** dès qu'il y a
  plus de 2 éléments à lister.
- Pas de jargon interne dans les rapports user : **pas** « promo »
  (dire « publier le rapport sous `docs/ops/bench-reports/…/` »),
  **pas** « ledger » (dire « mettre à jour le tableau de bord
  performance `docs/ops/bench-reports/track-a-performance-ledger.md` »),
  **pas** « Lot N » seul (préciser ce que le lot fait).

### Mode autonome
- Le user est en mode **autonomie maximale** : ne pas demander
  validation pour `cargo *`, `git add/commit/push origin main`,
  édits Rust, ajout de tests, lecture de fichiers, dispatch
  `gh workflow run`.
- Lui parler uniquement aux jalons (commit poussé, milestone) avec
  le format de rapport ci-dessus.
- Questions de scope / décisions réelles : **utiliser l'outil
  `AskUserQuestion`**, pas une liste en prose. Fournir contexte +
  coût + option recommandée en premier (suffixe ` (Recommended)`).

### Git & commits
- Branche unique : **`main`**. Pas de `develop`, pas de feature
  branch.
- **STRICT — aucune attribution Claude / Anthropic dans les
  commits** : pas de `Co-Authored-By: Claude`, pas de « Generated
  with Claude Code », pas de 🤖. Signature unique
  `antoinefa <fabien.antoine@m4x.org>`. Vérifier après chaque
  commit :
  `git log -1 --format="%B" | grep -iE "claude|anthropic|🤖|co-authored"`
  doit retourner vide.
- Messages en anglais conventionnel
  (`[wp-x] type(scope): subject`) ; réponses user en français.
- Push régulièrement sur `origin/main` après chaque commit qui
  passe les gates (cf. ci-dessous).

### Gates de qualité avant commit
- `cargo fmt --all -- --check` (instantané, autorisé en local).
- **PAS de `cargo test` / `cargo clippy` workspace en local** : le
  user l'a explicitement interdit (« tes travaux font planter
  systematiquement la session »). Validation **uniquement via
  `ci-k8s`** sur le cluster Scaleway poc-k8s.
- Si patch YAML / docs / plans uniquement, pas de gate à lancer.

### Validation K8s (remplace les tests locaux)
- Pour chaque changement Rust qui doit être validé :
  ```bash
  gh workflow run docker-build.yml -R rhanka/surch -r main
  # attendre ~6 min, puis :
  gh workflow run ci-k8s.yml -R rhanka/surch -r main -f job=ndcg-gate
  # attendre ~30 min, puis télécharger l'artefact et publier le rapport
  ```
- Jobs disponibles dans `ci-k8s` : `ndcg-gate` (SciFact + TREC-COVID
  paired NDCG/Recall + RSS), `insee-bench` (artillery latency),
  `b1-oracle-gate` (matchID parité ES 8.6.1), `00-init-corpora`.
- L'authentification `gh` peut être perdue ; si oui, restaurer
  depuis le cache git via
  `printf 'protocol=https\nhost=github.com\n\n' | git credential fill | awk -F= '/^password=/ {print $2}' | gh auth login -h github.com --with-token`.

### Code style
- **Pas de Python** dans la codebase (sauf tests externes
  ponctuels). Tout en Rust.
- Pas de comments superflus dans le code (« why » seulement, pas
  « what »).
- Pas d'attribution AI ni dans le code ni dans les comments.

## 2. État actuel (HEAD = `8a5150f`)

### Tracks A / B / C / D / E

| Track | Reste % | Statut | Détail |
|-------|---------|--------|--------|
| **A — perf/optim** | ~20% | actif | Lot 1 fermé + livré (incrémental bulk) ; Lot 1.5 livré (RAM), **validation K8s en attente** ; Lots 1.6 / 2 / 3 / 4 backlog. |
| **B — test-auto** | 0% | clos | Tous les rapports promus, paired RSS en place. |
| **C — ops** | ~8% | backlog | Lot 4 (`scripts/verify-release.sh`) ouvert. |
| **D — matchID** | B1 0% / Phase 4 inactive | dormant | `plan/wp-d-matchid-phase4.md` détaille 8 lots A10/A1/A13/A7/A2/A5/A6/A12 + B2 deces_v2. |
| **E — infra K8s** | 0% | clos | `ci-k8s` standard heavy, tolère SIGTERM sidecars, reconstruit fichiers RSS depuis driver log. |

### Derniers commits sur `main`

| SHA | Track | Sujet |
|-----|-------|-------|
| `8a5150f` | A | Lot 1.5 — free PostingsBuilder snapshot on `_refresh` |
| `8571bb9` | A E | Rapport bulk incrémental + fermeture Lot 1 |
| `04fde72` | E | Wait-loop tolère `exit=143` (SIGTERM sidecars) |
| `367acdc` | A | Lot 1 axe (c) — append incrémental sur bulk |
| `3689011` | A | Diagnostic Lot 1 (rebuild quadratique) |
| `7da0718` | A B E | Rapport paired RSS + ferme Track B + Track E |
| `137b352` | B E | RSS sampler argv[0] basename |
| `4df387f` | B E | Marqueurs RSS dans driver log + reconstruction workflow |
| `975eea4` | B E | `ps` → scan `/proc/<pid>/cmdline` |
| `91b1057` | hors-track | `npm audit fix` demo (4 alertes Svelte) |

### Rapports K8s publiés cette série

| Date | Répertoire | SHA / Run | Verdict |
|------|------------|-----------|---------|
| 2026-05-22 | `2026-05-22-ndcg-gate-7Gi-K8s/` | `d9cac15` / `26304471549` | Pool 7 GiB, full corpus TREC-COVID OK, RSS via `kubectl top` |
| 2026-05-23 | `2026-05-23-ndcg-gate-7Gi-RSS-K8s/` | `137b352` / `26340177506` | 1ère paire RSS `surch.bench.rss.v1` |
| 2026-05-24 | `2026-05-24-ndcg-gate-incremental-bulk-K8s/` | `04fde72` / `26350556060` | **Lot 1 fix prouvé : Surch TREC-COVID bulk 1002 s → 180 s (~5.6x)** |

### Plans de référence

- `PLAN.md` — vue racine A→E + Conductor Iteration Contract.
- `plan/wp-a-optim.md` — historique des lots Track A livrés.
- `plan/wp-a-perf-followups.md` — **forward queue Track A active**
  (Lots 1, 1.5, 1.6, 2, 3, 4).
- `plan/perf-replay-wp-a-algo-ledger.md` — replays historiques
  A-replay-1/2/3 (bloqué : SHAs anciens sans `docker-build.yml`).
- `plan/wp-b-test-auto.md` — Track B (clos).
- `plan/wp-c-ops.md` — Track C (Lot 4 ouvert).
- `plan/wp-d-matchid.md` + `plan/wp-d-matchid-phase4.md` — Track D.
- `plan/main-infra.md` — Track E (clos).
- `docs/ops/bench-reports/track-a-performance-ledger.md` —
  **tableau de bord performance** (à mettre à jour à chaque
  changement perf prouvé).

## 3. Actions concrètes à prendre en charge

### Action 1 (immédiate) — valider Lot 1.5 sur K8s

Lot 1.5 (`8a5150f`) ajoute :
- `terms_finalized: bool` sur `InMemoryIndex`.
- `IndexData::finalize_terms_for_refresh()` invoqué par
  `AppState::refresh_index` (qui était un no-op avant).
- `IndexData::append_to_index` retombe sur `rebuild_index` si
  `terms_finalized` est vrai.
- `IndexData::rebuild_index` n'appelle plus `finalize_postings`
  (la libération est centralisée dans `refresh_index`).
- Test `bulk_router_bulk_refresh_bulk_search_preserves_old_docs`.

Effet attendu en K8s : Surch RSS pic TREC-COVID redescend de
`5859 MiB` vers `~4800 MiB` ; gain bulk Lot 1 préservé.

```bash
# 1. Construire l'image (~6 min)
gh workflow run docker-build.yml -R rhanka/surch -r main

# 2. Attendre puis lancer le benchmark (~30 min)
gh workflow run ci-k8s.yml -R rhanka/surch -r main -f job=ndcg-gate

# 3. Télécharger l'artefact et publier le rapport
gh run download <run-id> -R rhanka/surch -D /tmp/ndcg-lot15
# Publier sous docs/ops/bench-reports/2026-05-NN-ndcg-gate-lot1.5-K8s/
# Mettre à jour docs/ops/bench-reports/track-a-performance-ledger.md
#   ligne "RSS / memory" (peak attendu ~4800 MiB)
#   ligne "Bulk indexing" (timings inchangés vs 04fde72)
# Mettre à jour plan/wp-a-perf-followups.md Lot 1.5 → fermé
# Mettre à jour PLAN.md Track A reste 20% → ~17%
```

### Action 2 — attaquer Lot 1.6 (term dictionary incrémental)

Décrit dans `plan/wp-a-perf-followups.md`. C'est la dernière source
du gap `2.06x` Surch/OS sur TREC-COVID bulk. Le coût dominant
restant est `terms.build()` au bout de chaque
`DocumentIndex::add_documents_with_mapping` (rebuild complet du FST
depuis `postings_builder` à chaque `_bulk` POST).

Pistes :
- Profiler `terms.build()` sur un corpus 171k (instrumenter avec
  un compteur dans `crates/surch-index/src/document_index.rs:148`).
- Décider entre : (i) construire le FST incrémentalement
  (accumulation cross-chunks, build une fois au refresh), ou
  (ii) merger des FST par segment façon Lucene.
- Tests + validation K8s + maj tableau de bord.

### Action 3 (à arbitrer avec le user)

Quand Lot 1.6 sera traité, demander au user via `AskUserQuestion`
laquelle des pistes suivantes attaquer :
- Track A Lot 2 — skip lists sur codec FoR (accélère la recherche,
  pas le bulk). Premier ~300 LoC dans `surch-codec` + `surch-search`.
- Track C Lot 4 — script `scripts/verify-release.sh` reproductible
  depuis les artefacts CI (signing, SBOM, image GHCR). Pertinent
  surtout avant un tag `v0.1.0`.
- Track D Phase 4 — démarrer A10 (write-time fan-out des sous-champs
  `.raw`/`.norm`). Premier lot débloquant matchID parité étendue
  ES 8.6.1, ~500 LoC dans indexation/storage.

### Risques connus / pièges

- **`gh` auth peut être expirée** : restaurer depuis cache git
  (cf. règle ci-dessus). Ne pas demander au user de re-auth tant
  que la voie git credential n'est pas explorée.
- **Dependabot demo** : 4 alertes Svelte sur `demo/package-lock.json`
  ont été closes par `91b1057`. Si elles reviennent, `cd demo &&
  npm audit fix` puis `npm run check && npm run build`.
- **Tests locaux interdits** : ne pas lancer `cargo test`,
  `cargo clippy --workspace`, ou autre commande lourde en local.
  Utiliser le runner GHA `ci` (déclenché sur push) ou
  `ci-k8s` (dispatch manuel) pour valider.
- **Wait-loop `ci-k8s`** historiquement fragile. Si une nouvelle
  fausse-fail apparaît, regarder le pattern terminé / exit codes
  dans `.github/workflows/ci-k8s.yml:178-235`.

### Conventions de nommage

- Rapports K8s : `docs/ops/bench-reports/YYYY-MM-DD-<job>-<axis>-K8s/`
  avec `README.md` + raw `summary.md` + `bench.json` + `job.yaml`
  + éventuels `rss-*.json`.
- Workpackages : `wp-<letter>-<topic>.md` sous `plan/`.
- Tags d'images GHCR : `sha-<full SHA>` (runtime) et
  `bench-sha-<full SHA>` (driver). `ci-k8s` exige les deux.

## 4. Préférences user vérifiées en mémoire persistante

Toutes ces règles sont également stockées dans
`~/.claude/projects/-home-antoinefa-src-surch/memory/` et chargées
automatiquement par Claude Code. Pour Codex, les principales :

- Branche unique `main`, push régulier après gates verts.
- Communication français, code/commits anglais.
- Pas d'attribution Claude/Anthropic dans les commits.
- Mode autonome maximal sur les actions de routine.
- Questions de scope via `AskUserQuestion` avec contexte +
  recommandation.
- Pas de tests locaux lourds — validation `ci-k8s` uniquement.
- Pas de jargon interne dans les rapports user.
- Reporting structuré (Fait / Reste / Attendu / Vérifications /
  Décision).

---

**Dernière session terminée le 2026-05-24** sur `8a5150f`. Worktree
propre. Prochaine action recommandée : Action 1 (valider Lot 1.5
sur K8s, ~40 min CI + ~15 min publication rapport).
