#!/usr/bin/env bash
set -euo pipefail

readonly OPENSEARCH_DEFAULT_IMAGE="opensearchproject/opensearch:2.17.1"
readonly OPENSEARCH_DEFAULT_CONTAINER_NAME="surch-ban-opensearch"
readonly OPENSEARCH_DEFAULT_HOST="127.0.0.1"
readonly OPENSEARCH_DEFAULT_PORT="9200"
readonly OPENSEARCH_DEFAULT_HEAP="512m"
readonly OPENSEARCH_DEFAULT_INDEX="ban_addresses"

bench_log() {
  printf '[surch-bench] %s\n' "$*"
}

bench_die() {
  printf '[surch-bench] error: %s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || bench_die "required command not found: $1"
}

find_container_runtime() {
  if [ "${OPENSEARCH_RUNTIME:-}" != "" ]; then
    case "$OPENSEARCH_RUNTIME" in
      docker|podman)
        require_command "$OPENSEARCH_RUNTIME"
        printf '%s\n' "$OPENSEARCH_RUNTIME"
        return 0
        ;;
      *)
        bench_die "OPENSEARCH_RUNTIME must be docker or podman"
        ;;
    esac
  fi

  if command -v docker >/dev/null 2>&1; then
    printf '%s\n' docker
    return 0
  fi

  if command -v podman >/dev/null 2>&1; then
    printf '%s\n' podman
    return 0
  fi

  bench_die "Docker or Podman is required to manage the local OpenSearch container"
}

opensearch_container_name() {
  printf '%s\n' "${OPENSEARCH_CONTAINER_NAME:-$OPENSEARCH_DEFAULT_CONTAINER_NAME}"
}

opensearch_image() {
  printf '%s\n' "${OPENSEARCH_IMAGE:-$OPENSEARCH_DEFAULT_IMAGE}"
}

opensearch_host() {
  printf '%s\n' "${OPENSEARCH_HOST:-$OPENSEARCH_DEFAULT_HOST}"
}

opensearch_port() {
  local port="${OPENSEARCH_PORT:-$OPENSEARCH_DEFAULT_PORT}"
  case "$port" in
    ''|*[!0-9]*)
      bench_die "OPENSEARCH_PORT must be a TCP port number"
      ;;
  esac

  if [ "$port" -lt 1 ] || [ "$port" -gt 65535 ]; then
    bench_die "OPENSEARCH_PORT must be between 1 and 65535"
  fi

  printf '%s\n' "$port"
}

opensearch_heap() {
  local heap="${OPENSEARCH_HEAP:-$OPENSEARCH_DEFAULT_HEAP}"
  local size="${heap%?}"
  local unit="${heap#"${size}"}"

  case "$size" in
    ''|*[!0-9]*)
      bench_die "OPENSEARCH_HEAP must look like 512m or 1g"
      ;;
  esac

  if [ "$size" -lt 1 ]; then
    bench_die "OPENSEARCH_HEAP must be greater than zero"
  fi

  case "$unit" in
    m|M|g|G)
      printf '%s\n' "$heap"
      ;;
    *)
      bench_die "OPENSEARCH_HEAP must use m, M, g, or G"
      ;;
  esac
}

opensearch_url() {
  if [ "${OPENSEARCH_URL:-}" != "" ]; then
    validate_opensearch_url "${OPENSEARCH_URL%/}"
    printf '%s\n' "${OPENSEARCH_URL%/}"
    return 0
  fi

  printf 'http://%s:%s\n' "$(opensearch_host)" "$(opensearch_port)"
}

validate_opensearch_url() {
  local url="$1"
  case "$url" in
    http://*|https://*)
      ;;
    *)
      bench_die "OPENSEARCH_URL must start with http:// or https://"
      ;;
  esac

  case "$url" in
    *\?*|*#*)
      bench_die "OPENSEARCH_URL must not include query parameters or fragments"
      ;;
  esac
}

opensearch_ban_index() {
  printf '%s\n' "${OPENSEARCH_BAN_INDEX:-$OPENSEARCH_DEFAULT_INDEX}"
}

validate_container_name() {
  local name="$1"
  case "$name" in
    '')
      bench_die "container name must not be empty"
      ;;
    *[!A-Za-z0-9_.-]*)
      bench_die "container name may only contain letters, numbers, dot, underscore, and dash"
      ;;
    surch-*|surch_*)
      return 0
      ;;
    *)
      bench_die "refusing non-dedicated container name: $name (use a surch- or surch_ prefix)"
      ;;
  esac
}

validate_index_name() {
  local name="$1"
  case "$name" in
    '')
      bench_die "index name must not be empty"
      ;;
    *[!a-z0-9_-]*)
      bench_die "index name may only contain lowercase letters, numbers, underscore, and dash"
      ;;
    ban_*|ban-*|surch_*|surch-*)
      return 0
      ;;
    *)
      bench_die "refusing non-dedicated BAN demo index name: $name"
      ;;
  esac
}

container_exists() {
  local runtime="$1"
  local name="$2"
  "$runtime" container inspect "$name" >/dev/null 2>&1
}

container_status() {
  local runtime="$1"
  local name="$2"
  "$runtime" container inspect --format '{{.State.Status}}' "$name" 2>/dev/null || true
}
