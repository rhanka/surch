#!/usr/bin/env bash
# fair-ab.sh — A/B LOCAL ÉQUITABLE Surch vs Elasticsearch, pinné cgroup v2.
#
# Objectif : mesurer honnêtement les 4 axes (RAM sous limite, latence, indexation,
# disque) avec des conditions STRICTEMENT équivalentes — ES déborde sur mem+CPU
# tant qu'il peut, donc sans cap dur c'est de la triche. On pin donc :
#   - CPU  : --cpuset-cpus identique aux deux moteurs (cœurs physiques dédiés)
#   - RAM  : --memory=M --memory-swap=M (swap conteneur OFF -> OOM contenu, hôte safe)
#   - ES   : Xmx=Xms=M/2 (l'autre moitié = mmap Lucene = page-cache, l'exact
#            équivalent du page-cache disque de Surch -> combat loyal)
# Les moteurs tournent EN SÉQUENCE (pas de contention croisée), même corpus brut
# INSEE indexé from-scratch dans les deux (inclut l'axe indexation).
#
# Sécurité session : --memory-swap=--memory => un dépassement tue le CONTENEUR,
# jamais l'hôte. On laisse aussi la moitié des cœurs à l'hôte+session.
#
# Usage :
#   MEM_LIMIT=1024m CORPUS_LINES=200000 POSTINGS_DISK=1 ./fair-ab.sh
#   (sweep : boucler sur MEM_LIMIT ex 768m 1024m 1536m 2g pour le plancher de survie)
set -uo pipefail
export LC_ALL=C   # décimales en point, pas virgule (parsing latence/heap)

# MEM_LIMIT (ex 4g, 1536m) -> Mio entiers
# NB : classes [gG] explicites — IGNORECASE est un gawk-isme ignoré par mawk (défaut Ubuntu),
# "3G" y était lu comme 3 Mio.
mem_to_mib(){ echo "$1" | awk '{v=$0; if(v ~ /[gG]/){sub(/[gG].*/,"",v); print int(v*1024)} else {sub(/[mM].*/,"",v); print int(v)}}'; }

# ---- paramètres (env) ----
CPUSET="${CPUSET:-0-7,16-23}"          # 8 cœurs physiques (threads N & N+16), 8 restants pour l'hôte
MEM_LIMIT="${MEM_LIMIT:-2g}"           # cap cgroup identique aux deux
CORPUS_LINES="${CORPUS_LINES:-100000}" # nb de docs indexés (head du fichier INSEE)
DATA_FILE="${DATA_FILE:-$HOME/Téléchargements/deces-2025.txt}"
ES_IMAGE="${ES_IMAGE:-docker.elastic.co/elasticsearch/elasticsearch:8.6.1}"
# Défaut = image du run "28M@4g PASS" (commit ea86930, doc-only ; code = b795b10) : la précédente
# (sha-69668db4..., commit [ci-k8s] antérieur à [segments S3] ac3f12a) est stale et n'expose PAS
# encore la gauge surch_index_segment_count -> segment_count/2a resterait toujours null avec elle.
SURCH_IMAGE="${SURCH_IMAGE:-ghcr.io/rhanka/surch:sha-b795b100682afcfa65ab7db14f36d543cf039b38}"
POSTINGS_DISK="${POSTINGS_DISK:-1}"    # Surch : 1 = read-path disque (C1b)
OUT_DIR="${OUT_DIR:-$HOME/.cache/fair-ab/$(printf '%s' "$MEM_LIMIT")}"   # HORS /tmp : tmpfs = RAM hôte
# Le feeder matérialise une copie complète du bulk avant le premier envoi. Ce
# bind mount évite donc l'overlay Docker ; le défaut suit OUT_DIR, usuellement
# sur le disque système, et non le data-root Docker monté séparément sur VM.
# Le répertoire choisi doit être sur un système de fichiers distinct du
# data-root : le préflight refuse sinon un déplacement qui ne résoudrait rien.
FEEDER_TMP_DIR="${FEEDER_TMP_DIR:-$OUT_DIR/feeder-tmp}"
FEEDER_TMP_MARGIN_MIB="${FEEDER_TMP_MARGIN_MIB:-512}"
PROBE_REQUESTS="${PROBE_REQUESTS:-1000}"
REFRESH_EACH="${REFRESH_EACH:-0}"   # 1 = refresh après chaque chunk (counts corrects ; Surch perd sinon ~1 chunk sous bulk rapide)
# ---- sonde random / cold (front #1 "latence honnête", brainstorm-4-fronts-2026-07-09.md b1) ----
PROBE_NAMES_N="${PROBE_NAMES_N:-10000}"          # 1a : taille de l'échantillon probe_names.tsv
# PROBE_FIELD_NOM/PROBE_FIELD_PRENOMS : clés JSON à extraire des docs $BULK pour peupler les
# sondes fixe/random ET construire leurs requêtes (mêmes clés = c'est le champ ES réellement mappé).
# Défaut = schéma du builder awk interne ("nom"/"prenoms"). Pour un BULK_FILE au schéma matchID
# réel (ex. deces-1.36M.ndjson / deces-28M.ndjson, mapping deces-mapping.json), passer
# PROBE_FIELD_NOM=NOM PROBE_FIELD_PRENOMS=PRENOMS — sans cela les sondes requêteraient un champ
# absent de la mapping et retourneraient zéro hit. La sonde fixe doit elle aussi suivre ce réglage.
PROBE_FIELD_NOM="${PROBE_FIELD_NOM:-nom}"
PROBE_FIELD_PRENOMS="${PROBE_FIELD_PRENOMS:-prenoms}"
PROBE_FIXED_TERM="${PROBE_FIXED_TERM:-MARTIN}"
COLD_PROBE="${COLD_PROBE:-1}"       # 1c : 1 = tenter la sonde cold (memory.reclaim cgroup v2), 0 = off
COLD_PROBE_REQUESTS="${COLD_PROBE_REQUESTS:-50}" # protocole L2 : reclaim avant chacune des 50 requêtes
# Seuil fixe du protocole L2 : le cache file résiduel doit être <= 4 MiB ou
# <= 20 % de la valeur avant reclaim, selon la borne la moins stricte.
COLD_FILE_CACHE_FLOOR_BYTES=4194304
# Profil L2 benchmark-only : OFF par défaut. A 1, le runner passe un chemin
# explicite dans le volume Surch pour un JSONL borné, exporté par phase.
SURCH_SOURCE_FETCH_PROFILE="${SURCH_SOURCE_FETCH_PROFILE:-0}"
SURCH_SOURCE_FETCH_PROFILE_FILE="${SURCH_SOURCE_FETCH_PROFILE_FILE:-/tmp/surch-source-fetch-profile.jsonl}"
HOLD_SECONDS="${HOLD_SECONDS:-0}"   # après mesures, tient le conteneur N s avant teardown (permet
                                     # à artillery-replay.sh de se brancher sur le réseau fair-ab, 1d)
# BULK_FILE   : NDJSON pré-construit (lignes alternées action/doc) indexé TEL QUEL (bypass builder awk interne).
# MAPPING_FILE: JSON de mapping ES appliqué à la création de l'index deces_bench (au lieu du mapping minimal en dur).
# Non fournis => comportement inchangé (builder awk + mapping minimal).
BULK_FILE="${BULK_FILE:-}"
MAPPING_FILE="${MAPPING_FILE:-}"
BULK_RETRIES="${BULK_RETRIES:-3}"   # nb de tentatives par chunk avant échec dur
# P2 est un protocole fermé, volontairement opt-in : les usages historiques
# (L1/L2 et comparaison ES) gardent donc exactement leur comportement par
# défaut. Le pilote p2-campaign.sh active cette voie pour les six mesures.
P2_MEASURE="${P2_MEASURE:-0}"
P2_VARIANT="${P2_VARIANT:-}"          # A = avant, B = parcours P2
P2_INPUT_DIR="${P2_INPUT_DIR:-$OUT_DIR/p2-inputs}"
P2_PAIR_COUNT="${P2_PAIR_COUNT:-1000}" # 1000 full ; 100 seulement pour le smoke
P2_WARM_PAIR_COUNT="${P2_WARM_PAIR_COUNT:-100}"
P2_EXPECTED_DOCS="${P2_EXPECTED_DOCS:-28917511}"
P2_REQUIRED_SEGMENTS="${P2_REQUIRED_SEGMENTS:-12}"
P2_SEGMENT_GATE="${P2_SEGMENT_GATE:-exact}" # exact (full) ou minimum (smoke)
P2_CPU_STEAL_MAX_PERCENT="${P2_CPU_STEAL_MAX_PERCENT:-1}"
P2_PROTOCOL_VERSION="p2-segmented-postings-v2"
# Déclaré ici aussi parce que P2 refuse explicitement un bypass avant le
# bloc historique de préflight qui consomme cette variable.
PREFLIGHT_FORCE="${PREFLIGHT_FORCE:-0}"
NET="fair-ab-net"

log(){ printf '\033[1;36m[fair-ab]\033[0m %s\n' "$*"; }
err(){ printf '\033[1;31m[fair-ab]\033[0m %s\n' "$*" >&2; }
mkdir -p "$OUT_DIR" || { err "création OUT_DIR impossible : $OUT_DIR"; exit 1; }
mkdir -p "$FEEDER_TMP_DIR" || { err "création FEEDER_TMP_DIR impossible : $FEEDER_TMP_DIR"; exit 1; }
FEEDER_TMP_DIR=$(readlink -f "$FEEDER_TMP_DIR") || { err "résolution FEEDER_TMP_DIR impossible : $FEEDER_TMP_DIR"; exit 1; }
case "$FEEDER_TMP_MARGIN_MIB" in ''|*[!0-9]*) err "FEEDER_TMP_MARGIN_MIB doit être un entier en Mio"; exit 1;; esac
case "$PROBE_REQUESTS" in ''|*[!0-9]*) err "PROBE_REQUESTS doit être un entier positif"; exit 1;; esac
[ "$PROBE_REQUESTS" -gt 0 ] || { err "PROBE_REQUESTS doit être > 0"; exit 1; }
case "$P2_MEASURE" in 0|1) ;; *) err "P2_MEASURE doit valoir 0 ou 1"; exit 1;; esac
if [ "$P2_MEASURE" = "1" ]; then
  [ "$P2_VARIANT" = "A" ] || [ "$P2_VARIANT" = "B" ] || {
    err "P2_VARIANT doit valoir A ou B quand P2_MEASURE=1"; exit 1;
  }
  case "$P2_PAIR_COUNT:$P2_WARM_PAIR_COUNT" in *[!0-9:]*|:*|*::*) err "P2_PAIR_COUNT/P2_WARM_PAIR_COUNT invalides"; exit 1;; esac
  [ "$P2_WARM_PAIR_COUNT" -eq 100 ] || {
    err "protocole P2 invalide : P2_WARM_PAIR_COUNT doit rester 100"; exit 1;
  }
  [ "$PROBE_REQUESTS" -eq $(( 2 * P2_PAIR_COUNT )) ] || {
    err "protocole P2 invalide : PROBE_REQUESTS doit valoir 2 × P2_PAIR_COUNT (reçu $PROBE_REQUESTS pour $P2_PAIR_COUNT paires)"; exit 1;
  }
  [ "$COLD_PROBE_REQUESTS" = "50" ] || {
    err "protocole P2 invalide : COLD_PROBE_REQUESTS doit rester 50 (reçu $COLD_PROBE_REQUESTS)"; exit 1;
  }
  # Cold reste tenté par défaut, mais n'est plus une précondition des quatre
  # phases chaudes. Son reclaim dépend des droits cgroup de l'hôte, alors que
  # warm/fixed/random/no_source conservent à elles seules la comparaison A/B
  # identique, le routage et la parité qui fondent la validité P2.
  [ "$COLD_PROBE" = "0" ] || [ "$COLD_PROBE" = "1" ] || {
    err "protocole P2 invalide : COLD_PROBE doit valoir 0 ou 1"; exit 1;
  }
  [ "$PREFLIGHT_FORCE" = "0" ] || {
    err "protocole P2 invalide : PREFLIGHT_FORCE=1 contourne le garde-fou mémoire"; exit 1;
  }
  [ "$SURCH_SOURCE_FETCH_PROFILE" = "0" ] || {
    err "protocole P2 invalide : SURCH_SOURCE_FETCH_PROFILE doit rester à 0"; exit 1;
  }
  [ -n "$BULK_FILE" ] && [ -n "$MAPPING_FILE" ] || {
    err "protocole P2 : BULK_FILE et MAPPING_FILE explicites sont obligatoires"; exit 1;
  }
fi
if [ "$SURCH_SOURCE_FETCH_PROFILE" = "1" ] && [ "$PROBE_REQUESTS" -ne 1000 ]; then
  err "protocole L2 profilé invalide : PROBE_REQUESTS doit rester 1000 (reçu $PROBE_REQUESTS)"
  exit 1
fi
case "$COLD_PROBE_REQUESTS" in ''|*[!0-9]*) err "COLD_PROBE_REQUESTS doit être un entier"; exit 1;; esac
if [ "$COLD_PROBE_REQUESTS" -ne 50 ]; then
  err "protocole L2 cold invalide : COLD_PROBE_REQUESTS doit rester 50 (reçu $COLD_PROBE_REQUESTS)"
  exit 1
fi
case "$(stat -fc %T "$OUT_DIR" 2>/dev/null)" in tmpfs)
  err "AVERTISSEMENT OUT_DIR=$OUT_DIR est sur tmpfs (=RAM hôte) — les gros artefacts (bulk.ndjson) pèseront sur la mémoire";;
esac

# ---- garde-fous ----
[ "$(stat -fc %T /sys/fs/cgroup 2>/dev/null)" = "cgroup2fs" ] || { err "cgroup v2 requis"; exit 1; }
gov="$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo '?')"
if [ "$gov" != "performance" ]; then
  # Sur une VM virtualisée, `cpufreq` n'est pas exposé du tout : la fréquence est
  # pilotée par l'hyperviseur et le gouverneur est INCONTRÔLABLE, pas mal réglé.
  # On distingue donc les deux cas : absence de cpufreq (accepté, tracé dans la
  # scorecard, le bruit résiduel étant identique pour A et B et compensé par les
  # paires contrebalancées) versus gouverneur présent mais non-performance (refusé,
  # car là on POURRAIT le corriger et ne pas le faire biaiserait la mesure).
  if [ "$P2_MEASURE" = "1" ] && [ "$gov" != "?" ]; then
    err "protocole P2 invalide : gouverneur=$gov, performance obligatoire (corrigible sur cette machine)"
    exit 1
  fi
  if [ "$P2_MEASURE" = "1" ]; then
    err "AVERTISSEMENT protocole P2 : cpufreq absent (VM) — fréquence non contrôlable, biais assumé et identique A/B"
  fi
  err "AVERTISSEMENT gouverneur=$gov (biais fréquence ; 'sudo cpupower frequency-set -g performance' pour un run rigoureux)"
fi

# ---- plafonnement ressources hôte (post-mortem 2026-07-11 : OOM global machine) ----
# La nuit du 10-11/07 un run 28M a contribué à un OOM GLOBAL de l'hôte (le noyau a tué des
# processus tiers). Failles côté harnais : conteneurs auxiliaires (feeder split/curl, sondes)
# SANS cap mémoire -> la page cache du split 14,7 Go n'était pas bornée ; partie hôte
# (awk corpus/sondes) non bornée ; OUT_DIR par défaut sur /tmp (tmpfs = RAM). Garde-fous,
# AVANT tout travail — un emballement doit tuer le RUN, jamais la machine :
#  (0) re-exec dans un scope systemd user plafonné (MemoryMax) : borne les process hôte du
#      harnais Y COMPRIS leur page cache de lecture corpus (comptée au scope en cgroup v2)
#  (1) verrou flock : un seul fair-ab à la fois
#  (2) préflight : refus de démarrer si MemAvailable < cap conteneur + cap scope + marge
#  (3) trap : teardown conteneurs/volumes/réseau même sur interruption (INT/TERM)
# Les conteneurs moteurs étaient déjà cappés (--memory) ; les auxiliaires le sont via $AUXCAP.
HARNESS_MEM_MAX="${HARNESS_MEM_MAX:-3G}"              # cap du scope hôte du harnais (suffixe MAJUSCULE : systemd)
PREFLIGHT_MARGIN_MIB="${PREFLIGHT_MARGIN_MIB:-2048}"  # marge exigée au-delà des deux caps
# 1 = passer outre le préflight (déconseillé) ; P2 le refuse plus haut.
PREFLIGHT_FORCE="${PREFLIGHT_FORCE:-0}"
AUX_MEM="${AUX_MEM:-512m}"                            # cap des conteneurs auxiliaires
AUXCAP="--memory=$AUX_MEM --memory-swap=$AUX_MEM"
# La sonde ne partage aucun cœur avec le moteur : son coût CPU reste ainsi
# mesurable séparément. On ne suppose pas une topologie SMT particulière ;
# l'ensemble complémentaire de CPUSET, borné par nproc, est la seule source.
HOST_CPU_COUNT=$(nproc)
case "$HOST_CPU_COUNT" in ''|*[!0-9]*|0) err "nproc invalide : $HOST_CPU_COUNT"; exit 1;; esac
[ -n "$CPUSET" ] || { err "CPUSET ne doit pas être vide"; exit 1; }
PROBE_CPUSET=$(awk -v n="$HOST_CPU_COUNT" -v selected="$CPUSET" '
  BEGIN {
    if (n < 2) exit 1
    count = split(selected, ranges, ",")
    for (i = 1; i <= count; i++) {
      if (ranges[i] ~ /^[0-9]+$/) {
        first = ranges[i]; last = ranges[i]
      } else if (ranges[i] ~ /^[0-9]+-[0-9]+$/) {
        split(ranges[i], bounds, "-"); first = bounds[1]; last = bounds[2]
      } else {
        exit 1
      }
      if (first < 0 || last < first || last >= n) exit 1
      for (cpu = first; cpu <= last; cpu++) used[cpu] = 1
    }
    for (cpu = 0; cpu < n; cpu++) {
      if (!(cpu in used)) out = out == "" ? cpu : out "," cpu
    }
    if (out == "") exit 1
    print out
  }
')
probe_cpuset_rc=$?
if [ "$probe_cpuset_rc" -ne 0 ] || [ -z "$PROBE_CPUSET" ]; then
  err "CPUSET invalide ou aucun cœur restant pour la sonde : CPUSET=$CPUSET, nproc=$HOST_CPU_COUNT"
  exit 1
fi
if [ "$P2_MEASURE" = "1" ]; then
  # L'ancienne exigence d'une b3-16 (nproc=4, CPUSET=0-2, sonde=3) était
  # erronée : le nombre total de cœurs n'influence pas une paire A/B quand
  # chaque moteur reçoit le même cpuset et que la sonde est isolée. Elle
  # rendait en outre P2 impossible avec ses caps mémoire fixes. Le cpuset
  # moteur est validé non vide ci-dessus ; PROBE_CPUSET est son complément
  # calculé et non vide, donc la sonde est forcément pinnée hors du moteur.
  # La configuration effectivement observée est inscrite dans chaque
  # scorecard et p2-report.sh refuse toute paire A/B qui ne l'apparie pas.
  for ((p2_cpu = 0; p2_cpu < HOST_CPU_COUNT; p2_cpu++)); do
    p2_cpu_gov=$(cat "/sys/devices/system/cpu/cpu${p2_cpu}/cpufreq/scaling_governor" 2>/dev/null || true)
    # Même distinction qu'au garde-fou global : cpufreq absent = VM (accepté),
    # gouverneur présent mais non-performance = corrigible donc refusé.
    [ -z "$p2_cpu_gov" ] || [ "$p2_cpu_gov" = "performance" ] || {
      err "protocole P2 invalide : gouverneur cpu$p2_cpu=$p2_cpu_gov, performance obligatoire (corrigible)"
      exit 1
    }
  done
  [ "$(mem_to_mib "$MEM_LIMIT")" -eq 6144 ] \
    && [ "$(mem_to_mib "$HARNESS_MEM_MAX")" -eq 3072 ] \
    && [ "$PREFLIGHT_MARGIN_MIB" -eq 2048 ] \
    && [ "$(mem_to_mib "$AUX_MEM")" -eq 512 ] || {
      err "protocole P2 invalide : caps attendus MEM_LIMIT=6g HARNESS_MEM_MAX=3G PREFLIGHT_MARGIN_MIB=2048 AUX_MEM=512m"
      exit 1
    }
  [ "$POSTINGS_DISK" = "1" ] \
    && [ "${SURCH_SOURCE_COMPRESS:-}" = "1" ] \
    && [ -z "${SURCH_SOURCE_FETCH_PARALLEL:-}" ] \
    && [ "$PROBE_FIELD_NOM" = "NOM" ] \
    && [ "$PROBE_FIELD_PRENOMS" = "PRENOMS" ] || {
      err "protocole P2 invalide : configuration postings/source différente du cahier des charges"
      exit 1
    }
  case "$P2_EXPECTED_DOCS:$P2_REQUIRED_SEGMENTS" in
    *[!0-9:]*|:*|*::*) err "paramètres P2 documents/segments invalides"; exit 1;;
  esac
  if [ "$P2_SEGMENT_GATE" = "exact" ]; then
    [ "$P2_PAIR_COUNT" -eq 1000 ] \
      && [ "${SURCH_FLUSH_BUDGET_BYTES:-}" = "268435456" ] \
      && [ "${SURCH_MERGE_FANIN:-}" = "8" ] \
      && [ "${SURCH_DENSIFY_BUDGET_DOCS:-}" = "1000000" ] \
      && [ "${SURCH_MERGE_MAX_DOCS:-}" = "7000000" ] || {
        err "P2 full invalide : budgets de segments différents du protocole"; exit 1;
      }
  elif [ "$P2_SEGMENT_GATE" = "minimum" ]; then
    [ "$P2_PAIR_COUNT" -eq 100 ] \
      && [ "${SURCH_FLUSH_BUDGET_BYTES:-}" = "33554432" ] \
      && [ "${SURCH_MERGE_FANIN:-}" = "64" ] || {
        err "P2 smoke invalide : budgets de segments différents du protocole"; exit 1;
      }
  else
    err "P2_SEGMENT_GATE doit valoir exact ou minimum"; exit 1
  fi
  case "$P2_CPU_STEAL_MAX_PERCENT" in ''|*[!0-9.]*|*.*.*) err "P2_CPU_STEAL_MAX_PERCENT invalide"; exit 1;; esac
fi
PROBE_AUXCAP="$AUXCAP --cpuset-cpus=$PROBE_CPUSET"
log "sonde CPUSET=$PROBE_CPUSET (moteur=$CPUSET, nproc=$HOST_CPU_COUNT)"
if [ "${FAIRAB_SCOPED:-0}" != "1" ] && command -v systemd-run >/dev/null 2>&1 \
   && systemd-run --user --scope -q -p MemoryMax=64M -- true >/dev/null 2>&1; then
  log "re-exec dans un scope systemd plafonné (MemoryMax=$HARNESS_MEM_MAX, swap scope 256M)"
  FAIRAB_SCOPED=1 exec systemd-run --user --scope -q \
    -p MemoryMax="$HARNESS_MEM_MAX" -p MemorySwapMax=256M -- "$0" "$@"
fi
[ "${FAIRAB_SCOPED:-0}" = "1" ] || err "AVERTISSEMENT scope systemd indisponible -> partie hôte du harnais NON plafonnée"
if [ "$P2_MEASURE" = "1" ] && [ "${FAIRAB_SCOPED:-0}" != "1" ]; then
  err "protocole P2 invalide : scope systemd MemoryMax indisponible"
  exit 1
fi
exec 9>"${XDG_RUNTIME_DIR:-/tmp}/fair-ab.lock"
flock -n 9 || { err "un autre fair-ab tourne déjà (verrou fair-ab.lock) — runs concurrents refusés"; exit 1; }
mem_avail_mib=$(awk '/^MemAvailable:/{print int($2/1024)}' /proc/meminfo)
need_mib=$(( $(mem_to_mib "$MEM_LIMIT") + $(mem_to_mib "$HARNESS_MEM_MAX") + PREFLIGHT_MARGIN_MIB ))
if [ "${mem_avail_mib:-0}" -lt "$need_mib" ]; then
  if [ "$PREFLIGHT_FORCE" = "1" ]; then
    err "AVERTISSEMENT préflight bypassé (PREFLIGHT_FORCE=1) : ${mem_avail_mib}MiB dispo < ${need_mib}MiB requis"
  else
    err "préflight mémoire : ${mem_avail_mib}MiB disponibles < ${need_mib}MiB requis (cap conteneur $(mem_to_mib "$MEM_LIMIT")MiB + scope harnais $(mem_to_mib "$HARNESS_MEM_MAX")MiB + marge ${PREFLIGHT_MARGIN_MIB}MiB) — libérer de la RAM ou PREFLIGHT_FORCE=1"
    exit 1
  fi
fi
swap_total_kib=$(awk '/^SwapTotal:/{print $2}' /proc/meminfo); swap_free_kib=$(awk '/^SwapFree:/{print $2}' /proc/meminfo)
[ "${swap_total_kib:-0}" -gt 0 ] && [ "${swap_free_kib:-0}" -lt $(( swap_total_kib / 10 )) ] \
  && err "AVERTISSEMENT swap hôte quasi plein ($(( (swap_total_kib - swap_free_kib) / 1024 ))/$(( swap_total_kib / 1024 ))MiB) : machine déjà sous pression mémoire"
# BULK_FILE fourni => on n'a plus besoin du corpus brut INSEE ; sinon il est requis
if [ -n "$BULK_FILE" ]; then
  [ -s "$BULK_FILE" ] || { err "BULK_FILE introuvable/vide : $BULK_FILE"; exit 1; }
  BULK_FILE="$(readlink -f "$BULK_FILE")"   # docker -v exige un chemin absolu
else
  [ -f "$DATA_FILE" ] || { err "corpus introuvable : $DATA_FILE"; exit 1; }
fi
if [ -n "$MAPPING_FILE" ]; then
  [ -s "$MAPPING_FILE" ] || { err "MAPPING_FILE introuvable/vide : $MAPPING_FILE"; exit 1; }
  MAPPING_FILE="$(readlink -f "$MAPPING_FILE")"
fi

# ---- 1. corpus : INSEE largeur fixe -> NDJSON bulk (mêmes docs pour les 2 moteurs) ----
# BULK_FILE fourni => on l'indexe TEL QUEL (bypass builder). Sinon on (re)construit depuis l'INSEE.
if [ -n "$BULK_FILE" ]; then
  BULK="$BULK_FILE"
  log "corpus fourni via BULK_FILE : $BULK (builder awk bypassé)"
else
  BULK="$OUT_DIR/bulk.ndjson"
  if [ ! -s "$BULK" ] || [ "$(( $(wc -l < "$BULK") / 2 ))" -ne "$CORPUS_LINES" ]; then
    log "construction du corpus ($CORPUS_LINES docs) depuis $DATA_FILE"
    head -n "$CORPUS_LINES" "$DATA_FILE" | awk '
    # esc() : neutralise ce qui casserait le JSON — backslash, guillemet et caractères
    # de contrôle sont RETIRÉS (le corpus INSEE en contient : ex. un "\D" -> escape JSON
    # invalide qui faisait rejeter tout le _bulk de 10k docs par Surch, silencieusement).
    function esc(x){ gsub(/[[:cntrl:]\\"]/,"",x); return x }
    {
    line=$0
    # INSEE deces largeur fixe : nom*prénoms/ (1-80), sexe(81), naissance AAAAMMJJ(82-89),
    # code lieu nais(90-94), libellé lieu nais(95-124), décès AAAAMMJJ(155-162)
    nomp=substr(line,1,80); sub(/ +$/,"",nomp)
    split(nomp, a, "[*/]"); nom=a[1]; prenoms=a[2]
    sexe=substr(line,81,1)
    dnais=substr(line,82,8)
    lieu=substr(line,95,30); sub(/ +$/,"",lieu)
    ddeces=substr(line,155,8)
    nom=esc(nom); prenoms=esc(prenoms); lieu=esc(lieu)
    printf "{\"index\":{\"_id\":\"%d\"}}\n", NR
    printf "{\"nom\":\"%s\",\"prenoms\":\"%s\",\"sexe\":\"%s\",\"date_naissance\":\"%s\",\"lieu_naissance\":\"%s\",\"date_deces\":\"%s\"}\n", nom, prenoms, sexe, dnais, lieu, ddeces
  }' > "$BULK"
  fi
fi
NDOCS=$(( $(wc -l < "$BULK") / 2 ))
log "corpus prêt : $NDOCS docs ($(du -h "$BULK" | cut -f1))"

# ---- P2 : entrées gelées, strictement éligibles au chemin bool.must disque ----
# Les analyseurs découpent les blancs, apostrophes et ponctuation. La sélection
# est volontairement plus stricte : ASCII alphanumérique seulement. Rejeter un
# terme potentiellement multi-token vaut mieux que mesurer le repli générique.
p2_sha256(){ sha256sum "$1" | awk '{print $1}'; }

p2_validate_pairs(){
  local pairs="$1" wanted="$P2_PAIR_COUNT"
  awk -F '\t' -v wanted="$wanted" '
    NF != 3 || $1 != NR || $2 !~ /^[[:alnum:]]+$/ || $3 !~ /^[[:alnum:]]+$/ { invalid = 1; exit }
    END { exit (invalid || NR != wanted) }
  ' "$pairs"
}

p2_validate_body_files(){
  local pairs="$1" random="$2" no_source="$3" warm="$4" fixed="$5"
  local expected_random=$(( 2 * P2_PAIR_COUNT )) expected_warm=$(( 2 * P2_WARM_PAIR_COUNT ))
  [ "$(wc -l < "$random" 2>/dev/null || echo 0)" -eq "$expected_random" ] \
    && [ "$(wc -l < "$no_source" 2>/dev/null || echo 0)" -eq "$expected_random" ] \
    && [ "$(wc -l < "$warm" 2>/dev/null || echo 0)" -eq "$expected_warm" ] \
    && [ "$(wc -l < "$fixed" 2>/dev/null || echo 0)" -eq "$expected_random" ] || return 1
  awk -F '\t' -v bodies="$random" -v control="$no_source" -v warm="$warm" \
      -v fnom="$PROBE_FIELD_NOM" -v fpre="$PROBE_FIELD_PRENOMS" \
      -v wanted="$P2_PAIR_COUNT" -v warm_pairs="$P2_WARM_PAIR_COUNT" '
    function bool_body(nom, pre, size) {
      return "{\"query\":{\"bool\":{\"must\":[{\"match\":{\"" fnom "\":\"" nom "\"}},{\"match\":{\"" fpre "\":\"" pre "\"}}]}},\"size\":" size "}"
    }
    function match_body(nom, size) {
      return "{\"query\":{\"match\":{\"" fnom "\":\"" nom "\"}},\"size\":" size "}"
    }
    {
      if ((getline b1 < bodies) <= 0 || (getline b2 < bodies) <= 0) { invalid = 1; exit }
      if ((getline c1 < control) <= 0 || (getline c2 < control) <= 0) { invalid = 1; exit }
      if (b1 != bool_body($2, $3, 10) || b2 != match_body($2, 10)) { invalid = 1; exit }
      if (c1 != bool_body($2, $3, 0) || c2 != match_body($2, 0)) { invalid = 1; exit }
      if (NR <= warm_pairs) {
        if ((getline w1 < warm) <= 0 || (getline w2 < warm) <= 0) { invalid = 1; exit }
        if (w1 != bool_body($2, $3, 10) || w2 != match_body($2, 10)) { invalid = 1; exit }
      }
    }
    END {
      close(bodies); close(control); close(warm)
      exit (invalid || NR != wanted)
    }
  ' "$pairs" || return 1
  awk -v n="$expected_random" -v fnom="$PROBE_FIELD_NOM" -v term="$PROBE_FIXED_TERM" '
    { expected = "{\"query\":{\"match\":{\"" fnom "\":\"" term "\"}},\"size\":10}"; if ($0 != expected) { invalid = 1; exit } }
    END { exit (invalid || NR != n) }
  ' "$fixed"
}

p2_manifest_value(){
  local key="$1" file="$2"
  sed -n "s/^${key}=//p" "$file" | tail -n 1
}

p2_validate_frozen_inputs(){
  local manifest="$1" expected actual key file
  [ "$(p2_manifest_value protocol "$manifest")" = "$P2_PROTOCOL_VERSION" ] || return 1
  for key_file in \
    "bulk_sha256:$BULK" \
    "mapping_sha256:$MAPPING_FILE" \
    "pairs_sha256:$P2_PAIRS" \
    "warm_bodies_sha256:$P2_WARM_BODIES" \
    "fixed_bodies_sha256:$PROBE_FIXED_BODIES" \
    "random_bodies_sha256:$PROBE_BODIES" \
    "no_source_bodies_sha256:$PROBE_CONTROL_BODIES" \
    "random_kinds_sha256:$P2_RANDOM_KINDS"; do
    key=${key_file%%:*}; file=${key_file#*:}
    expected=$(p2_manifest_value "$key" "$manifest")
    actual=$(p2_sha256 "$file" 2>/dev/null) || return 1
    [ -n "$expected" ] && [ "$expected" = "$actual" ] || return 1
  done
  expected=$(cat "$manifest.sha256" 2>/dev/null | awk '{print $1}')
  actual=$(p2_sha256 "$manifest" 2>/dev/null) || return 1
  [ -n "$expected" ] && [ "$expected" = "$actual" ] || return 1
  p2_validate_pairs "$P2_PAIRS" && p2_validate_body_files "$P2_PAIRS" "$PROBE_BODIES" "$PROBE_CONTROL_BODIES" "$P2_WARM_BODIES" "$PROBE_FIXED_BODIES"
}

p2_prepare_inputs(){
  local manifest tmp pairs_tmp fixed_term_ok expected_random=$(( 2 * P2_PAIR_COUNT ))
  P2_PAIRS="$P2_INPUT_DIR/probe_p2_pairs.tsv"
  P2_WARM_BODIES="$P2_INPUT_DIR/probe_p2_warm_bodies.ndjson"
  PROBE_FIXED_BODIES="$P2_INPUT_DIR/probe_p2_fixed_bodies.ndjson"
  PROBE_BODIES="$P2_INPUT_DIR/probe_p2_random_bodies.ndjson"
  PROBE_CONTROL_BODIES="$P2_INPUT_DIR/probe_p2_no_source_bodies.ndjson"
  P2_RANDOM_KINDS="$P2_INPUT_DIR/probe_p2_random_kinds.tsv"
  PROBE_MANIFEST="$P2_INPUT_DIR/probe_p2_inputs.manifest"
  manifest="$PROBE_MANIFEST"
  P2_INPUT_MANIFEST_SHA_FILE="$manifest.sha256"
  mkdir -p "$P2_INPUT_DIR" || return 1

  if [ -s "$manifest" ]; then
    if ! p2_validate_frozen_inputs "$manifest"; then
      err "P2 : entrées gelées invalides ou différentes dans $P2_INPUT_DIR (régénération interdite)"
      return 1
    fi
    P2_INPUT_MANIFEST_SHA256=$(awk '{print $1}' "$P2_INPUT_MANIFEST_SHA_FILE")
    log "P2 : entrées gelées vérifiées (manifeste SHA-256=$P2_INPUT_MANIFEST_SHA256)"
    return 0
  fi
  if [ -n "$(find "$P2_INPUT_DIR" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null)" ]; then
    err "P2 : $P2_INPUT_DIR contient déjà des fichiers sans manifeste, écrasement refusé"
    return 1
  fi
  printf '%s' "$PROBE_FIXED_TERM" | awk '/^[[:alnum:]]+$/ { ok = 1 } END { exit !ok }' || {
    err "P2 : PROBE_FIXED_TERM doit être mono-token ASCII"; return 1;
  }

  log "P2 : sélection déterministe de $P2_PAIR_COUNT documents NOM/PRENOMS mono-token"
  pairs_tmp="$P2_PAIRS.tmp"
  if ! awk -v ndocs="$NDOCS" -v wanted="$P2_PAIR_COUNT" -v fnom="$PROBE_FIELD_NOM" -v fpre="$PROBE_FIELD_PRENOMS" '
    function extract(line, key,    pat, val) {
      pat = "\"" key "\":\"[^\"]*\""
      if (!match(line, pat)) return ""
      val = substr(line, RSTART, RLENGTH)
      sub(/^"[^"]*":"/, "", val); sub(/"$/, "", val)
      return val
    }
    function mono(value) { return value ~ /^[[:alnum:]]+$/ }
    NR % 2 == 0 {
      doc++
      nom = extract($0, fnom); pre = extract($0, fpre)
      if (!mono(nom) || !mono(pre)) next
      bucket = int((doc - 1) * wanted / ndocs) + 1
      if (!(bucket in bucket_value)) {
        bucket_value[bucket] = nom "\t" pre; bucket_doc[bucket] = doc
      }
      if (fallback_count < wanted * 2) {
        fallback_count++; fallback_value[fallback_count] = nom "\t" pre; fallback_doc[fallback_count] = doc
      }
    }
    END {
      for (bucket = 1; bucket <= wanted; bucket++) {
        if (bucket in bucket_value) {
          selected_doc[bucket_doc[bucket]] = 1
          selected[++selected_count] = bucket_value[bucket]
        }
      }
      for (i = 1; i <= fallback_count && selected_count < wanted; i++) {
        if (!(fallback_doc[i] in selected_doc)) {
          selected_doc[fallback_doc[i]] = 1
          selected[++selected_count] = fallback_value[i]
        }
      }
      if (selected_count != wanted) exit 1
      for (i = 1; i <= wanted; i++) print i "\t" selected[i]
    }
  ' "$BULK" > "$pairs_tmp"; then
    rm -f "$pairs_tmp"
    err "P2 : moins de $P2_PAIR_COUNT documents éligibles NOM/PRENOMS mono-token"
    return 1
  fi
  if ! p2_validate_pairs "$pairs_tmp"; then
    rm -f "$pairs_tmp"
    err "P2 : sélection contient un terme multi-token ou est incomplète"
    return 1
  fi
  mv "$pairs_tmp" "$P2_PAIRS" || return 1

  if ! awk -F '\t' -v random="$PROBE_BODIES" -v control="$PROBE_CONTROL_BODIES" \
      -v warm="$P2_WARM_BODIES" -v kinds="$P2_RANDOM_KINDS" \
      -v fnom="$PROBE_FIELD_NOM" -v fpre="$PROBE_FIELD_PRENOMS" \
      -v warm_pairs="$P2_WARM_PAIR_COUNT" '
    function bool_body(nom, pre, size) {
      return "{\"query\":{\"bool\":{\"must\":[{\"match\":{\"" fnom "\":\"" nom "\"}},{\"match\":{\"" fpre "\":\"" pre "\"}}]}},\"size\":" size "}"
    }
    function match_body(nom, size) {
      return "{\"query\":{\"match\":{\"" fnom "\":\"" nom "\"}},\"size\":" size "}"
    }
    {
      bool10 = bool_body($2, $3, 10); match10 = match_body($2, 10)
      print bool10 >> random; print match10 >> random
      print bool_body($2, $3, 0) >> control; print match_body($2, 0) >> control
      print (2 * NR - 1) "\tbool" >> kinds; print (2 * NR) "\tmatch" >> kinds
      if (NR <= warm_pairs) { print bool10 >> warm; print match10 >> warm }
    }
  ' "$P2_PAIRS"; then
    err "P2 : génération des corps random/warm impossible"
    return 1
  fi
  if ! awk -v n="$expected_random" -v fnom="$PROBE_FIELD_NOM" -v term="$PROBE_FIXED_TERM" '
    BEGIN { for (i = 1; i <= n; i++) print "{\"query\":{\"match\":{\"" fnom "\":\"" term "\"}},\"size\":10}" }
  ' > "$PROBE_FIXED_BODIES"; then
    err "P2 : génération des $expected_random témoins fixed impossible"
    return 1
  fi
  if ! p2_validate_body_files "$P2_PAIRS" "$PROBE_BODIES" "$PROBE_CONTROL_BODIES" "$P2_WARM_BODIES" "$PROBE_FIXED_BODIES"; then
    err "P2 : corps générés invalides"
    return 1
  fi
  {
    printf 'protocol=%s\n' "$P2_PROTOCOL_VERSION"
    printf 'bulk_sha256=%s\n' "$(p2_sha256 "$BULK")"
    printf 'mapping_sha256=%s\n' "$(p2_sha256 "$MAPPING_FILE")"
    printf 'pairs_sha256=%s\n' "$(p2_sha256 "$P2_PAIRS")"
    printf 'warm_bodies_sha256=%s\n' "$(p2_sha256 "$P2_WARM_BODIES")"
    printf 'fixed_bodies_sha256=%s\n' "$(p2_sha256 "$PROBE_FIXED_BODIES")"
    printf 'random_bodies_sha256=%s\n' "$(p2_sha256 "$PROBE_BODIES")"
    printf 'no_source_bodies_sha256=%s\n' "$(p2_sha256 "$PROBE_CONTROL_BODIES")"
    printf 'random_kinds_sha256=%s\n' "$(p2_sha256 "$P2_RANDOM_KINDS")"
  } > "$manifest" || return 1
  p2_sha256 "$manifest" > "$P2_INPUT_MANIFEST_SHA_FILE" || return 1
  P2_INPUT_MANIFEST_SHA256=$(cat "$P2_INPUT_MANIFEST_SHA_FILE")
  log "P2 : entrées créées puis gelées (manifeste SHA-256=$P2_INPUT_MANIFEST_SHA256)"
}

# ---- manifeste L2 : invalider les sondes si corpus ou contrat changent ----
# Le nombre de lignes seul ne prouve pas que les bodies correspondent encore
# au mapping courant. Le manifeste capture l'identité stat du corpus (sans le
# relire intégralement), le mapping et tous les paramètres qui influencent les
# requêtes; les quatre fichiers dérivés sont régénérés ensemble si nécessaire.
if [ "$P2_MEASURE" = "1" ]; then
  [ "$NDOCS" -eq "$P2_EXPECTED_DOCS" ] || {
    err "P2 : count corpus $NDOCS différent du count attendu $P2_EXPECTED_DOCS"; exit 1;
  }
  p2_prepare_inputs || { err "P2 : préparation des entrées échouée"; exit 1; }
else
PROBE_PROTOCOL_VERSION="l2-source-fetch-v3"
PROBE_NAMES="$OUT_DIR/probe_names.tsv"
PROBE_IDX="$OUT_DIR/probe_idx.txt"
PROBE_BODIES="$OUT_DIR/probe_rand_bodies.ndjson"
PROBE_FIXED_BODIES="$OUT_DIR/probe_fixed_bodies.ndjson"
PROBE_CONTROL_BODIES="$OUT_DIR/probe_no_source_bodies.ndjson"
PROBE_MANIFEST="$OUT_DIR/probe_manifest.txt"
want_probe_n=$NDOCS; [ "$want_probe_n" -gt "$PROBE_NAMES_N" ] && want_probe_n=$PROBE_NAMES_N
probe_corpus_identity=$(stat -Lc '%d:%i:%s:%Y' "$BULK" 2>/dev/null) || {
  err "fingerprint corpus impossible : $BULK"; exit 1;
}
if [ -n "$MAPPING_FILE" ]; then
  probe_mapping_identity=$(stat -Lc '%d:%i:%s:%Y' "$MAPPING_FILE" 2>/dev/null) || {
    err "fingerprint mapping impossible : $MAPPING_FILE"; exit 1;
  }
else
  probe_mapping_identity="mapping-minimal-interne"
fi
PROBE_MANIFEST_PAYLOAD=$(printf '%s\n' \
  "protocol=$PROBE_PROTOCOL_VERSION" \
  "corpus_path=$BULK" \
  "corpus_identity=$probe_corpus_identity" \
  "corpus_docs=$NDOCS" \
  "mapping_identity=$probe_mapping_identity" \
  "probe_field_nom=$PROBE_FIELD_NOM" \
  "probe_field_prenoms=$PROBE_FIELD_PRENOMS" \
  "probe_fixed_term=$PROBE_FIXED_TERM" \
  "probe_requests=$PROBE_REQUESTS" \
  "probe_names_n=$PROBE_NAMES_N" \
  "probe_names_wanted=$want_probe_n" \
  "request_cache=false")
PROBE_FINGERPRINT=$(printf '%s\n' "$PROBE_MANIFEST_PAYLOAD" | cksum | awk '{print $1 ":" $2}')
PROBE_CACHE_STALE=0
if [ ! -s "$PROBE_MANIFEST" ] \
   || [ "$(sed -n 's/^fingerprint=//p' "$PROBE_MANIFEST" 2>/dev/null | tail -1)" != "$PROBE_FINGERPRINT" ]; then
  PROBE_CACHE_STALE=1
  log "manifeste de sondes stale/absent : régénération complète (fingerprint=$PROBE_FINGERPRINT)"
fi

# ---- 1a. probe_names.tsv : échantillon déterministe tiré du corpus $BULK ----
# JAMAIS shuf sans graine, JAMAIS $RANDOM : échantillonnage À PAS FIXE (1 doc toutes les
# NDOCS/PROBE_NAMES_N lignes-doc). Comme on échantillonne le corpus BRUT (pas une liste de noms
# uniques), c'est un tirage PONDÉRÉ PAR LA FRÉQUENCE -> distribution Zipf naturelle (MARTIN sort
# proportionnellement à sa fréquence dans le corpus) : exactement le trafic matchID réel, sans
# inventer de distribution. Généré UNE FOIS avant la boucle ENGINES -> fichier partagé, identique
# pour ES et surch.
if [ "$PROBE_CACHE_STALE" = "1" ] || [ ! -s "$PROBE_NAMES" ] || [ "$(wc -l < "$PROBE_NAMES")" -ne "$want_probe_n" ]; then
  log "génération probe_names.tsv ($want_probe_n paires, pas fixe depuis \$BULK, champs $PROBE_FIELD_NOM/$PROBE_FIELD_PRENOMS)"
  awk -v ndocs="$NDOCS" -v n="$PROBE_NAMES_N" -v fnom="$PROBE_FIELD_NOM" -v fpre="$PROBE_FIELD_PRENOMS" '
    function esc(x){ gsub(/[[:cntrl:]\\"]/,"",x); return x }
    function extract(line, key,    pat, val) {
      pat = "\"" key "\":\"[^\"]*\""
      if (match(line, pat)) {
        val = substr(line, RSTART, RLENGTH)
        sub(/^"[^"]*":"/, "", val)
        sub(/"$/, "", val)
        return esc(val)
      }
      return ""
    }
    BEGIN { stride = ndocs / n; if (stride < 1) stride = 1; nextd = 1; d = 0; got = 0 }
    NR % 2 == 0 {
      d++
      if (got < n && d >= nextd) {
        print extract($0, fnom) "\t" extract($0, fpre)
        got++
        nextd += stride
      }
    }
  ' "$BULK" > "$PROBE_NAMES" || { err "échec génération probe_names.tsv"; exit 1; }
fi
PROBE_NAMES_COUNT=$(wc -l < "$PROBE_NAMES" 2>/dev/null); PROBE_NAMES_COUNT=${PROBE_NAMES_COUNT:-0}
[ "$PROBE_NAMES_COUNT" -eq "$want_probe_n" ] || {
  err "probe_names.tsv invalide : $PROBE_NAMES_COUNT/$want_probe_n lignes"; exit 1;
}
log "probe_names.tsv prêt : $PROBE_NAMES_COUNT paires nom/prenoms"

# ---- 1b (préparation) : requêtes random pré-générées, IDENTIQUES pour les 2 moteurs ----
# Séquence d'indices déterministe dans probe_names.tsv (LCG à graine fixe -> reproductible,
# jamais $RANDOM/shuf) ; mix 50/50 match/bool décidé par la parité de l'itération. Le LCG ne
# choisit que le nom : sa précision flottante awk ne peut donc plus biaiser le mix. size:10
# OBLIGATOIRE (force le fetch _source, le poste
# page-cache le plus gros — cf brainstorm-4-fronts-2026-07-09.md P2). Un seul fichier de bodies
# généré ICI (pas par moteur) -> même fichier monté dans les 2 conteneurs = mêmes requêtes, même
# ordre, par construction (pas seulement "en principe").
if [ "$PROBE_CACHE_STALE" = "1" ] || [ ! -s "$PROBE_BODIES" ] || [ "$(wc -l < "$PROBE_BODIES")" -ne "$PROBE_REQUESTS" ]; then
  log "génération séquence random ($PROBE_REQUESTS requêtes, LCG graine fixe, fichier partagé ES/surch)"
  awk -v n="$PROBE_REQUESTS" -v maxidx="$PROBE_NAMES_COUNT" -v fnom="$PROBE_FIELD_NOM" -v fpre="$PROBE_FIELD_PRENOMS" \
      -v idxout="$PROBE_IDX" -v namesfile="$PROBE_NAMES" '
    BEGIN {
      while ((getline line < namesfile) > 0) { cnt++; names[cnt] = line }
      close(namesfile)
      seed = 42
      for (i = 1; i <= n; i++) {
        seed = (seed * 1103515245 + 12345) % 2147483648
        idx = (seed % maxidx) + 1
        print idx > idxout
        split(names[idx], f, "\t")
        if (i % 2 == 0) {
          printf "{\"query\":{\"match\":{\"%s\":\"%s\"}},\"size\":10}\n", fnom, f[1]
        } else {
          printf "{\"query\":{\"bool\":{\"must\":[{\"match\":{\"%s\":\"%s\"}},{\"match\":{\"%s\":\"%s\"}}]}},\"size\":10}\n", fnom, f[1], fpre, f[2]
        }
      }
      close(idxout)
      exit
    }
  ' > "$PROBE_BODIES" || { err "échec génération probe_rand_bodies.ndjson"; exit 1; }
fi
[ "$(wc -l < "$PROBE_IDX" 2>/dev/null || echo 0)" -eq "$PROBE_REQUESTS" ] || {
  err "probe_idx.txt invalide : nombre de lignes différent de $PROBE_REQUESTS"; exit 1;
}
[ "$(wc -l < "$PROBE_BODIES" 2>/dev/null || echo 0)" -eq "$PROBE_REQUESTS" ] || {
  err "probe_rand_bodies.ndjson invalide : nombre de lignes différent de $PROBE_REQUESTS"; exit 1;
}
if [ "$SURCH_SOURCE_FETCH_PROFILE" = "1" ]; then
  probe_match_count=$(grep -c '"query":{"match"' "$PROBE_BODIES" 2>/dev/null || true)
  probe_bool_count=$(grep -c '"query":{"bool"' "$PROBE_BODIES" 2>/dev/null || true)
  cold_match_count=$(head -n 50 "$PROBE_BODIES" | grep -c '"query":{"match"' || true)
  cold_bool_count=$(head -n 50 "$PROBE_BODIES" | grep -c '"query":{"bool"' || true)
  [ "$probe_match_count" -eq 500 ] && [ "$probe_bool_count" -eq 500 ] \
    && [ "$cold_match_count" -eq 25 ] && [ "$cold_bool_count" -eq 25 ] || {
      err "mix L2 invalide : warm match/bool=$probe_match_count/$probe_bool_count, cold=$cold_match_count/$cold_bool_count"; exit 1;
    }
fi
log "sonde random prête : $PROBE_REQUESTS requêtes pré-générées"

# L2 : toutes les sondes partent de corps pré-générés et les temps bruts sont
# conservés. Le témoin est volontairement la même séquence random, mais
# `size:0` : le code search court-circuite alors documents_by_internal_ids.
if [ "$PROBE_CACHE_STALE" = "1" ] || [ ! -s "$PROBE_FIXED_BODIES" ] || [ "$(wc -l < "$PROBE_FIXED_BODIES")" -ne "$PROBE_REQUESTS" ]; then
  awk -v n="$PROBE_REQUESTS" -v fnom="$PROBE_FIELD_NOM" -v term="$PROBE_FIXED_TERM" \
    'BEGIN { for (i = 1; i <= n; i++) printf "{\"query\":{\"match\":{\"%s\":\"%s\"}},\"size\":10}\n", fnom, term }' \
    > "$PROBE_FIXED_BODIES" || { err "échec génération probe_fixed_bodies.ndjson"; exit 1; }
fi
if [ "$PROBE_CACHE_STALE" = "1" ] || [ ! -s "$PROBE_CONTROL_BODIES" ] || [ "$(wc -l < "$PROBE_CONTROL_BODIES")" -ne "$PROBE_REQUESTS" ]; then
  sed 's/"size":10}/"size":0}/' "$PROBE_BODIES" > "$PROBE_CONTROL_BODIES" \
    || { err "échec génération probe_no_source_bodies.ndjson"; exit 1; }
fi
[ "$(wc -l < "$PROBE_FIXED_BODIES" 2>/dev/null || echo 0)" -eq "$PROBE_REQUESTS" ] || {
  err "probe_fixed_bodies.ndjson invalide : nombre de lignes différent de $PROBE_REQUESTS"; exit 1;
}
[ "$(wc -l < "$PROBE_CONTROL_BODIES" 2>/dev/null || echo 0)" -eq "$PROBE_REQUESTS" ] || {
  err "probe_no_source_bodies.ndjson invalide : nombre de lignes différent de $PROBE_REQUESTS"; exit 1;
}
[ "$(grep -c '"size":0}' "$PROBE_CONTROL_BODIES" 2>/dev/null || echo 0)" -eq "$PROBE_REQUESTS" ] || {
  err "probe_no_source_bodies.ndjson ne contient pas exactement $PROBE_REQUESTS témoins size:0"; exit 1;
}
printf '%s\nfingerprint=%s\n' "$PROBE_MANIFEST_PAYLOAD" "$PROBE_FINGERPRINT" > "$PROBE_MANIFEST" \
  || { err "écriture manifeste de sondes impossible"; exit 1; }
fi

# Le feeder ne doit jamais retomber dans l'overlay Docker. Chaque moteur reçoit
# son sous-répertoire dédié ; cela permet de nettoyer sans toucher aux fichiers
# éventuellement présents dans FEEDER_TMP_DIR, même après un échec ou un signal.
FEEDER_TMP_ACTIVE_DIR=""
cleanup_feeder_tmp(){
  local dir="${1:-}"
  [ -n "$dir" ] || return 0
  rm -f -- "$dir"/chunk_* "$dir"/resp.json || {
    err "nettoyage feeder impossible : $dir"
    return 1
  }
  rmdir -- "$dir" 2>/dev/null || true
}

cleanup_fair_ab(){
  cleanup_feeder_tmp "$FEEDER_TMP_ACTIVE_DIR" || true
  docker rm -f fairab-es fairab-surch >/dev/null 2>&1
  docker volume rm fairab-vol-es fairab-vol-surch >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
}

prepare_feeder_tmp(){
  local engine="$1" dir bulk_bytes required_bytes available_kib available_bytes
  local required_mib available_mib docker_root feeder_fs docker_fs feeder_type
  dir="$FEEDER_TMP_DIR/$engine"
  mkdir -p "$dir" || { err "création répertoire feeder impossible : $dir"; return 1; }
  cleanup_feeder_tmp "$dir" || return 1
  mkdir -p "$dir" || { err "recréation répertoire feeder impossible : $dir"; return 1; }

  # La copie split est de même taille que le bulk ; la marge couvre la réponse
  # courante, les métadonnées de milliers de fichiers et les écarts de blocs.
  bulk_bytes=$(wc -c < "$BULK" 2>/dev/null) || { err "taille bulk illisible : $BULK"; return 1; }
  required_bytes=$(( bulk_bytes + FEEDER_TMP_MARGIN_MIB * 1024 * 1024 ))
  available_kib=$(df -Pk "$dir" | awk 'NR == 2 { print $4 }')
  available_bytes=$(( ${available_kib:-0} * 1024 ))
  required_mib=$(( (required_bytes + 1024 * 1024 - 1) / 1024 / 1024 ))
  available_mib=$(( available_bytes / 1024 / 1024 ))
  if [ "$available_bytes" -lt "$required_bytes" ]; then
    err "préflight feeder : FEEDER_TMP_DIR=$FEEDER_TMP_DIR manque d'espace — requis ${required_mib} Mio (bulk $(( bulk_bytes / 1024 / 1024 )) Mio + marge ${FEEDER_TMP_MARGIN_MIB} Mio), disponible ${available_mib} Mio"
    return 1
  fi

  feeder_type=$(stat -fc %T "$dir" 2>/dev/null || true)
  if [ "$feeder_type" = "tmpfs" ]; then
    err "préflight feeder : FEEDER_TMP_DIR=$FEEDER_TMP_DIR est sur tmpfs ; choisir un disque hôte avec ${required_mib} Mio libres"
    return 1
  fi
  docker_root=$(docker info --format '{{.DockerRootDir}}' 2>/dev/null) || {
    err "préflight feeder : docker info impossible (data-root inconnu)"
    return 1
  }
  feeder_fs=$(df -Pk "$dir" | awk 'NR == 2 { print $1 }')
  docker_fs=$(df -Pk "$docker_root" | awk 'NR == 2 { print $1 }')
  if [ -z "$feeder_fs" ] || [ -z "$docker_fs" ]; then
    err "préflight feeder : système de fichiers illisible (feeder=$dir, data-root=$docker_root)"
    return 1
  fi
  # Partager le système de fichiers du data-root n'est un problème que si l'espace
  # ne suffit pas aux DEUX simultanément : chunks du feeder (~15 Gio) + index Surch
  # (~12,3 Gio) + images. C'est ce cumul qui a fait échouer la VM OVH (volume de
  # 30 Gio), pas le partage en soi — une machine à disque unique de 150 Gio l'absorbe
  # sans difficulté. On exige donc l'espace cumulé, pas des disques distincts.
  if [ "$feeder_fs" = "$docker_fs" ]; then
    feeder_shared_required=$(( required_mib + 20480 ))
    if [ "$available_mib" -lt "$feeder_shared_required" ]; then
      err "préflight feeder : FEEDER_TMP_DIR=$FEEDER_TMP_DIR partage $feeder_fs avec le data-root Docker=$docker_root et l'espace cumulé est insuffisant (${available_mib} Mio disponibles < ${feeder_shared_required} requis = ${required_mib} chunks + 20480 index/images) ; choisir un disque distinct ou libérer de l'espace"
      return 1
    fi
    log "feeder : système de fichiers $feeder_fs partagé avec le data-root, mais espace cumulé suffisant (${available_mib} Mio >= ${feeder_shared_required})"
  fi

  FEEDER_TMP_ACTIVE_DIR="$dir"
  log "feeder : chunks sur $dir (requis ${required_mib} Mio, disponible ${available_mib} Mio ; data-root Docker=$docker_root)"
}

trap cleanup_fair_ab EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

docker network create "$NET" >/dev/null 2>&1 || true

# ---- helper : mesure RSS conteneur (memory.current, page-cache inclus) ----
sample_rss(){ docker stats --no-stream --format '{{.MemUsage}}' "$1" 2>/dev/null | awk '{print $1}'; }

# Les fichiers de séries restent possédés par le runner. Le conteneur curl ne
# reçoit qu'un mount lecture seule des corps et écrit le couple client/réponse
# sur sa sortie standard : aucun UID de curlimages/curl ne peut alors rendre
# un bind mount hôte vide ou partiellement écrit.
probe_series_file_is_valid(){
  local raw="$1" wanted="$2" phase="$3" series="$4" got
  got=$(wc -l < "$raw" 2>/dev/null); got=${got:-0}
  if [ "$got" -ne "$wanted" ]; then
    PROBE_CAPTURE_REASON="${phase}_${series}_line_count_${got}_of_${wanted}"
    return 1
  fi
  if ! awk 'NF != 1 || $1 !~ /^[0-9]+([.][0-9]+)?$/ || ($1 + 0) < 0 { exit 1 }' "$raw"; then
    PROBE_CAPTURE_REASON="${phase}_${series}_non_numeric_sample"
    return 1
  fi
}

# Le coût de sonde peut être légèrement négatif à l'unité près : `took` est
# arrondi par le moteur, alors que le temps client est une durée continue.
probe_overhead_file_is_valid(){
  local raw="$1" wanted="$2" phase="$3" got
  got=$(wc -l < "$raw" 2>/dev/null); got=${got:-0}
  if [ "$got" -ne "$wanted" ]; then
    PROBE_CAPTURE_REASON="${phase}_probe_overhead_line_count_${got}_of_${wanted}"
    return 1
  fi
  if ! awk 'NF != 1 || $1 !~ /^-?[0-9]+([.][0-9]+)?$/ { exit 1 }' "$raw"; then
    PROBE_CAPTURE_REASON="${phase}_probe_overhead_non_numeric_sample"
    return 1
  fi
}

# Décode les réponses après une invocation curl. `took` et hits.total.value
# font partie du contrat Elasticsearch compatible ; une réponse sans l'un des
# deux, ou une requête qui ne matche rien, invalide la phase entière.
decode_probe_capture(){
  local capture="$1" client_raw="$2" took_raw="$3" overhead_raw="$4" wanted="$5" phase="$6"
  if ! awk -F '\t' '
    NF != 2 || $1 !~ /^[0-9]+([.][0-9]+)?$/ || $2 !~ /^\{/ { exit 1 }
    { print $1 }
  ' "$capture" > "$client_raw"; then
    PROBE_CAPTURE_REASON="${phase}_client_or_response_malformed"
    return 1
  fi
  if ! awk -F '\t' '{ sub(/^[^\t]*\t/, ""); print }' "$capture" \
    | jq -er 'if (.took | type) == "number" and .took >= 0 then .took else error("took absent ou invalide") end' \
      > "$took_raw"; then
    PROBE_CAPTURE_REASON="${phase}_took_missing_or_invalid"
    return 1
  fi
  if ! awk -F '\t' '{ sub(/^[^\t]*\t/, ""); print }' "$capture" \
    | jq -er 'if (.hits.total.value | type) == "number" and .hits.total.value > 0 then .hits.total.value else error("zero hit ou hits.total.value invalide") end' \
      >/dev/null; then
    PROBE_CAPTURE_REASON="${phase}_zero_hits_or_hits_total_invalid"
    return 1
  fi
  if ! probe_series_file_is_valid "$client_raw" "$wanted" "$phase" client_s \
     || ! probe_series_file_is_valid "$took_raw" "$wanted" "$phase" took_ms; then
    return 1
  fi
  if ! awk 'NR == FNR { client[NR] = $1; next } { printf "%.6f\n", client[FNR] * 1000 - $1 }' \
    "$client_raw" "$took_raw" > "$overhead_raw"; then
    PROBE_CAPTURE_REASON="${phase}_probe_overhead_compute_failed"
    return 1
  fi
  probe_overhead_file_is_valid "$overhead_raw" "$wanted" "$phase"
}

# Écrit les séries client (s), moteur `took` (ms) et leur écart (ms). Une
# seule invocation curl porte toutes les requêtes de la phase : `--next`
# sépare les corps, tout en laissant libcurl réemployer ses connexions entre
# transferts. Chaque URL conserve `request_cache=false` : ES désactive ainsi
# son shard request cache, y compris pour le témoin `size:0`. Le démarrage du
# conteneur reste hors `time_total`.
capture_probe_samples(){
  local base="$1" bodies="$2" client_raw="$3" took_raw="$4" overhead_raw="$5" wanted="$6" phase="$7" responses_raw="${8:-}"
  local capture="$client_raw.capture"
  PROBE_CAPTURE_REASON=""
  : > "$client_raw"; : > "$took_raw"; : > "$overhead_raw"; : > "$capture"
  if ! docker run --rm $PROBE_AUXCAP --network "$NET" \
    -v "$bodies:/bodies.ndjson:ro" curlimages/curl:8.10.1 sh -eu -c '
      base=$1
      mkdir -p /tmp/request /tmp/response
      i=0
      set --
      while IFS= read -r body; do
        i=$((i + 1))
        name=$(printf "%06d" "$i")
        printf "%s\\n" "$body" > "/tmp/request/$name.json"
        [ "$i" -le 1 ] || set -- "$@" --next
        set -- "$@" --fail-with-body --silent --show-error \
          --output "/tmp/response/$name.json" --write-out "%{time_total}\\n" \
          "$base/deces_bench/_search?request_cache=false" \
          -H "Content-Type: application/json" --data-binary "@/tmp/request/$name.json"
      done < /bodies.ndjson
      [ "$i" -gt 0 ]
      curl --disable "$@" > /tmp/client_s
      [ "$(wc -l < /tmp/client_s)" -eq "$i" ]
      i=0
      for response in /tmp/response/*.json; do
        [ -s "$response" ]
        i=$((i + 1))
        client=$(sed -n "${i}p" /tmp/client_s)
        printf "%s\\t" "$client"
        tr -d "\\r\\n" < "$response"
        printf "\\n"
      done
      [ "$i" -eq "$(wc -l < /tmp/client_s)" ]
    ' sh "$base" > "$capture"; then
    PROBE_CAPTURE_REASON="${phase}_curl_exit_nonzero"
    return 1
  fi
  if ! decode_probe_capture "$capture" "$client_raw" "$took_raw" "$overhead_raw" "$wanted" "$phase"; then
    return 1
  fi
  # P2 conserve la réponse brute séparément du temps client. Cette dérivation
  # ne change pas les séries historiques et permet une parité A/B qui ignore
  # uniquement `took`, jamais les hits, scores, ordre ou _source.
  if [ -n "$responses_raw" ]; then
    if ! awk -F '\t' '{ sub(/^[^\t]*\t/, ""); print }' "$capture" > "$responses_raw" \
       || [ "$(wc -l < "$responses_raw" 2>/dev/null || echo 0)" -ne "$wanted" ]; then
      PROBE_CAPTURE_REASON="${phase}_responses_raw_capture_invalid"
      return 1
    fi
  fi
}

probe_quantiles(){
  local raw="$1" scale="$2"
  sort -n "$raw" | awk -v scale="$scale" '
    function nearest_rank(n, q, raw, rank) {
      raw = n * q
      rank = int(raw)
      if (rank < raw) rank++
      return (rank < 1) ? 1 : rank
    }
    { a[NR] = $1 }
    END {
      if (NR == 0) exit 1
      p50 = nearest_rank(NR, 0.50)
      p95 = nearest_rank(NR, 0.95)
      p99 = nearest_rank(NR, 0.99)
      printf "%.2f %.2f %.2f", a[p50] * scale, a[p95] * scale, a[p99] * scale
    }'
}

# ---- P2 : preuve du routage et statistiques par corps ----
# Cette jauge est publiée pendant l'indexation : elle doit donc être présente
# dès le contrôle index_ready, avant toute sonde.
p2_index_ready_metric_names=(
  surch_index_segment_count
)

p2_cpu_stat(){
  awk '/^cpu / { total = 0; for (i = 2; i <= NF; i++) total += $i; print total, $9; exit }' /proc/stat
}

p2_cpu_steal_percent(){
  local before="$1" after="$2"
  awk -v before="$before" -v after="$after" '
    BEGIN {
      split(before, b, " "); split(after, a, " ")
      total = a[1] - b[1]; steal = a[2] - b[2]
      if (total <= 0 || steal < 0) exit 1
      printf "%.6f", 100 * steal / total
    }
  '
}

p2_metric_present(){
  local metric="$1" snapshot="$2"
  grep -Eq "^${metric}(\\{|[[:space:]])" "$snapshot"
}

p2_snapshot_metrics(){
  local base="$1" out="$2" phase="$3" metric
  P2_METRICS_REASON=""
  if ! docker run --rm $AUXCAP --network "$NET" curlimages/curl:8.10.1 \
    --fail-with-body --silent --show-error "$base/_prometheus_metrics" > "$out"; then
    P2_METRICS_REASON="${phase}_prometheus_curl_failed"
    return 1
  fi
  for metric in "${p2_index_ready_metric_names[@]}"; do
    if ! p2_metric_present "$metric" "$out"; then
      P2_METRICS_REASON="${phase}_prometheus_metric_missing_${metric}"
      return 1
    fi
  done
}

p2_metric_value(){
  local metric="$1" snapshot="$2"
  if ! p2_metric_present "$metric" "$snapshot"; then
    return 1
  fi
  awk -v metric="$metric" '
    $0 ~ "^" metric "(\\{|[[:space:]])" { sum += $NF; found = 1 }
    END { if (!found) exit 1; printf "%.17g", sum }
  ' "$snapshot"
}

# Le recorder Prometheus crée une série de compteur lors de son premier
# increment. Avant la première requête bool.must, les compteurs de route et de
# blocs peuvent donc être absents des deux images sans signaler une anomalie.
# Pour calculer le premier delta, cette absence représente le zéro initial ;
# p2_write_phase_status exige ensuite, en fin de phase bool, la série attendue
# et sa valeur de routage exacte. Ainsi le protocole reste fail-closed sans
# demander à l'image A figée d'initialiser artificiellement ses compteurs.
p2_counter_value(){
  local metric="$1" snapshot="$2"
  if ! p2_metric_present "$metric" "$snapshot"; then
    printf '0'
    return 0
  fi
  p2_metric_value "$metric" "$snapshot"
}

p2_number_equal(){ awk -v left="$1" -v right="$2" 'BEGIN { exit !((left + 0) == (right + 0)) }'; }
p2_number_le(){ awk -v left="$1" -v right="$2" 'BEGIN { exit !((left + 0) <= (right + 0)) }'; }

p2_segment_value_valid(){
  local value="$1"
  case "$P2_SEGMENT_GATE" in
    exact) p2_number_equal "$value" "$P2_REQUIRED_SEGMENTS" ;;
    minimum) p2_number_le "$P2_REQUIRED_SEGMENTS" "$value" ;;
    *) return 1 ;;
  esac
}

p2_write_phase_status(){
  local phase="$1" bool_requests="$2" match_requests="$3" before="$4" after="$5"
  local cpu_steal_percent="$6" cpu_steal_within_limit="$7" cpu_steal_reason="$8"
  local direct_before generic_before read_before total_before segments_before
  local direct_after generic_after read_after total_after segments_after
  local direct_delta generic_delta read_delta total_delta segment_delta ratio="null" blocks_metrics_emitted=false
  local blocks_ratio_target_pass="null" blocks_ratio_verdict="not_applicable"
  local valid=true reason=""
  direct_before=$(p2_counter_value surch_bool_direct_must_fused_total "$before") || valid=false
  generic_before=$(p2_counter_value surch_dbg_generic_total "$before") || valid=false
  read_before=$(p2_counter_value surch_postings_disk_blocks_read_total "$before") || valid=false
  total_before=$(p2_counter_value surch_postings_disk_blocks_total "$before") || valid=false
  segments_before=$(p2_metric_value surch_index_segment_count "$before") || valid=false
  direct_after=$(p2_counter_value surch_bool_direct_must_fused_total "$after") || valid=false
  generic_after=$(p2_counter_value surch_dbg_generic_total "$after") || valid=false
  read_after=$(p2_counter_value surch_postings_disk_blocks_read_total "$after") || valid=false
  total_after=$(p2_counter_value surch_postings_disk_blocks_total "$after") || valid=false
  segments_after=$(p2_metric_value surch_index_segment_count "$after") || valid=false
  if [ "$valid" != true ]; then
    reason="metric_value_unreadable"
  else
    if p2_metric_present surch_postings_disk_blocks_read_total "$after" \
       && p2_metric_present surch_postings_disk_blocks_total "$after"; then
      blocks_metrics_emitted=true
    fi
    direct_delta=$(awk -v a="$direct_after" -v b="$direct_before" 'BEGIN { printf "%.17g", a - b }')
    generic_delta=$(awk -v a="$generic_after" -v b="$generic_before" 'BEGIN { printf "%.17g", a - b }')
    read_delta=$(awk -v a="$read_after" -v b="$read_before" 'BEGIN { printf "%.17g", a - b }')
    total_delta=$(awk -v a="$total_after" -v b="$total_before" 'BEGIN { printf "%.17g", a - b }')
    segment_delta=$(awk -v a="$segments_after" -v b="$segments_before" 'BEGIN { printf "%.17g", a - b }')
    if ! p2_segment_value_valid "$segments_before" || ! p2_segment_value_valid "$segments_after"; then
      valid=false; reason="segment_count_${segments_before}_to_${segments_after}_invalid"
    elif [ "$P2_VARIANT" = "A" ]; then
      if ! p2_number_equal "$direct_delta" 0 || ! p2_number_equal "$generic_delta" "$bool_requests"; then
        valid=false; reason="route_A_direct_${direct_delta}_generic_${generic_delta}_expected_0_${bool_requests}"
      elif [ "$bool_requests" -gt 0 ] \
        && ! p2_metric_present surch_dbg_generic_total "$after"; then
        valid=false; reason="route_A_generic_metric_missing_after_bool"
      fi
    elif ! p2_number_equal "$direct_delta" "$bool_requests" || ! p2_number_equal "$generic_delta" 0; then
      valid=false; reason="route_B_direct_${direct_delta}_generic_${generic_delta}_expected_${bool_requests}_0"
    elif [ "$bool_requests" -gt 0 ] \
      && ! p2_metric_present surch_bool_direct_must_fused_total "$after"; then
      valid=false; reason="route_B_direct_metric_missing_after_bool"
    fi
    # Une phase sans bool (fixed match) doit être neutre pour chaque compteur
    # P2. Dans les phases alternées, l'égalité au nombre de bool prouve que les
    # 1000 (ou 25/100) match n'ont ajouté aucun passage direct/générique.
    if [ "$valid" = true ] && [ "$bool_requests" -eq 0 ]; then
      if ! p2_number_equal "$read_delta" 0 || ! p2_number_equal "$total_delta" 0; then
        valid=false; reason="match_phase_p2_blocks_nonzero_read_${read_delta}_total_${total_delta}"
      fi
    fi
    if [ "$valid" = true ] && [ "$P2_VARIANT" = "B" ]; then
      if [ "$blocks_metrics_emitted" != true ]; then
        valid=false; reason="blocks_B_metrics_missing_after_phase"
      elif [ "$bool_requests" -gt 0 ] && { ! p2_number_le 1 "$total_delta" || ! p2_number_le 0 "$read_delta"; }; then
        valid=false; reason="blocks_B_non_positive_read_${read_delta}_total_${total_delta}"
      elif [ "$bool_requests" -gt 0 ]; then
        ratio=$(awk -v read="$read_delta" -v total="$total_delta" 'BEGIN { printf "%.9f", read / total }')
        if ! p2_number_le "$ratio" 0.25; then
          blocks_ratio_target_pass=false; blocks_ratio_verdict="fail"
        else
          blocks_ratio_target_pass=true; blocks_ratio_verdict="pass"
        fi
      fi
    fi
  fi
  P2_PHASE_VALID="$valid"
  P2_PHASE_REASON="$reason"
  # Le ratio de blocs qualifie l'effet du saut P2, pas la comparabilité de la
  # mesure : routage, corps, segments et réponses restent déjà fail-closed.
  # Un ratio hors cible est donc enregistré comme ÉCHEC P2 exploitable plutôt
  # que de transformer la phase A/B correcte en mesure invalide.
  printf '{"phase":"%s","variant":"%s","bool_requests":%s,"match_requests":%s,"valid":%s,"reason":%s,"cpu_steal_percent":%s,"cpu_steal_limit_percent":%s,"cpu_steal_within_limit":%s,"cpu_steal_reason":%s,"blocks_metrics_emitted":%s,"blocks_read_over_total":%s,"blocks_ratio_target":0.25,"blocks_ratio_target_pass":%s,"blocks_ratio_verdict":"%s","metrics":{"direct":{"before":%s,"after":%s,"delta":%s},"generic":{"before":%s,"after":%s,"delta":%s},"blocks_read":{"before":%s,"after":%s,"delta":%s},"blocks_total":{"before":%s,"after":%s,"delta":%s},"segments":{"before":%s,"after":%s,"delta":%s}}}\n' \
    "$phase" "$P2_VARIANT" "$bool_requests" "$match_requests" "$valid" \
    "$( [ -n "$reason" ] && printf '\"%s\"' "$reason" || printf 'null' )" \
    "$cpu_steal_percent" "$P2_CPU_STEAL_MAX_PERCENT" "$cpu_steal_within_limit" \
    "$( [ -n "$cpu_steal_reason" ] && printf '\"%s\"' "$cpu_steal_reason" || printf 'null' )" \
    "$blocks_metrics_emitted" "$ratio" "$blocks_ratio_target_pass" "$blocks_ratio_verdict" \
    "${direct_before:-null}" "${direct_after:-null}" "${direct_delta:-null}" \
    "${generic_before:-null}" "${generic_after:-null}" "${generic_delta:-null}" \
    "${read_before:-null}" "${read_after:-null}" "${read_delta:-null}" \
    "${total_before:-null}" "${total_after:-null}" "${total_delta:-null}" \
    "${segments_before:-null}" "${segments_after:-null}" "${segment_delta:-null}" \
    >> "$P2_PHASE_STATUS"
  [ "$valid" = true ]
}

p2_capture_phase(){
  local base="$1" phase="$2" bodies="$3" client_raw="$4" took_raw="$5" overhead_raw="$6" wanted="$7" bool_requests="$8" match_requests="$9"
  local before="$OUT_DIR/$ENGINE.p2.prom.${phase}.before.prom"
  local after="$OUT_DIR/$ENGINE.p2.prom.${phase}.after.prom"
  local responses_raw="$OUT_DIR/$ENGINE.p2.responses.${phase}.raw.ndjson"
  local responses_canonical="$OUT_DIR/$ENGINE.p2.responses.${phase}.canonical.ndjson"
  local cpu_before="" cpu_after="" cpu_steal_percent="null" cpu_steal_within_limit=false cpu_steal_reason=""
  # Le steal est borné à la fenêtre de chaque phase chaude. L'indexation peut
  # durer des dizaines de minutes et accumuler un steal sans rapport avec les
  # latences A/B observées ; une lecture locale protège réellement la mesure.
  cpu_before=$(p2_cpu_stat 2>/dev/null || true)
  if ! p2_snapshot_metrics "$base" "$before" "${phase}_before"; then
    PROBE_CAPTURE_REASON="${P2_METRICS_REASON:-${phase}_prometheus_before_failed}"
    return 1
  fi
  if ! capture_probe_samples "$base" "$bodies" "$client_raw" "$took_raw" "$overhead_raw" "$wanted" "$phase" "$responses_raw"; then
    return 1
  fi
  if ! jq -cS '{hits_total: .hits.total, max_score: .hits.max_score, hits: [.hits.hits[] | {_id: ._id, _score: ._score, _source: ._source}]}' \
      "$responses_raw" > "$responses_canonical" \
     || [ "$(wc -l < "$responses_canonical" 2>/dev/null || echo 0)" -ne "$wanted" ]; then
    PROBE_CAPTURE_REASON="${phase}_response_canonicalization_failed"
    return 1
  fi
  if ! p2_snapshot_metrics "$base" "$after" "${phase}_after"; then
    PROBE_CAPTURE_REASON="${P2_METRICS_REASON:-${phase}_prometheus_after_failed}"
    return 1
  fi
  cpu_after=$(p2_cpu_stat 2>/dev/null || true)
  if [ -n "$cpu_before" ] && [ -n "$cpu_after" ] \
     && p2_cpu_steal_percent "$cpu_before" "$cpu_after" >/dev/null 2>&1; then
    cpu_steal_percent=$(p2_cpu_steal_percent "$cpu_before" "$cpu_after")
    if p2_number_le "$cpu_steal_percent" "$P2_CPU_STEAL_MAX_PERCENT"; then
      cpu_steal_within_limit=true
    else
      cpu_steal_reason="cpu_steal_${cpu_steal_percent}_above_${P2_CPU_STEAL_MAX_PERCENT}"
    fi
  else
    cpu_steal_reason="cpu_steal_unreadable"
  fi
  if ! p2_write_phase_status "$phase" "$bool_requests" "$match_requests" "$before" "$after" \
      "$cpu_steal_percent" "$cpu_steal_within_limit" "$cpu_steal_reason"; then
    PROBE_CAPTURE_REASON="${phase}_routing_invalid_${P2_PHASE_REASON:-unknown}"
    return 1
  fi
}

p2_quantiles_with_max(){
  local raw="$1" scale="$2"
  sort -n "$raw" | awk -v scale="$scale" '
    function nearest_rank(n, q, raw, rank) {
      raw = n * q; rank = int(raw); if (rank < raw) rank++
      return (rank < 1) ? 1 : rank
    }
    { a[NR] = $1 }
    END {
      if (NR == 0) exit 1
      printf "%.2f %.2f %.2f %.2f", a[nearest_rank(NR, 0.50)] * scale, a[nearest_rank(NR, 0.95)] * scale, a[nearest_rank(NR, 0.99)] * scale, a[NR] * scale
    }
  '
}

p2_write_stat(){
  local phase="$1" kind="$2" metric="$3" raw="$4" scale="$5" wanted="$6"
  local p50 p95 p99 max actual
  actual=$(wc -l < "$raw" 2>/dev/null); actual=${actual:-0}
  [ "$actual" -eq "$wanted" ] || return 1
  if [ "$metric" = "probe" ]; then
    probe_overhead_file_is_valid "$raw" "$wanted" "p2_${phase}_${kind}" || return 1
  else
    probe_series_file_is_valid "$raw" "$wanted" "p2_${phase}_${kind}" "$metric" || return 1
  fi
  read -r p50 p95 p99 max < <(p2_quantiles_with_max "$raw" "$scale") || return 1
  printf '{"phase":"%s","kind":"%s","metric":"%s","raw_file":"%s","n":%s,"unit":"ms","p50":%s,"p95":%s,"p99":%s,"max":%s}\n' \
    "$phase" "$kind" "$metric" "$raw" "$wanted" "$p50" "$p95" "$p99" "$max" >> "$P2_STATS_JSONL"
}

p2_split_and_stat_phase(){
  local phase="$1" client="$2" took="$3" overhead="$4" bool_count="$5" match_count="$6"
  local kind src out metric scale wanted
  for kind in bool match; do
    case "$kind" in
      bool) wanted="$bool_count" ;;
      match) wanted="$match_count" ;;
    esac
    [ "$wanted" -gt 0 ] || continue
    for spec in "client:$client:1000" "took:$took:1" "probe:$overhead:1"; do
      metric=${spec%%:*}; spec=${spec#*:}; src=${spec%:*}; scale=${spec##*:}
      out="$OUT_DIR/$ENGINE.p2.${phase}.${kind}.${metric}$( [ "$metric" = client ] && printf '_s' || printf '_ms' )"
      if [ "$bool_count" -eq 0 ] || [ "$match_count" -eq 0 ]; then
        cp "$src" "$out"
      elif [ "$kind" = "bool" ]; then
        awk 'NR % 2 == 1 { print }' "$src" > "$out"
      else
        awk 'NR % 2 == 0 { print }' "$src" > "$out"
      fi
      p2_write_stat "$phase" "$kind" "$metric" "$out" "$scale" "$wanted" || return 1
    done
  done
}

# Snapshot Prometheus brut par phase. Les compteurs restent bornés côté
# serveur; les distributions exactes sont dans le JSONL, jamais dans des
# summaries Prometheus.
snapshot_source_fetch_metrics(){
  local base="$1" out="$2" phase="$3" require_metrics="$4"
  SOURCE_FETCH_SCRAPE_REASON=""
  : > "$out"
  if ! docker run --rm $AUXCAP --network "$NET" curlimages/curl:8.10.1 \
    --fail-with-body --silent --show-error "$base/_prometheus_metrics" \
    | awk '/^# (HELP|TYPE) surch_source_fetch_|^surch_source_fetch_/' > "$out"; then
    SOURCE_FETCH_SCRAPE_REASON="${phase}_prometheus_curl_exit_nonzero"
    return 1
  fi
  if [ "$require_metrics" = "1" ] \
     && { [ ! -s "$out" ] \
       || ! grep -q '^surch_source_fetch_requests_total' "$out" \
       || ! grep -q '^surch_source_fetch_requested_ids_total' "$out" \
       || ! grep -q '^surch_source_fetch_workers_total' "$out" \
       || ! grep -q '^surch_source_fetch_profile_records_total' "$out"; }; then
    SOURCE_FETCH_SCRAPE_REASON="${phase}_prometheus_expected_metrics_missing"
    return 1
  fi
}

# Soustrait seulement les compteurs monotones. Les summaries et les gauges ne
# passent jamais cette barrière : aucun quantile Prometheus n'est donc delta.
source_fetch_phase_delta(){
  local before="$1" after="$2" out="$3"
  awk '
    function key(line, value_start, result) {
      result = line
      sub(/[[:space:]][^[:space:]]+$/, "", result)
      return result
    }
    NR == FNR {
      if ($0 ~ /^surch_source_fetch_.*_total(\{|[[:space:]])/) previous[key($0)] = $NF + 0
      next
    }
    $0 ~ /^surch_source_fetch_.*_total(\{|[[:space:]])/ {
      sample = key($0)
      printf "%s %.17g\n", sample, (($NF + 0) - ((sample in previous) ? previous[sample] : 0))
    }
  ' "$before" "$after" > "$out"
}

# Somme un compteur dans un delta Prometheus. Les familles sont connues et
# sans labels libres; un nom absent vaut zero pour rendre les gates explicites.
source_fetch_counter_total(){
  local delta="$1" metric="$2"
  awk -v metric="$metric" '
    substr($1, 1, length(metric)) == metric && (substr($1, length(metric) + 1, 1) == "" || substr($1, length(metric) + 1, 1) == "{") {
      total += $NF
    }
    END { printf "%.0f", total + 0 }
  ' "$delta"
}

# Exporte le prefixe nouveau d'un JSONL append-only apres une phase stable.
# Le runner ne lit le fichier qu'apres le retour de tous les curls : il n'y a
# donc pas de lecture concurrente d'une ligne en cours d'ecriture. Le JSONL
# annote la phase ici, sous controle du harnais, pour les filtres/bootstrap.
source_fetch_export_phase(){
  local cid="$1" container_file="$2" snapshot="$3" out="$4" phase="$5" previous="$6"
  SOURCE_FETCH_ARTIFACT_REASON=""
  if ! docker cp "$cid:$container_file" "$snapshot" >/dev/null 2>&1; then
    SOURCE_FETCH_ARTIFACT_REASON="${phase}_profile_jsonl_copy_failed"
    return 1
  fi
  local total start
  total=$(wc -l < "$snapshot" 2>/dev/null); total=${total:-0}
  case "$total:$previous" in *[!0-9:]* )
    SOURCE_FETCH_ARTIFACT_REASON="${phase}_profile_jsonl_count_invalid"; return 1;;
  esac
  if [ "$total" -lt "$previous" ]; then
    SOURCE_FETCH_ARTIFACT_REASON="${phase}_profile_jsonl_non_monotonic"; return 1
  fi
  start=$((previous + 1))
  if ! tail -n +"$start" "$snapshot" | awk -v phase="$phase" '
    function has_number(name) {
      return match($0, "\\\"" name "\\\":[0-9]+")
    }
    {
      if ($0 !~ /^\{"mode":"(parallel|sequential)",/)
        invalid = 1
      split("requested_ids hydrated_hits bytes fetch_wall_us pread_sum_us pread_max_us decode_sum_us decode_max_us json_sum_us json_max_us workers_effectifs", names, " ")
      for (i in names) if (!has_number(names[i])) invalid = 1
      if (invalid) exit 1
      sub(/}$/, ",\"phase\":\"" phase "\"}")
      print
    }
  ' > "$out"; then
    SOURCE_FETCH_ARTIFACT_REASON="${phase}_profile_jsonl_invalid"
    return 1
  fi
  SOURCE_FETCH_ARTIFACT_RECORDS=$((total - previous))
}

# Le nombre de lignes exportees doit être celui que le compteur monotone dit
# avoir ecrit. Toute erreur d'I/O ou troncature de la borne 4096 invalide L2.
source_fetch_artifact_matches_delta(){
  local delta="$1" artifact="$2"
  local expected actual dropped failures
  expected=$(source_fetch_counter_total "$delta" surch_source_fetch_profile_records_total)
  dropped=$(source_fetch_counter_total "$delta" surch_source_fetch_profile_dropped_total)
  failures=$(source_fetch_counter_total "$delta" surch_source_fetch_profile_write_failures_total)
  actual=$(wc -l < "$artifact" 2>/dev/null); actual=${actual:-0}
  [ "$expected" -eq "$actual" ] && [ "$dropped" -eq 0 ] && [ "$failures" -eq 0 ]
}

source_fetch_random_hydrated_eight_plus(){
  awk '
    /"phase":"random"/ {
      value = $0
      sub(/^.*"hydrated_hits":/, "", value)
      sub(/,.*/, "", value)
      if (value ~ /^[0-9]+$/ && value + 0 >= 8) count++
    }
    END { printf "%.0f", count + 0 }
  ' "$1"
}

source_fetch_no_source_delta_is_zero(){
  awk '
    /^surch_source_fetch_(requests|hits|bytes|decoded_bytes)_total(\{|[[:space:]])/ {
      seen = 1
      if (($NF + 0) != 0) invalid = 1
    }
    END { exit !(seen && !invalid) }
  ' "$1"
}

# Sonde froide L2 : chaque requête est précédée d'un reclaim cgroup v2. La
# quantité demandée vise le cache `file` réellement observé. Chaque couple
# before/after est conservé : aucune éviction partielle ambigüe n'est admise.
capture_cold_samples(){
  local base="$1" bodies="$2" client_raw="$3" took_raw="$4" overhead_raw="$5" audit="$6" reclaim_path="$7" stat_path="$8" current_path="$9" writer="${10}" wanted="${11}" responses_raw="${12:-}"
  local body file_before file_after file_after_max memory_current reclaim_target
  local completed=0 request_no one_body one_client one_took one_overhead one_responses
  COLD_CAPTURE_REASON=""
  COLD_CAPTURE_COMPLETED=0
  : > "$client_raw"; : > "$took_raw"; : > "$overhead_raw"
  : > "$audit"
  one_body="$client_raw.one_body.ndjson"
  one_client="$client_raw.one_client_s"
  one_took="$client_raw.one_took_ms"
  one_overhead="$client_raw.one_probe_overhead_ms"
  one_responses="$client_raw.one_responses.ndjson"
  [ -z "$responses_raw" ] || : > "$responses_raw"

  while IFS= read -r body; do
    [ "$completed" -ge "$wanted" ] && break
    request_no=$((completed + 1))
    file_before=$(awk '/^file /{print $2; exit}' "$stat_path" 2>/dev/null)
    memory_current=$(cat "$current_path" 2>/dev/null)
    case "$file_before" in ''|*[!0-9]*)
      COLD_CAPTURE_REASON="memory_stat_or_current_invalid_before_request_$request_no"
      COLD_CAPTURE_COMPLETED="$completed"
      return 1;;
    esac
    case "$memory_current" in ''|*[!0-9]*)
      COLD_CAPTURE_REASON="memory_stat_or_current_invalid_before_request_$request_no"
      COLD_CAPTURE_COMPLETED="$completed"
      return 1;;
    esac
    file_after_max=$(( file_before / 5 ))
    if [ "$file_after_max" -lt "$COLD_FILE_CACHE_FLOOR_BYTES" ]; then
      file_after_max="$COLD_FILE_CACHE_FLOOR_BYTES"
    fi
    reclaim_target="$file_before"
    if [ "$reclaim_target" -gt "$memory_current" ]; then
      reclaim_target="$memory_current"
    fi

    if [ "$writer" = "sudo" ]; then
      if ! sudo -n sh -c 'printf "%s\\n" "$1" > "$2"' sh "$reclaim_target" "$reclaim_path" >/dev/null 2>&1; then
        COLD_CAPTURE_REASON="memory_reclaim_write_failed_before_request_$request_no"
        COLD_CAPTURE_COMPLETED="$completed"
        return 1
      fi
    elif ! printf '%s\n' "$reclaim_target" > "$reclaim_path"; then
      COLD_CAPTURE_REASON="memory_reclaim_write_failed_before_request_$request_no"
      COLD_CAPTURE_COMPLETED="$completed"
      return 1
    fi
    file_after=$(awk '/^file /{print $2; exit}' "$stat_path" 2>/dev/null)
    case "$file_after" in ''|*[!0-9]*)
      COLD_CAPTURE_REASON="memory_stat_file_absent_after_request_$request_no"
      COLD_CAPTURE_COMPLETED="$completed"
      return 1;;
    esac
    if [ "$file_after" -gt "$file_after_max" ]; then
      COLD_CAPTURE_REASON="memory_reclaim_target_missed_before_request_${request_no}_after_${file_after}_max_${file_after_max}"
      COLD_CAPTURE_COMPLETED="$completed"
      return 1
    fi
    printf '%s\t%s\t%s\t%s\n' "$request_no" "$file_before" "$file_after" "$file_after_max" >> "$audit"

    printf '%s\n' "$body" > "$one_body"
    # Le reclaim doit précéder chaque transfert cold : cette phase garde donc
    # une invocation curl isolée par requête, explicitement déclarée dans la
    # scorecard, plutôt que de simuler une réutilisation incompatible avec le
    # protocole d'éviction vérifié.
    if [ -n "$responses_raw" ]; then
      capture_probe_samples "$base" "$one_body" "$one_client" "$one_took" "$one_overhead" 1 cold "$one_responses"
    else
      capture_probe_samples "$base" "$one_body" "$one_client" "$one_took" "$one_overhead" 1 cold
    fi
    if [ "$?" -ne 0 ]; then
      COLD_CAPTURE_REASON="${PROBE_CAPTURE_REASON}_after_reclaim_$request_no"
      COLD_CAPTURE_COMPLETED="$completed"
      return 1
    fi
    cat "$one_client" >> "$client_raw"
    cat "$one_took" >> "$took_raw"
    cat "$one_overhead" >> "$overhead_raw"
    [ -z "$responses_raw" ] || cat "$one_responses" >> "$responses_raw"
    completed=$((completed + 1))
  done < "$bodies"

  COLD_CAPTURE_COMPLETED="$completed"
  if [ "$completed" -ne "$wanted" ]; then
    COLD_CAPTURE_REASON="probe_bodies_short_${completed}_of_${wanted}"
    return 1
  fi
  [ "$(wc -l < "$audit" 2>/dev/null || echo 0)" -eq "$wanted" ] || {
    COLD_CAPTURE_REASON="cold_reclaim_audit_count_invalid"
    return 1
  }
  if ! probe_series_file_is_valid "$client_raw" "$wanted" cold client_s \
     || ! probe_series_file_is_valid "$took_raw" "$wanted" cold took_ms; then
    COLD_CAPTURE_REASON="$PROBE_CAPTURE_REASON"
    return 1
  fi
  if ! probe_overhead_file_is_valid "$overhead_raw" "$wanted" cold; then
    COLD_CAPTURE_REASON="$PROBE_CAPTURE_REASON"
    return 1
  fi
  if [ -n "$responses_raw" ] && [ "$(wc -l < "$responses_raw" 2>/dev/null || echo 0)" -ne "$wanted" ]; then
    COLD_CAPTURE_REASON="cold_responses_raw_line_count_invalid"
    return 1
  fi
}

# Un échec de sonde reste visible dans la scorecard sans inventer des
# quantiles à zéro. Le code appelant reçoit aussi un statut non nul pour
# arrêter le run avec un échec fermé après avoir conservé ce diagnostic.
record_invalid_measurement(){
  local engine="$1" count="$2" indexed="$3" item_errors="$4" reason="$5"
  printf '{"engine":"%s","mem_limit":"%s","cpuset":"%s","probe_cpuset":"%s","survived_boot":true,"survived_index":true,"count":%s,"expected":%s,"indexed":%s,"item_errors":%s,"measurement_valid":false,"measurement_invalid_reason":"%s","lat_p50_ms":null,"lat_p95_ms":null,"lat_p99_ms":null,"lat_rand_p50_ms":null,"lat_rand_p95_ms":null,"lat_rand_p99_ms":null,"lat_no_source_p50_ms":null,"lat_no_source_p95_ms":null,"lat_no_source_p99_ms":null,"lat_cold_p50_ms":null,"lat_cold_p95_ms":null,"lat_cold_p99_ms":null,"lat_fixed_client_s_file":null,"lat_fixed_took_ms_file":null,"lat_fixed_probe_overhead_ms_file":null,"lat_fixed_client_p50_ms":null,"lat_fixed_client_p95_ms":null,"lat_fixed_client_p99_ms":null,"lat_fixed_took_p50_ms":null,"lat_fixed_took_p95_ms":null,"lat_fixed_took_p99_ms":null,"lat_fixed_probe_overhead_p50_ms":null,"lat_fixed_probe_overhead_p95_ms":null,"lat_fixed_probe_overhead_p99_ms":null,"lat_rand_client_s_file":null,"lat_rand_took_ms_file":null,"lat_rand_probe_overhead_ms_file":null,"lat_rand_client_p50_ms":null,"lat_rand_client_p95_ms":null,"lat_rand_client_p99_ms":null,"lat_rand_took_p50_ms":null,"lat_rand_took_p95_ms":null,"lat_rand_took_p99_ms":null,"lat_rand_probe_overhead_p50_ms":null,"lat_rand_probe_overhead_p95_ms":null,"lat_rand_probe_overhead_p99_ms":null,"lat_no_source_client_s_file":null,"lat_no_source_took_ms_file":null,"lat_no_source_probe_overhead_ms_file":null,"lat_no_source_client_p50_ms":null,"lat_no_source_client_p95_ms":null,"lat_no_source_client_p99_ms":null,"lat_no_source_took_p50_ms":null,"lat_no_source_took_p95_ms":null,"lat_no_source_took_p99_ms":null,"lat_no_source_probe_overhead_p50_ms":null,"lat_no_source_probe_overhead_p95_ms":null,"lat_no_source_probe_overhead_p99_ms":null,"lat_cold_client_s_file":null,"lat_cold_took_ms_file":null,"lat_cold_probe_overhead_ms_file":null,"lat_cold_client_p50_ms":null,"lat_cold_client_p95_ms":null,"lat_cold_client_p99_ms":null,"lat_cold_took_p50_ms":null,"lat_cold_took_p95_ms":null,"lat_cold_took_p99_ms":null,"lat_cold_probe_overhead_p50_ms":null,"lat_cold_probe_overhead_p95_ms":null,"lat_cold_probe_overhead_p99_ms":null}\n' \
    "$engine" "$MEM_LIMIT" "$CPUSET" "$PROBE_CPUSET" "$count" "$NDOCS" "$indexed" "$item_errors" "$reason" > "$OUT_DIR/$engine.json"
}

run_engine(){
  local ENGINE="$1" CID PORT HEAP HALF
  prepare_feeder_tmp "$ENGINE" || return 1
  if [ "$P2_MEASURE" = "1" ]; then
    [ "$ENGINE" = "surch" ] || { err "P2 ne mesure que Surch (ENGINE=$ENGINE refusé)"; return 1; }
    P2_PHASE_STATUS="$OUT_DIR/$ENGINE.p2.phase-status.jsonl"
    P2_STATS_JSONL="$OUT_DIR/$ENGINE.p2.stats.jsonl"
    : > "$P2_PHASE_STATUS"; : > "$P2_STATS_JSONL"
  fi
  CID="fairab-$ENGINE"
  docker rm -f "$CID" >/dev/null 2>&1 || true
  # heap ES = M/2 (Lucene mmap = l'autre moitié = page-cache, analogue au disque Surch)
  local MB; MB=$(mem_to_mib "$MEM_LIMIT"); HALF=$(( MB / 2 ))
  local VOL="fairab-vol-$ENGINE"
  docker volume rm "$VOL" >/dev/null 2>&1 || true; docker volume create "$VOL" >/dev/null 2>&1
  local BASE="http://$CID:9200"

  log "=== $ENGINE : cpuset=$CPUSET mem=$MEM_LIMIT (heap ES ${HALF}m) ==="
  if [ "$ENGINE" = "es" ]; then
    docker run -d --name "$CID" --network "$NET" \
      --cpuset-cpus="$CPUSET" --memory="$MEM_LIMIT" --memory-swap="$MEM_LIMIT" \
      -v "$VOL:/usr/share/elasticsearch/data" \
      -e discovery.type=single-node -e xpack.security.enabled=false \
      -e "ES_JAVA_OPTS=-Xms${HALF}m -Xmx${HALF}m" -e bootstrap.memory_lock=false \
      "$ES_IMAGE" >/dev/null 2>&1
  else
    # surch écrit ses segments sous TMPDIR (=/tmp) : on y monte un volume pour mesurer le disque
    docker run -d --name "$CID" --network "$NET" \
      --cpuset-cpus="$CPUSET" --memory="$MEM_LIMIT" --memory-swap="$MEM_LIMIT" \
      -v "$VOL:/tmp" \
      -e SURCH_PORT=9200 -e SURCH_ELASTIC_PRODUCT_COMPAT=1 \
      -e SURCH_POSTINGS_DISK="$POSTINGS_DISK" \
      ${SURCH_FLUSH_BUDGET_BYTES:+-e SURCH_FLUSH_BUDGET_BYTES="$SURCH_FLUSH_BUDGET_BYTES"} \
      ${SURCH_MERGE_FANIN:+-e SURCH_MERGE_FANIN="$SURCH_MERGE_FANIN"} \
      ${SURCH_MERGE_MAX_DOCS:+-e SURCH_MERGE_MAX_DOCS="$SURCH_MERGE_MAX_DOCS"} \
      ${SURCH_DENSIFY_BUDGET_DOCS:+-e SURCH_DENSIFY_BUDGET_DOCS="$SURCH_DENSIFY_BUDGET_DOCS"} \
      ${SURCH_SOURCE_COMPRESS:+-e SURCH_SOURCE_COMPRESS="$SURCH_SOURCE_COMPRESS"} \
      ${SURCH_SOURCE_FETCH_PARALLEL:+-e SURCH_SOURCE_FETCH_PARALLEL="$SURCH_SOURCE_FETCH_PARALLEL"} \
      -e SURCH_SOURCE_FETCH_PROFILE="$SURCH_SOURCE_FETCH_PROFILE" \
      -e SURCH_SOURCE_FETCH_PROFILE_FILE="$SURCH_SOURCE_FETCH_PROFILE_FILE" \
      "$SURCH_IMAGE" >/dev/null 2>&1
  fi

  # attendre healthy (ou détecter OOM précoce)
  local up=0 i
  for i in $(seq 1 60); do
    if docker run --rm $AUXCAP --network "$NET" curlimages/curl:8.10.1 \
      --fail-with-body --silent --show-error "$BASE/" >/dev/null 2>&1; then up=1; break; fi
    [ "$(docker inspect -f '{{.State.OOMKilled}}' "$CID" 2>/dev/null)" = "true" ] && break
    sleep 2
  done
  if [ "$up" != 1 ]; then
    err "$ENGINE : pas UP (OOMKilled=$(docker inspect -f '{{.State.OOMKilled}}' "$CID" 2>/dev/null))"
    echo "{\"engine\":\"$ENGINE\",\"mem_limit\":\"$MEM_LIMIT\",\"survived_boot\":false}" > "$OUT_DIR/$ENGINE.json"
    docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1
  fi

  # créer l'index : mapping fourni (MAPPING_FILE) ou mapping minimal texte français en dur
  if [ -n "$MAPPING_FILE" ]; then
    if ! docker run --rm $AUXCAP --network "$NET" -v "$MAPPING_FILE:/mapping.json:ro" curlimages/curl:8.10.1 \
      --fail-with-body --silent --show-error -XPUT "$BASE/deces_bench" -H 'Content-Type: application/json' --data-binary @/mapping.json >/dev/null; then
      err "$ENGINE : création index avec mapping impossible"
      docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1
    fi
  else
    if ! docker run --rm $AUXCAP --network "$NET" curlimages/curl:8.10.1 --fail-with-body --silent --show-error -XPUT "$BASE/deces_bench" \
      -H 'Content-Type: application/json' -d '{"mappings":{"properties":{"nom":{"type":"text"},"prenoms":{"type":"text"},"lieu_naissance":{"type":"text"},"sexe":{"type":"keyword"},"date_naissance":{"type":"keyword"},"date_deces":{"type":"keyword"}}}}' >/dev/null; then
      err "$ENGINE : création index avec mapping minimal impossible"
      docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1
    fi
  fi

  # indexation chronométrée : bulk SÉRIE chunké (10k docs/req = 20k lignes NDJSON)
  # un _bulk unique de ~100 Mo étoufferait les moteurs ; on découpe.
  # ROBUSTE : par chunk on vérifie http_code==200 ET réponse non-vide ET on parse les items ;
  # RETRY (jusqu'à $BULK_RETRIES) un chunk transient (code!=200 / réponse vide / tronquée) ;
  # ÉCHEC DUR consigné si un chunk reste KO après retries (au lieu d'un count silencieusement court).
  local t0 t1 oom bulk_rc
  local BOUT="$OUT_DIR/$ENGINE.bulklog"
  t0=$(date +%s.%N)
  docker run --rm $AUXCAP --network "$NET" --user "$(id -u):$(id -g)" \
    -v "$BULK:/bulk.ndjson:ro" -v "$FEEDER_TMP_ACTIVE_DIR:/tmp:rw" curlimages/curl:8.10.1 sh -c "
    BASE='$BASE'; REFRESH_EACH='$REFRESH_EACH'; MAXTRY='$BULK_RETRIES'
    cleanup(){ rm -f -- /tmp/chunk_* /tmp/resp.json; }
    trap cleanup 0
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM
    split -l 20000 -a 4 /bulk.ndjson /tmp/chunk_   # -a 4 : >676 chunks au 28M (57,8M lignes = 2892 chunks)
    indexed=0; item_err=0; hard=0; failed=''; n=0; dead=0
    for c in /tmp/chunk_*; do
      n=\$((n+1)); lines=\$(wc -l < \"\$c\"); docs=\$((lines/2)); ok=0; try=0
      while [ \$try -lt \$MAXTRY ]; do
        try=\$((try+1))
        code=\$(curl -s -o /tmp/resp.json -w '%{http_code}' --max-time 300 \
                 -XPOST \"\$BASE/deces_bench/_bulk\" \
                 -H 'Content-Type: application/x-ndjson' --data-binary @\"\$c\")
        sz=\$(wc -c < /tmp/resp.json 2>/dev/null); sz=\${sz:-0}
        if [ \"\$code\" = '200' ] && [ \"\$sz\" -gt 40 ]; then
          if grep -q '\"errors\":true' /tmp/resp.json; then
            # erreurs PAR ITEM (doc source invalide) : déterministe -> pas de retry, on compte les rejets
            ferr=\$(grep -o '\"status\":[45][0-9][0-9]' /tmp/resp.json | wc -l)
            item_err=\$((item_err+ferr)); indexed=\$((indexed+docs-ferr))
            echo \"[chunk \$n] 200 errors:true item_errors=\$ferr (docs source rejetes, cf ES)\" >&2
          else
            indexed=\$((indexed+docs))
          fi
          ok=1; break
        fi
        echo \"[chunk \$n] tentative \$try/\$MAXTRY KO code=\$code size=\$sz -> retry\" >&2
        sleep 2
      done
      if [ \$ok -ne 1 ]; then
        hard=1; failed=\"\$failed \$n(code\$code)\"
        echo \"[chunk \$n] ECHEC DUR apres \$MAXTRY tentatives code=\$code\" >&2
        echo \"  resp head: \$(head -c 300 /tmp/resp.json 2>/dev/null)\" >&2
        # moteur MORT (code 000 = connexion refusee/reset, ex. OOM-kill) 3 chunks d'affilee :
        # inutile de moudre les milliers de chunks restants (2892 au 28M) -> arret anticipe.
        case \$code in 000) dead=\$((dead+1));; *) dead=0;; esac
        if [ \$dead -ge 3 ]; then echo '[abort] moteur injoignable (3 chunks code=000 consecutifs) — arret anticipe' >&2; break; fi
      else
        dead=0
      fi
      [ \"\$REFRESH_EACH\" = '1' ] && curl -s -XPOST \"\$BASE/deces_bench/_refresh\" >/dev/null
    done
    echo \"BULKRESULT indexed=\$indexed item_errors=\$item_err hard_fail=\$hard failed_chunks=[\$failed ]\"
    [ \$hard -eq 0 ]
  " > "$BOUT" 2>&1
  bulk_rc=$?
  cleanup_feeder_tmp "$FEEDER_TMP_ACTIVE_DIR" || bulk_rc=1
  FEEDER_TMP_ACTIVE_DIR=""
  docker run --rm $AUXCAP --network "$NET" curlimages/curl:8.10.1 -s -XPOST "$BASE/deces_bench/_refresh" >/dev/null 2>&1
  t1=$(date +%s.%N)   # throughput = jusqu'au 1er refresh (loyal, hors matérialisation tardive)
  # 2e refresh + attente : surch ne matérialise pas le dernier lot sur un seul refresh final ;
  # ES insensible. Hors timing pour ne léser personne.
  sleep 2; docker run --rm $AUXCAP --network "$NET" curlimages/curl:8.10.1 -s -XPOST "$BASE/deces_bench/_refresh" >/dev/null 2>&1; sleep 1
  oom=$(docker inspect -f '{{.State.OOMKilled}}' "$CID" 2>/dev/null)
  local running; running=$(docker inspect -f '{{.State.Running}}' "$CID" 2>/dev/null)
  local cnt; cnt=$(docker run --rm $AUXCAP --network "$NET" curlimages/curl:8.10.1 -s "$BASE/deces_bench/_count" 2>/dev/null | grep -o '"count":[0-9]*' | cut -d: -f2)
  cnt=${cnt:-0}
  # résumé machine du bulk (indexé / erreurs item / échec dur / chunks KO)
  local summary indexed item_err hard_fail failed_chunks expected_indexed
  summary=$(grep '^BULKRESULT' "$BOUT" 2>/dev/null | tail -1)
  indexed=$(printf '%s' "$summary" | sed -n 's/.*indexed=\([0-9]*\).*/\1/p'); indexed=${indexed:-0}
  item_err=$(printf '%s' "$summary" | sed -n 's/.*item_errors=\([0-9]*\).*/\1/p'); item_err=${item_err:-0}
  hard_fail=$(printf '%s' "$summary" | sed -n 's/.*hard_fail=\([0-9]*\).*/\1/p'); hard_fail=${hard_fail:-1}
  failed_chunks=$(printf '%s' "$summary" | sed -n 's/.*failed_chunks=\(\[[^]]*\]\).*/\1/p'); failed_chunks=${failed_chunks:-'[?]'}
  expected_indexed=$(( NDOCS - item_err ))   # ce qu'on DOIT retrouver (docs source invalides exclus, perdus des 2 côtés)

  # ---- décision, dans l'ordre : (1) mémoire, (2) échec bulk dur / perte silencieuse ----
  # (1) OOM ou conteneur mort = échec MÉMOIRE.
  if [ "$oom" = "true" ] || [ "$running" != "true" ]; then
    err "$ENGINE : OOM/mort sous $MEM_LIMIT (count=$cnt/$NDOCS OOM=$oom running=$running)"
    echo "{\"engine\":\"$ENGINE\",\"mem_limit\":\"$MEM_LIMIT\",\"survived_boot\":true,\"survived_index\":false,\"oom\":\"$oom\",\"count\":$cnt,\"expected\":$NDOCS,\"indexed\":$indexed,\"item_errors\":$item_err}" > "$OUT_DIR/$ENGINE.json"
    docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1
  fi
  # (2) échec bulk : chunk KO après retries (hard_fail), OU count < ce qu'on a réellement indexé
  #     (perte silencieuse). On ÉCHOUE BRUYAMMENT avec le détail — jamais un faux succès.
  if [ "$bulk_rc" -ne 0 ] || [ "$hard_fail" != "0" ] || [ "$cnt" -lt "$expected_indexed" ]; then
    err "$ENGINE : ÉCHEC INDEXATION — count=$cnt attendu=$NDOCS (indexé=$indexed, rejets item=$item_err, attendu_indexé=$expected_indexed)"
    err "  chunks KO=$failed_chunks — extrait bulklog :"
    grep -E '^\[chunk|ECHEC|resp head|BULKRESULT' "$BOUT" 2>/dev/null | tail -12 | while IFS= read -r l; do err "    $l"; done
    echo "{\"engine\":\"$ENGINE\",\"mem_limit\":\"$MEM_LIMIT\",\"survived_boot\":true,\"survived_index\":false,\"bulk_failed\":true,\"count\":$cnt,\"expected\":$NDOCS,\"indexed\":$indexed,\"item_errors\":$item_err,\"failed_chunks\":\"$failed_chunks\"}" > "$OUT_DIR/$ENGINE.json"
    docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1
  fi
  # succès : count >= attendu_indexé. Si des docs source ont été rejetés (item_err>0), c'est LÉGITIME
  # (JSON source invalide, perdu aussi par ES) et explicitement signalé — pas un count silencieusement court.
  [ "$item_err" -gt 0 ] && log "$ENGINE : $item_err doc(s) source rejeté(s) (JSON invalide) — count $cnt = $NDOCS-$item_err attendu, OK"
  if [ "$P2_MEASURE" = "1" ] && { [ "$cnt" -ne "$P2_EXPECTED_DOCS" ] || [ "$item_err" -ne 0 ] || [ "$indexed" -ne "$P2_EXPECTED_DOCS" ]; }; then
    err "P2 : count exact requis ($P2_EXPECTED_DOCS) ; reçu count=$cnt indexed=$indexed item_errors=$item_err"
    record_invalid_measurement "$ENGINE" "$cnt" "$indexed" "$item_err" "p2_exact_count_required"
    docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1
  fi

  local dps rss disk
  dps=$(awk -v c="$NDOCS" -v a="$t0" -v b="$t1" 'BEGIN{printf "%.0f", c/(b-a)}')
  rss=$(sample_rss "$CID")
  # disque : du du volume de données côté hôte (via alpine, l'image moteur n'a pas de shell)
  disk=$(docker run --rm $AUXCAP -v "fairab-vol-$ENGINE:/d" alpine:3 du -sm /d 2>/dev/null | awk '{print $1}')
  disk=${disk:-?}

  # ---- 2a. ventilation disque (surch uniquement) + vérif réclamation post-merge ----
  # Familles de fichiers connues (cf crates/surch-index/postings.rs, document_index.rs,
  # surch-api/state.rs::source_store) + comparaison au nb de segments VIVANTS déclaré par la
  # gauge surch_index_segment_count (/_prometheus_metrics) : si files_postings_count >
  # segment_count, un Arc<Segment> est retenu par un registre après merge/Drop -> fichier(s)
  # orphelin(s) invisibles à un simple comptage de segments logiques. null côté ES (pas de
  # gauge segment_count ; le disque agrégé est déjà couvert par disk_mib ci-dessus).
  local disk_bytes_postings="null" disk_bytes_subfields="null" disk_bytes_source="null" \
        disk_bytes_fst_merge="null" disk_bytes_other="null" files_postings_count="null" segment_count="null"
  if [ "$ENGINE" = "surch" ]; then
    local bp=0 bsub=0 bsrc=0 bfst=0 both=0 fpc=0 vent_listing
    vent_listing=$(docker run --rm $AUXCAP -v "fairab-vol-$ENGINE:/d" alpine:3 sh -c '
      cd /d 2>/dev/null || exit 0
      for f in *; do
        [ -f "$f" ] || continue
        # %b*512 = octets réellement OCCUPÉS (pas la taille apparente : les stores sont
        # préalloués/creux -> stat %s surestime, cf gate 1,36M : 1375 MiB apparents vs 1040 du)
        blk=$(stat -c "%b" "$f" 2>/dev/null); blk=${blk:-0}
        echo "$f $(( blk * 512 ))"
      done' 2>/dev/null)
    while IFS=' ' read -r fname fsize; do
      [ -z "$fname" ] && continue
      case "$fname" in
        surch-postings-*) bp=$((bp+fsize)); fpc=$((fpc+1)) ;;
        surch-subfields-*) bsub=$((bsub+fsize)) ;;
        surch-source-*) bsrc=$((bsrc+fsize)) ;;
        surch-fst-merge-*) bfst=$((bfst+fsize)) ;;
        *) both=$((both+fsize)) ;;
      esac
    done <<< "$vent_listing"
    disk_bytes_postings=$bp; disk_bytes_subfields=$bsub; disk_bytes_source=$bsrc
    disk_bytes_fst_merge=$bfst; disk_bytes_other=$both; files_postings_count=$fpc
    segment_count=$(docker run --rm $AUXCAP --network "$NET" curlimages/curl:8.10.1 --fail-with-body --silent --show-error "$BASE/_prometheus_metrics" 2>/dev/null | awk '/^surch_index_segment_count\{/{print $NF; exit}')
    [ -z "$segment_count" ] && segment_count="null"
  fi
  if [ "$P2_MEASURE" = "1" ]; then
    local p2_index_snapshot p2_index_segments
    p2_index_snapshot="$OUT_DIR/$ENGINE.p2.prom.index_ready.prom"
    if ! p2_snapshot_metrics "$BASE" "$p2_index_snapshot" index_ready; then
      err "$ENGINE : P2 métriques initiales invalides : $P2_METRICS_REASON"
      record_invalid_measurement "$ENGINE" "$cnt" "$indexed" "$item_err" "${P2_METRICS_REASON:-p2_index_prometheus_invalid}"
      docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1
    fi
    p2_index_segments=$(p2_metric_value surch_index_segment_count "$p2_index_snapshot") || p2_index_segments=""
    if ! p2_segment_value_valid "${p2_index_segments:-invalid}"; then
      err "$ENGINE : P2 segments invalides : $p2_index_segments (gate $P2_SEGMENT_GATE/$P2_REQUIRED_SEGMENTS)"
      record_invalid_measurement "$ENGINE" "$cnt" "$indexed" "$item_err" "p2_segment_count_invalid_${p2_index_segments:-missing}"
      docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1
    fi
    segment_count="$p2_index_segments"
  fi

  # Sondes L2 : les séries client et moteur restent séparées pour bootstrap/
  # IC95. La sonde fixe utilise le champ configuré, donc `NOM` sur le mapping
  # 28M et non l'ancien `nom` zéro-hit.
  local fixed_raw="$OUT_DIR/$ENGINE.lat_fixed_client_s"
  local fixed_took_raw="$OUT_DIR/$ENGINE.lat_fixed_took_ms"
  local fixed_overhead_raw="$OUT_DIR/$ENGINE.lat_fixed_probe_overhead_ms"
  local random_raw="$OUT_DIR/$ENGINE.lat_rand_client_s"
  local random_took_raw="$OUT_DIR/$ENGINE.lat_rand_took_ms"
  local random_overhead_raw="$OUT_DIR/$ENGINE.lat_rand_probe_overhead_ms"
  local no_source_raw="$OUT_DIR/$ENGINE.lat_no_source_client_s"
  local no_source_took_raw="$OUT_DIR/$ENGINE.lat_no_source_took_ms"
  local no_source_overhead_raw="$OUT_DIR/$ENGINE.lat_no_source_probe_overhead_ms"
  local source_fetch_profile_enabled=false source_fetch_profile_valid=true source_fetch_profile_reason=""
  local source_fetch_before_fixed="null" source_fetch_after_fixed="null" source_fetch_after_random="null"
  local source_fetch_after_no_source="null" source_fetch_after_cold="null"
  local source_fetch_fixed_delta="null" source_fetch_random_delta="null" source_fetch_no_source_delta="null"
  local source_fetch_cold_delta="null" source_fetch_random_hydrated_8plus_requests="null"
  local source_fetch_random_hydrated_8plus_gate="null" source_fetch_random_worker_participations="null"
  local source_fetch_profile_snapshot="null" source_fetch_fixed_jsonl="null" source_fetch_random_jsonl="null"
  local source_fetch_no_source_jsonl="null" source_fetch_cold_jsonl="null" source_fetch_profile_previous=0
  if [ "$ENGINE" = "surch" ] && [ "$SURCH_SOURCE_FETCH_PROFILE" = "1" ]; then
    source_fetch_profile_enabled=true
    source_fetch_before_fixed="$OUT_DIR/$ENGINE.source_fetch.before_fixed.prom"
    source_fetch_after_fixed="$OUT_DIR/$ENGINE.source_fetch.after_fixed.prom"
    source_fetch_after_random="$OUT_DIR/$ENGINE.source_fetch.after_random.prom"
    source_fetch_after_no_source="$OUT_DIR/$ENGINE.source_fetch.after_no_source.prom"
    source_fetch_after_cold="$OUT_DIR/$ENGINE.source_fetch.after_cold.prom"
    source_fetch_fixed_delta="$OUT_DIR/$ENGINE.source_fetch.fixed.delta.prom"
    source_fetch_random_delta="$OUT_DIR/$ENGINE.source_fetch.random.delta.prom"
    source_fetch_no_source_delta="$OUT_DIR/$ENGINE.source_fetch.no_source.delta.prom"
    source_fetch_cold_delta="$OUT_DIR/$ENGINE.source_fetch.cold.delta.prom"
    source_fetch_profile_snapshot="$OUT_DIR/$ENGINE.source_fetch.profile.snapshot.jsonl"
    source_fetch_fixed_jsonl="$OUT_DIR/$ENGINE.source_fetch.fixed.jsonl"
    source_fetch_random_jsonl="$OUT_DIR/$ENGINE.source_fetch.random.jsonl"
    source_fetch_no_source_jsonl="$OUT_DIR/$ENGINE.source_fetch.no_source.jsonl"
    source_fetch_cold_jsonl="$OUT_DIR/$ENGINE.source_fetch.cold.jsonl"
    if ! snapshot_source_fetch_metrics "$BASE" "$source_fetch_before_fixed" before_fixed 0; then
      source_fetch_profile_valid=false
      source_fetch_profile_reason="$SOURCE_FETCH_SCRAPE_REASON"
    fi
  fi
  if [ "$P2_MEASURE" = "1" ]; then
    local warm_raw="$OUT_DIR/$ENGINE.p2.warm_client_s" warm_took_raw="$OUT_DIR/$ENGINE.p2.warm_took_ms" warm_overhead_raw="$OUT_DIR/$ENGINE.p2.warm_probe_overhead_ms"
    if ! p2_capture_phase "$BASE" warm "$P2_WARM_BODIES" "$warm_raw" "$warm_took_raw" "$warm_overhead_raw" "$(( 2 * P2_WARM_PAIR_COUNT ))" "$P2_WARM_PAIR_COUNT" "$P2_WARM_PAIR_COUNT"; then
      err "$ENGINE : chauffe P2 invalide : $PROBE_CAPTURE_REASON"
      record_invalid_measurement "$ENGINE" "$cnt" "$indexed" "$item_err" "$PROBE_CAPTURE_REASON"
      docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1
    fi
  fi
  if [ "$P2_MEASURE" = "1" ]; then
    p2_capture_phase "$BASE" fixed "$PROBE_FIXED_BODIES" "$fixed_raw" "$fixed_took_raw" "$fixed_overhead_raw" "$PROBE_REQUESTS" 0 "$PROBE_REQUESTS"
  else
    capture_probe_samples "$BASE" "$PROBE_FIXED_BODIES" "$fixed_raw" "$fixed_took_raw" "$fixed_overhead_raw" "$PROBE_REQUESTS" fixed
  fi
  if [ "$?" -ne 0 ]; then
    err "$ENGINE : série fixe invalide : $PROBE_CAPTURE_REASON"
    record_invalid_measurement "$ENGINE" "$cnt" "$indexed" "$item_err" "$PROBE_CAPTURE_REASON"
    docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1
  fi
  local lat50 lat95 lat99 lat_fixed_took50 lat_fixed_took95 lat_fixed_took99 lat_fixed_probe50 lat_fixed_probe95 lat_fixed_probe99
  read -r lat50 lat95 lat99 < <(probe_quantiles "$fixed_raw" 1000)
  read -r lat_fixed_took50 lat_fixed_took95 lat_fixed_took99 < <(probe_quantiles "$fixed_took_raw" 1)
  read -r lat_fixed_probe50 lat_fixed_probe95 lat_fixed_probe99 < <(probe_quantiles "$fixed_overhead_raw" 1)
  if [ "$P2_MEASURE" = "1" ]; then
    p2_split_and_stat_phase fixed "$fixed_raw" "$fixed_took_raw" "$fixed_overhead_raw" 0 "$PROBE_REQUESTS" || {
      err "$ENGINE : statistiques P2 fixed invalides"; record_invalid_measurement "$ENGINE" "$cnt" "$indexed" "$item_err" "p2_fixed_stats_invalid"
      docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1;
    }
  fi
  if [ "$source_fetch_profile_enabled" = true ]; then
    if ! snapshot_source_fetch_metrics "$BASE" "$source_fetch_after_fixed" after_fixed 1; then
      source_fetch_profile_valid=false
      source_fetch_profile_reason="${source_fetch_profile_reason:-$SOURCE_FETCH_SCRAPE_REASON}"
    fi
    source_fetch_phase_delta "$source_fetch_before_fixed" "$source_fetch_after_fixed" "$source_fetch_fixed_delta"
    if ! source_fetch_export_phase "$CID" "$SURCH_SOURCE_FETCH_PROFILE_FILE" "$source_fetch_profile_snapshot" "$source_fetch_fixed_jsonl" fixed "$source_fetch_profile_previous" \
       || ! source_fetch_artifact_matches_delta "$source_fetch_fixed_delta" "$source_fetch_fixed_jsonl"; then
      source_fetch_profile_valid=false
      source_fetch_profile_reason="${SOURCE_FETCH_ARTIFACT_REASON:-fixed_profile_jsonl_counter_mismatch}"
    else
      source_fetch_profile_previous=$((source_fetch_profile_previous + SOURCE_FETCH_ARTIFACT_RECORDS))
    fi
  fi
  if [ "$P2_MEASURE" = "1" ]; then
    p2_capture_phase "$BASE" random "$PROBE_BODIES" "$random_raw" "$random_took_raw" "$random_overhead_raw" "$PROBE_REQUESTS" "$P2_PAIR_COUNT" "$P2_PAIR_COUNT"
  else
    capture_probe_samples "$BASE" "$PROBE_BODIES" "$random_raw" "$random_took_raw" "$random_overhead_raw" "$PROBE_REQUESTS" random
  fi
  if [ "$?" -ne 0 ]; then
    err "$ENGINE : série random invalide : $PROBE_CAPTURE_REASON"
    record_invalid_measurement "$ENGINE" "$cnt" "$indexed" "$item_err" "$PROBE_CAPTURE_REASON"
    docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1
  fi
  local latr50 latr95 latr99 lat_rand_took50 lat_rand_took95 lat_rand_took99 lat_rand_probe50 lat_rand_probe95 lat_rand_probe99
  read -r latr50 latr95 latr99 < <(probe_quantiles "$random_raw" 1000)
  read -r lat_rand_took50 lat_rand_took95 lat_rand_took99 < <(probe_quantiles "$random_took_raw" 1)
  read -r lat_rand_probe50 lat_rand_probe95 lat_rand_probe99 < <(probe_quantiles "$random_overhead_raw" 1)
  if [ "$P2_MEASURE" = "1" ]; then
    p2_split_and_stat_phase random "$random_raw" "$random_took_raw" "$random_overhead_raw" "$P2_PAIR_COUNT" "$P2_PAIR_COUNT" || {
      err "$ENGINE : statistiques P2 random invalides"; record_invalid_measurement "$ENGINE" "$cnt" "$indexed" "$item_err" "p2_random_stats_invalid"
      docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1;
    }
  fi
  if [ "$source_fetch_profile_enabled" = true ]; then
    if ! snapshot_source_fetch_metrics "$BASE" "$source_fetch_after_random" after_random 1; then
      source_fetch_profile_valid=false
      source_fetch_profile_reason="${source_fetch_profile_reason:-$SOURCE_FETCH_SCRAPE_REASON}"
    fi
    source_fetch_phase_delta "$source_fetch_after_fixed" "$source_fetch_after_random" "$source_fetch_random_delta"
    if ! source_fetch_export_phase "$CID" "$SURCH_SOURCE_FETCH_PROFILE_FILE" "$source_fetch_profile_snapshot" "$source_fetch_random_jsonl" random "$source_fetch_profile_previous" \
       || ! source_fetch_artifact_matches_delta "$source_fetch_random_delta" "$source_fetch_random_jsonl"; then
      source_fetch_profile_valid=false
      source_fetch_profile_reason="${SOURCE_FETCH_ARTIFACT_REASON:-random_profile_jsonl_counter_mismatch}"
    else
      source_fetch_profile_previous=$((source_fetch_profile_previous + SOURCE_FETCH_ARTIFACT_RECORDS))
    fi
    source_fetch_random_hydrated_8plus_requests=$(source_fetch_random_hydrated_eight_plus "$source_fetch_random_jsonl")
    source_fetch_random_worker_participations=$(source_fetch_counter_total "$source_fetch_random_delta" surch_source_fetch_workers_total)
    if [ "$source_fetch_random_hydrated_8plus_requests" -gt 0 ]; then
      source_fetch_random_hydrated_8plus_gate="pass"
    else
      source_fetch_random_hydrated_8plus_gate="fail"
      source_fetch_profile_valid=false
      source_fetch_profile_reason="${source_fetch_profile_reason:-random_hydrated_8plus_absent}"
    fi
  fi
  if [ "$P2_MEASURE" = "1" ]; then
    p2_capture_phase "$BASE" no_source "$PROBE_CONTROL_BODIES" "$no_source_raw" "$no_source_took_raw" "$no_source_overhead_raw" "$PROBE_REQUESTS" "$P2_PAIR_COUNT" "$P2_PAIR_COUNT"
  else
    capture_probe_samples "$BASE" "$PROBE_CONTROL_BODIES" "$no_source_raw" "$no_source_took_raw" "$no_source_overhead_raw" "$PROBE_REQUESTS" no_source
  fi
  if [ "$?" -ne 0 ]; then
    err "$ENGINE : série témoin size:0 invalide : $PROBE_CAPTURE_REASON"
    record_invalid_measurement "$ENGINE" "$cnt" "$indexed" "$item_err" "$PROBE_CAPTURE_REASON"
    docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1
  fi
  local latn50 latn95 latn99 lat_no_source_took50 lat_no_source_took95 lat_no_source_took99 lat_no_source_probe50 lat_no_source_probe95 lat_no_source_probe99
  read -r latn50 latn95 latn99 < <(probe_quantiles "$no_source_raw" 1000)
  read -r lat_no_source_took50 lat_no_source_took95 lat_no_source_took99 < <(probe_quantiles "$no_source_took_raw" 1)
  read -r lat_no_source_probe50 lat_no_source_probe95 lat_no_source_probe99 < <(probe_quantiles "$no_source_overhead_raw" 1)
  if [ "$P2_MEASURE" = "1" ]; then
    p2_split_and_stat_phase no_source "$no_source_raw" "$no_source_took_raw" "$no_source_overhead_raw" "$P2_PAIR_COUNT" "$P2_PAIR_COUNT" || {
      err "$ENGINE : statistiques P2 size:0 invalides"; record_invalid_measurement "$ENGINE" "$cnt" "$indexed" "$item_err" "p2_no_source_stats_invalid"
      docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return 1;
    }
  fi
  if [ "$source_fetch_profile_enabled" = true ]; then
    if ! snapshot_source_fetch_metrics "$BASE" "$source_fetch_after_no_source" after_no_source 1; then
      source_fetch_profile_valid=false
      source_fetch_profile_reason="${source_fetch_profile_reason:-$SOURCE_FETCH_SCRAPE_REASON}"
    fi
    source_fetch_phase_delta "$source_fetch_after_random" "$source_fetch_after_no_source" "$source_fetch_no_source_delta"
    if ! source_fetch_export_phase "$CID" "$SURCH_SOURCE_FETCH_PROFILE_FILE" "$source_fetch_profile_snapshot" "$source_fetch_no_source_jsonl" no_source "$source_fetch_profile_previous" \
       || ! source_fetch_artifact_matches_delta "$source_fetch_no_source_delta" "$source_fetch_no_source_jsonl"; then
      source_fetch_profile_valid=false
      source_fetch_profile_reason="${SOURCE_FETCH_ARTIFACT_REASON:-no_source_profile_jsonl_counter_mismatch}"
    else
      source_fetch_profile_previous=$((source_fetch_profile_previous + SOURCE_FETCH_ARTIFACT_RECORDS))
    fi
    if ! source_fetch_no_source_delta_is_zero "$source_fetch_no_source_delta"; then
      source_fetch_profile_valid=false
      source_fetch_profile_reason="${source_fetch_profile_reason:-size_zero_hydration_delta_nonzero}"
    fi
  fi

  # ---- 1c. sonde COLD : 50 requêtes, reclaim vérifié AVANT chacune ----
  # Une unique éviction avant 1 000 requêtes se réchauffait et ne mesurait pas
  # un accès froid. Le protocole L2 garde au plus COLD_PROBE_REQUESTS corps
  # random, et invalide toute série où un reclaim n'est pas vérifié.
  local latc50="null" latc95="null" latc99="null" lat_cold_took50="null" lat_cold_took95="null" lat_cold_took99="null" lat_cold_probe50="null" lat_cold_probe95="null" lat_cold_probe99="null" cold_attempted=false cold_ok=false cold_skip_reason="" cold_method=""
  local cold_raw="$OUT_DIR/$ENGINE.lat_cold_client_s" cold_took_raw="$OUT_DIR/$ENGINE.lat_cold_took_ms" cold_overhead_raw="$OUT_DIR/$ENGINE.lat_cold_probe_overhead_ms" cold_reclaimed_requests=0
  local cold_reclaim_audit="$OUT_DIR/$ENGINE.cold_reclaim.tsv" cold_reclaim_audit_records=0
  local p2_cold_before="" p2_cold_after="" p2_cold_responses_raw="" p2_cold_responses_canonical="" p2_cold_phase_valid=true p2_cold_metrics_ready=true
  local p2_cold_cpu_before="" p2_cold_cpu_after="" p2_cold_cpu_steal_percent="null" p2_cold_cpu_steal_within_limit=false p2_cold_cpu_steal_reason=""
  : > "$cold_reclaim_audit"
  if [ "$P2_MEASURE" = "1" ]; then
    p2_cold_before="$OUT_DIR/$ENGINE.p2.prom.cold.before.prom"
    p2_cold_after="$OUT_DIR/$ENGINE.p2.prom.cold.after.prom"
    p2_cold_responses_raw="$OUT_DIR/$ENGINE.p2.responses.cold.raw.ndjson"
    p2_cold_responses_canonical="$OUT_DIR/$ENGINE.p2.responses.cold.canonical.ndjson"
  fi
  local mem_anon_warm="null" mem_file_warm="null" mem_anon_cold="null" mem_file_cold="null"
  local full_id cg_scope reclaim_path stat_path current_path reclaim_writer=""
  full_id=$(docker inspect -f '{{.Id}}' "$CID" 2>/dev/null)
  cg_scope="/sys/fs/cgroup/system.slice/docker-${full_id}.scope"
  reclaim_path="$cg_scope/memory.reclaim"
  stat_path="$cg_scope/memory.stat"
  current_path="$cg_scope/memory.current"
  if [ -r "$stat_path" ]; then
    local v
    v=$(awk '/^anon /{print $2}' "$stat_path" 2>/dev/null); [ -n "$v" ] && mem_anon_warm="$v"
    v=$(awk '/^file /{print $2}' "$stat_path" 2>/dev/null); [ -n "$v" ] && mem_file_warm="$v"
  fi
  if [ "$COLD_PROBE" != "1" ]; then
    cold_skip_reason="cold_probe_disabled"
  elif [ ! -r "$stat_path" ] || [ ! -r "$current_path" ]; then
    cold_skip_reason="cgroup_memory_stat_unreadable"
  elif [ -w "$reclaim_path" ]; then
    reclaim_writer="direct"
  elif sudo -n test -w "$reclaim_path" >/dev/null 2>&1; then
    reclaim_writer="sudo"
  else
    cold_skip_reason="cgroup_memory_reclaim_unwritable"
  fi
  if [ -n "$reclaim_writer" ]; then
    cold_attempted=true
    if [ "$P2_MEASURE" = "1" ]; then
      p2_cold_cpu_before=$(p2_cpu_stat 2>/dev/null || true)
      if ! p2_snapshot_metrics "$BASE" "$p2_cold_before" cold_before; then
        # Un scrape cold indisponible décrit seulement ce diagnostic L2 ; il
        # n'altère ni les quatre séries chaudes ni leurs preuves A/B.
        p2_cold_metrics_ready=false
        cold_skip_reason="${P2_METRICS_REASON:-p2_cold_before_invalid}"
      fi
      capture_cold_samples "$BASE" "$PROBE_BODIES" "$cold_raw" "$cold_took_raw" "$cold_overhead_raw" "$cold_reclaim_audit" "$reclaim_path" "$stat_path" "$current_path" "$reclaim_writer" "$COLD_PROBE_REQUESTS" "$p2_cold_responses_raw"
    else
      capture_cold_samples "$BASE" "$PROBE_BODIES" "$cold_raw" "$cold_took_raw" "$cold_overhead_raw" "$cold_reclaim_audit" "$reclaim_path" "$stat_path" "$current_path" "$reclaim_writer" "$COLD_PROBE_REQUESTS"
    fi
    if [ "$?" -eq 0 ]; then
      cold_ok=true
      cold_method="memory_reclaim_file_target_verified_each_request"
      cold_reclaimed_requests="$COLD_CAPTURE_COMPLETED"
      read -r latc50 latc95 latc99 < <(probe_quantiles "$cold_raw" 1000)
      read -r lat_cold_took50 lat_cold_took95 lat_cold_took99 < <(probe_quantiles "$cold_took_raw" 1)
      read -r lat_cold_probe50 lat_cold_probe95 lat_cold_probe99 < <(probe_quantiles "$cold_overhead_raw" 1)
    else
      cold_skip_reason="${COLD_CAPTURE_REASON:-cold_capture_failed}"
      cold_reclaimed_requests="$COLD_CAPTURE_COMPLETED"
    fi
  fi
  [ -f "$cold_raw" ] || : > "$cold_raw"
  [ -f "$cold_took_raw" ] || : > "$cold_took_raw"
  [ -f "$cold_overhead_raw" ] || : > "$cold_overhead_raw"
  cold_reclaim_audit_records=$(wc -l < "$cold_reclaim_audit" 2>/dev/null); cold_reclaim_audit_records=${cold_reclaim_audit_records:-0}
  if [ "$cold_ok" = true ]; then
    # Une série n'est cold que si ses N reclamations ont toutes été vérifiées.
    if [ "$cold_reclaimed_requests" -ne "$COLD_PROBE_REQUESTS" ]; then
      cold_ok=false
      cold_skip_reason="reclaim_count_${cold_reclaimed_requests}_of_${COLD_PROBE_REQUESTS}"
    fi
    if [ "$cold_reclaim_audit_records" -ne 50 ]; then
      cold_ok=false
      cold_skip_reason="reclaim_audit_count_${cold_reclaim_audit_records}_of_50"
    fi
  fi
  if [ -r "$stat_path" ]; then
    local v2
    v2=$(awk '/^anon /{print $2}' "$stat_path" 2>/dev/null); [ -n "$v2" ] && mem_anon_cold="$v2"
    v2=$(awk '/^file /{print $2}' "$stat_path" 2>/dev/null); [ -n "$v2" ] && mem_file_cold="$v2"
  fi
  if [ "$P2_MEASURE" = "1" ]; then
    if [ "$cold_ok" != true ]; then
      p2_cold_phase_valid=false
      cold_skip_reason="${cold_skip_reason:-p2_cold_capture_failed}"
    elif [ "$p2_cold_metrics_ready" != true ]; then
      p2_cold_phase_valid=false
    elif ! jq -cS '{hits_total: .hits.total, max_score: .hits.max_score, hits: [.hits.hits[] | {_id: ._id, _score: ._score, _source: ._source}]}' \
        "$p2_cold_responses_raw" > "$p2_cold_responses_canonical" \
      || [ "$(wc -l < "$p2_cold_responses_canonical" 2>/dev/null || echo 0)" -ne 50 ]; then
      p2_cold_phase_valid=false
      cold_skip_reason="p2_cold_response_canonicalization_failed"
    elif ! p2_snapshot_metrics "$BASE" "$p2_cold_after" cold_after; then
      p2_cold_phase_valid=false
      cold_skip_reason="${P2_METRICS_REASON:-p2_cold_routing_invalid}"
    else
      p2_cold_cpu_after=$(p2_cpu_stat 2>/dev/null || true)
      if [ -n "$p2_cold_cpu_before" ] && [ -n "$p2_cold_cpu_after" ] \
         && p2_cpu_steal_percent "$p2_cold_cpu_before" "$p2_cold_cpu_after" >/dev/null 2>&1; then
        p2_cold_cpu_steal_percent=$(p2_cpu_steal_percent "$p2_cold_cpu_before" "$p2_cold_cpu_after")
        if p2_number_le "$p2_cold_cpu_steal_percent" "$P2_CPU_STEAL_MAX_PERCENT"; then
          p2_cold_cpu_steal_within_limit=true
        else
          p2_cold_cpu_steal_reason="cpu_steal_${p2_cold_cpu_steal_percent}_above_${P2_CPU_STEAL_MAX_PERCENT}"
        fi
      else
        p2_cold_cpu_steal_reason="cpu_steal_unreadable"
      fi
      if ! p2_write_phase_status cold 25 25 "$p2_cold_before" "$p2_cold_after" \
          "$p2_cold_cpu_steal_percent" "$p2_cold_cpu_steal_within_limit" "$p2_cold_cpu_steal_reason"; then
        p2_cold_phase_valid=false
        cold_skip_reason="${P2_PHASE_REASON:-p2_cold_routing_invalid}"
      fi
    fi
    if [ "$p2_cold_phase_valid" = true ] && ! p2_split_and_stat_phase cold "$cold_raw" "$cold_took_raw" "$cold_overhead_raw" 25 25; then
      p2_cold_phase_valid=false
      cold_skip_reason="p2_cold_stats_invalid"
    fi
  fi
  local cold_skip_json="null"; [ -n "$cold_skip_reason" ] && cold_skip_json="\"$cold_skip_reason\""
  local cold_method_json="null"; [ -n "$cold_method" ] && cold_method_json="\"$cold_method\""

  if [ "$source_fetch_profile_enabled" = true ] && [ "$cold_ok" = true ]; then
    if ! snapshot_source_fetch_metrics "$BASE" "$source_fetch_after_cold" after_cold 1; then
      source_fetch_profile_valid=false
      source_fetch_profile_reason="${source_fetch_profile_reason:-$SOURCE_FETCH_SCRAPE_REASON}"
    fi
    source_fetch_phase_delta "$source_fetch_after_no_source" "$source_fetch_after_cold" "$source_fetch_cold_delta"
    if ! source_fetch_export_phase "$CID" "$SURCH_SOURCE_FETCH_PROFILE_FILE" "$source_fetch_profile_snapshot" "$source_fetch_cold_jsonl" cold "$source_fetch_profile_previous" \
       || ! source_fetch_artifact_matches_delta "$source_fetch_cold_delta" "$source_fetch_cold_jsonl"; then
      source_fetch_profile_valid=false
      source_fetch_profile_reason="${SOURCE_FETCH_ARTIFACT_REASON:-cold_profile_jsonl_counter_mismatch}"
    else
      source_fetch_profile_previous=$((source_fetch_profile_previous + SOURCE_FETCH_ARTIFACT_RECORDS))
    fi
  fi

  local measurement_valid=true measurement_invalid_reason="null"
  if [ "$P2_MEASURE" != "1" ] \
     && { [ "$cold_reclaimed_requests" -ne 50 ] || [ "$cold_reclaim_audit_records" -ne 50 ] || [ "$cold_ok" != true ]; }; then
    measurement_valid=false
    measurement_invalid_reason="\"${cold_skip_reason:-cold_capture_failed}\""
  fi
  if [ "$source_fetch_profile_enabled" = true ] && [ "$source_fetch_profile_valid" != true ]; then
    measurement_valid=false
    measurement_invalid_reason="\"${source_fetch_profile_reason:-source_fetch_profile_invalid}\""
  fi
  local p2_json=""
  if [ "$P2_MEASURE" = "1" ]; then
    local p2_phase_records p2_hot_phase_records p2_image_id p2_image_digest p2_bulk_sha256 p2_mapping_sha256 p2_cpu_steal_percent
    p2_phase_records=$(wc -l < "$P2_PHASE_STATUS" 2>/dev/null); p2_phase_records=${p2_phase_records:-0}
    p2_hot_phase_records=$(jq -se '[.[] | select(.phase == "warm" or .phase == "fixed" or .phase == "random" or .phase == "no_source")] | length' "$P2_PHASE_STATUS" 2>/dev/null || printf '0')
    p2_cpu_steal_percent=$(jq -ser '[.[] | select(.phase == "warm" or .phase == "fixed" or .phase == "random" or .phase == "no_source") | .cpu_steal_percent | numbers] | if length == 4 then max else null end' "$P2_PHASE_STATUS" 2>/dev/null || printf 'null')
    # Les quatre phases chaudes portent la causalité P2. Cold reste un
    # diagnostic historique : ses droits de reclaim ne changent ni les corps,
    # ni le routage, ni les réponses canoniques des mesures chaudes.
    if ! jq -se '
      ([.[] | select(.phase == "warm" or .phase == "fixed" or .phase == "random" or .phase == "no_source")]) as $hot
      | ($hot | length == 4)
      and ([ $hot[] | .phase ] | sort == ["fixed", "no_source", "random", "warm"])
      and all($hot[]; .valid == true and .cpu_steal_within_limit == true)
    ' "$P2_PHASE_STATUS" 2>/dev/null | grep -qx true; then
      measurement_valid=false
      measurement_invalid_reason="\"p2_required_hot_phase_invalid_${p2_hot_phase_records}_of_4\""
    fi
    p2_image_id=$(docker image inspect -f '{{.Id}}' "$SURCH_IMAGE" 2>/dev/null || true)
    p2_image_digest=$(docker image inspect -f '{{index .RepoDigests 0}}' "$SURCH_IMAGE" 2>/dev/null || true)
    [ -n "$p2_image_id" ] || p2_image_id="null"
    [ -n "$p2_image_digest" ] || p2_image_digest="$p2_image_id"
    p2_bulk_sha256=$(p2_manifest_value bulk_sha256 "$PROBE_MANIFEST")
    p2_mapping_sha256=$(p2_manifest_value mapping_sha256 "$PROBE_MANIFEST")
    p2_json=",\"p2\":{\"protocol\":\"$P2_PROTOCOL_VERSION\",\"variant\":\"$P2_VARIANT\",\"input_manifest\":\"$PROBE_MANIFEST\",\"input_manifest_sha256\":\"$P2_INPUT_MANIFEST_SHA256\",\"bulk_sha256\":\"$p2_bulk_sha256\",\"mapping_sha256\":\"$p2_mapping_sha256\",\"phase_status_jsonl\":\"$P2_PHASE_STATUS\",\"stats_jsonl\":\"$P2_STATS_JSONL\",\"image\":\"$SURCH_IMAGE\",\"image_id\":\"$p2_image_id\",\"image_digest\":\"$p2_image_digest\",\"observed_cpu_configuration\":{\"nproc\":$HOST_CPU_COUNT,\"engine_cpuset\":\"$CPUSET\",\"probe_cpuset\":\"$PROBE_CPUSET\"},\"cpu_steal_percent\":$p2_cpu_steal_percent,\"cpu_steal_limit_percent\":$P2_CPU_STEAL_MAX_PERCENT,\"cpu_steal_scope\":\"max_required_hot_phase\",\"required_hot_phase_records\":4,\"hot_phase_records\":$p2_hot_phase_records,\"cold_phase_optional\":true,\"blocks_ratio_target\":0.25,\"segment_gate\":\"$P2_SEGMENT_GATE\",\"required_segments\":$P2_REQUIRED_SEGMENTS,\"expected_docs\":$P2_EXPECTED_DOCS,\"phase_records\":$p2_phase_records}"
  fi
  local source_fetch_profile_reason_json="null" source_fetch_random_gate_json="null"
  [ -n "$source_fetch_profile_reason" ] && source_fetch_profile_reason_json="\"$source_fetch_profile_reason\""
  [ "$source_fetch_random_hydrated_8plus_gate" != "null" ] && source_fetch_random_gate_json="\"$source_fetch_random_hydrated_8plus_gate\""
  local source_fetch_before_json="null" source_fetch_fixed_json="null" source_fetch_random_json="null"
  local source_fetch_no_source_json="null" source_fetch_cold_json="null"
  local source_fetch_fixed_delta_json="null" source_fetch_random_delta_json="null"
  local source_fetch_no_source_delta_json="null" source_fetch_cold_delta_json="null"
  local source_fetch_fixed_jsonl_json="null" source_fetch_random_jsonl_json="null"
  local source_fetch_no_source_jsonl_json="null" source_fetch_cold_jsonl_json="null"
  [ "$source_fetch_before_fixed" = "null" ] || source_fetch_before_json="\"$source_fetch_before_fixed\""
  [ "$source_fetch_after_fixed" = "null" ] || source_fetch_fixed_json="\"$source_fetch_after_fixed\""
  [ "$source_fetch_after_random" = "null" ] || source_fetch_random_json="\"$source_fetch_after_random\""
  [ "$source_fetch_after_no_source" = "null" ] || source_fetch_no_source_json="\"$source_fetch_after_no_source\""
  [ "$source_fetch_after_cold" = "null" ] || source_fetch_cold_json="\"$source_fetch_after_cold\""
  [ "$source_fetch_fixed_delta" = "null" ] || source_fetch_fixed_delta_json="\"$source_fetch_fixed_delta\""
  [ "$source_fetch_random_delta" = "null" ] || source_fetch_random_delta_json="\"$source_fetch_random_delta\""
  [ "$source_fetch_no_source_delta" = "null" ] || source_fetch_no_source_delta_json="\"$source_fetch_no_source_delta\""
  [ "$source_fetch_cold_delta" = "null" ] || source_fetch_cold_delta_json="\"$source_fetch_cold_delta\""
  [ "$source_fetch_fixed_jsonl" = "null" ] || source_fetch_fixed_jsonl_json="\"$source_fetch_fixed_jsonl\""
  [ "$source_fetch_random_jsonl" = "null" ] || source_fetch_random_jsonl_json="\"$source_fetch_random_jsonl\""
  [ "$source_fetch_no_source_jsonl" = "null" ] || source_fetch_no_source_jsonl_json="\"$source_fetch_no_source_jsonl\""
  [ "$source_fetch_cold_jsonl" = "null" ] || source_fetch_cold_jsonl_json="\"$source_fetch_cold_jsonl\""
  local source_fetch_prometheus_json="$source_fetch_no_source_json"
  [ "$cold_ok" = true ] && source_fetch_prometheus_json="$source_fetch_cold_json"
  local source_fetch_metrics_json="$source_fetch_prometheus_json,\"measurement_valid\":$measurement_valid,\"measurement_invalid_reason\":$measurement_invalid_reason,\"source_fetch_profile_valid\":$source_fetch_profile_valid,\"source_fetch_profile_invalid_reason\":$source_fetch_profile_reason_json,\"source_fetch_random_worker_participations\":$source_fetch_random_worker_participations,\"source_fetch_random_hydrated_8plus_requests\":$source_fetch_random_hydrated_8plus_requests,\"source_fetch_random_hydrated_8plus_gate\":$source_fetch_random_gate_json,\"source_fetch_jsonl\":{\"fixed\":$source_fetch_fixed_jsonl_json,\"random\":$source_fetch_random_jsonl_json,\"no_source\":$source_fetch_no_source_jsonl_json,\"cold\":$source_fetch_cold_jsonl_json},\"source_fetch_snapshots\":{\"before_fixed\":$source_fetch_before_json,\"after_fixed\":$source_fetch_fixed_json,\"after_random\":$source_fetch_random_json,\"after_no_source\":$source_fetch_no_source_json,\"after_cold\":$source_fetch_cold_json},\"source_fetch_deltas\":{\"fixed\":$source_fetch_fixed_delta_json,\"random\":$source_fetch_random_delta_json,\"no_source\":$source_fetch_no_source_delta_json,\"cold\":$source_fetch_cold_delta_json}"

  echo "{\"engine\":\"$ENGINE\",\"mem_limit\":\"$MEM_LIMIT\",\"cpuset\":\"$CPUSET\",\"probe_cpuset\":\"$PROBE_CPUSET\",\"probe_cpu_count\":$HOST_CPU_COUNT,\"probe_connection_reuse\":{\"fixed\":\"single_curl_next\",\"random\":\"single_curl_next\",\"no_source\":\"single_curl_next\",\"cold\":\"one_curl_per_reclaim\"},\"survived_boot\":true,\"survived_index\":true,\"count\":$cnt,\"expected\":$NDOCS,\"indexed\":$indexed,\"item_errors\":$item_err,\"index_doc_s\":$dps,\"rss_container\":\"$rss\",\"disk_mib\":\"$disk\",\"lat_p50_ms\":$lat50,\"lat_p95_ms\":$lat95,\"lat_p99_ms\":$lat99,\"lat_fixed_raw_s_file\":\"$fixed_raw\",\"lat_fixed_client_s_file\":\"$fixed_raw\",\"lat_fixed_took_ms_file\":\"$fixed_took_raw\",\"lat_fixed_probe_overhead_ms_file\":\"$fixed_overhead_raw\",\"lat_fixed_client_p50_ms\":$lat50,\"lat_fixed_client_p95_ms\":$lat95,\"lat_fixed_client_p99_ms\":$lat99,\"lat_fixed_took_p50_ms\":$lat_fixed_took50,\"lat_fixed_took_p95_ms\":$lat_fixed_took95,\"lat_fixed_took_p99_ms\":$lat_fixed_took99,\"lat_fixed_probe_overhead_p50_ms\":$lat_fixed_probe50,\"lat_fixed_probe_overhead_p95_ms\":$lat_fixed_probe95,\"lat_fixed_probe_overhead_p99_ms\":$lat_fixed_probe99,\"lat_rand_p50_ms\":$latr50,\"lat_rand_p95_ms\":$latr95,\"lat_rand_p99_ms\":$latr99,\"lat_rand_raw_s_file\":\"$random_raw\",\"lat_rand_client_s_file\":\"$random_raw\",\"lat_rand_took_ms_file\":\"$random_took_raw\",\"lat_rand_probe_overhead_ms_file\":\"$random_overhead_raw\",\"lat_rand_client_p50_ms\":$latr50,\"lat_rand_client_p95_ms\":$latr95,\"lat_rand_client_p99_ms\":$latr99,\"lat_rand_took_p50_ms\":$lat_rand_took50,\"lat_rand_took_p95_ms\":$lat_rand_took95,\"lat_rand_took_p99_ms\":$lat_rand_took99,\"lat_rand_probe_overhead_p50_ms\":$lat_rand_probe50,\"lat_rand_probe_overhead_p95_ms\":$lat_rand_probe95,\"lat_rand_probe_overhead_p99_ms\":$lat_rand_probe99,\"lat_no_source_p50_ms\":$latn50,\"lat_no_source_p95_ms\":$latn95,\"lat_no_source_p99_ms\":$latn99,\"lat_no_source_raw_s_file\":\"$no_source_raw\",\"lat_no_source_client_s_file\":\"$no_source_raw\",\"lat_no_source_took_ms_file\":\"$no_source_took_raw\",\"lat_no_source_probe_overhead_ms_file\":\"$no_source_overhead_raw\",\"lat_no_source_client_p50_ms\":$latn50,\"lat_no_source_client_p95_ms\":$latn95,\"lat_no_source_client_p99_ms\":$latn99,\"lat_no_source_took_p50_ms\":$lat_no_source_took50,\"lat_no_source_took_p95_ms\":$lat_no_source_took95,\"lat_no_source_took_p99_ms\":$lat_no_source_took99,\"lat_no_source_probe_overhead_p50_ms\":$lat_no_source_probe50,\"lat_no_source_probe_overhead_p95_ms\":$lat_no_source_probe95,\"lat_no_source_probe_overhead_p99_ms\":$lat_no_source_probe99,\"cold_probe_attempted\":$cold_attempted,\"cold_probe_ok\":$cold_ok,\"cold_probe_requests\":$COLD_PROBE_REQUESTS,\"cold_reclaimed_requests\":$cold_reclaimed_requests,\"cold_reclaim_audit_tsv\":\"$cold_reclaim_audit\",\"cold_reclaim_audit_records\":$cold_reclaim_audit_records,\"cold_skip_reason\":$cold_skip_json,\"cold_method\":$cold_method_json,\"lat_cold_p50_ms\":$latc50,\"lat_cold_p95_ms\":$latc95,\"lat_cold_p99_ms\":$latc99,\"lat_cold_raw_s_file\":\"$cold_raw\",\"lat_cold_client_s_file\":\"$cold_raw\",\"lat_cold_took_ms_file\":\"$cold_took_raw\",\"lat_cold_probe_overhead_ms_file\":\"$cold_overhead_raw\",\"lat_cold_client_p50_ms\":$latc50,\"lat_cold_client_p95_ms\":$latc95,\"lat_cold_client_p99_ms\":$latc99,\"lat_cold_took_p50_ms\":$lat_cold_took50,\"lat_cold_took_p95_ms\":$lat_cold_took95,\"lat_cold_took_p99_ms\":$lat_cold_took99,\"lat_cold_probe_overhead_p50_ms\":$lat_cold_probe50,\"lat_cold_probe_overhead_p95_ms\":$lat_cold_probe95,\"lat_cold_probe_overhead_p99_ms\":$lat_cold_probe99,\"mem_anon_bytes_warm\":$mem_anon_warm,\"mem_file_bytes_warm\":$mem_file_warm,\"mem_anon_bytes_cold\":$mem_anon_cold,\"mem_file_bytes_cold\":$mem_file_cold,\"disk_bytes_postings\":$disk_bytes_postings,\"disk_bytes_subfields\":$disk_bytes_subfields,\"disk_bytes_source\":$disk_bytes_source,\"disk_bytes_fst_merge\":$disk_bytes_fst_merge,\"disk_bytes_other\":$disk_bytes_other,\"files_postings_count\":$files_postings_count,\"segment_count\":$segment_count,\"source_fetch_profile_enabled\":$source_fetch_profile_enabled,\"source_fetch_prometheus_file\":$source_fetch_metrics_json$p2_json}" > "$OUT_DIR/$ENGINE.json"
  log "$ENGINE : mesure valide=$measurement_valid | ${dps} doc/s | RSS $rss | disk ${disk}MiB | fixe client ${lat50}/${lat95}/${lat99}, moteur ${lat_fixed_took50}/${lat_fixed_took95}/${lat_fixed_took99}, sonde ${lat_fixed_probe50}/${lat_fixed_probe95}/${lat_fixed_probe99} | rand client ${latr50}/${latr95}/${latr99}, moteur ${lat_rand_took50}/${lat_rand_took95}/${lat_rand_took99}, sonde ${lat_rand_probe50}/${lat_rand_probe95}/${lat_rand_probe99} | témoin client ${latn50}/${latn95}/${latn99}, moteur ${lat_no_source_took50}/${lat_no_source_took95}/${lat_no_source_took99}, sonde ${lat_no_source_probe50}/${lat_no_source_probe95}/${lat_no_source_probe99} | cold client ${latc50}/${latc95}/${latc99}, moteur ${lat_cold_took50}/${lat_cold_took95}/${lat_cold_took99}, sonde ${lat_cold_probe50}/${lat_cold_probe95}/${lat_cold_probe99} (reclaims=$cold_reclaimed_requests/50 audit=$cold_reclaim_audit_records/50 ok=$cold_ok skip=${cold_skip_reason:-none}) ms"
  [ "$HOLD_SECONDS" -gt 0 ] 2>/dev/null && { log "$ENGINE : HOLD_SECONDS=$HOLD_SECONDS avant teardown (brancher artillery-replay.sh sur $NET / $CID)"; sleep "$HOLD_SECONDS"; }
  docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1
  [ "$measurement_valid" = true ]
}

ENGINES="${ENGINES:-es surch}"   # ex: ENGINES=surch pour rejouer un seul moteur
run_failed=0
for _e in $ENGINES; do run_engine "$_e" || run_failed=1; done

log "=== SCORECARD ($MEM_LIMIT, $NDOCS docs, cpuset $CPUSET) ==="
for e in es surch; do cat "$OUT_DIR/$e.json" 2>/dev/null; echo; done
docker network rm "$NET" >/dev/null 2>&1 || true
exit "$run_failed"
