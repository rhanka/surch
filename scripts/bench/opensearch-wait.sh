#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/bench/opensearch-common.sh
. "$SCRIPT_DIR/opensearch-common.sh"

require_command curl

url="$(opensearch_url)"
timeout_seconds="${OPENSEARCH_WAIT_TIMEOUT:-120}"
interval_seconds="${OPENSEARCH_WAIT_INTERVAL:-2}"

case "$timeout_seconds" in
  ''|*[!0-9]*)
    bench_die "OPENSEARCH_WAIT_TIMEOUT must be a positive integer"
    ;;
esac

case "$interval_seconds" in
  ''|*[!0-9]*)
    bench_die "OPENSEARCH_WAIT_INTERVAL must be a positive integer"
    ;;
esac

if [ "$timeout_seconds" -lt 1 ] || [ "$interval_seconds" -lt 1 ]; then
  bench_die "OPENSEARCH_WAIT_TIMEOUT and OPENSEARCH_WAIT_INTERVAL must be positive"
fi

deadline=$((SECONDS + timeout_seconds))
bench_log "waiting for OpenSearch at $url"

while [ "$SECONDS" -le "$deadline" ]; do
  if root_body="$(curl --silent --show-error --max-time 2 "$url/" 2>/dev/null)" \
    && printf '%s' "$root_body" | grep -q '"distribution"[[:space:]]*:[[:space:]]*"opensearch"'; then
    if health_body="$(curl --silent --show-error --max-time 2 "$url/_cluster/health?wait_for_status=yellow&timeout=1s" 2>/dev/null)" \
      && printf '%s' "$health_body" | grep -Eq '"status"[[:space:]]*:[[:space:]]*"(green|yellow)"'; then
      bench_log "OpenSearch health ok: $health_body"
      exit 0
    fi
  fi

  sleep "$interval_seconds"
done

bench_die "OpenSearch did not become healthy within ${timeout_seconds}s at $url"
