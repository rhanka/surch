#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/bench/opensearch-common.sh
. "$SCRIPT_DIR/opensearch-common.sh"

runtime="$(find_container_runtime)"
container_name="$(opensearch_container_name)"

validate_container_name "$container_name"

if ! container_exists "$runtime" "$container_name"; then
  bench_log "no dedicated OpenSearch container found: $container_name"
  exit 0
fi

bench_log "stopping dedicated OpenSearch container: $container_name"
"$runtime" stop "$container_name" >/dev/null

if [ "${OPENSEARCH_REMOVE_CONTAINER:-1}" = "1" ]; then
  bench_log "removing dedicated OpenSearch container: $container_name"
  "$runtime" rm "$container_name" >/dev/null
fi

bench_log "OpenSearch container stopped"
