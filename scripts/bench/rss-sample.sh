#!/usr/bin/env bash
# rss-sample.sh — sample the resident set size (RSS) of a process at 1 Hz
# and emit a minimal JSON envelope suitable for the bench_report aggregator.
#
# Usage: rss-sample.sh <pid> <duration_s> <out_json>
#
# Reads /proc/<pid>/status's VmRSS line (kB) once per second for
# <duration_s> seconds, falling back to `ps -o rss= -p <pid>` when /proc is
# not available (e.g. macOS). Captures peak and final samples. If the pid
# disappears mid-run, the last valid sample is kept as final_kb.
#
# JSON schema: surch.bench.rss.v1
#   { "schema": "surch.bench.rss.v1",
#     "pid": <int>, "duration_s": <int>, "samples": <int>,
#     "peak_kb": <int>, "peak_mb": <int>,
#     "final_kb": <int>, "final_mb": <int> }
#
# Exit codes:
#   0 = ok, JSON written
#   2 = pid invalid at start (no /proc entry and `ps` returned nothing)
#   1 = argument / IO error

set -u
set -o pipefail

if [ "$#" -ne 3 ]; then
  printf 'usage: %s <pid> <duration_s> <out_json>\n' "$0" >&2
  exit 1
fi

PID="$1"
DURATION="$2"
OUT="$3"

case "$PID" in
  ''|*[!0-9]*)
    printf 'rss-sample: pid must be a positive integer: %s\n' "$PID" >&2
    exit 1
    ;;
esac

case "$DURATION" in
  ''|*[!0-9]*)
    printf 'rss-sample: duration_s must be a positive integer: %s\n' "$DURATION" >&2
    exit 1
    ;;
esac

if [ "$DURATION" -lt 1 ]; then
  printf 'rss-sample: duration_s must be >= 1 (got %s)\n' "$DURATION" >&2
  exit 1
fi

if [ "$OUT" = "" ]; then
  printf 'rss-sample: out_json must not be empty\n' >&2
  exit 1
fi

# Read one RSS sample (in kB). Echoes empty string when the pid is gone.
sample_rss_kb() {
  local pid="$1"
  local value=""

  if [ -r "/proc/$pid/status" ]; then
    value="$(awk '/^VmRSS:/ { print $2; exit }' "/proc/$pid/status" 2>/dev/null || true)"
  fi

  if [ "$value" = "" ]; then
    # Fallback: ps -o rss= returns kB on Linux/macOS. Trim whitespace.
    value="$(ps -o rss= -p "$pid" 2>/dev/null | awk '{ print $1 }' || true)"
  fi

  printf '%s' "$value"
}

# Initial sample — used as a liveness check for the pid.
first="$(sample_rss_kb "$PID")"
if [ "$first" = "" ]; then
  printf 'rss-sample: pid %s is not alive or has no RSS info\n' "$PID" >&2
  exit 2
fi

peak_kb=0
final_kb=0
samples=0

update_with_sample() {
  local value="$1"
  case "$value" in
    ''|*[!0-9]*)
      return 0
      ;;
  esac
  final_kb="$value"
  if [ "$value" -gt "$peak_kb" ]; then
    peak_kb="$value"
  fi
  samples=$((samples + 1))
}

update_with_sample "$first"

# Sample once per second for DURATION seconds total (we already took 1
# sample at t=0, so loop DURATION-1 times with a 1 s sleep between each).
remaining=$((DURATION - 1))
while [ "$remaining" -gt 0 ]; do
  sleep 1
  value="$(sample_rss_kb "$PID")"
  if [ "$value" = "" ]; then
    # pid gone — stop sampling but keep the last good values.
    break
  fi
  update_with_sample "$value"
  remaining=$((remaining - 1))
done

# Round kB -> MB (integer division by 1024).
peak_mb=$(( peak_kb / 1024 ))
final_mb=$(( final_kb / 1024 ))

# Ensure the output directory exists.
out_dir="$(dirname "$OUT")"
if [ "$out_dir" != "" ] && [ "$out_dir" != "." ]; then
  mkdir -p "$out_dir"
fi

# Write the JSON envelope. Plain printf keeps us dependency-free.
{
  printf '{'
  printf '"schema":"surch.bench.rss.v1",'
  printf '"pid":%s,' "$PID"
  printf '"duration_s":%s,' "$DURATION"
  printf '"samples":%s,' "$samples"
  printf '"peak_kb":%s,' "$peak_kb"
  printf '"peak_mb":%s,' "$peak_mb"
  printf '"final_kb":%s,' "$final_kb"
  printf '"final_mb":%s' "$final_mb"
  printf '}\n'
} > "$OUT"

exit 0
