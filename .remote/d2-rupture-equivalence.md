# D2 — rupture des suites d'équivalence C1 / C2 : cause racine et correctif

Base : `main` @ `34e2e18` (le commit fautif est `81baa39`, `[D2] postings :
bit-packing adaptatif…`). Run CI de référence : `30473822257`, job `cargo test`,
`73 passed; 9 failed`. `cargo clippy`, `cargo fmt --check` et le harnais P3 sont
verts — le défaut est fonctionnel.

---

## 1. Ce que les 9 échecs disent RÉELLEMENT

Premier constat, et il change tout le diagnostic : **aucun des 9 échecs n'est une
assertion d'équivalence.** Les neuf lignes citées par le run sont, sans
exception, l'assertion qui vérifie que le chemin optimisé **s'engage** :

| Test | Ligne | Assertion qui échoue |
|---|---|---|
| `c2::ex_aequo_massifs_disque_rendent_le_meme_ordre` | `search.rs:7822` | `assert!(comparer_toutes_fenetres(…))` → engagement |
| `c2::plusieurs_segments_rendent_le_meme_ordre` | `search.rs:7839` | idem |
| `c2::scores_varies_restent_bit_a_bit_identiques` | `search.rs:7866` | idem |
| `c2::df_faible_et_df_nul` | `search.rs:7883` | idem |
| `c2::size_zero_conserve_le_total_sans_hydrater` | `search.rs:7907` | `assert!(comparer(&state, index, &query, 0))` → engagement |
| `c2::documents_supprimes_traites_a_l_identique` | `search.rs:7929` | engagement |
| `c1::ex_aequo_massifs_le_seuil_elague_sans_changer_la_fenetre` | `search.rs:8230` | `assert!(elagues > 0, …)` |
| `c1::plusieurs_segments_conservent_la_fenetre` | `search.rs:8262` | `assert!(elagues > 0, …)` |
| `c1::df_tres_grand_de_part_et_d_autre_du_plafond_de_dix_mille` | `search.rs:8317` | `assert!(elagues > 0, …)` |

Les `assert_eq!(streame, reference)` de `c2::comparer` (`search.rs:7722`) et les
`assert_eq!(streame, attendu)` / `assert_eq!(reference, attendu)` de
`c1::comparer` (`search.rs:8082` et `:8086`) **passent tous**. Aucun document,
aucun ordre, aucun score, aucun `total` n'a bougé.

Trois témoins négatifs confirment la lecture, et ils sont décisifs :

1. `c2::ex_aequo_massifs_ram_rendent_le_meme_ordre` (mode RAM, `search.rs:7809`)
   **passe** — seul le mode postings-disque est touché ;
2. `c1::scores_reellement_distincts_conservent_la_fenetre` (`search.rs:8240`)
   **passe** : il balaie exactement les mêmes fenêtres en mode disque, mais
   n'exige AUCUN élagage — donc l'équivalence tient bien en mode disque ;
3. `c2::size_zero_…` échoue sur `comparer(…, 0)`, c'est-à-dire à `limit == 0`.
   Or à `limit == 0` le chemin streamé rend `Some((vec![], total))` en
   `state.rs:6009`, **avant d'avoir ouvert le moindre curseur**. Le déclin est
   donc antérieur à toute lecture de bloc : il vient de
   `data.index.segmented_postings_checked(field, &token)` (`state.rs:5995`), qui
   rend `Err(_)` — et `state.rs:6002` traduit toute erreur checked par un déclin.

Le chantier D2 n'a donc pas cassé un RÉSULTAT : il a éteint le routage. Les deux
chemins de latence livrés juste avant (C2 `match` mono-terme streamé, C1
terminaison anticipée) ne s'engagent plus jamais dès que les postings sont sur
disque, et retombent silencieusement sur la référence. C'est précisément ce que
ces tests existent pour attraper : sans l'assertion « le chemin streamé doit
s'engager » et sans `elagues > 0`, l'équivalence aurait été prouvée à vide et le
gain de latence serait parti sans un bruit.

## 2. Cause RACINE, prouvée

**`crates/surch-index/src/postings.rs:2585-2589`, dans `TermEntry::validate()` —
avant correctif :**

```rust
if self.postings_count == 0
    || u64::from(self.postings_count).saturating_mul(2) > u64::from(self.postings_len)
{
    return Err(PostingsReadError::Corrupt);
}
```

Cette garde exige **au moins 2 octets de payload par posting**. Ce n'était pas
une marge : c'était la traduction EXACTE du plancher du format antérieur, où
chaque posting coûtait obligatoirement un varint de delta (>= 1 octet) **et** un
varint de fréquence (>= 1 octet). L'égalité `postings_len == 2 * postings_count`
était atteinte sur le cas nominal (deltas < 128, `tf = 1`), donc la garde passait
tout juste.

**D2 supprime les deux planchers à la fois** : le canal des fréquences DISPARAÎT
quand elles valent toutes 1 (`postings_block.rs:589-601`, mode
`FREQ_MODE_ALL_ONES`), et les deltas sont bit-packés à 1 bit dans le cas dense
(`postings_block.rs:555-558`). Le payload passe donc structurellement sous le
plancher, et `validate()` déclare `Corrupt` **tout terme d'un segment disque
sain**.

### Arithmétique du témoin, vérifiée à l'octet

Corpus des deux suites : 600 documents, `nom` vaut `durand` un document sur 97,
`martin` sinon (`search.rs:7770-7794`).

`nom = durand` : `doc_id` = 0, 97, 194, 291, 388, 485, 582 ; `df = 7` ; `tf = 1`.

| | avant D2 | après D2 |
|---|---:|---:|
| canal doc_id | 7 varints (1 + 6 × 1) = **7 o** | en-tête 1 o + varint(0) 1 o + 6 varints de 97 = **8 o** |
| canal freq | 7 varints de 1 = **7 o** | **0 o** (mode « toutes à 1 ») |
| `postings_len` | **14** | **8** |
| garde `2 × df` | 14 > 14 → **faux**, accepté | 14 > 8 → **vrai**, `Corrupt` |

Le format antérieur tombait **exactement** sur la borne. Le nouveau passe
dessous. Même arithmétique sur `nom = martin` (`df = 593`, 5 blocs, payload
≈ 110 o contre 1 186 o exigés) et sur le témoin `BODY = commun` des tests
`document_index` (`df = 2` par segment : 4 o avant, 3 o après, borne à 4).

### Chaîne complète, fichier:ligne

1. `postings.rs:2585` → `Err(PostingsReadError::Corrupt)` pour tout terme disque.
2. `postings.rs:3439` — `disk_cursor_p2_checked` appelle `entry.validate()?` et
   propage l'erreur (et incrémente `p2_runtime_metrics.fallbacks`,
   `postings.rs:3472`).
3. `document_index.rs:2336-2364` — `segmented_postings_checked` interrompt la
   construction entière au premier segment en erreur.
4. `state.rs:6002` — `single_term_match_topk_streamed` rend `None` (« décliner
   est la seule réponse à toute situation où l'équivalence bit-à-bit n'est pas
   démontrable »), donc `single_term_match_topk` rend `None`.
5. `search.rs:7716` — `engage == false` ; `search.rs:2461` — le top-K retombe sur
   `topk_scored_documents_reference`, d'où des résultats justes et zéro élagage
   C1 (`elagues == 0`).

Cette chaîne explique les 9 échecs et **seulement** eux : elle est inerte en mode
RAM (`document_index.rs:2343` prend `postings_with_block_metas_checked`, qui ne
passe pas par `TermEntry`), et elle ne change aucun résultat puisque le déclin
est un repli complet, jamais un préfixe partiel.

### Casualty non visible dans le run

`cargo test --workspace` s'arrête au **premier binaire** en échec (pas de
`--no-fail-fast` dans `.github/workflows/ci.yml:67`). Les 9 échecs sont ceux du
binaire `surch-api` (84 `#[test]`, cohérent avec `73 + 9 = 82`). Les binaires
suivants n'ont jamais tourné depuis D2. La même arithmétique montre qu'y étaient
aussi cassés, par la MÊME cause :

- `surch-index` (unité) : `p2_cursor_checked_lit_seulement_les_blocs_necessaires`
  (384 postings, 59 o contre 768 exigés), `checked_cursor_reconstruit_un_repertoire_persistant_invalide`,
  `checked_cursor_abandonne_un_payload_corrompu_apres_un_prefixe_valide` ;
- `document_index` (unité) : `p2_segmented_postings_checked_couvre_ram_disque_mixte_et_df_global`,
  `p2_segmented_postings_checked_refuse_un_segment_illisible_sans_vue_partielle`.

Le correctif les traite toutes. **Je ne les ai pas vues échouer** : je les déduis
de la même chaîne, seule la CI le confirmera.

## 3. Ce qui n'est PAS en cause — vérifié, pas supposé

Les pistes du cahier des charges ont été instruites et écartées :

- **Le `total` / `global_df()`** : intact. `total` vient de
  `postings.global_df()` (`state.rs:6005`), somme des `local_df` par segment ;
  D2 ne touche ni `TermEntry::postings_count` ni son écriture
  (`postings.rs:1697`, inchangé). `c1::df_faible_et_df_nul` passe et vérifie
  `total == 7`.
- **L'ORDRE et les ex æquo** : intacts. `scored_pair_ordering` n'existe qu'en un
  exemplaire, et les trois chemins comparés rendent la même empreinte dans les
  tests qui n'exigent pas d'élagage.
- **Le premier `doc_id` reste ABSOLU** : vérifié, `postings_block.rs:547`
  (`write_varint_u32(out, doc_chunk[0])`), hors de tout mode.
- **`BlockDirEntry` inchangé octet pour octet** : vérifié, `postings_block.rs:914-926`
  et `encode_block_dir_entry` (`postings.rs:2632`) sont identiques à `ae2c12d`
  (le diff `git diff ae2c12d 81baa39` ne les touche pas), et le nombre d'entrées
  reste `df.div_ceil(128)` (`postings_block.rs:687-706`), ce qu'exige
  `p2_directory_has_expected_shape` (`postings.rs:3756`).
- **Le codec lui-même** : relu intégralement (aller-retour des trois modes doc,
  des quatre modes freq, canonicité, bornes de largeur, positions d'exception).
  Je n'y ai **pas** trouvé de défaut, et le fait que `c1::scores_reellement_distincts…`
  et `c1::df_faible_et_df_nul` rendent les bons documents et le bon `total` en
  mode disque à travers le chemin de repli corrobore que les postings sont
  correctement écrits. Ce n'est pas une preuve d'absence de bug : c'est une
  absence de preuve de bug.

## 4. Correctif appliqué

Un seul point de code, `crates/surch-index/src/postings.rs` :

1. **Nouvelle constante `MAX_POSTINGS_PER_PAYLOAD_BYTE = 8`** (juste avant
   `impl TermEntry`), documentée avec sa démonstration.
2. **`TermEntry::validate()`** : `postings_count × 2 > postings_len` devient
   `postings_count.div_ceil(8) > postings_len`, c'est-à-dire « au plus 8 postings
   par octet de payload ».

Démonstration de la nouvelle borne, sur la disposition D2 (`postings_block.rs:628-643`).
Pour un bloc de `c <= 128` postings, l'encodeur écrit toujours 1 octet d'en-tête,
puis le varint du premier `doc_id` (>= 1 octet), puis le canal doc_id — dont
aucun des trois modes ne descend sous `ceil((c-1)/8)` octets :

| mode | coût | minoration |
|---|---|---|
| `varint` | `Σ len(varint(delta))` | `>= c - 1 >= ceil((c-1)/8)` |
| `packed` | `1 + ceil((c-1)·w/8)`, `w >= 1` | `>= ceil((c-1)/8)` |
| `patched` | `2 + ceil((c-1)·w/8) + Σ(1 + varint)` | strictement davantage |

Un bloc pèse donc au moins `2 + ceil((c-1)/8) >= ceil(c/8)` octets ; en sommant
sur les blocs (`Σ ceil(cᵢ/8) >= ceil(Σcᵢ/8)`), `postings_len >= ceil(df/8)`.
Le pire cas réellement atteignable est un bloc plein à deltas d'un bit :
`1 + 1 + 1 + 16 = 19` octets pour 128 postings, soit **6,7 postings/octet**. La
borne de 8 ne peut donc jamais refuser un payload canonique, et elle garde
`postings_count` proportionnel à `postings_len` — ce qui est la RAISON D'ÊTRE de
la garde : borner l'allocation avant d'interpréter un payload.

Ce qui n'est pas relâché : `postings_len == 0` reste la sentinelle « pas de
couverture » (avec la distinction `MissingCoverage` / `Corrupt` sur
`postings_offset`), `postings_count == 0` reste `Corrupt`, et tout ce que la
garde laisse désormais passer est rattrapé en aval sans exception — attestation
BLAKE3 des pages (`read_verified_range`), `p2_directory_has_expected_shape`
(`postings.rs:3750`), contrôle de ré-encodage canonique
(`decode_postings_payload_checked`, `postings.rs:2665`) et
`decoded_block_matches_directory` (`postings.rs:4210`). J'ai vérifié à la main
les quatre tests qui exigent aujourd'hui un `Err(Corrupt)` en aval de
`validate()` : ils l'obtiennent tous par une autre garde
(`checked_postings_rejette_un_compteur_qui_cache_un_suffixe` par le
ré-encodage ; `checked_postings_distingue_absence_et_erreurs_de_lecture`, cas
`postings_len = 1`, par l'EOF du décodeur ;
`p2_rejette_les_scalaires_term_entry_non_attestes_avant_lecture` et
`p2_rejette_un_digest_resident_altere` par le digest, qui est évalué AVANT
`validate()`).

### Verrou de non-régression ajouté

`d2_payload_bit_packe_sous_deux_octets_par_posting_reste_lisible_checked`
(`postings.rs`, module `tests`) : construit un terme dense de `4 × 128 + 7`
postings à `tf = 1` en mode disque, **exige d'abord que le payload franchisse
l'ancien plancher** (`postings_count × 2 > postings_len` — sans quoi le test se
viderait), puis exige que `disk_cursor_p2_checked` ouvre le curseur et rende la
suite complète des `doc_id`. Avant le correctif il échoue sur le `expect` de
`disk_cursor_p2_checked` ; après, il passe.

## 5. Pourquoi je n'ai PAS touché aux tests d'équivalence

Ils ont fait exactement leur travail, et ils ont attrapé un défaut qu'aucun autre
garde-fou du dépôt ne pouvait voir. La preuve : les tests d'ÉQUIVALENCE proprement
dits passaient — les résultats étaient justes tout du long. Ce qui a sauvé le
chantier, ce sont les deux assertions « anti-preuve à vide » :
`assert!(comparer_toutes_fenetres(…))` (le chemin optimisé doit s'engager) et
`assert!(elagues > 0)` (l'élagage doit réellement avoir lieu). Sans elles, D2
serait passé vert en éteignant deux chantiers de latence, et la campagne de
mesure en cours aurait mesuré le chemin de référence en croyant mesurer C1 et C2
— exactement le scénario « gain non réalisé passé inaperçu » que ce dépôt a déjà
vécu.

Aucune ligne de `c1_terminaison_anticipee_tests` ni de `c2_lecture_streamee_tests`
n'a été modifiée. Aucun test n'a été assoupli, désactivé ni réécrit, dans aucun
crate. Le seul changement de test du lot est un AJOUT.

## 6. Ce que je ne peux PAS garantir

1. **Rien n'a été compilé ni exécuté.** L'interdiction de `cargo build/check/
   test/clippy` en local est absolue ; seul `cargo fmt --all -- --check` est
   passé, et il est VERT. La cause racine est établie par lecture de code et
   arithmétique à l'octet, pas par une exécution. **La CI est le seul juge.**
2. **Je n'ai pas vu échouer les tests de §2 « casualty non visible ».** `cargo
   test` s'est arrêté au binaire `surch-api` ; les binaires `surch-index`,
   `document_index` et `surch-codec` n'ont pas tourné depuis D2. Je déduis leur
   état de la même arithmétique. Si l'un d'eux échoue encore après ce correctif,
   c'est un SECOND défaut de D2, indépendant de celui-ci.
3. **Le codec D2 n'a toujours reçu aucune relecture indépendante aboutie.** Je
   l'ai relu intégralement et je n'y ai pas trouvé de défaut, mais les 20 tests
   de `surch-codec` ajoutés par D2 n'ont **jamais été exécutés**, pas plus que
   `d2_postings_bit_packes_relus_a_l_identique`. Mon verdict « le codec est
   correct » est une opinion de relecture, pas un fait attesté.
4. **Aucune mesure.** Ce correctif restaure l'ENGAGEMENT des chemins C1/C2 en
   mode disque ; il ne dit rien de la latence obtenue avec le nouveau format, ni
   du −21,4 % de disque annoncé par D2, qui reste un calcul non mesuré par le
   moteur.
5. **La garde `validate()` est désormais 16× plus lâche** (8 postings/octet au
   lieu de 0,5). C'est le prix exact du gain de format : la borne suit le codec.
   Elle reste proportionnelle au payload, et toutes les gardes aval sont
   inchangées — mais un `postings_count` corrompu réserve désormais un `Vec`
   jusqu'à 8 fois plus grand avant d'être rejeté par le décodage.
6. **Le format D2 n'est PAS jugé incompatible avec un invariant du moteur.** Le
   seul invariant qu'il violait était calibré sur l'ancien codec, pas sur une
   propriété du moteur. Un retour en arrière sur D2 n'est donc pas nécessaire ;
   si la CI révélait un second défaut structurel, la question se reposerait.

---

D2FIX_DONE
