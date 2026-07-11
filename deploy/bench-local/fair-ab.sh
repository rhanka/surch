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
mem_to_mib(){ echo "$1" | awk 'BEGIN{IGNORECASE=1}{v=$0; if(v ~ /g/){sub(/[gG].*/,"",v); print int(v*1024)} else {sub(/[mM].*/,"",v); print int(v)}}'; }

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
OUT_DIR="${OUT_DIR:-/tmp/fair-ab-$(printf '%s' "$MEM_LIMIT")}"
PROBE_REQUESTS="${PROBE_REQUESTS:-1000}"
REFRESH_EACH="${REFRESH_EACH:-0}"   # 1 = refresh après chaque chunk (counts corrects ; Surch perd sinon ~1 chunk sous bulk rapide)
# ---- sonde random / cold (front #1 "latence honnête", brainstorm-4-fronts-2026-07-09.md b1) ----
PROBE_NAMES_N="${PROBE_NAMES_N:-10000}"          # 1a : taille de l'échantillon probe_names.tsv
# PROBE_FIELD_NOM/PROBE_FIELD_PRENOMS : clés JSON à extraire des docs $BULK pour peupler la
# sonde random ET pour construire ses requêtes (mêmes clés = c'est le champ ES réellement mappé).
# Défaut = schéma du builder awk interne ("nom"/"prenoms"). Pour un BULK_FILE au schéma matchID
# réel (ex. deces-1.36M.ndjson / deces-28M.ndjson, mapping deces-mapping.json), passer
# PROBE_FIELD_NOM=NOM PROBE_FIELD_PRENOMS=PRENOMS — SINON la sonde random requêterait un champ
# absent de la mapping (0 hit garanti), exactement le défaut dont souffre déjà silencieusement la
# sonde FIXE historique ci-dessous sur ce type de corpus (elle reste "nom" à l'identique, par
# continuité historique — seule la sonde random doit être correcte pour être honnête).
PROBE_FIELD_NOM="${PROBE_FIELD_NOM:-nom}"
PROBE_FIELD_PRENOMS="${PROBE_FIELD_PRENOMS:-prenoms}"
COLD_PROBE="${COLD_PROBE:-1}"       # 1c : 1 = tenter la sonde cold (memory.reclaim cgroup v2), 0 = off
HOLD_SECONDS="${HOLD_SECONDS:-0}"   # après mesures, tient le conteneur N s avant teardown (permet
                                     # à artillery-replay.sh de se brancher sur le réseau fair-ab, 1d)
# BULK_FILE   : NDJSON pré-construit (lignes alternées action/doc) indexé TEL QUEL (bypass builder awk interne).
# MAPPING_FILE: JSON de mapping ES appliqué à la création de l'index deces_bench (au lieu du mapping minimal en dur).
# Non fournis => comportement inchangé (builder awk + mapping minimal).
BULK_FILE="${BULK_FILE:-}"
MAPPING_FILE="${MAPPING_FILE:-}"
BULK_RETRIES="${BULK_RETRIES:-3}"   # nb de tentatives par chunk avant échec dur
NET="fair-ab-net"

mkdir -p "$OUT_DIR"
log(){ printf '\033[1;36m[fair-ab]\033[0m %s\n' "$*"; }
err(){ printf '\033[1;31m[fair-ab]\033[0m %s\n' "$*" >&2; }

# ---- garde-fous ----
[ "$(stat -fc %T /sys/fs/cgroup 2>/dev/null)" = "cgroup2fs" ] || { err "cgroup v2 requis"; exit 1; }
gov="$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo '?')"
[ "$gov" = "performance" ] || err "AVERTISSEMENT gouverneur=$gov (biais fréquence ; 'sudo cpupower frequency-set -g performance' pour un run rigoureux)"
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

# ---- 1a. probe_names.tsv : échantillon déterministe tiré du corpus $BULK ----
# JAMAIS shuf sans graine, JAMAIS $RANDOM : échantillonnage À PAS FIXE (1 doc toutes les
# NDOCS/PROBE_NAMES_N lignes-doc). Comme on échantillonne le corpus BRUT (pas une liste de noms
# uniques), c'est un tirage PONDÉRÉ PAR LA FRÉQUENCE -> distribution Zipf naturelle (MARTIN sort
# proportionnellement à sa fréquence dans le corpus) : exactement le trafic matchID réel, sans
# inventer de distribution. Généré UNE FOIS avant la boucle ENGINES -> fichier partagé, identique
# pour ES et surch.
PROBE_NAMES="$OUT_DIR/probe_names.tsv"
want_probe_n=$NDOCS; [ "$want_probe_n" -gt "$PROBE_NAMES_N" ] && want_probe_n=$PROBE_NAMES_N
if [ ! -s "$PROBE_NAMES" ] || [ "$(wc -l < "$PROBE_NAMES")" -ne "$want_probe_n" ]; then
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
  ' "$BULK" > "$PROBE_NAMES"
fi
PROBE_NAMES_COUNT=$(wc -l < "$PROBE_NAMES" 2>/dev/null); PROBE_NAMES_COUNT=${PROBE_NAMES_COUNT:-0}
log "probe_names.tsv prêt : $PROBE_NAMES_COUNT paires nom/prenoms"

# ---- 1b (préparation) : requêtes random pré-générées, IDENTIQUES pour les 2 moteurs ----
# Séquence d'indices déterministe dans probe_names.tsv (LCG à graine fixe -> reproductible,
# jamais $RANDOM/shuf) ; mix 50/50 match/bool décidé par la parité de l'index (même convention
# que scripts/bench/artillery-replay.sh). size:10 OBLIGATOIRE (force le fetch _source, le poste
# page-cache le plus gros — cf brainstorm-4-fronts-2026-07-09.md P2). Un seul fichier de bodies
# généré ICI (pas par moteur) -> même fichier monté dans les 2 conteneurs = mêmes requêtes, même
# ordre, par construction (pas seulement "en principe").
PROBE_IDX="$OUT_DIR/probe_idx.txt"
PROBE_BODIES="$OUT_DIR/probe_rand_bodies.ndjson"
if [ "$PROBE_NAMES_COUNT" -gt 0 ] && { [ ! -s "$PROBE_BODIES" ] || [ "$(wc -l < "$PROBE_BODIES")" -ne "$PROBE_REQUESTS" ]; }; then
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
        if (idx % 2 == 0) {
          printf "{\"query\":{\"match\":{\"%s\":\"%s\"}},\"size\":10}\n", fnom, f[1]
        } else {
          printf "{\"query\":{\"bool\":{\"must\":[{\"match\":{\"%s\":\"%s\"}},{\"match\":{\"%s\":\"%s\"}}]}},\"size\":10}\n", fnom, f[1], fpre, f[2]
        }
      }
      close(idxout)
      exit
    }
  ' > "$PROBE_BODIES"
fi
log "sonde random prête : $(wc -l < "$PROBE_BODIES" 2>/dev/null || echo 0) requêtes pré-générées"

docker network create "$NET" >/dev/null 2>&1 || true

# ---- helper : mesure RSS conteneur (memory.current, page-cache inclus) ----
sample_rss(){ docker stats --no-stream --format '{{.MemUsage}}' "$1" 2>/dev/null | awk '{print $1}'; }

run_engine(){
  local ENGINE="$1" CID PORT HEAP HALF
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
      "$SURCH_IMAGE" >/dev/null 2>&1
  fi

  # attendre healthy (ou détecter OOM précoce)
  local up=0 i
  for i in $(seq 1 60); do
    if docker run --rm --network "$NET" curlimages/curl:8.10.1 -s "$BASE/" >/dev/null 2>&1; then up=1; break; fi
    [ "$(docker inspect -f '{{.State.OOMKilled}}' "$CID" 2>/dev/null)" = "true" ] && break
    sleep 2
  done
  if [ "$up" != 1 ]; then
    err "$ENGINE : pas UP (OOMKilled=$(docker inspect -f '{{.State.OOMKilled}}' "$CID" 2>/dev/null))"
    echo "{\"engine\":\"$ENGINE\",\"mem_limit\":\"$MEM_LIMIT\",\"survived_boot\":false}" > "$OUT_DIR/$ENGINE.json"
    docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return
  fi

  # créer l'index : mapping fourni (MAPPING_FILE) ou mapping minimal texte français en dur
  if [ -n "$MAPPING_FILE" ]; then
    docker run --rm --network "$NET" -v "$MAPPING_FILE:/mapping.json:ro" curlimages/curl:8.10.1 \
      -s -XPUT "$BASE/deces_bench" -H 'Content-Type: application/json' --data-binary @/mapping.json >/dev/null 2>&1
  else
    docker run --rm --network "$NET" curlimages/curl:8.10.1 -s -XPUT "$BASE/deces_bench" \
      -H 'Content-Type: application/json' -d '{"mappings":{"properties":{"nom":{"type":"text"},"prenoms":{"type":"text"},"lieu_naissance":{"type":"text"},"sexe":{"type":"keyword"},"date_naissance":{"type":"keyword"},"date_deces":{"type":"keyword"}}}}' >/dev/null 2>&1
  fi

  # indexation chronométrée : bulk SÉRIE chunké (10k docs/req = 20k lignes NDJSON)
  # un _bulk unique de ~100 Mo étoufferait les moteurs ; on découpe.
  # ROBUSTE : par chunk on vérifie http_code==200 ET réponse non-vide ET on parse les items ;
  # RETRY (jusqu'à $BULK_RETRIES) un chunk transient (code!=200 / réponse vide / tronquée) ;
  # ÉCHEC DUR consigné si un chunk reste KO après retries (au lieu d'un count silencieusement court).
  local t0 t1 oom bulk_rc
  local BOUT="$OUT_DIR/$ENGINE.bulklog"
  t0=$(date +%s.%N)
  docker run --rm --network "$NET" -v "$BULK:/bulk.ndjson:ro" curlimages/curl:8.10.1 sh -c "
    BASE='$BASE'; REFRESH_EACH='$REFRESH_EACH'; MAXTRY='$BULK_RETRIES'
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
  docker run --rm --network "$NET" curlimages/curl:8.10.1 -s -XPOST "$BASE/deces_bench/_refresh" >/dev/null 2>&1
  t1=$(date +%s.%N)   # throughput = jusqu'au 1er refresh (loyal, hors matérialisation tardive)
  # 2e refresh + attente : surch ne matérialise pas le dernier lot sur un seul refresh final ;
  # ES insensible. Hors timing pour ne léser personne.
  sleep 2; docker run --rm --network "$NET" curlimages/curl:8.10.1 -s -XPOST "$BASE/deces_bench/_refresh" >/dev/null 2>&1; sleep 1
  oom=$(docker inspect -f '{{.State.OOMKilled}}' "$CID" 2>/dev/null)
  local running; running=$(docker inspect -f '{{.State.Running}}' "$CID" 2>/dev/null)
  local cnt; cnt=$(docker run --rm --network "$NET" curlimages/curl:8.10.1 -s "$BASE/deces_bench/_count" 2>/dev/null | grep -o '"count":[0-9]*' | cut -d: -f2)
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
    docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return
  fi
  # (2) échec bulk : chunk KO après retries (hard_fail), OU count < ce qu'on a réellement indexé
  #     (perte silencieuse). On ÉCHOUE BRUYAMMENT avec le détail — jamais un faux succès.
  if [ "$bulk_rc" -ne 0 ] || [ "$hard_fail" != "0" ] || [ "$cnt" -lt "$expected_indexed" ]; then
    err "$ENGINE : ÉCHEC INDEXATION — count=$cnt attendu=$NDOCS (indexé=$indexed, rejets item=$item_err, attendu_indexé=$expected_indexed)"
    err "  chunks KO=$failed_chunks — extrait bulklog :"
    grep -E '^\[chunk|ECHEC|resp head|BULKRESULT' "$BOUT" 2>/dev/null | tail -12 | while IFS= read -r l; do err "    $l"; done
    echo "{\"engine\":\"$ENGINE\",\"mem_limit\":\"$MEM_LIMIT\",\"survived_boot\":true,\"survived_index\":false,\"bulk_failed\":true,\"count\":$cnt,\"expected\":$NDOCS,\"indexed\":$indexed,\"item_errors\":$item_err,\"failed_chunks\":\"$failed_chunks\"}" > "$OUT_DIR/$ENGINE.json"
    docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return
  fi
  # succès : count >= attendu_indexé. Si des docs source ont été rejetés (item_err>0), c'est LÉGITIME
  # (JSON source invalide, perdu aussi par ES) et explicitement signalé — pas un count silencieusement court.
  [ "$item_err" -gt 0 ] && log "$ENGINE : $item_err doc(s) source rejeté(s) (JSON invalide) — count $cnt = $NDOCS-$item_err attendu, OK"

  local dps rss disk
  dps=$(awk -v c="$NDOCS" -v a="$t0" -v b="$t1" 'BEGIN{printf "%.0f", c/(b-a)}')
  rss=$(sample_rss "$CID")
  # disque : du du volume de données côté hôte (via alpine, l'image moteur n'a pas de shell)
  disk=$(docker run --rm -v "fairab-vol-$ENGINE:/d" alpine:3 du -sm /d 2>/dev/null | awk '{print $1}')
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
    vent_listing=$(docker run --rm -v "fairab-vol-$ENGINE:/d" alpine:3 sh -c '
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
    segment_count=$(docker run --rm --network "$NET" curlimages/curl:8.10.1 -s "$BASE/_prometheus_metrics" 2>/dev/null | awk '/^surch_index_segment_count\{/{print $NF; exit}')
    [ -z "$segment_count" ] && segment_count="null"
  fi

  # sonde latence FIXE (continuité historique — CONSERVÉE À L'IDENTIQUE, cf brainstorm
  # b1/P2) : PROBE_REQUESTS requêtes match dans UN SEUL conteneur curl (éviter de mesurer le
  # démarrage conteneur au lieu de la requête).
  local lat50 lat95 lat99
  read -r lat50 lat95 lat99 < <(docker run --rm --network "$NET" curlimages/curl:8.10.1 sh -c "
    for i in \$(seq 1 $PROBE_REQUESTS); do
      curl -s -w '%{time_total}\n' -o /dev/null '$BASE/deces_bench/_search' \
        -H 'Content-Type: application/json' \
        -d '{\"query\":{\"match\":{\"nom\":\"MARTIN\"}},\"size\":10}'
    done" 2>/dev/null | sort -n | awk -v n="$PROBE_REQUESTS" '{a[NR]=$1} END{printf "%.2f %.2f %.2f", a[int(n*0.5)]*1000, a[int(n*0.95)]*1000, a[int(n*0.99)]*1000}')
  lat50=${lat50:-0}; lat95=${lat95:-0}; lat99=${lat99:-0}

  # ---- 1b. sonde RANDOM warm : PROBE_BODIES pré-généré, IDENTIQUE pour les 2 moteurs ----
  # Mix 50/50 match/bool, size:10 (fetch _source). Même mécanique que la sonde fixe (un seul
  # conteneur curl, boucle, time_total -> seul le démarrage du CONTENEUR docker est hors mesure,
  # comme pour la sonde fixe ; curl lui-même mesure sa propre requête via -w, pas le shell).
  local latr50=0 latr95=0 latr99=0
  if [ -s "$PROBE_BODIES" ]; then
    read -r latr50 latr95 latr99 < <(docker run --rm --network "$NET" -v "$PROBE_BODIES:/bodies.ndjson:ro" curlimages/curl:8.10.1 sh -c "
      while IFS= read -r body; do
        curl -s -w '%{time_total}\n' -o /dev/null '$BASE/deces_bench/_search' \
          -H 'Content-Type: application/json' -d \"\$body\"
      done < /bodies.ndjson" 2>/dev/null | sort -n | awk -v n="$PROBE_REQUESTS" '{a[NR]=$1} END{printf "%.2f %.2f %.2f", a[int(n*0.5)]*1000, a[int(n*0.95)]*1000, a[int(n*0.99)]*1000}')
    latr50=${latr50:-0}; latr95=${latr95:-0}; latr99=${latr99:-0}
  fi

  # ---- 1c. sonde COLD : éviction du page cache DU CONTENEUR SEUL (cgroup v2 memory.reclaim), ----
  # puis re-sonde random IDENTIQUE. Best-effort : sudo -n d'abord (écriture root requise),
  # sinon écriture directe (perms locales), sinon SKIP PROPRE documenté — ne casse jamais le run.
  # memory.stat (anon vs file) est world-readable (pas besoin de root) et capturé dans tous les
  # cas, avant et après la tentative, pour distinguer résident applicatif et cache (b1/P3).
  local latc50=0 latc95=0 latc99=0 cold_attempted=false cold_ok=false cold_skip_reason="" cold_method=""
  local mem_anon_warm="null" mem_file_warm="null" mem_anon_cold="null" mem_file_cold="null"
  local full_id cg_scope reclaim_path stat_path
  full_id=$(docker inspect -f '{{.Id}}' "$CID" 2>/dev/null)
  cg_scope="/sys/fs/cgroup/system.slice/docker-${full_id}.scope"
  reclaim_path="$cg_scope/memory.reclaim"
  stat_path="$cg_scope/memory.stat"
  if [ -r "$stat_path" ]; then
    local v
    v=$(awk '/^anon /{print $2}' "$stat_path" 2>/dev/null); [ -n "$v" ] && mem_anon_warm="$v"
    v=$(awk '/^file /{print $2}' "$stat_path" 2>/dev/null); [ -n "$v" ] && mem_file_warm="$v"
  fi
  if [ "$COLD_PROBE" != "1" ]; then
    cold_skip_reason="cold_probe_disabled"
  elif [ ! -s "$PROBE_BODIES" ]; then
    cold_skip_reason="probe_bodies_missing"
  elif [ ! -e "$reclaim_path" ]; then
    cold_skip_reason="cgroup_memory_reclaim_absent"
  else
    local mb bytes
    mb=$(mem_to_mib "$MEM_LIMIT"); bytes=$(( mb * 1024 * 1024 * 2 ))   # agressif : 2x le cap
    cold_attempted=true
    if sudo -n sh -c "echo $bytes > '$reclaim_path'" >/dev/null 2>&1; then
      cold_ok=true; cold_method="memory_reclaim"
    elif echo "$bytes" > "$reclaim_path" 2>/dev/null; then
      cold_ok=true; cold_method="memory_reclaim"
    else
      # FALLBACK sans root (gate 1,36M : memory.reclaim = Permission denied sans sudo) :
      # abaisser temporairement le cap à anon+128 MiB via docker update force le noyau à
      # évincer le page cache du conteneur (l'anon, non-évictable, tient dans la marge ;
      # le moteur est au repos pendant la fenêtre). Même mécanisme pour les 2 moteurs.
      # Impossible si anon+128 >= cap (ex. ES @1536m, heap ~= cap) -> skip documenté.
      local anon_now squeeze_mib cap_mib
      anon_now=$(awk '/^anon /{print $2}' "$stat_path" 2>/dev/null); anon_now=${anon_now:-0}
      cap_mib=$(mem_to_mib "$MEM_LIMIT")
      squeeze_mib=$(( anon_now / 1048576 + 128 ))
      if [ "$squeeze_mib" -lt "$cap_mib" ] && \
         docker update --memory="${squeeze_mib}m" --memory-swap="${squeeze_mib}m" "$CID" >/dev/null 2>&1; then
        sleep 3
        docker update --memory="$MEM_LIMIT" --memory-swap="$MEM_LIMIT" "$CID" >/dev/null 2>&1
        if [ "$(docker inspect -f '{{.State.OOMKilled}}' "$CID" 2>/dev/null)" = "true" ]; then
          cold_skip_reason="oom_during_squeeze"
        else
          cold_ok=true; cold_method="docker_update_squeeze"
        fi
      else
        cold_skip_reason="no_write_perm_and_squeeze_impossible_anon${squeeze_mib}m_cap${cap_mib}m"
      fi
    fi
  fi
  if [ "$cold_ok" = true ]; then
    sleep 1   # laisser le noyau appliquer le reclaim avant re-sonde
    read -r latc50 latc95 latc99 < <(docker run --rm --network "$NET" -v "$PROBE_BODIES:/bodies.ndjson:ro" curlimages/curl:8.10.1 sh -c "
      while IFS= read -r body; do
        curl -s -w '%{time_total}\n' -o /dev/null '$BASE/deces_bench/_search' \
          -H 'Content-Type: application/json' -d \"\$body\"
      done < /bodies.ndjson" 2>/dev/null | sort -n | awk -v n="$PROBE_REQUESTS" '{a[NR]=$1} END{printf "%.2f %.2f %.2f", a[int(n*0.5)]*1000, a[int(n*0.95)]*1000, a[int(n*0.99)]*1000}')
    latc50=${latc50:-0}; latc95=${latc95:-0}; latc99=${latc99:-0}
  fi
  if [ -r "$stat_path" ]; then
    local v2
    v2=$(awk '/^anon /{print $2}' "$stat_path" 2>/dev/null); [ -n "$v2" ] && mem_anon_cold="$v2"
    v2=$(awk '/^file /{print $2}' "$stat_path" 2>/dev/null); [ -n "$v2" ] && mem_file_cold="$v2"
  fi
  local cold_skip_json="null"; [ -n "$cold_skip_reason" ] && cold_skip_json="\"$cold_skip_reason\""
  local cold_method_json="null"; [ -n "$cold_method" ] && cold_method_json="\"$cold_method\""

  echo "{\"engine\":\"$ENGINE\",\"mem_limit\":\"$MEM_LIMIT\",\"cpuset\":\"$CPUSET\",\"survived_boot\":true,\"survived_index\":true,\"count\":$cnt,\"expected\":$NDOCS,\"indexed\":$indexed,\"item_errors\":$item_err,\"index_doc_s\":$dps,\"rss_container\":\"$rss\",\"disk_mib\":\"$disk\",\"lat_p50_ms\":$lat50,\"lat_p95_ms\":$lat95,\"lat_p99_ms\":$lat99,\"lat_rand_p50_ms\":$latr50,\"lat_rand_p95_ms\":$latr95,\"lat_rand_p99_ms\":$latr99,\"cold_probe_attempted\":$cold_attempted,\"cold_probe_ok\":$cold_ok,\"cold_skip_reason\":$cold_skip_json,\"cold_method\":$cold_method_json,\"lat_cold_p50_ms\":$latc50,\"lat_cold_p95_ms\":$latc95,\"lat_cold_p99_ms\":$latc99,\"mem_anon_bytes_warm\":$mem_anon_warm,\"mem_file_bytes_warm\":$mem_file_warm,\"mem_anon_bytes_cold\":$mem_anon_cold,\"mem_file_bytes_cold\":$mem_file_cold,\"disk_bytes_postings\":$disk_bytes_postings,\"disk_bytes_subfields\":$disk_bytes_subfields,\"disk_bytes_source\":$disk_bytes_source,\"disk_bytes_fst_merge\":$disk_bytes_fst_merge,\"disk_bytes_other\":$disk_bytes_other,\"files_postings_count\":$files_postings_count,\"segment_count\":$segment_count}" > "$OUT_DIR/$ENGINE.json"
  log "$ENGINE OK : ${dps} doc/s | RSS $rss | disk ${disk}MiB | fixe ${lat50}/${lat95}/${lat99} | rand ${latr50}/${latr95}/${latr99} | cold ${latc50}/${latc95}/${latc99} (attempted=$cold_attempted ok=$cold_ok skip=${cold_skip_reason:-none}) ms"
  [ "$HOLD_SECONDS" -gt 0 ] 2>/dev/null && { log "$ENGINE : HOLD_SECONDS=$HOLD_SECONDS avant teardown (brancher artillery-replay.sh sur $NET / $CID)"; sleep "$HOLD_SECONDS"; }
  docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1
}

ENGINES="${ENGINES:-es surch}"   # ex: ENGINES=surch pour rejouer un seul moteur
for _e in $ENGINES; do run_engine "$_e"; done

log "=== SCORECARD ($MEM_LIMIT, $NDOCS docs, cpuset $CPUSET) ==="
for e in es surch; do cat "$OUT_DIR/$e.json" 2>/dev/null; echo; done
docker network rm "$NET" >/dev/null 2>&1 || true
