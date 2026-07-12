# Plan 2c — packing de la side-table `_source` (8 o/doc), 2026-07-12

Levier « index frugal » recommandé par la contre-expertise 2b (§C Q4) : remplacer les
~24 o/doc de `dense.documents: Vec<Option<SourceBlob>>` (`state.rs:713`) par un `Vec<u64>`
packé → **−433 Mio de RAM anonyme @28,9M docs**, la marge exacte qui manque au transient
d'OOM @4g (~4,15 Go mesuré, cf. `verdict-28M-6g-2026-07-11.md` §1). Plan rédigé en direct
(crédit Codex épuisé), audit code fait sur main @`cc56063`.

## Constat structurel (audit)

1. Depuis mmap M1, **tous les blobs vivants de `dense.documents` sont `OnDisk { offset: u64,
   length: u32, codec: u8 }`** : `SourceBlob::Raw` n'est plus construit nulle part, et
   `SourceBlob::Compressed` n'est produit que par `compact_after_refresh` (`#[allow(dead_code)]`,
   plus appelé). Le packing n'a donc AUCUN cas hybride à gérer côté dense.
2. `documents_dirty: HashMap<u32, SourceBlob>` (`state.rs:784`) reste tel quel : borné par les
   écritures entre deux `_refresh`, ce n'est pas le poste dominant.
3. Accès : `DenseIdMaps::blob()` (`state.rs:735`) retourne aujourd'hui `Option<&SourceBlob>` ;
   appelants à adapter : `blob_for_doc_id` (1151), 1284, 1365, 2556, et 1635 (déjà `.cloned()`).

## Encodage

`u64` = `[codec:1][length:23][offset:40]` :
- **offset 40 bits = 1 Tio** de `source.dat` — @28,9M on mesure ~15 Go orphelins compris, marge 64×.
- **length 23 bits = 8 Mio/doc** — les `_source` observés font < 1 Mio (matchID/BEIR) ; le
  contrat actuel `u32` (4 Gio) est théorique. Garde : au pack, si `length > 2^23−1` → erreur
  explicite (« doc _source > 8 MiB non supporté par la side-table packée ») plutôt que
  troncature silencieuse. Escape-hatch optionnel si un jour nécessaire : petite
  `HashMap<u32, SourceBlob>` d'exceptions, consultée sur length saturée.
- **codec 1 bit** = raw/zstd — le tag `u8` de 2b n'a que 2 valeurs ; un 3e codec (zstd+dict)
  consommera un bit de length (22 bits = 4 Mio, toujours ample) ou passera l'encodage à
  2 bits en réduisant offset à 39 bits (512 Gio) — trancher au moment du dict, pas avant.
- **Trou (`None`)** : `length == 0` (un JSON réel fait ≥ 2 octets — `{}`). `offset` ignoré.

## Étapes (ordre de commit)

1. `DenseIdMaps::documents: Vec<u64>` + helpers `pack(offset, length, codec) -> u64` /
   `unpack(u64) -> Option<(u64, u32, u8)>` (None si length==0) + tests unitaires round-trip,
   trou, bornes (offset max, length max, length trop grande → erreur).
2. `DenseIdMaps::blob()` retourne `Option<SourceBlob>` PAR VALEUR (`OnDisk` est trivialement
   copiable ; la variante `Compressed` du chemin dirty se clone en `Arc` pas cher).
   `blob_for_doc_id` fusionne comme avant (tombstones → dirty → dense) et retourne par valeur ;
   adapter les 5 sites d'appel (mécanique).
3. `densify_full` : pousse le packé depuis `documents_dirty` (match sur `OnDisk` obligatoire —
   `unreachable!` documenté sur `Raw`/`Compressed`, cohérent avec le constat d'audit).
   `densify_append_only` : la copie du préfixe dense devient un memcpy de `u64` (plus rapide
   qu'aujourd'hui) ; la tranche neuve packe depuis dirty.
4. Gates : oracle-local 0 divergence (flag compression on ET off), fair-ab 1,36M@1536m
   (`mem_anon_bytes_warm` attendu ≈ −22 Mio à 1,36M ; indexation/latences inchangées), puis
   28M@6g (anon attendu ~3,25 → ~2,82 Go) et RE-TENTATIVE @4g (le transient ~4,15 Go doit
   passer sous ~3,7 Go → le plancher 4g redevient atteignable, à confirmer par 2 runs).

## Risque

FAIBLE-MODÉRÉ : changement localisé (une struct, deux fonctions densify, un accesseur, 5 sites),
zéro format disque touché (`source.dat` inchangé), zéro chemin dirty touché, réversible par
revert simple. Le seul piège réel est le contrat length ≤ 8 Mio — couvert par l'erreur explicite
et le test de borne. Gain : −433 Mio anon steady-state ET une part du transient de densify
(l'ancien + le nouveau buffer coexistent pendant `densify_full` : jusqu'à −0,8 Go sur le pic).
