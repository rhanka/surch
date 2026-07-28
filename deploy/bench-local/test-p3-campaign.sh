#!/usr/bin/env bash
# Régression d'intégration du pilote P3 : aucun Docker ni VM.
#
# Le vrai p2-campaign.sh est exécuté sous set -euo pipefail. Docker,
# fair-ab, le rapport et le gate sont remplacés aux frontières par des
# doubles déterministes injectés par PATH et variables d'environnement.
set -Eeuo pipefail
export LC_ALL=C

die(){ printf 'test-p3-campaign: %s\n' "$*" >&2; exit 1; }
fake_log(){ printf '%s\n' "$1" >> "$P3_CAMPAIGN_FAKE_LOG"; }

fake_docker(){
  local image=''
  local token=''
  case "${1:-}" in
    info) printf '%s\n' "$P2_DOCKER_ROOT" ;;
    build)
      shift
      while [ "$#" -gt 0 ]; do
        if [ "$1" = '-t' ]; then image="$2"; shift 2; continue; fi
        shift
      done
      [ -n "$image" ] || exit 64
      fake_log "build:$image"
      cat >/dev/null
      ;;
    image)
      [ "${2:-}" = inspect ] || exit 64
      image="${!#}"
      token="${image%%:*}"
      if [[ " $* " == *'RepoDigests'* ]]; then
        printf 'fake.registry/%s@sha256:fake-digest-%s\n' "$token" "$token"
      else
        printf 'sha256:fake-image-%s\n' "$token"
      fi
      ;;
    ps) fake_log 'docker-ps' ;;
    volume) fake_log 'docker-volume' ;;
    *) printf 'faux docker: commande non prévue: %s\n' "$*" >&2; exit 64 ;;
  esac
}

fake_findmnt(){
  if [[ " $* " == *' -o SOURCE '* ]]; then
    printf '%s\n' "$P2_DOCKER_CLASSIC_SOURCE"
  else
    printf '%s /faux ext4\n' "$P2_DOCKER_CLASSIC_SOURCE"
  fi
}
fake_sync(){ :; }
fake_sleep(){ fake_log "sleep:${1:-}"; }
fake_sudo(){ fake_log 'sudo-drop-caches'; }

fake_fair_ab(){
  local token
  local image_id
  local digest
  local manifest
  local manifest_sha
  local telemetry
  local phase_status
  local integrity_required=false
  local phase_valid=true
  local phase
  local rss
  local rss_anon
  local file_cache
  fake_log "run:$P2_VARIANT:${OUT_DIR##*/}"
  if [ "${P3_FAKE_FAIL_VARIANT:-}" = "$P2_VARIANT" ]; then
    printf 'échec injecté de fair-ab pour %s\n' "$P2_VARIANT" >&2
    exit 42
  fi
  mkdir -p "$OUT_DIR" "$P2_INPUT_DIR"
  manifest="$P2_INPUT_DIR/probe_p2_inputs.manifest"
  [ -s "$manifest" ] || printf 'inputs P3 figés\n' > "$manifest"
  manifest_sha=$(sha256sum "$manifest" | awk '{print $1}')
  token="${SURCH_IMAGE%%:*}"
  image_id="sha256:fake-image-$token"
  digest="fake.registry/$token@sha256:fake-digest-$token"
  telemetry="$OUT_DIR/telemetry.jsonl"
  phase_status="$OUT_DIR/phase-status.jsonl"
  case "$P2_VARIANT" in
    A) rss=100; rss_anon=100; file_cache=200 ;;
    B) rss=200; rss_anon=200; file_cache=100 ;;
    C) rss=110; rss_anon=110; file_cache=190; integrity_required=true ;;
    *) die "variante fausse dans le double fair-ab: $P2_VARIANT" ;;
  esac
  if [ "${P3_FAKE_C1_INVALID:-0}" = 1 ] && [ "$P2_VARIANT" = C ] && [ "${OUT_DIR##*/}" = C1 ]; then
    phase_valid=false
  fi
  jq -nc --argjson rss "$rss" --argjson rss_anon "$rss_anon" --argjson file "$file_cache" \
    '{phase:"index_ready",boundary:"snapshot",process:{rss_bytes:$rss,rss_anon_bytes:$rss_anon},cgroup:{memory_stat:{file:$file}}}' \
    > "$telemetry"
  : > "$phase_status"
  for phase in warm_match match_control warm_bool bool_size10 bool_size0 fixed_martin; do
    jq -nc --arg phase "$phase" --arg variant "$P2_VARIANT" --argjson required "$integrity_required" --argjson valid "$phase_valid" \
      '{phase:$phase,variant:$variant,valid:$valid,integrity:{required:$required,bytes:{before:17825791,after:17825791},hash_failures:{before:0,after:0},fallbacks:{before:0,after:0},fallback_fields:{before:0,after:0}}}' \
      >> "$phase_status"
  done
  for phase in warm_match match_control warm_bool bool_size10 bool_size0 fixed_martin; do
    printf '{"réponse":"canonique"}\n' > "$OUT_DIR/surch.p2.responses.${phase}.canonical.ndjson"
  done
  jq -n \
    --arg variant "$P2_VARIANT" --arg image "$SURCH_IMAGE" --arg image_id "$image_id" --arg digest "$digest" \
    --arg execution_id "$P2_EXECUTION_ID" --arg manifest "$manifest" --arg manifest_sha "$manifest_sha" \
    --arg telemetry "$telemetry" --arg phase_status "$phase_status" --argjson docs "$P2_EXPECTED_DOCS" \
    --argjson segments "$P2_REQUIRED_SEGMENTS" \
    '{measurement_valid:true,count:$docs,indexed:$docs,item_errors:0,cold_probe_ok:false,probe_cpu_count:3,cpuset:"0-2",probe_cpuset:"0-2",p2:{variant:$variant,protocol:"p2-segmented-postings-v4-termes-analyses",expected_docs:$docs,required_segments:$segments,causal_phase_records:5,replay_mix_5050:0,phase_records:6,telemetry_records:13,observed_cpu_configuration:{nproc:3,engine_cpuset:"0-2",probe_cpuset:"0-2"},image:$image,image_id:$image_id,image_digest:$digest,execution_id:$execution_id,input_manifest:$manifest,input_manifest_sha256:$manifest_sha,telemetry_jsonl:$telemetry,phase_status_jsonl:$phase_status}}' \
    > "$OUT_DIR/surch.json"
}

fake_pair_report(){
  local a=''
  local b=''
  local out=''
  local a_name
  local b_name
  local ratio=1
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --a) a="$2"; shift 2 ;;
      --b) b="$2"; shift 2 ;;
      --out) out="$2"; shift 2 ;;
      *) die "argument faux p2-report: $1" ;;
    esac
  done
  [ -n "$a" ] && [ -n "$b" ] && [ -n "$out" ] || die 'double p2-report incomplet'
  a_name="${a##*/}"
  b_name="${b##*/}"
  if [[ "$a_name" = A* || "$a_name" = smoke-A ]] && [[ "$b_name" = C* || "$b_name" = smoke-C ]]; then
    ratio=0.5
    [ "${P3_FAKE_HARD_STOP_FAIL:-0}" = 1 ] && ratio=0.81
  fi
  fake_log "pair:$a_name:$b_name"
  if [ "${P3_FAKE_PAIR_FAIL:-0}" = 1 ]; then
    printf 'échec injecté de p2-report pour %s/%s\n' "$a_name" "$b_name" >&2
    exit 43
  fi
  mkdir -p "$out"
  jq -n --argjson ratio "$ratio" \
    '{records:[{phase:"bool_size10",kind:"bool",metric:"took",b_over_a:{p95:$ratio}},{phase:"bool_size0",kind:"bool",metric:"took",b_over_a:{p95:1}},{phase:"match_control",kind:"match",metric:"took",b_over_a:{p95:1}}]}' \
    > "$out/pair-summary.json"
}
fake_gate(){
  local campaign=''
  [ "${1:-}" = --campaign ] || die 'double gate sans campagne'
  campaign="$2"
  fake_log gate
  [ "${P3_FAKE_GATE_FAIL:-0}" != 1 ] || return 1
  printf '# Verdict P3 de test\n' > "$campaign/README.md"
}

run_as_fake(){
  case "${0##*/}" in
    docker) fake_docker "$@" ;;
    findmnt) fake_findmnt "$@" ;;
    sync) fake_sync "$@" ;;
    sleep) fake_sleep "$@" ;;
    sudo) fake_sudo "$@" ;;
    fake-fair-ab) fake_fair_ab "$@" ;;
    fake-pair-report) fake_pair_report "$@" ;;
    fake-gate) fake_gate "$@" ;;
    *) return 1 ;;
  esac
}
if run_as_fake "$@"; then exit 0; fi

# Une erreur du pilote est normalement capturée dans *.out afin que les
# assertions puissent l'inspecter. Sans ce piège, un échec avant assertion
# rendait le job CI muet. Les valeurs sont initialisées avant tout prérequis
# pour que même un échec de préparation garde son contexte.
CURRENT_STEP='initialisation du test'
CURRENT_ASSERTION='aucune'
FAILED_COMMAND='aucune'
FAILED_LINE='inconnue'
CAPTURED_OUTPUT=''
CAPTURED_DRIVER_LOG=''
EXPECTED_ARTIFACTS=''
TMP_DIR=''

tool_version(){
  local tool="$1"
  local output=''
  local status=0
  if ! command -v "$tool" >/dev/null 2>&1; then
    printf 'absent'
    return
  fi
  case "$tool" in
    awk) output=$(awk -W version 2>&1) || status=$? ;;
    *) output=$("$tool" --version 2>&1) || status=$? ;;
  esac
  output=${output%%$'\n'*}
  if [ "$status" -eq 0 ]; then
    printf '%s' "$output"
  else
    printf 'échec(code=%s): %s' "$status" "$output"
  fi
}

print_versions(){
  printf 'test-p3-campaign: versions bash=%s jq=%s awk=%s git=%s\n' \
    "$BASH_VERSION" "$(tool_version jq)" "$(tool_version awk)" "$(tool_version git)" >&2
}

set_step(){
  CURRENT_STEP="$1"
  CURRENT_ASSERTION="$2"
  CAPTURED_OUTPUT="${3:-}"
  CAPTURED_DRIVER_LOG="${4:-}"
  EXPECTED_ARTIFACTS="${5:-}"
}

remember_error(){
  local status="$1"
  FAILED_COMMAND="$2"
  FAILED_LINE="$3"
  return "$status"
}

print_excerpt(){
  local label="$1"
  local path="$2"
  printf 'test-p3-campaign: %s: %s\n' "$label" "$path" >&2
  if [ -s "$path" ]; then
    tail -n 120 "$path" >&2
  elif [ -e "$path" ]; then
    printf 'test-p3-campaign: %s est vide\n' "$path" >&2
  else
    printf 'test-p3-campaign: %s est absent\n' "$path" >&2
  fi
}

cleanup_and_report(){
  local status=$?
  local artifact
  trap - EXIT
  set +e
  if [ "$status" -ne 0 ]; then
    printf 'test-p3-campaign: ECHEC (code de sortie %s)\n' "$status" >&2
    printf 'test-p3-campaign: étape: %s\n' "$CURRENT_STEP" >&2
    printf 'test-p3-campaign: assertion/commande: %s\n' "$CURRENT_ASSERTION" >&2
    printf 'test-p3-campaign: commande observée (ligne %s): %s\n' \
      "$FAILED_LINE" "$FAILED_COMMAND" >&2
    for artifact in $EXPECTED_ARTIFACTS; do
      if [ -s "$artifact" ]; then
        printf 'test-p3-campaign: artefact attendu présent: %s\n' "$artifact" >&2
        head -c 4096 "$artifact" >&2
        printf '\n' >&2
      else
        printf 'test-p3-campaign: artefact attendu absent ou vide: %s\n' "$artifact" >&2
      fi
    done
    [ -z "$CAPTURED_OUTPUT" ] || print_excerpt 'extrait du pilote capturé' "$CAPTURED_OUTPUT"
    [ -z "$CAPTURED_DRIVER_LOG" ] || print_excerpt 'extrait du journal faux pilote' "$CAPTURED_DRIVER_LOG"
  fi
  if [ -n "$TMP_DIR" ] && [ "${P3_CAMPAIGN_KEEP_TMP:-0}" != 1 ]; then
    rm -rf -- "$TMP_DIR"
  elif [ -n "$TMP_DIR" ] && [ "$status" -ne 0 ]; then
    printf 'test-p3-campaign: temporaires conservés dans %s\n' "$TMP_DIR" >&2
  fi
  exit "$status"
}

trap 'remember_error "$?" "$BASH_COMMAND" "$LINENO"' ERR
trap cleanup_and_report EXIT
print_versions

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
CAMPAIGN="$ROOT_DIR/deploy/bench-local/p2-campaign.sh"

assert_no_grouped_declarations(){
  local script
  for script in \
    "$ROOT_DIR/deploy/bench-local/p2-campaign.sh" \
    "$ROOT_DIR/deploy/bench-local/fair-ab.sh" \
    "$ROOT_DIR/deploy/bench-local/p2-gate.sh" \
    "$ROOT_DIR/deploy/bench-local/p2-report.sh" \
    "$ROOT_DIR/deploy/bench-local/test-p3-harness.sh"; do
    awk -v script="$script" '
      /^[[:space:]]*(local|declare|readonly)[[:space:]]/ {
        line = $0
        sub(/^[[:space:]]*(local|declare|readonly)[[:space:]]+/, "", line)
        gsub(/\$\(\([^)]*\)\)/, "ARITH", line)
        gsub(/=[(][^)]*[)]/, "=ARRAY", line)
        count = 0
        n = split(line, fields, /[[:space:]]+/)
        for (i = 1; i <= n; i++) {
          if (fields[i] != "" && fields[i] !~ /^-/) count++
        }
        if (count > 1) {
          printf "%s:%d: déclaration groupée interdite: %s\\n", script, NR, $0 > "/dev/stderr"
          invalid = 1
        }
      }
      END { exit invalid }
    ' "$script" || exit 1
  done
}
set_step 'garde des déclarations locales' 'aucune déclaration locale groupée ne doit subsister'
assert_no_grouped_declarations

assert_bash43_empty_array_guard(){
  awk '
    /local -a awk_inputs=\(/ { initialized = 1 }
    /awk_inputs\+=\("\$replay"\)/ { optional = 1 }
    /"\$\{awk_inputs\[@\]\}"/ { expanded = 1 }
    /replay_input/ { legacy = 1 }
    END { exit !(initialized && optional && expanded && !legacy) }
  ' "$ROOT_DIR/deploy/bench-local/fair-ab.sh" \
    || die 'les entrées awk doivent rester non vides sous Bash 4.3 et set -u'
}
set_step 'garde Bash 4.3 des entrées AWK' 'le tableau AWK optionnel doit rester non vide sous set -u'
assert_bash43_empty_array_guard

set_step 'création des fixtures' 'mktemp doit créer le répertoire temporaire'
TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/surch-p3-campaign.XXXXXX")
FAKE_BIN="$TMP_DIR/bin"
mkdir -p "$FAKE_BIN" "$TMP_DIR/docker-root"
SELF=$(readlink -f -- "$0")
for fake in docker findmnt sync sleep sudo fake-fair-ab fake-pair-report fake-gate; do
  ln -s "$SELF" "$FAKE_BIN/$fake"
done
printf '{"index":{}}\n{"NOM":"DUPONT"}\n' > "$TMP_DIR/bulk.ndjson"
printf '{"mappings":{}}\n' > "$TMP_DIR/mapping.json"

BODY_HELPERS="$TMP_DIR/p2-body-helpers.sh"
awk '
  /^p2_validate_body_files\(\)\{/ { capture = 1 }
  /^p2_manifest_value\(\)\{/ { capture = 0 }
  capture { print }
' "$ROOT_DIR/deploy/bench-local/fair-ab.sh" > "$BODY_HELPERS"
# Cette vraie fonction de fair-ab doit accepter le chemin replay optionnel
# alors que P2_REPLAY_MIX_5050=0 ne l'ajoute pas aux entrées AWK. La garde
# AWK ci-dessus impose un tableau toujours non vide, sûr sous Bash 4.3 avec
# set -u, et sensible à la réintroduction du tableau vide historique.
P2_PAIR_COUNT=1
P2_WARM_TERM_COUNT=1
P2_REPLAY_MIX_5050=0
P2_ASCIIFOLD_AWK="$ROOT_DIR/deploy/bench-local/p2-asciifold.awk"
PROBE_FIELD_NOM=NOM
PROBE_FIELD_PRENOMS=PRENOMS
PROBE_FIXED_TERM=MARTIN
source "$BODY_HELPERS"
printf '1\tDUPONT\tALICE\n' > "$TMP_DIR/bool-pairs.tsv"
printf '1\tMARTIN\n' > "$TMP_DIR/control-names.tsv"
printf '1\tDURAND\tBOB\n' > "$TMP_DIR/warm-pairs.tsv"
printf '{"query":{"bool":{"must":[{"match":{"NOM":"DUPONT"}},{"match":{"PRENOMS":"ALICE"}}]}},"size":10}\n' > "$TMP_DIR/bool10.ndjson"
printf '{"query":{"bool":{"must":[{"match":{"NOM":"DUPONT"}},{"match":{"PRENOMS":"ALICE"}}]}},"size":0}\n' > "$TMP_DIR/bool0.ndjson"
printf '{"query":{"match":{"NOM":"MARTIN"}},"size":10}\n' > "$TMP_DIR/control10.ndjson"
printf '{"query":{"match":{"NOM":"DURAND"}},"size":10}\n{"query":{"bool":{"must":[{"match":{"NOM":"DURAND"}},{"match":{"PRENOMS":"BOB"}}]}},"size":10}\n' > "$TMP_DIR/warm.ndjson"
printf '{"query":{"match":{"NOM":"MARTIN"}},"size":10}\n' > "$TMP_DIR/fixed.ndjson"
: > "$TMP_DIR/replay.ndjson"
set_step 'validation des corps P3 synthétiques' 'les corps générés doivent rester canoniques'
p2_validate_body_files \
  "$TMP_DIR/bool-pairs.tsv" "$TMP_DIR/control-names.tsv" "$TMP_DIR/warm-pairs.tsv" \
  "$TMP_DIR/bool10.ndjson" "$TMP_DIR/bool0.ndjson" "$TMP_DIR/control10.ndjson" \
  "$TMP_DIR/warm.ndjson" "$TMP_DIR/fixed.ndjson" "$TMP_DIR/replay.ndjson" \
  || die 'validation fair-ab avec replay optionnel refusée'

fail(){ CURRENT_ASSERTION="$*"; die "$*"; }
assert_file(){ EXPECTED_ARTIFACTS="$1"; [ -s "$1" ] || fail "artefact absent ou vide: $1"; }
assert_log(){
  local log="$1"
  local expected="$2"
  local actual
  actual=$(awk -F: '/^(build|run|pair|sleep):|^gate$/ { print }' "$log")
  [ "$actual" = "$expected" ] || fail "séquencement inattendu ($log):\n$actual"
}
campaign_env(){
  local mode="$1"
  local directory="$2"
  local smoke="${3:-}"
  local log="$4"
  env \
    PATH="$FAKE_BIN:$PATH" \
    P3_CAMPAIGN_FAKE_LOG="$log" \
    P2_MODE="$mode" P2_CAMPAIGN_DIR="$directory" P2_SMOKE_DIR="$smoke" \
    P2_DOCKER_ROOT="$TMP_DIR/docker-root" P2_DOCKER_CLASSIC_SOURCE=/dev/p3-faux \
    P2_REST_SECONDS=300 P2_RECOVERY_MEM_TOLERANCE_MIB=999999 \
    P2_RECOVERY_DISK_TOLERANCE_MIB=999999 P2_RECOVERY_LOAD_TOLERANCE=999999 \
    BULK_FILE="$TMP_DIR/bulk.ndjson" MAPPING_FILE="$TMP_DIR/mapping.json" \
    FAIR_AB="$FAKE_BIN/fake-fair-ab" PAIR_REPORT="$FAKE_BIN/fake-pair-report" \
    GATE_REPORT="$FAKE_BIN/fake-gate" \
    bash "$CAMPAIGN"
}

SMOKE_DIR="$TMP_DIR/smoke"
SMOKE_LOG="$TMP_DIR/smoke.log"
set_step 'smoke du pilote P3' \
  'campaign_env smoke doit construire et vérifier les variantes A/B/C' \
  "$TMP_DIR/smoke.out" "$SMOKE_LOG" \
  "$SMOKE_DIR/campaign-provenance.json $SMOKE_DIR/smoke-proof.json $SMOKE_DIR/README.md"
campaign_env smoke "$SMOKE_DIR" '' "$SMOKE_LOG" > "$TMP_DIR/smoke.out" 2>&1
for artifact in campaign-provenance.json smoke-proof.json smoke-formulas.json README.md; do
  assert_file "$SMOKE_DIR/$artifact"
done
for variant in A B C; do
  assert_file "$SMOKE_DIR/image-$variant.json"
  assert_file "$SMOKE_DIR/runs/smoke-$variant/surch.json"
done
jq -e '.verdict == "PASS SMOKE P3" and (.variants | keys == ["A","B","C"])' "$SMOKE_DIR/smoke-proof.json" >/dev/null \
  || fail 'preuve smoke non rejouable'
assert_log "$SMOKE_LOG" $'build:surch-p2-a:961ade10ffb7\nbuild:surch-p2-b:6ce390e55da3\nbuild:surch-p2-c:d0accd6e4809\nrun:A:smoke-A\nsleep:300\nrun:B:smoke-B\nsleep:300\nrun:C:smoke-C\npair:smoke-A:smoke-B\npair:smoke-B:smoke-C\npair:smoke-A:smoke-C\nsleep:300'

FULL_DIR="$TMP_DIR/full"
FULL_LOG="$TMP_DIR/full.log"
set_step 'campagne complète synthétique' \
  'campaign_env full doit produire les neuf runs et le verdict' \
  "$TMP_DIR/full.out" "$FULL_LOG" \
  "$FULL_DIR/preselection-c1.json $FULL_DIR/preselection-triplet-1.json $FULL_DIR/README.md"
campaign_env full "$FULL_DIR" "$SMOKE_DIR" "$FULL_LOG" > "$TMP_DIR/full.out" 2>&1
assert_file "$FULL_DIR/preselection-c1.json"
assert_file "$FULL_DIR/preselection-triplet-1.json"
assert_file "$FULL_DIR/README.md"
[ "$(find "$FULL_DIR/runs" -mindepth 1 -maxdepth 1 -type d | wc -l)" -eq 9 ] \
  || fail 'la campagne full n a pas produit les neuf runs'
jq -e '.hard_stop == "passed" and .integrity_target_bytes == 17825792' "$FULL_DIR/preselection-c1.json" >/dev/null \
  || fail 'hard-stop C1 non atteint'
assert_log "$FULL_LOG" $'build:surch-p2-a:961ade10ffb7\nbuild:surch-p2-b:6ce390e55da3\nbuild:surch-p2-c:d0accd6e4809\nrun:C:C1\nsleep:300\nrun:A:A1\nsleep:300\nrun:B:B1\npair:A1:B1\npair:B1:C1\npair:A1:C1\nsleep:300\nrun:A:A2\nsleep:300\nrun:B:B2\nsleep:300\nrun:C:C2\npair:A2:B2\npair:B2:C2\npair:A2:C2\nsleep:300\nrun:B:B3\nsleep:300\nrun:C:C3\nsleep:300\nrun:A:A3\npair:A3:B3\npair:B3:C3\npair:A3:C3\nsleep:300\ngate'

FAIL_DIR="$TMP_DIR/fair-ab-echec"
FAIL_LOG="$TMP_DIR/fair-ab-echec.log"
set_step 'propagation de l échec fair-ab' \
  'un fair-ab rouge doit arrêter le pilote avant la variante C' \
  "$TMP_DIR/fair-ab-echec.out" "$FAIL_LOG" "$FAIL_DIR/smoke-proof.json"
if P3_FAKE_FAIL_VARIANT=B campaign_env smoke "$FAIL_DIR" '' "$FAIL_LOG" > "$TMP_DIR/fair-ab-echec.out" 2>&1; then
  fail 'un échec de fair-ab doit arrêter le pilote'
fi
grep -q 'run smoke-B invalide' "$TMP_DIR/fair-ab-echec.out" || fail 'code fair-ab non propagé'
if grep -qx 'run:C:smoke-C' "$FAIL_LOG"; then
  fail 'le pilote a continué après l échec de B'
fi
[ ! -e "$FAIL_DIR/smoke-proof.json" ] || fail 'preuve smoke écrite après échec de fair-ab'

PAIR_FAIL_DIR="$TMP_DIR/pair-report-echec"
PAIR_FAIL_LOG="$TMP_DIR/pair-report-echec.log"
set_step 'propagation de l échec p2-report' \
  'un rapport statistique rouge doit arrêter le smoke' \
  "$TMP_DIR/pair-report-echec.out" "$PAIR_FAIL_LOG" "$PAIR_FAIL_DIR/smoke-proof.json"
if P3_FAKE_PAIR_FAIL=1 campaign_env smoke "$PAIR_FAIL_DIR" '' "$PAIR_FAIL_LOG" > "$TMP_DIR/pair-report-echec.out" 2>&1; then
  fail 'un échec de p2-report doit arrêter le pilote'
fi
grep -q 'rapport statistique invalide pour smoke-A-smoke-B' "$TMP_DIR/pair-report-echec.out" || fail 'code p2-report non propagé'
if grep -qx 'pair:smoke-B:smoke-C' "$PAIR_FAIL_LOG" || grep -qx 'pair:smoke-A:smoke-C' "$PAIR_FAIL_LOG"; then
  fail 'le pilote a continué après l échec de p2-report'
fi
[ ! -e "$PAIR_FAIL_DIR/smoke-proof.json" ] || fail 'preuve smoke écrite après échec de p2-report'

STOP_DIR="$TMP_DIR/hard-stop-echec"
STOP_LOG="$TMP_DIR/hard-stop-echec.log"
set_step 'hard-stop du premier triplet' \
  'un hard-stop rouge doit empêcher A2' \
  "$TMP_DIR/hard-stop-echec.out" "$STOP_LOG" "$STOP_DIR/preselection-triplet-1.json"
if P3_FAKE_HARD_STOP_FAIL=1 campaign_env full "$STOP_DIR" "$SMOKE_DIR" "$STOP_LOG" > "$TMP_DIR/hard-stop-echec.out" 2>&1; then
  fail 'un hard-stop rouge doit arrêter le pilote'
fi
assert_file "$STOP_DIR/preselection-triplet-1.json"
grep -q 'hard-stop premier triplet' "$TMP_DIR/hard-stop-echec.out" || fail 'hard-stop non propagé'
if grep -qx 'run:A:A2' "$STOP_LOG"; then
  fail 'le pilote a continué après le hard-stop'
fi

C1_STOP_DIR="$TMP_DIR/c1-hard-stop-echec"
C1_STOP_LOG="$TMP_DIR/c1-hard-stop-echec.log"
set_step 'hard-stop C1' \
  'un hard-stop C1 rouge doit empêcher A1 et B1' \
  "$TMP_DIR/c1-hard-stop-echec.out" "$C1_STOP_LOG" "$C1_STOP_DIR/preselection-c1.json"
if P3_FAKE_C1_INVALID=1 campaign_env full "$C1_STOP_DIR" "$SMOKE_DIR" "$C1_STOP_LOG" > "$TMP_DIR/c1-hard-stop-echec.out" 2>&1; then
  fail 'un hard-stop C1 rouge doit arrêter le pilote'
fi
grep -q 'hard-stop C1' "$TMP_DIR/c1-hard-stop-echec.out" || fail 'hard-stop C1 non propagé'
if grep -qx 'run:A:A1' "$C1_STOP_LOG" || grep -qx 'run:B:B1' "$C1_STOP_LOG"; then
  fail 'le pilote a lancé A1 ou B1 après le hard-stop C1'
fi
[ ! -e "$C1_STOP_DIR/preselection-c1.json" ] || fail 'preuve C1 écrite malgré un hard-stop rouge'

GATE_FAIL_DIR="$TMP_DIR/gate-echec"
GATE_FAIL_LOG="$TMP_DIR/gate-echec.log"
set_step 'propagation de l échec final du gate' \
  'un gate rouge doit empêcher README.md' \
  "$TMP_DIR/gate-echec.out" "$GATE_FAIL_LOG" "$GATE_FAIL_DIR/README.md"
if P3_FAKE_GATE_FAIL=1 campaign_env full "$GATE_FAIL_DIR" "$SMOKE_DIR" "$GATE_FAIL_LOG" > "$TMP_DIR/gate-echec.out" 2>&1; then
  fail 'un échec de gate doit arrêter le pilote'
fi
grep -q 'gates P2 non satisfaites' "$TMP_DIR/gate-echec.out" || fail 'code gate non propagé'
grep -qx gate "$GATE_FAIL_LOG" || fail 'gate final non atteint avant échec'
[ ! -e "$GATE_FAIL_DIR/README.md" ] || fail 'README écrit malgré un gate rouge'

printf 'test-p3-campaign: PASS\n'
