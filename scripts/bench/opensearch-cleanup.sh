#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/bench/opensearch-common.sh
. "$SCRIPT_DIR/opensearch-common.sh"

require_command curl

url="$(opensearch_url)"
index_name="$(opensearch_ban_index)"

validate_index_name "$index_name"

bench_log "deleting dedicated BAN demo index if it exists: $index_name"

if ! status="$(
  curl \
    --silent \
    --show-error \
    --output /dev/null \
    --write-out '%{http_code}' \
    --request DELETE \
    "$url/$index_name"
)"; then
  bench_die "cleanup failed: could not reach OpenSearch at $url"
fi

case "$status" in
  200|404)
    bench_log "cleanup ok: DELETE /$index_name returned $status"
    ;;
  *)
    bench_die "cleanup failed: DELETE /$index_name returned $status"
    ;;
esac
