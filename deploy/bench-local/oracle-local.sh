#!/usr/bin/env bash
# oracle-local.sh — réplique LOCALE du job k8s b1-oracle-gate (parité surch vs ES 8.6.1).
#
# Pourquoi : le gate de parité ne doit pas dépendre du cluster SCW (cher, pools en
# rescaling). L'image bench-sha-<sha> est AUTOPORTANTE : binaire /usr/local/bin/b1_oracle
# + slice INSEE 10k + mapping + replay sous /usr/local/share/deces/. On rejoue donc
# exactement la même invocation que deploy/k8s/jobs/b1-oracle-gate.yaml, en docker.
#
# Usage : SURCH_SHA=<sha long> ./oracle-local.sh   (défaut: HEAD du repo)
# Sortie : exit 0 = 0 divergence ; non-zéro = divergences (JSON affiché).
set -euo pipefail
SURCH_SHA="${SURCH_SHA:-$(git -C "$(dirname "$0")/../.." rev-parse HEAD)}"
NET="oracle-local"; OUT="${OUT:-/tmp/oracle-local}"
mkdir -p "$OUT"; chmod 777 "$OUT"   # l'image bench tourne nonroot (uid 65532) et doit écrire le rapport
docker network create "$NET" >/dev/null 2>&1 || true
cleanup(){ docker rm -f ol-surch ol-es ol-driver >/dev/null 2>&1 || true; docker network rm "$NET" >/dev/null 2>&1 || true; }
# KEEP=1 : conserver les conteneurs pour autopsie (debug)
[ "${KEEP:-0}" = "1" ] || trap cleanup EXIT

# Engines — mêmes env/limites que le manifest k8s (surch 512Mi / ES 1536Mi heap 1g)
docker run -d --name ol-surch --network "$NET" --memory=512m --memory-swap=512m \
  -e SURCH_HOST=0.0.0.0 -e SURCH_PORT=7700 -e RUST_LOG=warn \
  ${SURCH_FLUSH_BUDGET_BYTES:+-e SURCH_FLUSH_BUDGET_BYTES="$SURCH_FLUSH_BUDGET_BYTES"} \
  "ghcr.io/rhanka/surch:sha-${SURCH_SHA}" >/dev/null
docker run -d --name ol-es --network "$NET" --memory=2048m --memory-swap=2048m \
  -e discovery.type=single-node -e xpack.security.enabled=false \
  -e "ES_JAVA_OPTS=-Xms1g -Xmx1g" -e bootstrap.memory_lock=false \
  docker.elastic.co/elasticsearch/elasticsearch:8.6.1 >/dev/null

# Driver — invocation IDENTIQUE au manifest
docker run --name ol-driver --network "$NET" \
  -e SURCH_URL=http://ol-surch:7700 -e ES_URL=http://ol-es:9200 -e REPORTS_DIR=/reports \
  -v "$OUT:/reports" \
  "ghcr.io/rhanka/surch:bench-sha-${SURCH_SHA}" /bin/sh -c '
    set -eu
    echo "Waiting for both engines..."
    until wget -qO- "$SURCH_URL/" >/dev/null 2>&1; do sleep 0.5; done
    until wget -qO- "$ES_URL/_cluster/health?wait_for_status=yellow&timeout=60s" >/dev/null 2>&1; do sleep 1; done
    ORACLE_RC=0
    /usr/local/bin/b1_oracle \
      --url-surch "$SURCH_URL" \
      --url-es "$ES_URL" \
      --mapping /usr/local/share/deces/mapping.json \
      --slice /usr/local/share/deces/slice-10000.ndjson.gz \
      --replay /usr/local/share/deces/replays/deces_v1.json \
      --out "$REPORTS_DIR/b1-oracle.json" || ORACLE_RC=$?
    echo "BEGIN_SURCH_B1_ORACLE"; cat "$REPORTS_DIR/b1-oracle.json" 2>/dev/null || echo "{\"error\":\"missing\"}"; echo; echo "END_SURCH_B1_ORACLE"
    exit "$ORACLE_RC"'
RC=$?
echo "oracle-local exit=$RC (0 = parité, sinon divergences dans $OUT/b1-oracle.json)"
exit $RC
