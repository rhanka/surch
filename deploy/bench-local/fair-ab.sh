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
SURCH_IMAGE="${SURCH_IMAGE:-ghcr.io/rhanka/surch:sha-69668db407fe49631b44e6f2e5ea0afafd968caa}"
POSTINGS_DISK="${POSTINGS_DISK:-1}"    # Surch : 1 = read-path disque (C1b)
OUT_DIR="${OUT_DIR:-/tmp/fair-ab-$(printf '%s' "$MEM_LIMIT")}"
PROBE_REQUESTS="${PROBE_REQUESTS:-1000}"
REFRESH_EACH="${REFRESH_EACH:-0}"   # 1 = refresh après chaque chunk (counts corrects ; Surch perd sinon ~1 chunk sous bulk rapide)
NET="fair-ab-net"

mkdir -p "$OUT_DIR"
log(){ printf '\033[1;36m[fair-ab]\033[0m %s\n' "$*"; }
err(){ printf '\033[1;31m[fair-ab]\033[0m %s\n' "$*" >&2; }

# ---- garde-fous ----
[ "$(stat -fc %T /sys/fs/cgroup 2>/dev/null)" = "cgroup2fs" ] || { err "cgroup v2 requis"; exit 1; }
[ -f "$DATA_FILE" ] || { err "corpus introuvable : $DATA_FILE"; exit 1; }
gov="$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo '?')"
[ "$gov" = "performance" ] || err "AVERTISSEMENT gouverneur=$gov (biais fréquence ; 'sudo cpupower frequency-set -g performance' pour un run rigoureux)"

# ---- 1. corpus : INSEE largeur fixe -> NDJSON bulk (mêmes docs pour les 2 moteurs) ----
BULK="$OUT_DIR/bulk.ndjson"
if [ ! -s "$BULK" ] || [ "$(( $(wc -l < "$BULK") / 2 ))" -ne "$CORPUS_LINES" ]; then
  log "construction du corpus ($CORPUS_LINES docs) depuis $DATA_FILE"
  head -n "$CORPUS_LINES" "$DATA_FILE" | awk '{
    line=$0
    # INSEE deces largeur fixe : nom*prénoms/ (1-80), sexe(81), naissance AAAAMMJJ(82-89),
    # code lieu nais(90-94), libellé lieu nais(95-124), décès AAAAMMJJ(155-162)
    nomp=substr(line,1,80); sub(/ +$/,"",nomp)
    split(nomp, a, "[*/]"); nom=a[1]; prenoms=a[2]
    sexe=substr(line,81,1)
    dnais=substr(line,82,8)
    lieu=substr(line,95,30); sub(/ +$/,"",lieu)
    ddeces=substr(line,155,8)
    gsub(/"/,"",nom); gsub(/"/,"",prenoms); gsub(/"/,"",lieu)
    printf "{\"index\":{\"_id\":\"%d\"}}\n", NR
    printf "{\"nom\":\"%s\",\"prenoms\":\"%s\",\"sexe\":\"%s\",\"date_naissance\":\"%s\",\"lieu_naissance\":\"%s\",\"date_deces\":\"%s\"}\n", nom, prenoms, sexe, dnais, lieu, ddeces
  }' > "$BULK"
fi
NDOCS=$(( $(wc -l < "$BULK") / 2 ))
log "corpus prêt : $NDOCS docs ($(du -h "$BULK" | cut -f1))"

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

  # créer l'index (mapping minimal texte français)
  docker run --rm --network "$NET" curlimages/curl:8.10.1 -s -XPUT "$BASE/deces_bench" \
    -H 'Content-Type: application/json' -d '{"mappings":{"properties":{"nom":{"type":"text"},"prenoms":{"type":"text"},"lieu_naissance":{"type":"text"},"sexe":{"type":"keyword"},"date_naissance":{"type":"keyword"},"date_deces":{"type":"keyword"}}}}' >/dev/null 2>&1

  # indexation chronométrée : bulk SÉRIE chunké (10k docs/req = 20k lignes NDJSON)
  # un _bulk unique de ~100 Mo étoufferait les moteurs ; on découpe.
  local t0 t1 oom
  t0=$(date +%s.%N)
  # bulk chunké ; on capte les erreurs par chunk (errors:true) et on refresh entre chunks
  local berr; berr=$(docker run --rm --network "$NET" -v "$BULK:/bulk.ndjson:ro" curlimages/curl:8.10.1 sh -c "
    split -l 20000 /bulk.ndjson /tmp/chunk_
    e=0
    for c in /tmp/chunk_*; do
      r=\$(curl -s -XPOST '$BASE/deces_bench/_bulk' -H 'Content-Type: application/x-ndjson' --data-binary @\"\$c\")
      echo \"\$r\" | grep -q '\"errors\":true' && e=\$((e+1))
      [ '$REFRESH_EACH' = '1' ] && curl -s -XPOST '$BASE/deces_bench/_refresh' >/dev/null
    done
    echo \$e" 2>/dev/null | tail -1)
  docker run --rm --network "$NET" curlimages/curl:8.10.1 -s -XPOST "$BASE/deces_bench/_refresh" >/dev/null 2>&1
  t1=$(date +%s.%N)   # throughput = jusqu'au 1er refresh (loyal, hors matérialisation tardive)
  # 2e refresh + attente : surch ne matérialise pas le dernier lot sur un seul refresh final ;
  # ES insensible. Hors timing pour ne léser personne.
  sleep 2; docker run --rm --network "$NET" curlimages/curl:8.10.1 -s -XPOST "$BASE/deces_bench/_refresh" >/dev/null 2>&1; sleep 1
  oom=$(docker inspect -f '{{.State.OOMKilled}}' "$CID" 2>/dev/null)
  local running; running=$(docker inspect -f '{{.State.Running}}' "$CID" 2>/dev/null)
  local cnt; cnt=$(docker run --rm --network "$NET" curlimages/curl:8.10.1 -s "$BASE/deces_bench/_count" 2>/dev/null | grep -o '"count":[0-9]*' | cut -d: -f2)
  cnt=${cnt:-0}
  berr=${berr:-?}

  # ÉCHEC = OOM ou conteneur mort. Un count légèrement court SANS OOM = perte bulk (data), pas mémoire.
  if [ "$oom" = "true" ] || [ "$running" != "true" ]; then
    err "$ENGINE : OOM/mort sous $MEM_LIMIT (count=$cnt/$NDOCS OOM=$oom running=$running)"
    echo "{\"engine\":\"$ENGINE\",\"mem_limit\":\"$MEM_LIMIT\",\"survived_boot\":true,\"survived_index\":false,\"oom\":\"$oom\",\"count\":$cnt,\"expected\":$NDOCS,\"bulk_err_chunks\":\"$berr\"}" > "$OUT_DIR/$ENGINE.json"
    docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1; return
  fi
  [ "$cnt" -lt "$NDOCS" ] && err "$ENGINE : SURVÉCU mais count $cnt/$NDOCS (perte bulk, chunks_err=$berr) — pas un échec mémoire"

  local dps rss disk
  dps=$(awk -v c="$NDOCS" -v a="$t0" -v b="$t1" 'BEGIN{printf "%.0f", c/(b-a)}')
  rss=$(sample_rss "$CID")
  # disque : du du volume de données côté hôte (via alpine, l'image moteur n'a pas de shell)
  disk=$(docker run --rm -v "fairab-vol-$ENGINE:/d" alpine:3 du -sm /d 2>/dev/null | awk '{print $1}')
  disk=${disk:-?}

  # sonde latence : PROBE_REQUESTS requêtes match dans UN SEUL conteneur curl
  # (éviter de mesurer le démarrage conteneur au lieu de la requête)
  local lat50 lat95 lat99
  read -r lat50 lat95 lat99 < <(docker run --rm --network "$NET" curlimages/curl:8.10.1 sh -c "
    for i in \$(seq 1 $PROBE_REQUESTS); do
      curl -s -w '%{time_total}\n' -o /dev/null '$BASE/deces_bench/_search' \
        -H 'Content-Type: application/json' \
        -d '{\"query\":{\"match\":{\"nom\":\"MARTIN\"}},\"size\":10}'
    done" 2>/dev/null | sort -n | awk -v n="$PROBE_REQUESTS" '{a[NR]=$1} END{printf "%.2f %.2f %.2f", a[int(n*0.5)]*1000, a[int(n*0.95)]*1000, a[int(n*0.99)]*1000}')
  lat50=${lat50:-0}; lat95=${lat95:-0}; lat99=${lat99:-0}

  echo "{\"engine\":\"$ENGINE\",\"mem_limit\":\"$MEM_LIMIT\",\"cpuset\":\"$CPUSET\",\"survived_boot\":true,\"survived_index\":true,\"count\":$cnt,\"expected\":$NDOCS,\"bulk_err_chunks\":\"$berr\",\"index_doc_s\":$dps,\"rss_container\":\"$rss\",\"disk_mib\":\"$disk\",\"lat_p50_ms\":$lat50,\"lat_p95_ms\":$lat95,\"lat_p99_ms\":$lat99}" > "$OUT_DIR/$ENGINE.json"
  log "$ENGINE OK : ${dps} doc/s | RSS $rss | disk ${disk}MiB | p50/95/99 ${lat50}/${lat95}/${lat99} ms"
  docker rm -f "$CID" >/dev/null 2>&1; docker volume rm "fairab-vol-$ENGINE" >/dev/null 2>&1
}

run_engine es
run_engine surch

log "=== SCORECARD ($MEM_LIMIT, $NDOCS docs, cpuset $CPUSET) ==="
for e in es surch; do cat "$OUT_DIR/$e.json" 2>/dev/null; echo; done
docker network rm "$NET" >/dev/null 2>&1 || true
