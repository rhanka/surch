#!/usr/bin/env bash
# run-pair.sh — execute a single bench workload against Surch and then
# against OpenSearch, dumping each engine's raw output side-by-side under
# <out_dir>/. Both engines are torn down on EXIT/INT/TERM even on crash.
#
# Usage: run-pair.sh <workload> <out_dir>
#
# Supported workloads:
#   ban25k         -> scripts/bench/bench.sh
#   insee25k       -> scripts/bench/insee-bench.sh
#   insee25k-multi -> scripts/bench/insee-bench-multi.sh
#   scifact        -> scripts/bench/scifact-ndcg.sh
#   trec-covid     -> scripts/bench/trec-covid-ndcg.sh
#
# Outputs:
#   <out_dir>/<workload>-surch.out
#   <out_dir>/<workload>-os.out
#   <out_dir>/<workload>-pair.json   (schema surch.bench.pair.v1)

set -u
set -o pipefail

if [ "$#" -ne 2 ]; then
  printf 'usage: %s <workload> <out_dir>\n' "$0" >&2
  exit 1
fi

WORKLOAD="$1"
OUT_DIR="$2"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

SURCH_URL="${SURCH_URL:-http://127.0.0.1:7700}"
OS_URL="${OS_URL:-http://127.0.0.1:9200}"
SURCH_BIN="$REPO_ROOT/target/release/surch-api"
SURCH_HEALTH_TIMEOUT="${SURCH_HEALTH_TIMEOUT:-60}"
SURCH_STOP_TIMEOUT="${SURCH_STOP_TIMEOUT:-15}"

# ---------------------------------------------------------------------------
# Workload dispatch
# ---------------------------------------------------------------------------
case "$WORKLOAD" in
  ban25k)
    BENCH_SCRIPT="$SCRIPT_DIR/bench.sh"
    ;;
  insee25k)
    BENCH_SCRIPT="$SCRIPT_DIR/insee-bench.sh"
    ;;
  insee25k-multi)
    BENCH_SCRIPT="$SCRIPT_DIR/insee-bench-multi.sh"
    ;;
  scifact)
    BENCH_SCRIPT="$SCRIPT_DIR/scifact-ndcg.sh"
    ;;
  trec-covid)
    BENCH_SCRIPT="$SCRIPT_DIR/trec-covid-ndcg.sh"
    ;;
  *)
    printf 'run-pair: unknown workload "%s" (expected: ban25k|insee25k|insee25k-multi|scifact|trec-covid)\n' "$WORKLOAD" >&2
    exit 1
    ;;
esac

if [ ! -x "$BENCH_SCRIPT" ] && [ ! -r "$BENCH_SCRIPT" ]; then
  printf 'run-pair: bench script not found for workload %s: %s\n' "$WORKLOAD" "$BENCH_SCRIPT" >&2
  exit 1
fi

if [ "$OUT_DIR" = "" ]; then
  printf 'run-pair: out_dir must not be empty\n' >&2
  exit 1
fi
mkdir -p "$OUT_DIR"

SURCH_OUT="$OUT_DIR/${WORKLOAD}-surch.out"
OS_OUT="$OUT_DIR/${WORKLOAD}-os.out"
PAIR_JSON="$OUT_DIR/${WORKLOAD}-pair.json"

SURCH_PID=""
OS_STARTED=0

log() {
  printf '[run-pair] %s\n' "$*"
}

# ---------------------------------------------------------------------------
# Lifecycle helpers
# ---------------------------------------------------------------------------
stop_surch() {
  if [ "$SURCH_PID" = "" ]; then
    return 0
  fi

  if kill -0 "$SURCH_PID" 2>/dev/null; then
    log "stopping surch-api (pid=$SURCH_PID)"
    kill -INT "$SURCH_PID" 2>/dev/null || true
    local waited=0
    while [ "$waited" -lt "$SURCH_STOP_TIMEOUT" ] && kill -0 "$SURCH_PID" 2>/dev/null; do
      sleep 1
      waited=$((waited + 1))
    done
    if kill -0 "$SURCH_PID" 2>/dev/null; then
      log "surch-api still alive after ${SURCH_STOP_TIMEOUT}s, sending TERM"
      kill -TERM "$SURCH_PID" 2>/dev/null || true
      sleep 2
      kill -KILL "$SURCH_PID" 2>/dev/null || true
    fi
  fi
  SURCH_PID=""

  # Wait for port to be free again (best-effort, 10 s max).
  local port="${SURCH_URL##*:}"
  port="${port%%/*}"
  local tries=0
  while [ "$tries" -lt 10 ]; do
    if ! curl -fsS --max-time 1 "$SURCH_URL/" >/dev/null 2>&1; then
      break
    fi
    sleep 1
    tries=$((tries + 1))
  done
}

stop_opensearch() {
  if [ "$OS_STARTED" = "1" ]; then
    log "stopping OpenSearch"
    bash "$SCRIPT_DIR/opensearch-stop.sh" || true
    OS_STARTED=0
  fi
}

cleanup() {
  local rc=$?
  stop_surch || true
  stop_opensearch || true
  exit "$rc"
}
trap cleanup EXIT INT TERM

ensure_surch_binary() {
  if [ -x "$SURCH_BIN" ]; then
    return 0
  fi
  log "surch-api release binary missing, building"
  ( cd "$REPO_ROOT" && cargo build --release -p surch-api --locked )
  if [ ! -x "$SURCH_BIN" ]; then
    printf 'run-pair: cargo build did not produce %s\n' "$SURCH_BIN" >&2
    exit 1
  fi
}

start_surch() {
  ensure_surch_binary
  log "starting surch-api -> $SURCH_URL"
  ( cd "$REPO_ROOT" && RUST_LOG="${RUST_LOG:-warn}" nohup "$SURCH_BIN" >"$OUT_DIR/${WORKLOAD}-surch.server.log" 2>&1 & echo $! >"$OUT_DIR/.surch.pid" )
  SURCH_PID="$(cat "$OUT_DIR/.surch.pid")"
  rm -f "$OUT_DIR/.surch.pid"

  local waited=0
  while [ "$waited" -lt "$SURCH_HEALTH_TIMEOUT" ]; do
    if curl -fsS --max-time 1 "$SURCH_URL/" >/dev/null 2>&1; then
      log "surch-api healthy after ${waited}s"
      return 0
    fi
    if ! kill -0 "$SURCH_PID" 2>/dev/null; then
      printf 'run-pair: surch-api died before becoming healthy (see %s)\n' "$OUT_DIR/${WORKLOAD}-surch.server.log" >&2
      SURCH_PID=""
      exit 1
    fi
    sleep 1
    waited=$((waited + 1))
  done
  printf 'run-pair: surch-api did not become healthy within %ss\n' "$SURCH_HEALTH_TIMEOUT" >&2
  exit 1
}

start_opensearch() {
  log "starting OpenSearch -> $OS_URL"
  bash "$SCRIPT_DIR/opensearch-start.sh"
  OS_STARTED=1
  bash "$SCRIPT_DIR/opensearch-wait.sh"
  log "OpenSearch healthy"
}

run_bench() {
  local label="$1"
  local out_file="$2"
  local url="$3"

  : > "$out_file"
  log "running $WORKLOAD against $url -> $out_file"
  bash "$BENCH_SCRIPT" "$label" "$out_file" "$url"
}

emit_pair_json() {
  {
    printf '{'
    printf '"schema":"surch.bench.pair.v1",'
    printf '"workload":"%s",' "$WORKLOAD"
    printf '"surch_out":"%s",' "$SURCH_OUT"
    printf '"os_out":"%s"' "$OS_OUT"
    printf '}\n'
  } > "$PAIR_JSON"
  log "wrote $PAIR_JSON"
}

# ---------------------------------------------------------------------------
# Sequence: Surch first, then OpenSearch
# ---------------------------------------------------------------------------
start_surch
run_bench "${WORKLOAD}-surch" "$SURCH_OUT" "$SURCH_URL"
stop_surch

start_opensearch
run_bench "${WORKLOAD}-os" "$OS_OUT" "$OS_URL"
stop_opensearch

emit_pair_json

log "run-pair done: $WORKLOAD"
