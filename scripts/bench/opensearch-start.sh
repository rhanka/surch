#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/bench/opensearch-common.sh
. "$SCRIPT_DIR/opensearch-common.sh"

runtime="$(find_container_runtime)"
container_name="$(opensearch_container_name)"
image="$(opensearch_image)"
port="$(opensearch_port)"
heap="$(opensearch_heap)"

validate_container_name "$container_name"

if container_exists "$runtime" "$container_name"; then
  status="$(container_status "$runtime" "$container_name")"
  if [ "$status" = "running" ]; then
    bench_log "OpenSearch container already running: $container_name"
    bench_log "URL: $(opensearch_url)"
    exit 0
  fi

  bench_log "removing stopped dedicated container: $container_name"
  "$runtime" rm "$container_name" >/dev/null
fi

bench_log "starting OpenSearch for BAN demo"
bench_log "runtime: $runtime"
bench_log "container: $container_name"
bench_log "image: $image"
bench_log "url: $(opensearch_url)"
bench_log "heap: $heap"

"$runtime" run -d \
  --name "$container_name" \
  -p "$port:9200" \
  -e "cluster.name=surch-ban-local" \
  -e "node.name=surch-ban-node" \
  -e "discovery.type=single-node" \
  -e "DISABLE_SECURITY_PLUGIN=true" \
  -e "DISABLE_INSTALL_DEMO_CONFIG=true" \
  -e "OPENSEARCH_JAVA_OPTS=-Xms$heap -Xmx$heap" \
  "$image" >/dev/null

bench_log "container started; run $SCRIPT_DIR/opensearch-wait.sh before loading BAN data"
