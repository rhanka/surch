#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# shellcheck source=scripts/bench/opensearch-common.sh
. "$SCRIPT_DIR/opensearch-common.sh"

readonly DEFAULT_SURCH_URL="http://127.0.0.1:7700"
readonly DEFAULT_BAN_TINY_DATASET="tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson"
readonly BAN_TINY_INDEX="ban_tiny"
readonly HTTP_STATUS_MARKER="__SURCH_BENCH_HTTP_STATUS__"

HTTP_BODY=""
HTTP_STATUS=""

validate_base_url() {
  local name="$1"
  local value="$2"

  case "$value" in
    '')
      bench_die "$name must not be empty"
      ;;
    http://*|https://*)
      ;;
    *)
      bench_die "$name must start with http:// or https://"
      ;;
  esac

  case "$value" in
    *\?*|*#*)
      bench_die "$name must not include query parameters or fragments"
      ;;
    *[[:space:]]*)
      bench_die "$name must not include whitespace"
      ;;
  esac
}

validate_http_timeout() {
  local value="$1"

  case "$value" in
    ''|*[!0-9]*)
      bench_die "BAN_HTTP_TIMEOUT must be a positive integer"
      ;;
  esac

  if [ "$value" -lt 1 ]; then
    bench_die "BAN_HTTP_TIMEOUT must be positive"
  fi
}

resolve_dataset_path() {
  local dataset="$1"

  if [ -f "$dataset" ]; then
    printf '%s\n' "$dataset"
    return 0
  fi

  case "$dataset" in
    /*)
      ;;
    *)
      if [ -f "$REPO_ROOT/$dataset" ]; then
        printf '%s\n' "$REPO_ROOT/$dataset"
        return 0
      fi
      ;;
  esac

  bench_die "BAN tiny dataset not found: $dataset"
}

http_request() {
  local engine="$1"
  local method="$2"
  local url="$3"
  local content_type="${4:-}"
  local payload="${5:-}"
  local response=""
  local -a args=(
    --silent
    --show-error
    --max-time "$http_timeout"
    --request "$method"
    --write-out "$HTTP_STATUS_MARKER%{http_code}"
  )

  if [ "$content_type" != "" ]; then
    args+=(--header "Content-Type: $content_type")
  fi

  if [ "$payload" != "" ]; then
    args+=(--data "$payload")
  fi

  args+=("$url")

  if ! response="$(curl "${args[@]}")"; then
    bench_die "$engine request failed: $method $url"
  fi

  HTTP_STATUS="${response##*$HTTP_STATUS_MARKER}"
  HTTP_BODY="${response%$HTTP_STATUS_MARKER$HTTP_STATUS}"

  if [ "$HTTP_STATUS" = "$response" ]; then
    bench_die "$engine request did not return an HTTP status: $method $url"
  fi
}

http_request_file() {
  local engine="$1"
  local method="$2"
  local url="$3"
  local content_type="$4"
  local file_path="$5"
  local response=""
  local -a args=(
    --silent
    --show-error
    --max-time "$http_timeout"
    --request "$method"
    --header "Content-Type: $content_type"
    --data-binary "@$file_path"
    --write-out "$HTTP_STATUS_MARKER%{http_code}"
    "$url"
  )

  if ! response="$(curl "${args[@]}")"; then
    bench_die "$engine request failed: $method $url"
  fi

  HTTP_STATUS="${response##*$HTTP_STATUS_MARKER}"
  HTTP_BODY="${response%$HTTP_STATUS_MARKER$HTTP_STATUS}"

  if [ "$HTTP_STATUS" = "$response" ]; then
    bench_die "$engine request did not return an HTTP status: $method $url"
  fi
}

expect_status() {
  local engine="$1"
  local operation="$2"
  shift 2
  local expected=""

  for expected in "$@"; do
    if [ "$HTTP_STATUS" = "$expected" ]; then
      return 0
    fi
  done

  bench_die "$engine $operation returned HTTP $HTTP_STATUS, expected $*: $HTTP_BODY"
}

expect_body_match() {
  local engine="$1"
  local operation="$2"
  local pattern="$3"

  if printf '%s' "$HTTP_BODY" | grep -Eq "$pattern"; then
    return 0
  fi

  bench_die "$engine $operation response did not match $pattern: $HTTP_BODY"
}

check_surch_health() {
  http_request "Surch" GET "$surch_url/" "" ""
  expect_status "Surch" "GET /" 200
  expect_body_match "Surch" "GET /" '"distribution"[[:space:]]*:[[:space:]]*"opensearch"'
  bench_log "Surch health ok: $surch_url"
}

check_opensearch_health() {
  http_request "OpenSearch" GET "$opensearch_url_value/" "" ""
  expect_status "OpenSearch" "GET /" 200
  expect_body_match "OpenSearch" "GET /" '"distribution"[[:space:]]*:[[:space:]]*"opensearch"'

  http_request "OpenSearch" GET "$opensearch_url_value/_cluster/health?wait_for_status=yellow&timeout=1s" "" ""
  expect_status "OpenSearch" "GET /_cluster/health" 200
  expect_body_match "OpenSearch" "GET /_cluster/health" '"status"[[:space:]]*:[[:space:]]*"(green|yellow)"'
  bench_log "OpenSearch health ok: $opensearch_url_value"
}

reset_index() {
  local engine="$1"
  local base_url="$2"

  http_request "$engine" DELETE "$base_url/$BAN_TINY_INDEX" "" ""
  expect_status "$engine" "DELETE /$BAN_TINY_INDEX" 200 404

  http_request "$engine" PUT "$base_url/$BAN_TINY_INDEX" "application/json" '{}'
  expect_status "$engine" "PUT /$BAN_TINY_INDEX" 200
  expect_body_match "$engine" "PUT /$BAN_TINY_INDEX" '"acknowledged"[[:space:]]*:[[:space:]]*true'
  bench_log "$engine index reset ok: $BAN_TINY_INDEX"
}

load_ban_tiny() {
  local engine="$1"
  local base_url="$2"

  http_request_file "$engine" POST "$base_url/_bulk" "application/x-ndjson" "$dataset_path"
  expect_status "$engine" "POST /_bulk" 200
  expect_body_match "$engine" "POST /_bulk" '"errors"[[:space:]]*:[[:space:]]*false'

  http_request "$engine" POST "$base_url/$BAN_TINY_INDEX/_refresh" "" ""
  expect_status "$engine" "POST /$BAN_TINY_INDEX/_refresh" 200
  expect_body_match "$engine" "POST /$BAN_TINY_INDEX/_refresh" '"failed"[[:space:]]*:[[:space:]]*0'
  bench_log "$engine load and refresh ok"
}

verify_count() {
  local engine="$1"
  local base_url="$2"

  http_request "$engine" GET "$base_url/$BAN_TINY_INDEX/_count" "" ""
  expect_status "$engine" "GET /$BAN_TINY_INDEX/_count" 200
  expect_body_match "$engine" "GET /$BAN_TINY_INDEX/_count" '"count"[[:space:]]*:[[:space:]]*3([,}[:space:]]|$)'
  bench_log "$engine count ok: 3"
}

require_command curl
require_command grep

surch_url="${SURCH_URL:-$DEFAULT_SURCH_URL}"
surch_url="${surch_url%/}"
opensearch_url_value="$(opensearch_url)"
opensearch_url_value="${opensearch_url_value%/}"
dataset_input="${BAN_HTTP_DATASET:-${DATASET:-$DEFAULT_BAN_TINY_DATASET}}"
dataset_path="$(resolve_dataset_path "$dataset_input")"
http_timeout="${BAN_HTTP_TIMEOUT:-10}"

validate_base_url "SURCH_URL" "$surch_url"
validate_base_url "OPENSEARCH_URL" "$opensearch_url_value"
validate_index_name "$BAN_TINY_INDEX"
validate_http_timeout "$http_timeout"

if [ ! -r "$dataset_path" ]; then
  bench_die "BAN tiny dataset is not readable: $dataset_path"
fi

bench_log "BAN HTTP smoke starting"
bench_log "dataset: $dataset_path"
bench_log "index: $BAN_TINY_INDEX"
bench_log "Surch URL: $surch_url"
bench_log "OpenSearch URL: $opensearch_url_value"

check_surch_health
check_opensearch_health
reset_index "Surch" "$surch_url"
reset_index "OpenSearch" "$opensearch_url_value"
load_ban_tiny "Surch" "$surch_url"
load_ban_tiny "OpenSearch" "$opensearch_url_value"
verify_count "Surch" "$surch_url"
verify_count "OpenSearch" "$opensearch_url_value"

bench_log "BAN HTTP smoke ok"
