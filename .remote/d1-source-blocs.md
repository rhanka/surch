# D1 — Compression `_source` par blocs

Chantier D1 du programme R&D (`.remote/rd-latence-programme.md` §5, ligne 486).
Base : `main` @ `333b902`. Périmètre : `crates/surch-api/src/state.rs` uniquement.

---

## 1. Vérification du diagnostic (avant de coder)

### 1.1 La compression EST bien par document — confirmé, fichier:ligne

| Fait | Emplacement (HEAD `333b902`, avant ce commit) |
|---|---|
| Compression appliquée sur `serde_json::to_vec(&source)` d'UN document | `state.rs:1811-1812` — `if source_compress_enabled() { (SourceBlob::encode_zstd(&serialized), SOURCE_CODEC_ZSTD) }` |
| Un `pwrite` par document, immédiatement après | `state.rs:1816` — `self.source_store.append(&stored_bytes)` |
| `encode_zstd` = un `zstd::bulk::Compressor::compress` par appel, donc **une trame zstd complète par document** | `state.rs:751-763` |
| Décodage symétrique par document | `state.rs:775-796` (`decode_zstd`), appelé par `read_on_disk_bytes` `state.rs:803-812` |
| Le commentaire du code le dit lui-même : « compression zstd **PAR-DOC** inline » | `state.rs:1793-1800` |
| Slot packé `[codec:1][length:23][offset:40]`, `codec` sur **1 bit**, avec la note « un 3e codec grignotera un bit de `length` ou d'`offset` » | `state.rs:1100-1125` |

**Le diagnostic R&D est exact.** Rien à ajuster sur la prémisse.

### 1.2 Points où l'analyse R&D est imprécise (corrigés ici)

1. **« ratio de seulement 1,91× »** — vrai sur le corpus 28,9 M
   (13 824 → 7 248 Mio, `docs/paper/verdict-28M-6g-2026-07-11.md:47-60`), mais
   l'analyse attribue ce ratio à des documents « de ~480 octets ». Sur
   l'échantillon réel disponible localement les documents font **234 octets**
   en moyenne (voir §1.3) ; c'est le corpus complet qui est à ~501 o/doc
   (13 824 Mio / 28,9 M). Les deux tailles donnent la même conclusion, mais il
   faut savoir que la mesure locale porte sur des documents **deux fois plus
   petits** que ceux du corpus de référence.

2. **« gain attendu 1,7 à 2× supplémentaires »** — **sous-estimé d'un facteur 2**.
   La mesure locale donne **3,5 à 4,0×** supplémentaires (§1.3). L'analyse a
   raisonné en ordre de grandeur ; la mesure est nettement plus favorable.

3. **« le vrai risque n'est pas le CPU mais le `pread` »** — partiellement
   faux. Le passage en blocs rend zstd **plus rapide dans les deux sens**
   (§4), et le `pread` passe de ~123 o à ~2,9 Kio, soit de 1 à 2 pages de 4 Kio :
   marginal. Le seul risque CPU réel est un **balayage** (`rebuild_index`,
   `documents_paginated`) qui décompresserait le même bloc une fois par
   document — traité par un cache borné d'une entrée (§2.4).

4. **`fst/other = 0`** — confirmé indirectement : le store `_source` est le
   seul fichier écrit par `SourceSegment` et il est **détruit au `Drop`**
   (`state.rs:211-215`, `remove_file`). Voir §3 : cela change complètement la
   nature de la question de compatibilité.

### 1.3 Re-mesure du ratio (mesure locale, artefact reproductible)

Corpus : `tests/matchid_compat/deces/slice-10000.ndjson.gz` (10 000 documents
`deces` réels, 2 338 110 octets bruts, 233,8 o/doc en moyenne). Outil : `zstd`
CLI niveau 3 (le même niveau que `ZSTD_SOURCE_LEVEL`). Commandes reproductibles :

```sh
gunzip -c tests/matchid_compat/deces/slice-10000.ndjson.gz \
  | awk 'NR%2==0' > docs.ndjson          # lignes de documents (hors actions bulk)
tr -d '\n' < docs.ndjson > docs.raw       # concaténation, comme dans un bloc

# par document (1 fichier = 1 document, 1 trame zstd chacun)
split -l 1 -d -a 5 docs.ndjson perdoc/d && truncate -s -1 perdoc/d* && zstd -3 -q perdoc/d*

# par blocs, plusieurs tailles cibles
zstd -b3 -B4096 -i3 docs.raw ; zstd -b3 -B8192 -i3 docs.raw
zstd -b3 -B16384 -i3 docs.raw ; zstd -b3 -B32768 -i3 docs.raw
```

| Mode | Octets compressés | Ratio | Gain vs par-doc |
|---|---:|---:|---:|
| **par document** (l'existant) | 1 615 104 | **×1,448** | — |
| blocs 4 Kio | 495 215 | ×4,721 | ×3,26 |
| blocs 8 Kio | 442 004 | ×5,290 | ×3,65 |
| **blocs 16 Kio** | **405 728** | **×5,763** | **×3,98** |
| blocs 32 Kio | 382 582 | ×6,111 | ×4,22 |
| blocs 64 Kio | 365 323 | ×6,400 | ×4,42 |

**Borne pessimiste** — le même corpus **mélangé** (`shuf`, destruction de toute
localité d'insertion), qui simule un ordre d'arrivée défavorable :

| Mode (corpus mélangé) | Ratio | Gain vs par-doc |
|---|---:|---:|
| blocs 8 Kio | ×4,676 | ×3,23 |
| blocs 16 Kio | ×5,055 | ×3,49 |

Conclusion : **même dans le pire ordre d'arrivée, le regroupement en blocs de
16 Kio multiplie le ratio par 3,5**, contre les 1,7-2× annoncés par l'analyse.
Le rendement décroît nettement au-delà de 16 Kio (5,76 → 6,11 → 6,40) alors que
le coût de décompression par hit croît linéairement : **16 Kio est le point
d'arrêt retenu**.

---

## 2. Format de bloc retenu

### 2.1 Structure sur disque

Un **bloc** = la concaténation des octets `_source` bruts de N documents
consécutifs, compressée en **une seule trame zstd** niveau 3, écrite en **un
seul `pwrite`** dans `source.dat`. Aucun en-tête, aucun séparateur : les
frontières intra-bloc sont portées par les slots, pas par le fichier.

Le bloc est **scellé** dès que le tampon d'accumulation atteint ou dépasse
16 Kio bruts. Invariant qui en découle et qui commande tout le reste : **un
document commence toujours à un offset intra-bloc < 16 384**.

### 2.2 Adressage d'un document

Le slot packé passe de `[codec:1][length:23][offset:40]` à
**`[codec:2][length:22][offset:40]`** — c'est exactement le bit prévu par le
plan 2c pour financer un 3e codec (`state.rs`, commentaire d'origine
« un 3e codec grignotera un bit de `length` »). Conséquence assumée : le
plafond par document passe de 8 Mio à **4 Mio** (aucun `_source`
deces/matchID/BEIR n'en approche ; le garde-fou reste un panic explicite).

Pour `codec = SOURCE_CODEC_BLOCK` (= 2), les deux champs sont **réinterprétés** :

| Champ | Codecs 0/1 (existants) | Codec 2 (bloc) |
|---|---|---|
| `offset` (40 b) | offset fichier | **locator** `[block_id:26][intra_offset:14]` |
| `length` (22 b) | octets écrits | **longueur BRUTE** du document dans le bloc |

26 bits de `block_id` × 16 Kio = **1 Tio de `_source` brut adressable** — même
enveloppe que les 1 Tio d'offsets fichier du contrat 2c.

**Ni la taille du slot ni celle de la side-table ne changent : toujours 8 octets
par document.** C'est le point qui rend le chantier compatible avec la
contrainte « à mémoire équivalente ».

### 2.3 Répertoire de blocs

Une seule structure nouvelle : `SourceStore::blocks: Vec<u64>`, indexée par
`block_id`, chaque entrée packée `[compressed_len:24][file_offset:40]`.

- Coût mémoire : **8 octets par bloc**. À 28,9 M docs × ~500 o = 13,8 Gio bruts
  / 16 Kio ≈ 900 k blocs = **~7 Mio**. Contre un budget de 6 Gio : 0,11 %.
- Append-only : une entrée n'est jamais réécrite, donc **un bloc scellé est
  immuable** — c'est la propriété qui autorise le cache de §2.4.

### 2.4 Bloc en cours et cache

Deux tampons, tous deux **bornés** :

1. `SourceStore::pending: Vec<u8>` — le bloc en cours, en octets bruts.
   Capacité ≤ 16 Kio + taille du plus gros document vu. Un document qui vient
   d'y être écrit est **lisible immédiatement**, avant tout `pwrite` : son
   `block_id` implicite est `blocks.len()`, connu **avant** le scellement. C'est
   ce qui évite toute reprise de slot a posteriori (aucune structure « liste des
   documents de ce bloc », aucune passe de correction).
2. Un cache **thread-local d'UNE seule entrée**, clé `(epoch, block_id)`, refusé
   au-delà de 64 Kio décompressés. Coût borné : **≤ 64 Kio par thread hydrateur**
   (~32 threads observés ⇒ ~2 Mio au pire).

Le cache n'existe **pas** pour la latence top-K (10 hits dispersés ne se
partagent quasiment jamais un bloc) : il existe pour empêcher une régression
O(documents par bloc) sur les chemins qui **balaient** les `doc_id` en ordre
croissant (`rebuild_index`, `documents_paginated`), où `doc_id` croissant
implique `block_id` croissant — une entrée suffit alors à annuler la régression.

`epoch` est un compteur global re-tiré à la construction **et à chaque
`reset()`**. Sans lui, deux index du même process auraient des `block_id` qui se
recouvrent, et un `reset()` ferait rendre l'ancien contenu sous le même
`block_id`. Un test couvre chacun des deux cas (§5).

### 2.5 Scellement au `densify`

`InMemoryIndex::densify()` appelle `flush_pending_block()` en tête,
inconditionnellement (idempotent, O(bloc courant)). Sans cela un index de moins
de 16 Kio de `_source` n'écrirait **jamais** rien dans `source.dat` et les jauges
disque resteraient à 0 alors que les documents existent.

---

## 3. Décision de compatibilité : **AUCUNE RÉINDEXATION**

**Décision : les trois codecs sont reconnus à la lecture, en permanence, sans
drapeau.** `read_on_disk_bytes` dispatche sur le tag `codec` du slot ; un store
dont une partie a été écrite en brut, une autre en zstd par document et une
troisième en blocs se relit intégralement. C'est exactement l'usage prévu par
l'amendement 3 du contrat 2b, qui a introduit ce tag pour cette raison.

**Justification** — et il faut être précis, parce que la question posée
(« un index existant doit-il rester lisible ? ») a une réponse plus forte que
prévu :

1. **Il n'existe aujourd'hui aucun index à réindexer.** `source.dat` est un
   fichier temporaire créé dans `std::env::temp_dir()` par
   `SourceSegment::default()` et **supprimé au `Drop`** (`state.rs:211-215`).
   Aucun manifeste, aucune reprise après redémarrage : le format `_source` est
   **process-local**. La rétro-compatibilité inter-versions ne se pose donc pas
   encore — c'est P2 (manifeste atomique) qui la posera.
2. **Ce qui se pose vraiment, c'est la compatibilité INTRA-process**, et elle
   est réelle : le mode d'écriture est un `OnceLock` d'environnement, et un
   store peut légitimement contenir des blobs des trois codecs (bascule de
   drapeau, chemins de test, `compact_after_refresh`). Le dispatch par tag la
   garantit.
3. **Coût de garder les deux anciens codecs : nul.** Deux bras de `match` et
   une fonction `decode_zstd` déjà présente. Il n'y a aucune raison
   d'imposer une réindexation pour économiser cela — et le jour où P2 rendra
   les segments persistants, cette décision aura d'avance évité une migration.

**Ce que la décision ne couvre pas** : le bit retiré à `length` abaisse le
plafond par document de 8 à 4 Mio. Un `_source` de plus de 4 Mio, écrit sous
l'ancien format, paniquerait au `densify` — mais il paniquait déjà au-delà de
8 Mio, et aucun corpus du dépôt n'en approche. C'est un rétrécissement de
contrat, documenté, pas une régression silencieuse.

### Drapeau d'A/B

`SURCH_SOURCE_COMPRESS` reste le maître (OFF par défaut = brut, inchangé).
Nouveau : `SURCH_SOURCE_COMPRESS_MODE` — `doc` = ancien zstd par document (le
**témoin** du chantier), toute autre valeur ou absence = **blocs** (défaut D1).
Le défaut est le bloc parce qu'il n'existe aucun scénario où l'on souhaite
« compresser mais mal ».

---

## 4. Gains et coûts attendus, avec le calcul

### 4.1 Disque

Point de départ mesuré (`docs/paper/verdict-28M-6g-2026-07-11.md:47-60`) :
`_source` brut 13 824 Mio → zstd par document **7 248 Mio** (×1,907).

Deux modèles de projection, à partir des mesures §1.3 :

| Modèle | Calcul | `_source` post-D1 | Gain |
|---|---|---:|---:|
| A — report du **facteur d'amélioration** pessimiste (×3,49, corpus mélangé) | 7 248 / 3,49 | 2 077 Mio | −5 171 Mio |
| A' — idem, ordre naturel (×3,98) | 7 248 / 3,98 | 1 821 Mio | −5 427 Mio |
| B — report du **ratio absolu** pessimiste (×5,06) | 13 824 / 5,06 | 2 732 Mio | −4 516 Mio |
| **C — plancher volontairement dégradé** (ratio bloc supposé ×4,0, en dessous de TOUTES les mesures) | 13 824 / 4,0 | 3 456 Mio | **−3 792 Mio** |

Je retiens **C comme chiffre annoncé : −3 800 Mio**, et **−4 500 à −5 200 Mio**
comme fourchette probable. Pourquoi dégrader ainsi : les documents du corpus
complet font ~501 o contre 234 o dans l'échantillon, donc la compression par
document y capte déjà plus de redondance (×1,907 contre ×1,448) et la marge
restante est mécaniquement plus faible.

Total disque projeté : **12 296 − 3 792 = 8 504 Mio**, contre ES à 9 115 Mio,
soit **0,93× ES** — l'axe disque bascule de perdant (1,349×) à **gagnant**, par
ce seul chantier. Avec la fourchette probable : 7 100-7 800 Mio, soit 0,78-0,86×.

Ce chiffre dépasse l'estimation R&D (−3 000 à −3 650 Mio) parce que celle-ci
supposait 1,7-2× d'amélioration là où la mesure en donne 3,5-4,0×.

### 4.2 Latence — **meilleure que le budget accordé**

Mesures `zstd -b3` sur le même corpus (machine locale, 3 itérations) :

| Mode | Compression | Décompression |
|---|---:|---:|
| par document (`-B234`) | 85,8 Mo/s | 232,6 Mo/s |
| blocs 16 Kio (`-B16384`) | **678,3 Mo/s** | **2 012,7 Mo/s** |

Le débit par document est effondré par le coût fixe de trame zstd, invisible
sur 234 octets. Coût d'hydratation pour 10 hits, à 501 o/doc :

- aujourd'hui : 10 × 501 o / 232,6 Mo/s = **21,5 µs**
- en blocs : 10 × 16 384 o / 2 012,7 Mo/s = **81,4 µs**
- **surcoût = +60 µs, soit +0,060 ms au p50** — contre un budget accordé de
  +0,15 à 0,3 ms, et contre un p50 mesuré de 25-28 ms.

`pread` : ~2,9 Kio par hit (bloc compressé) au lieu de ~123 o, soit 1 à 2 pages
de 4 Kio au lieu d'une. Le volume lu augmente mais reste dans l'ordre de
grandeur d'une page ; il n'y a pas de lecture de 16 Kio comme le craignait
l'analyse, puisque c'est le bloc **compressé** qu'on lit.

### 4.3 Indexation — **améliorée, pas dégradée**

Le piège rappelé dans la mission (un `pwrite` par token = −40 % d'indexation)
joue ici **dans le bon sens** :

- `pwrite` : **1 par document → 1 par bloc**, soit ~32 documents à 501 o/doc.
  Réduction stricte du nombre de syscalls d'un facteur ~32.
- CPU zstd : 501 o / 85,8 Mo/s = **5,8 µs/doc** aujourd'hui contre
  501 o / 678,3 Mo/s = **0,74 µs/doc** en blocs. **−5 µs de CPU par document.**
- Allocations : un `Vec` de sortie zstd par bloc au lieu d'un par document ; le
  tampon `pending` est `clear()`é, jamais réalloué.

Aucun mécanisme identifié par lequel l'indexation pourrait régresser.

### 4.4 Mémoire

| Poste | Coût à 28,9 M docs |
|---|---:|
| répertoire de blocs (`Vec<u64>`, 8 o/bloc) | **~7 Mio** |
| tampon `pending` (1 par index) | ≤ 16 Kio + plus gros document |
| cache thread-local (1 entrée, ≤ 64 Kio) | ≤ 64 Kio × threads (~2 Mio à 32 threads) |
| side-table par document | **inchangée, 8 o/doc** |

Total ≈ **9 Mio**, soit 0,15 % d'un budget de 6 Gio. Le cache est borné en
nombre d'entrées (1) **et** en taille (64 Kio) : un bloc géant n'est jamais
retenu.

---

## 5. Tests écrits (module `source_block_store_tests`, `state.rs`)

Tous passent par les deux points uniques du code réel — `store_source_bytes`
(seul endroit qui choisit le format d'écriture) et `read_on_disk_bytes` (seul
point de lecture) — et non par des helpers de test parallèles.

| Test | Ce qu'il couvre |
|---|---|
| `block_locator_round_trip` | `pack`/`unpack` du locator, y compris aux bornes exactes (2^26−1 blocs, intra 16 383) |
| `block_id_overflow_panics` | dépassement du nombre de blocs ⇒ panic explicite, pas de troncature |
| `pending_block_is_readable_before_flush` | **bloc partiel jamais scellé** : 5 documents lisibles alors que `bytes_written() == 0` |
| `sealed_and_partial_blocks_round_trip` | 2 000 documents ⇒ >3 blocs scellés + un bloc partiel de queue **déterministe** ; relecture en ordre croissant (chemin balayage) **et** dispersé (chemin top-K) ; vérifie aussi que les octets écrits < octets bruts |
| `document_larger_than_a_block_round_trips` | **document plus gros qu'un bloc**, dans les deux positions : accolé à un document précédent (déborde le bloc courant) et seul en tête de bloc ; le second dépasse 64 Kio donc exerce la branche « trop gros pour le cache », et il est relu une seconde fois après avoir touché d'autres blocs |
| `tiny_documents_round_trip` | `_source` minimal `{}` (2 o) et document de **longueur nulle** en bordure |
| `mixed_codec_store_stays_readable` | **COMPATIBILITÉ** : brut + zstd par-doc + bloc + brut dans le MÊME store, tags vérifiés, tout relu à l'identique |
| `explicit_flush_is_transparent_and_idempotent` | scellement explicite (équivalent `_refresh`) : idempotent, ne crée pas de bloc vide, ne change aucun octet restitué |
| `two_stores_do_not_share_cached_blocks` | **plusieurs index dans le même process** : `block_id` qui se recouvrent, lus en alternance 3 fois — une confusion de cache rendrait le `_source` de A pour B |
| `reset_invalidates_the_block_cache` | `reset()` remet `block_id` à 0 sur un contenu différent ; échoue si l'époque n'est pas re-tirée |
| `unknown_block_fails_closed` | locator vers un bloc inexistant ⇒ panic de contrat |
| `out_of_range_slice_fails_closed` | tranche débordant du bloc ⇒ panic, pas de troncature |
| `round_trip_raw_and_zstd` (étendu) | le slot packé round-trippe pour les **trois** codecs, y compris aux bornes |
| `length_overflow_panics…` / `max_bounds_round_trip` (ajustés) | nouveau plafond 4 Mio |

**L'égalité stricte octet pour octet** est vérifiée par `assert_reads_back` dans
chacun des tests de round-trip : elle compare le `Vec<u8>` restitué à la source
originale, pas une valeur JSON re-sérialisée.

---

## 6. Ce que je ne peux PAS garantir

À lire comme la partie la plus importante du rapport.

1. **RIEN N'EST COMPILÉ NI EXÉCUTÉ.** L'interdiction de `cargo build/check/test/
   clippy` en local est absolue ; seul `cargo fmt --check` a été passé (**vert**).
   Le code et les 14 tests sont **ÉCRITS, PAS ENCORE VERTS**. La CI est le seul
   juge. Elle tourne en `-D warnings` : un lint Clippy suffit à tout casser.
2. **Le chemin `source_write_mode()` (lecture des variables d'environnement)
   n'est couvert par AUCUN test.** Les drapeaux sont des `OnceLock` figés au
   premier appel du process ; un test d'intégration ne peut pas les faire varier
   de façon fiable. Les tests exercent les trois modes via `SourceWriteMode`
   explicite, mais **le câblage `SURCH_SOURCE_COMPRESS_MODE` → mode est une
   ligne non testée**.
3. **Aucun chiffre de ce rapport ne vient de surch lui-même.** Les ratios et
   débits viennent du binaire `zstd` CLI niveau 3 sur un échantillon de
   **10 000 documents**, pas des 28,9 M, et pas du code de surch. La projection
   à 28,9 M est un **calcul**, pas une mesure. Le −3 792 Mio est un plancher
   raisonné, il n'est pas prouvé.
4. **Le surcoût de latence +0,060 ms est calculé, pas mesuré**, à partir de
   débits `zstd -b` sur cette machine. Il ne tient pas compte des effets de
   cache CPU réels ni du coût d'allocation du `Vec` de sortie par hit.
5. **Le gain d'indexation n'est pas mesuré non plus.** L'argument (32× moins de
   `pwrite`, 7,9× de débit zstd) est solide mécaniquement, mais le corpus 28,9 M
   n'a pas été rejoué.
6. **Le harnais de bench n'a pas été touché** (interdiction explicite :
   `deploy/bench-local/` appartient à un autre chantier). Pour faire l'A/B
   disque, il faudra que son propriétaire propage `SURCH_SOURCE_COMPRESS_MODE`
   au conteneur, à côté du `SURCH_SOURCE_COMPRESS` déjà propagé
   (`fair-ab.sh:2274`). **Sans cela, le témoin par-document n'est pas
   atteignable depuis le harnais.**
7. **`compact_after_refresh` n'a pas été retesté.** Elle est
   `#[allow(dead_code)]` et appelée nulle part depuis `mmap M1` ; elle lit via
   `read_on_disk_bytes` donc elle gère le nouveau codec par construction, mais
   ce chemin mort n'a pas de couverture nouvelle.
8. **Les jauges disque comptent désormais uniquement les blocs scellés** : entre
   deux `_refresh`, jusqu'à 16 Kio de `_source` peuvent ne pas encore être
   écrits. C'est négligeable mais réel, et je n'ai pas vérifié qu'aucun test
   d'intégration existant ne dépend d'un `disk_segment_bytes` au document près
   (la recherche n'a trouvé **aucune** assertion sur ces jauges dans
   `crates/surch-api/tests/`, mais l'absence de preuve n'est pas une preuve).
9. **Les orphelins d'update grossissent légèrement** : mettre à jour un document
   laisse ses anciens octets dans le bloc, comme aujourd'hui dans le fichier. Le
   bulk `deces` étant append-only, l'effet est nul sur le corpus de référence,
   mais il n'est pas nul en général et n'a pas été chiffré.

---

D1_DONE
