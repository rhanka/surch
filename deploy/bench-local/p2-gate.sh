#!/usr/bin/env bash
# p2-gate.sh — applique les gates P2 pré-engagées après trois paires valides.
# Les rapports restent JSON pour le pilote, mais les décisions sont prises en
# Bash et awk : aucun interpréteur Python n'est requis par le harnais local.
set -euo pipefail
export LC_ALL=C

CAMPAIGN=""
P2_REQUIRE_P3_INTEGRITY="${P2_REQUIRE_P3_INTEGRITY:-1}"
P3_A_SHA="961ade10ffb74d78156aee8148f1e5c6bbbe6ba2"
P3_B_SHA="6ce390e55da3593242ec11e2b09d4dee1057726d"
P3_C_SHA="d0accd6e4809bc7340a6cd55cef0a94fcb6c062d"
P3_INTEGRITY_TARGET_BYTES=17825792
P3_PROTOCOL_VERSION="p2-segmented-postings-v4-termes-analyses"
P3_RUNS=(A1 A2 A3 B1 B2 B3 C1 C2 C3)

usage(){ printf 'usage: p2-gate.sh --campaign RÉPERTOIRE\n' >&2; }
die(){ printf '[p2-gate] %s\n' "$*" >&2; exit 1; }

# Un artefact structurel absent ou incohérent n'est pas un échec de
# performance : il invalide la campagne. Écrire un verdict lisible avant de
# sortir évite qu'un appelant réduise un arrêt précoce à un simple log perdu.
invalidate_campaign(){
  local detail="$1"
  printf '[p2-gate] INVALIDE P3: %s\n' "$detail" >&2
  jq -n --arg detail "$detail" \
    '{schema:"surch.bench.p3.campaign.v1",verdict:"INVALIDE P3",invalid_reason:$detail,checks:[]}' \
    > "$CAMPAIGN/campaign-summary.json" || die 'écriture impossible de campaign-summary.json invalide'
  {
    printf '%s\n\n' '# P3 — verdict de campagne'
    printf 'Verdict: **INVALIDE P3**.\n\n'
    printf 'Raison: %s\n' "$detail"
  } > "$CAMPAIGN/README.md" || die 'écriture impossible de README.md invalide'
  exit 1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --campaign) CAMPAIGN=${2:-}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage; die "option inconnue: $1" ;;
  esac
done
[ -n "$CAMPAIGN" ] || { usage; die '--campaign est obligatoire'; }
[ -d "$CAMPAIGN" ] || die "campagne introuvable: $CAMPAIGN"
for command in jq sha256sum readlink; do
  command -v "$command" >/dev/null 2>&1 || die "commande requise absente: $command"
done
[ "$P2_REQUIRE_P3_INTEGRITY" = "1" ] || die 'la campagne P3 exige P2_REQUIRE_P3_INTEGRITY=1'

telemetry_jsonl_valid(){
  local score="$1" telemetry replay expected variant require_p3=false
  replay=$(jq -er '.p2.replay_mix_5050 | select(. == 0 or . == 1)' "$score") || return 1
  variant=$(jq -er '.p2.variant | strings' "$score") || return 1
  [ "$variant" != C ] || require_p3=true
  telemetry=$(jq -er '.p2.telemetry_jsonl | strings' "$score") || return 1
  [ -r "$telemetry" ] || return 1
  expected=13
  [ "$replay" = 0 ] || expected=15
  jq -se --argjson expected "$expected" --argjson require_p3 "$require_p3" '
    def number: type == "number";
    def psi:
      type == "object"
      and (.some | type == "object") and (.full | type == "object")
      and ([.some.avg10,.some.avg60,.some.avg300,.some.total,.full.avg10,.full.avg60,.full.avg300,.full.total] | all(number));
    def snapshot:
      (.metrics | type == "object")
      and (.metrics.index.postings_directory_bytes | number)
      and (.metrics.index.total_bytes | number)
      and (.metrics.jemalloc.allocated | number)
      and (.metrics.jemalloc.active | number)
      and (.metrics.jemalloc.resident | number)
      and (.metrics.jemalloc.retained | number)
      and (.process.rss_bytes | number)
      and (.process.rss_anon_bytes | number)
      and (.process.vmhwm_bytes | number)
      and (.cgroup.memory_current | number)
      and ([.cgroup.memory_stat.anon,.cgroup.memory_stat.file,.cgroup.memory_stat.workingset_refault_file,.cgroup.memory_stat.workingset_activate_file,.cgroup.memory_stat.pgmajfault,.cgroup.cpu_stat.nr_throttled,.cgroup.cpu_stat.throttled_usec] | all(number))
      and (.cgroup.memory_psi | psi) and (.cgroup.io_psi | psi)
      and (.cgroup.io_stat | type == "array" and length > 0)
      and all(.cgroup.io_stat[]; (.device | type == "string" and length > 0) and (.metric | type == "string" and length > 0) and (.value | number))
      and (if $require_p3 then
        (.metrics.p3_integrity | type == "object")
        and ([.metrics.p3_integrity.bytes,.metrics.p3_integrity.pages,.metrics.p3_integrity.verified_bytes,.metrics.p3_integrity.hash_failures,.metrics.p3_integrity.fallbacks,.metrics.p3_integrity.fallback_fields,.metrics.p3_integrity.term_occurrences,.metrics.p3_integrity.blocks,.metrics.p3_integrity.fields,.metrics.p3_integrity.term_payload_bytes,.metrics.p3_integrity.csr_bytes,.metrics.p3_integrity.directory_bytes] | all(number))
      else .metrics.p3_integrity == null end);
    (length == $expected)
    and all(.[]; (.phase | type == "string") and (.boundary | type == "string") and snapshot)
    and ([.[] | select(.phase == "index_ready" and .boundary == "snapshot")] | length == 1)
    and ([.[] | select(.phase == "warm_match" or .phase == "match_control" or .phase == "warm_bool" or .phase == "bool_size10" or .phase == "bool_size0" or .phase == "fixed_martin") | .phase + ":" + .boundary] | sort == ["bool_size0:after","bool_size0:before","bool_size10:after","bool_size10:before","fixed_martin:after","fixed_martin:before","match_control:after","match_control:before","warm_bool:after","warm_bool:before","warm_match:after","warm_match:before"])
    and all(.[] | select(.boundary == "before" or .boundary == "snapshot"); .cgroup.io_stat_delta_from_before == null)
    and all(.[] | select(.boundary == "after"); .cgroup.io_stat_delta_from_before | type == "array" and length > 0 and all(.[]; (.device | type == "string") and (.metric | type == "string") and (.delta | number)))
  ' "$telemetry" 2>/dev/null | grep -qx true
}

validate_campaign_provenance(){
  local provenance score variant metadata manifest manifest_sha canonical_manifest="" expected_variant run_dir
  local -a run_dirs
  provenance="$CAMPAIGN/campaign-provenance.json"
  [ -r "$provenance" ] || return 1
  jq -e --arg a "$P3_A_SHA" --arg b "$P3_B_SHA" --arg c "$P3_C_SHA" '
    .schema == "surch.bench.p3.provenance.v1"
    and .protocol == "p3-campagne-plan-v1"
    and .variants.A.commit == $a and .variants.B.commit == $b and .variants.C.commit == $c
    and ([.variants[] | .image, .image_id, .digest] | all(type == "string" and length > 0))
  ' "$provenance" >/dev/null || return 1
  RUN_SCORES=("$CAMPAIGN"/runs/*/surch.json)
  [ -e "${RUN_SCORES[0]}" ] || RUN_SCORES=()
  [ "${#RUN_SCORES[@]}" -eq 9 ] || return 1
  run_dirs=("$CAMPAIGN"/runs/*)
  [ -e "${run_dirs[0]}" ] || run_dirs=()
  [ "${#run_dirs[@]}" -eq 9 ] || return 1
  for run_dir in "${run_dirs[@]}"; do
    [ -d "$run_dir" ] || return 1
  done
  for run in "${P3_RUNS[@]}"; do
    run_dir="$CAMPAIGN/runs/$run"
    [ -d "$run_dir" ] || return 1
    score="$CAMPAIGN/runs/$run/surch.json"
    [ -s "$score" ] || return 1
    expected_variant=${run:0:1}
    variant=$(jq -er '.p2.variant | strings' "$score") || return 1
    [ "$variant" = "$expected_variant" ] || return 1
    case "$variant" in
      A|B|C) metadata="$CAMPAIGN/image-$variant.json" ;;
      *) return 1 ;;
    esac
    [ -r "$metadata" ] || return 1
    jq -e --arg variant "$variant" --arg protocol "$P3_PROTOCOL_VERSION" \
      --slurpfile provenance "$provenance" --slurpfile metadata "$metadata" '
        ($provenance[0].variants[$variant]) as $expected
        | ($metadata[0]) as $actual
        | $actual == $expected
        and .p2.protocol == $protocol
        and .p2.image == $expected.image
        and .p2.image_id == $expected.image_id
        and .p2.image_digest == $expected.digest
      ' "$score" >/dev/null || return 1
    manifest=$(jq -er '.p2.input_manifest | strings' "$score") || return 1
    [ -r "$manifest" ] || return 1
    manifest_sha=$(sha256sum "$manifest" | awk '{print $1}') || return 1
    [ "$(jq -er '.p2.input_manifest_sha256 | strings' "$score")" = "$manifest_sha" ] || return 1
    manifest=$(readlink -f -- "$manifest") || return 1
    if [ -z "$canonical_manifest" ]; then
      canonical_manifest="$manifest"
    elif [ "$manifest" != "$canonical_manifest" ]; then
      return 1
    fi
    telemetry_jsonl_valid "$score" || return 1
  done
}

# Les trois contrastes ne sont valides que s'ils décrivent exactement les
# trois mêmes triplets. Le nom du répertoire, parity.json, pair-summary.json,
# scorecard et manifeste sont tous liés ici : une cardinalité de trois ne
# suffit jamais à prouver l'identité des répétitions.
validate_pair_group(){
  local root="$1"; shift
  local summary pair_dir parity pair a_run b_run expected_a expected_b
  local summary_a summary_b score_a score_b manifest_a manifest_b
  local summaries=("$root"/*/pair-summary.json)
  local pair_dirs=("$root"/*)
  [ -e "${summaries[0]}" ] || summaries=()
  [ -e "${pair_dirs[0]}" ] || pair_dirs=()
  [ "${#summaries[@]}" -eq "$#" ] || return 1
  [ "${#pair_dirs[@]}" -eq "$#" ] || return 1
  for pair in "$@"; do
    expected_a=${pair%%-*}; expected_b=${pair#*-}
    pair_dir="$root/$pair"
    summary="$pair_dir/pair-summary.json"
    parity="$pair_dir/parity.json"
    [ -d "$pair_dir" ] && [ -r "$summary" ] && [ -r "$parity" ] || return 1
    a_run=$(jq -er '.a_run | strings' "$parity") || return 1
    b_run=$(jq -er '.b_run | strings' "$parity") || return 1
    [ "$a_run" = "$expected_a" ] && [ "$b_run" = "$expected_b" ] || return 1
    score_a="$CAMPAIGN/runs/$a_run/surch.json"
    score_b="$CAMPAIGN/runs/$b_run/surch.json"
    [ -r "$score_a" ] && [ -r "$score_b" ] || return 1
    summary_a=$(jq -er '.a_dir | strings' "$summary" 2>/dev/null) || return 1
    summary_b=$(jq -er '.b_dir | strings' "$summary" 2>/dev/null) || return 1
    jq -e '.schema == "surch.bench.p2.pair.v1"' "$summary" >/dev/null || return 1
    [ "$(readlink -f -- "$summary_a")" = "$(readlink -f -- "$CAMPAIGN/runs/$a_run")" ] || return 1
    [ "$(readlink -f -- "$summary_b")" = "$(readlink -f -- "$CAMPAIGN/runs/$b_run")" ] || return 1
    manifest_a=$(jq -er '.p2.input_manifest_sha256 | strings' "$score_a") || return 1
    manifest_b=$(jq -er '.p2.input_manifest_sha256 | strings' "$score_b") || return 1
    jq -e --arg pair "$pair" --arg a "$a_run" --arg b "$b_run" --arg ma "$manifest_a" --arg mb "$manifest_b" '
      .pair == $pair and .a_run == $a and .b_run == $b and .parity == true
      and .a_manifest_sha256 == $ma and .b_manifest_sha256 == $mb and $ma == $mb
    ' "$parity" >/dev/null || return 1
  done
}

validate_p3_bijection(){
  validate_pair_group "$CAMPAIGN/pairs" A1-B1 A2-B2 A3-B3 \
    && validate_pair_group "$CAMPAIGN/p3-primary-pairs" A1-C1 A2-C2 A3-C3 \
    && validate_pair_group "$CAMPAIGN/p3-cost-pairs" B1-C1 B2-C2 B3-C3
}

phase_status_valid(){
  local run_dir="$1" require_p3="$2" score status
  score="$run_dir/surch.json"
  jq -e '.measurement_valid == true and .p2.causal_phase_records == 5 and ((.p2.replay_mix_5050 == 0 and .p2.phase_records == 6 and .p2.telemetry_records == 13) or (.p2.replay_mix_5050 == 1 and .p2.phase_records == 7 and .p2.telemetry_records == 15))' "$score" >/dev/null 2>&1 || return 1
  status=$(jq -er '.p2.phase_status_jsonl | strings' "$score" 2>/dev/null) || return 1
  [ -f "$status" ] || return 1
  # Les cinq phases causales portent la preuve A/B. Cold reste disponible
  # dans les scorecards, mais ses droits de reclaim ne peuvent pas invalider
  # le routage, les corps ou la parité déjà vérifiés sur ces cinq phases.
  jq -se --argjson require_p3 "$require_p3" '
    ([.[] | select(.phase == "warm_match" or .phase == "match_control" or .phase == "warm_bool" or .phase == "bool_size10" or .phase == "bool_size0")]) as $causal
    | ($causal | length == 5)
    and ([ $causal[] | .phase ] | sort == ["bool_size0", "bool_size10", "match_control", "warm_bool", "warm_match"])
    and all($causal[]; .valid == true and .cpu_steal_within_limit == true and .request_cache == false and .hits_total_positive == true)
    and ([.[] | select(.phase == "fixed_martin")] | length == 1)
    and ([.[] | select(.phase == "fixed_martin")] | all(.[]; .valid == true and .cpu_steal_within_limit == true))
    and (if $require_p3 then
      all((.[] | select(.phase == "warm_match" or .phase == "match_control" or .phase == "warm_bool" or .phase == "bool_size10" or .phase == "bool_size0" or .phase == "fixed_martin"));
        .integrity.required == true
        and (.integrity.bytes.before | type) == "number"
        and (.integrity.bytes.after | type) == "number"
        and (.integrity.hash_failures.before | type) == "number"
        and (.integrity.hash_failures.after | type) == "number"
        and (.integrity.fallback_fields.before | type) == "number"
        and (.integrity.fallback_fields.after | type) == "number"
        and .integrity.bytes.before > 0 and .integrity.bytes.after > 0
        and .integrity.bytes.before <= 33554432
        and .integrity.bytes.after <= 33554432
        and .integrity.hash_failures.before == 0
        and .integrity.hash_failures.after == 0
        and (.integrity.fallbacks.before | type) == "number"
        and (.integrity.fallbacks.after | type) == "number"
        and .integrity.fallbacks.before == 0
        and .integrity.fallbacks.after == 0
        and .integrity.fallback_fields.before == 0
        and .integrity.fallback_fields.after == 0
      )
    else true end)
  ' "$status" 2>/dev/null | grep -qx true
}

record_ratio(){
  local summary="$1" phase="$2" kind="$3" metric="$4" quantile="$5"
  jq -er --arg phase "$phase" --arg kind "$kind" --arg metric "$metric" --arg quantile "$quantile" '
    first(.records[] | select(.phase == $phase and .kind == $kind and .metric == $metric) | .b_over_a[$quantile])
    | if . == null then error("ratio indéfini") else tonumber end
  ' "$summary"
}

record_probe_delta(){
  jq -er '
    (first(.records[] | select(.phase == "bool_size10" and .kind == "bool" and .metric == "probe") | .b.p95)) as $b
    | (first(.records[] | select(.phase == "bool_size10" and .kind == "bool" and .metric == "probe") | .a.p95)) as $a
    | ($b - $a | abs)
  ' "$1"
}

median_three(){
  [ "$#" -eq 3 ] || return 1
  printf '%s\n' "$@" | awk '
    { value[NR] = $1 + 0 }
    END {
      if (NR != 3) exit 1
      for (i = 1; i <= 3; i++) for (j = i + 1; j <= 3; j++) if (value[i] > value[j]) { tmp = value[i]; value[i] = value[j]; value[j] = tmp }
      printf "%.12g", value[2]
    }
  '
}

number_le(){ awk -v left="$1" -v right="$2" 'BEGIN { exit !((left + 0) <= (right + 0)) }'; }
numbers_all_le(){ local limit="$1"; shift; for value in "$@"; do number_le "$value" "$limit" || return 1; done; }
numbers_all_between(){ local low="$1" high="$2"; shift 2; for value in "$@"; do awk -v value="$value" -v low="$low" -v high="$high" 'BEGIN { exit !((value + 0) >= (low + 0) && (value + 0) <= (high + 0)) }' || return 1; done; }
numbers_all_ge(){ local limit="$1"; shift; for value in "$@"; do awk -v value="$value" -v limit="$limit" 'BEGIN { exit !((value + 0) >= (limit + 0)) }' || return 1; done; }
numbers_json(){ printf '%s\n' "$@" | jq -Rsc 'split("\n") | map(select(length > 0) | tonumber)'; }

BASELINE_PAIR_SUMMARIES=("$CAMPAIGN"/pairs/*/pair-summary.json)
[ -e "${BASELINE_PAIR_SUMMARIES[0]}" ] || BASELINE_PAIR_SUMMARIES=()
[ "${#BASELINE_PAIR_SUMMARIES[@]}" -eq 3 ] \
  || invalidate_campaign "P3 exige exactement trois paires A/B, trouvé ${#BASELINE_PAIR_SUMMARIES[@]}"
PAIR_SUMMARIES=("$CAMPAIGN"/p3-primary-pairs/*/pair-summary.json)
[ -e "${PAIR_SUMMARIES[0]}" ] || PAIR_SUMMARIES=()
[ "${#PAIR_SUMMARIES[@]}" -eq 3 ] \
  || invalidate_campaign "P3 exige exactement trois paires C/A, trouvé ${#PAIR_SUMMARIES[@]}"
COST_PAIR_SUMMARIES=("$CAMPAIGN"/p3-cost-pairs/*/pair-summary.json)
[ -e "${COST_PAIR_SUMMARIES[0]}" ] || COST_PAIR_SUMMARIES=()
[ "${#COST_PAIR_SUMMARIES[@]}" -eq 3 ] \
  || invalidate_campaign "P3 exige exactement trois paires C/B, trouvé ${#COST_PAIR_SUMMARIES[@]}"
validate_campaign_provenance \
  || invalidate_campaign 'provenance, scorecards, image ou manifeste P3 incohérents (bijection impossible)'
validate_p3_bijection \
  || invalidate_campaign 'bijection P3 A1/A2/A3, B1/B2/B3, C1/C2/C3 ou liens de paires incohérents'

VALIDITIES=()
PRODUCT_TOOK=()
PRODUCT_CLIENT=()
CORE_TOOK95=()
CORE_TOOK99=()
FIXED_MATCH=()
RANDOM_MATCH=()
RANDOM_MATCH_CLIENT=()
PROBE_DELTA=()
BOOTSTRAP_UPPER=()
PAIR_DIRS=()
BLOCK_RATIO_DIAGNOSTICS=()
BLOCK_RATIO_C_DIAGNOSTICS=()
P3_INTEGRITY_DIAGNOSTICS=()
BASELINE_VALIDITIES=()
BASELINE_PAIR_DIRS=()
COST_VALIDITIES=()
COST_PAIR_DIRS=()
P3_COST_TOOK=()
P3_COST_SIZE0_TOOK=()
PRIMARY_A_RUNS=()
PRIMARY_C_RUNS=()
PRIMARY_PAIRS=()
declare -A BASELINE_B_FOR_A=()
declare -A COST_B_FOR_C=()

for summary in "${BASELINE_PAIR_SUMMARIES[@]}"; do
  pair_dir=${summary%/pair-summary.json}
  parity="$pair_dir/parity.json"
  [ -f "$parity" ] || die "artefact A/B illisible: $parity"
  a_run=$(jq -er '.a_run | strings' "$parity") || die "artefact A/B illisible: $parity"
  b_run=$(jq -er '.b_run | strings' "$parity") || die "artefact A/B illisible: $parity"
  pair=$(jq -er '.pair | strings' "$parity") || die "artefact A/B illisible: $parity"
  valid=false
  if jq -e '.parity == true and .a_manifest_sha256 == .b_manifest_sha256' "$parity" >/dev/null 2>&1 \
     && phase_status_valid "$CAMPAIGN/runs/$a_run" false \
     && phase_status_valid "$CAMPAIGN/runs/$b_run" false; then
    valid=true
  fi
  BASELINE_VALIDITIES+=("$valid")
  BASELINE_PAIR_DIRS+=("$pair_dir")
  BASELINE_B_FOR_A["$a_run"]="$b_run"
  b_status=$(jq -er '.p2.phase_status_jsonl | strings' "$CAMPAIGN/runs/$b_run/surch.json") || die "statut P2 B absent: $b_run"
  [ -f "$b_status" ] || die "statut P2 B illisible: $b_status"
  while IFS= read -r diagnostic; do
    [ -n "$diagnostic" ] && BLOCK_RATIO_DIAGNOSTICS+=("$diagnostic")
  done < <(jq -c --arg pair "$pair" --arg run "$b_run" '
    select(.variant == "B" and .bool_requests > 0 and (.phase == "warm_bool" or .phase == "bool_size10" or .phase == "bool_size0"))
    | {pair:$pair,run:$run,phase:.phase,ratio:.blocks_read_over_total,target:.blocks_ratio_target,pass:.blocks_ratio_target_pass,verdict:.blocks_ratio_verdict}
  ' "$b_status")
done

for summary in "${PAIR_SUMMARIES[@]}"; do
  pair_dir=${summary%/pair-summary.json}
  parity="$pair_dir/parity.json"
  [ -f "$parity" ] || die "artefact C/A illisible: $parity"
  a_run=$(jq -er '.a_run | strings' "$parity") || die "artefact C/A illisible: $parity"
  c_run=$(jq -er '.b_run | strings' "$parity") || die "artefact C/A illisible: $parity"
  pair=$(jq -er '.pair | strings' "$parity") || die "artefact C/A illisible: $parity"
  c_requires_p3=false
  [ "$P2_REQUIRE_P3_INTEGRITY" = "1" ] && c_requires_p3=true
  valid=false
  if jq -e '.parity == true and .a_manifest_sha256 == .b_manifest_sha256' "$parity" >/dev/null 2>&1 \
     && phase_status_valid "$CAMPAIGN/runs/$a_run" false \
     && phase_status_valid "$CAMPAIGN/runs/$c_run" "$c_requires_p3"; then
    valid=true
  fi
  VALIDITIES+=("$valid")
  PAIR_DIRS+=("$pair_dir")
  PRIMARY_A_RUNS+=("$a_run")
  PRIMARY_C_RUNS+=("$c_run")
  PRIMARY_PAIRS+=("$pair")
  PRODUCT_TOOK+=("$(record_ratio "$summary" bool_size10 bool took p95)") || die "ratio C/A absent: $summary bool_size10/bool/took/p95"
  PRODUCT_CLIENT+=("$(record_ratio "$summary" bool_size10 bool client p95)") || die "ratio C/A absent: $summary bool_size10/bool/client/p95"
  CORE_TOOK95+=("$(record_ratio "$summary" bool_size0 bool took p95)") || die "ratio C/A absent: $summary bool_size0/bool/took/p95"
  CORE_TOOK99+=("$(record_ratio "$summary" bool_size0 bool took p99)") || die "ratio C/A absent: $summary bool_size0/bool/took/p99"
  FIXED_MATCH+=("$(record_ratio "$summary" fixed_martin match took p95)") || die "ratio C/A absent: $summary fixed_martin/match/took/p95"
  RANDOM_MATCH+=("$(record_ratio "$summary" match_control match took p95)") || die "ratio C/A absent: $summary match_control/match/took/p95"
  RANDOM_MATCH_CLIENT+=("$(record_ratio "$summary" match_control match client p95)") || die "ratio C/A absent: $summary match_control/match/client/p95"
  PROBE_DELTA+=("$(record_probe_delta "$summary")") || die "sonde C/A absente: $summary"
  BOOTSTRAP_UPPER+=("$(jq -er '.primary_bootstrap.ci95_high | tonumber' "$summary")") || die "IC95 C/A absent: $summary"
  if [ "$P2_REQUIRE_P3_INTEGRITY" = "1" ]; then
    c_status=$(jq -er '.p2.phase_status_jsonl | strings' "$CAMPAIGN/runs/$c_run/surch.json") || die "statut P3 C absent: $c_run"
    [ -f "$c_status" ] || die "statut P3 C illisible: $c_status"
    while IFS= read -r diagnostic; do
      [ -n "$diagnostic" ] && P3_INTEGRITY_DIAGNOSTICS+=("$diagnostic")
    done < <(jq -c --arg pair "$pair" --arg run "$c_run" '
      select(.variant == "C" and (.phase == "warm_match" or .phase == "match_control" or .phase == "warm_bool" or .phase == "bool_size10" or .phase == "bool_size0" or .phase == "fixed_martin"))
      | {pair:$pair,run:$run,phase:.phase,before_bytes:.integrity.bytes.before,bytes:.integrity.bytes.after,hash_failures:.integrity.hash_failures.after,fallbacks:.integrity.fallbacks.after,fallback_fields:.integrity.fallback_fields.after,verified_bytes:.integrity.verified_bytes.after}
    ' "$c_status")
  fi
  c_status=$(jq -er '.p2.phase_status_jsonl | strings' "$CAMPAIGN/runs/$c_run/surch.json") || die "statut P3 C absent: $c_run"
  [ -f "$c_status" ] || die "statut P3 C illisible: $c_status"
  while IFS= read -r diagnostic; do
    [ -n "$diagnostic" ] && BLOCK_RATIO_C_DIAGNOSTICS+=("$diagnostic")
  done < <(jq -c --arg pair "$pair" --arg run "$c_run" '
    select(.variant == "C" and .bool_requests > 0 and (.phase == "warm_bool" or .phase == "bool_size10" or .phase == "bool_size0"))
    | {pair:$pair,run:$run,phase:.phase,ratio:.blocks_read_over_total,target:.blocks_ratio_target,pass:.blocks_ratio_target_pass,verdict:.blocks_ratio_verdict}
  ' "$c_status")
done

for summary in "${COST_PAIR_SUMMARIES[@]}"; do
  pair_dir=${summary%/pair-summary.json}
  parity="$pair_dir/parity.json"
  [ -f "$parity" ] || die "artefact C/B illisible: $parity"
  b_run=$(jq -er '.a_run | strings' "$parity") || die "artefact C/B illisible: $parity"
  c_run=$(jq -er '.b_run | strings' "$parity") || die "artefact C/B illisible: $parity"
  pair=$(jq -er '.pair | strings' "$parity") || die "artefact C/B illisible: $parity"
  c_requires_p3=false
  [ "$P2_REQUIRE_P3_INTEGRITY" = "1" ] && c_requires_p3=true
  valid=false
  if jq -e '.parity == true and .a_manifest_sha256 == .b_manifest_sha256' "$parity" >/dev/null 2>&1 \
     && phase_status_valid "$CAMPAIGN/runs/$b_run" false \
     && phase_status_valid "$CAMPAIGN/runs/$c_run" "$c_requires_p3"; then
    valid=true
  fi
  COST_VALIDITIES+=("$valid")
  COST_PAIR_DIRS+=("$pair_dir")
  COST_B_FOR_C["$c_run"]="$b_run"
  P3_COST_TOOK+=("$(record_ratio "$summary" bool_size10 bool took p95)") || die "ratio C/B absent: $summary bool_size10/bool/took/p95"
  P3_COST_SIZE0_TOOK+=("$(record_ratio "$summary" bool_size0 bool took p95)") || die "ratio C/B absent: $summary bool_size0/bool/took/p95"
done

telemetry_value(){
  local run="$1" path="$2" score telemetry
  score="$CAMPAIGN/runs/$run/surch.json"
  telemetry=$(jq -er '.p2.telemetry_jsonl | strings' "$score") || return 1
  jq -ser --arg path "$path" '
    first(.[] | select(.phase == "index_ready" and .boundary == "snapshot") | getpath($path | split(".")))
    | if type == "number" then . else error("valeur index_ready absente") end
  ' "$telemetry"
}

match_control_derivatives(){
  local run="$1" score telemetry
  score="$CAMPAIGN/runs/$run/surch.json"
  telemetry=$(jq -er '.p2.telemetry_jsonl | strings' "$score") || return 1
  jq -ser '
    (first(.[] | select(.phase == "match_control" and .boundary == "before"))) as $before
    | (first(.[] | select(.phase == "match_control" and .boundary == "after"))) as $after
    | if ($before == null or $after == null) then error("bornes match_control absentes") else . end
    | ([$after.cgroup.io_stat_delta_from_before[] | select(.metric == "rbytes") | .delta]) as $reads
    | if ($reads | length) == 0 then error("lectures disque match_control absentes") else . end
    | {
        refault_file_delta: ($after.cgroup.memory_stat.workingset_refault_file - $before.cgroup.memory_stat.workingset_refault_file),
        disk_read_bytes: ($reads | add),
        memory_psi_some_total_delta: ($after.cgroup.memory_psi.some.total - $before.cgroup.memory_psi.some.total),
        io_psi_some_total_delta: ($after.cgroup.io_psi.some.total - $before.cgroup.io_psi.some.total)
      }
    | if ([.[]] | all(type == "number")) then . else error("dérivé match_control non numérique") end
  ' "$telemetry"
}

recovery_ratio(){
  local a="$1" b="$2" c="$3" mode="$4"
  awk -v a="$a" -v b="$b" -v c="$c" -v mode="$mode" '
    BEGIN {
      if (mode == "rss") { denominator = b - a; numerator = b - c }
      else if (mode == "file") { denominator = a - b; numerator = c - b }
      else exit 1
      if (denominator <= 0) exit 1
      printf "%.12g", numerator / denominator
    }
  '
}

COMPACTION=()
RSS_RECOVERY=()
RSS_ANON_RECOVERY=()
FILE_RECOVERY=()
P3_MEMORY_DERIVATIVES=()
for index in 0 1 2; do
  a_run="${PRIMARY_A_RUNS[$index]}"
  c_run="${PRIMARY_C_RUNS[$index]}"
  pair="${PRIMARY_PAIRS[$index]}"
  b_run="${BASELINE_B_FOR_A[$a_run]:-}"
  [ -n "$b_run" ] && [ "${COST_B_FOR_C[$c_run]:-}" = "$b_run" ] \
    || die "triplet P3 incohérent pour $pair (A=$a_run, B=$b_run, C=$c_run)"
  a_directory=$(telemetry_value "$a_run" metrics.index.postings_directory_bytes) || die "directory_bytes A absent: $a_run"
  b_directory=$(telemetry_value "$b_run" metrics.index.postings_directory_bytes) || die "directory_bytes B absent: $b_run"
  c_directory=$(telemetry_value "$c_run" metrics.index.postings_directory_bytes) || die "directory_bytes C absent: $c_run"
  compaction=$(awk -v b="$b_directory" -v c="$c_directory" 'BEGIN { if (b <= 0) exit 1; printf "%.12g", c / b }') || die "compaction C/B indéfinie: $pair"
  a_rss=$(telemetry_value "$a_run" process.rss_bytes) || die "RSS A absent: $a_run"
  b_rss=$(telemetry_value "$b_run" process.rss_bytes) || die "RSS B absent: $b_run"
  c_rss=$(telemetry_value "$c_run" process.rss_bytes) || die "RSS C absent: $c_run"
  a_anon=$(telemetry_value "$a_run" process.rss_anon_bytes) || die "RssAnon A absent: $a_run"
  b_anon=$(telemetry_value "$b_run" process.rss_anon_bytes) || die "RssAnon B absent: $b_run"
  c_anon=$(telemetry_value "$c_run" process.rss_anon_bytes) || die "RssAnon C absent: $c_run"
  a_file=$(telemetry_value "$a_run" cgroup.memory_stat.file) || die "cache fichier A absent: $a_run"
  b_file=$(telemetry_value "$b_run" cgroup.memory_stat.file) || die "cache fichier B absent: $b_run"
  c_file=$(telemetry_value "$c_run" cgroup.memory_stat.file) || die "cache fichier C absent: $c_run"
  rss_recovery=$(recovery_ratio "$a_rss" "$b_rss" "$c_rss" rss) || die "récupération RSS indéfinie: $pair"
  anon_recovery=$(recovery_ratio "$a_anon" "$b_anon" "$c_anon" rss) || die "récupération RssAnon indéfinie: $pair"
  file_recovery=$(recovery_ratio "$a_file" "$b_file" "$c_file" file) || die "récupération cache fichier indéfinie: $pair"
  a_match=$(match_control_derivatives "$a_run") || die "dérivés match_control A absents: $a_run"
  b_match=$(match_control_derivatives "$b_run") || die "dérivés match_control B absents: $b_run"
  c_match=$(match_control_derivatives "$c_run") || die "dérivés match_control C absents: $c_run"
  COMPACTION+=("$compaction")
  RSS_RECOVERY+=("$rss_recovery")
  RSS_ANON_RECOVERY+=("$anon_recovery")
  FILE_RECOVERY+=("$file_recovery")
  P3_MEMORY_DERIVATIVES+=("$(jq -cn --arg pair "$pair" --arg a_run "$a_run" --arg b_run "$b_run" --arg c_run "$c_run" \
    --argjson compaction "$compaction" --argjson rss_recovery "$rss_recovery" --argjson rss_anon_recovery "$anon_recovery" --argjson file_recovery "$file_recovery" \
    --argjson a_match "$a_match" --argjson b_match "$b_match" --argjson c_match "$c_match" \
    '{pair:$pair,runs:{A:$a_run,B:$b_run,C:$c_run},compaction_directory_c_over_b:$compaction,recovery:{rss:$rss_recovery,rss_anon:$rss_anon_recovery,file:$file_recovery},match_control:{A:$a_match,B:$b_match,C:$c_match}}')")
done

MEDIAN_CORE95=$(median_three "${CORE_TOOK95[@]}")
MEDIAN_CORE99=$(median_three "${CORE_TOOK99[@]}")
MEDIAN_PRODUCT_TOOK=$(median_three "${PRODUCT_TOOK[@]}")
MEDIAN_PRODUCT_CLIENT=$(median_three "${PRODUCT_CLIENT[@]}")
MEDIAN_RANDOM_MATCH_CLIENT=$(median_three "${RANDOM_MATCH_CLIENT[@]}")
MEDIAN_RSS_RECOVERY=$(median_three "${RSS_RECOVERY[@]}")
MEDIAN_RSS_ANON_RECOVERY=$(median_three "${RSS_ANON_RECOVERY[@]}")
MEDIAN_FILE_RECOVERY=$(median_three "${FILE_RECOVERY[@]}")
VALIDITIES_JSON=$(printf '%s\n' "${VALIDITIES[@]}" | jq -Rsc 'split("\n") | map(select(length > 0) == "true")')
BASELINE_VALIDITIES_JSON=$(printf '%s\n' "${BASELINE_VALIDITIES[@]}" | jq -Rsc 'split("\n") | map(select(length > 0) == "true")')
COST_VALIDITIES_JSON=$(printf '%s\n' "${COST_VALIDITIES[@]}" | jq -Rsc 'split("\n") | map(select(length > 0) == "true")')
BLOCK_RATIO_JSON=$(printf '%s\n' "${BLOCK_RATIO_DIAGNOSTICS[@]}" | jq -sc '.')
BLOCK_RATIO_C_JSON=$(printf '%s\n' "${BLOCK_RATIO_C_DIAGNOSTICS[@]}" | jq -sc '.')
P3_INTEGRITY_JSON=$(printf '%s\n' "${P3_INTEGRITY_DIAGNOSTICS[@]}" | jq -sc '.')
P3_COST_TOOK_JSON=$(numbers_json "${P3_COST_TOOK[@]}")
P3_COST_SIZE0_TOOK_JSON=$(numbers_json "${P3_COST_SIZE0_TOOK[@]}")
COMPACTION_JSON=$(numbers_json "${COMPACTION[@]}")
RSS_RECOVERY_JSON=$(numbers_json "${RSS_RECOVERY[@]}")
RSS_ANON_RECOVERY_JSON=$(numbers_json "${RSS_ANON_RECOVERY[@]}")
FILE_RECOVERY_JSON=$(numbers_json "${FILE_RECOVERY[@]}")
P3_MEMORY_DERIVATIVES_JSON=$(printf '%s\n' "${P3_MEMORY_DERIVATIVES[@]}" | jq -sc '.')
CHECKS_JSONL=$(mktemp "${TMPDIR:-/tmp}/surch-p2-gate.XXXXXX") || die 'mktemp impossible'
trap 'rm -f -- "$CHECKS_JSONL"' EXIT
ALL_PASSED=true

add_check(){
  local name="$1" passed="$2" detail="$3"
  jq -n --arg name "$name" --arg detail "$detail" --argjson passed "$passed" '{name:$name, pass:$passed, detail:$detail}' >> "$CHECKS_JSONL"
  [ "$passed" = true ] || ALL_PASSED=false
}

all_valid=true
for valid in "${VALIDITIES[@]}"; do [ "$valid" = true ] || all_valid=false; done
add_check 'validité C/A route/parité/count/segments' "$all_valid" "paires valides: $VALIDITIES_JSON"
baseline_valid=true
for valid in "${BASELINE_VALIDITIES[@]}"; do [ "$valid" = true ] || baseline_valid=false; done
add_check 'validité A/B route/parité/count/segments' "$baseline_valid" "paires valides: $BASELINE_VALIDITIES_JSON"
cost_valid=true
for valid in "${COST_VALIDITIES[@]}"; do [ "$valid" = true ] || cost_valid=false; done
add_check 'validité C/B route/parité/count/segments' "$cost_valid" "paires valides: $COST_VALIDITIES_JSON"
number_le "$MEDIAN_CORE95" 0.50 && pass=true || pass=false
add_check 'bool size:0 p95 took' "$pass" "médiane=$(printf '%.4f' "$MEDIAN_CORE95"), cible <= 0.50"
number_le "$MEDIAN_CORE99" 0.70 && pass=true || pass=false
add_check 'bool size:0 p99 took' "$pass" "médiane=$(printf '%.4f' "$MEDIAN_CORE99"), cible <= 0.70"
number_le "$MEDIAN_PRODUCT_TOOK" 0.70 && pass=true || pass=false
add_check 'bool size:10 p95 took' "$pass" "médiane=$(printf '%.4f' "$MEDIAN_PRODUCT_TOOK"), cible <= 0.70"
number_le "$MEDIAN_PRODUCT_CLIENT" 0.70 && pass=true || pass=false
add_check 'bool size:10 p95 client' "$pass" "médiane=$(printf '%.4f' "$MEDIAN_PRODUCT_CLIENT"), cible <= 0.70"
numbers_all_le 0.80 "${PRODUCT_TOOK[@]}" && pass=true || pass=false
add_check 'trois paires bool size:10 <= 0.80' "$pass" "ratios=$(numbers_json "${PRODUCT_TOOK[@]}")"
# La borne haute est stricte dans le protocole : 0,90 lui-même échoue.
strict_upper=true
for value in "${BOOTSTRAP_UPPER[@]}"; do
  awk -v value="$value" 'BEGIN { exit !((value + 0) < .90) }' || strict_upper=false
done
add_check 'IC95 bootstrap primaire' "$strict_upper" "bornes supérieures=$(numbers_json "${BOOTSTRAP_UPPER[@]}"), cible < 0.90"
REPLAY_REQUIRED=false
numbers_all_ge 0.95 "${FIXED_MATCH[@]}" && pass=true || { pass=false; REPLAY_REQUIRED=true; }
add_check 'témoin fixed match : comparabilité >= 0.95' "$pass" "ratios=$(numbers_json "${FIXED_MATCH[@]}")"
numbers_all_le 1.05 "${FIXED_MATCH[@]}" && pass=true || pass=false
add_check 'témoin fixed match : absence de régression <= 1.05' "$pass" "ratios=$(numbers_json "${FIXED_MATCH[@]}")"
numbers_all_ge 0.95 "${RANDOM_MATCH[@]}" && pass=true || { pass=false; REPLAY_REQUIRED=true; }
add_check 'témoin match autonome : comparabilité >= 0.95' "$pass" "ratios=$(numbers_json "${RANDOM_MATCH[@]}")"
numbers_all_le 1.05 "${RANDOM_MATCH[@]}" && pass=true || pass=false
add_check 'témoin match autonome : took <= 1.05' "$pass" "ratios=$(numbers_json "${RANDOM_MATCH[@]}")"
number_le "$MEDIAN_RANDOM_MATCH_CLIENT" 1.05 && pass=true || pass=false
add_check 'témoin match autonome : médiane p95 client <= 1.05' "$pass" "médiane=$(printf '%.4f' "$MEDIAN_RANDOM_MATCH_CLIENT"), ratios=$(numbers_json "${RANDOM_MATCH_CLIENT[@]}")"
numbers_all_le 1.10 "${RANDOM_MATCH_CLIENT[@]}" && pass=true || pass=false
add_check 'témoin match autonome : aucune p95 client > 1.10' "$pass" "ratios=$(numbers_json "${RANDOM_MATCH_CLIENT[@]}")"
numbers_all_le 2.0 "${PROBE_DELTA[@]}" && pass=true || pass=false
add_check 'écart sonde p95' "$pass" "écarts ms=$(numbers_json "${PROBE_DELTA[@]}"), cible <= 2"
[ "${#BLOCK_RATIO_DIAGNOSTICS[@]}" -eq 9 ] && jq -e 'all(.[]; (.ratio | type) == "number" and .ratio <= 0.25 and .target == 0.25 and .pass == true)' <<< "$BLOCK_RATIO_JSON" >/dev/null && pass=true || pass=false
add_check 'ratio de blocs B (résultat)' "$pass" "observations=$BLOCK_RATIO_JSON, cible <= 0.25"
[ "${#BLOCK_RATIO_C_DIAGNOSTICS[@]}" -eq 9 ] && jq -e 'all(.[]; (.ratio | type) == "number" and .ratio <= 0.25 and .target == 0.25 and .pass == true)' <<< "$BLOCK_RATIO_C_JSON" >/dev/null && pass=true || pass=false
add_check 'ratio de blocs C (résultat)' "$pass" "observations=$BLOCK_RATIO_C_JSON, cible <= 0.25"
MEDIAN_P3_COST_TOOK=$(median_three "${P3_COST_TOOK[@]}")
MEDIAN_P3_COST_SIZE0_TOOK=$(median_three "${P3_COST_SIZE0_TOOK[@]}")
number_le "$MEDIAN_P3_COST_TOOK" 1.05 && pass=true || pass=false
add_check 'coût P3 C/B bool size:10 : médiane p95 took <= 1.05' "$pass" "médiane=$(printf '%.4f' "$MEDIAN_P3_COST_TOOK"), ratios=$P3_COST_TOOK_JSON"
numbers_all_le 1.10 "${P3_COST_TOOK[@]}" && pass=true || pass=false
add_check 'coût P3 C/B bool size:10 : aucune répétition > 1.10' "$pass" "ratios=$P3_COST_TOOK_JSON"
number_le "$MEDIAN_P3_COST_SIZE0_TOOK" 1.05 && pass=true || pass=false
add_check 'coût P3 C/B bool size:0 : médiane p95 took <= 1.05' "$pass" "médiane=$(printf '%.4f' "$MEDIAN_P3_COST_SIZE0_TOOK"), ratios=$P3_COST_SIZE0_TOOK_JSON"
numbers_all_le 1.10 "${P3_COST_SIZE0_TOOK[@]}" && pass=true || pass=false
add_check 'coût P3 C/B bool size:0 : aucune répétition > 1.10' "$pass" "ratios=$P3_COST_SIZE0_TOOK_JSON"
[ "${#P3_INTEGRITY_DIAGNOSTICS[@]}" -eq 18 ] \
  && jq -e 'all(.[]; (.before_bytes | type) == "number" and (.bytes | type) == "number" and (.hash_failures | type) == "number" and (.fallbacks | type) == "number" and (.fallback_fields | type) == "number" and (.before_bytes > 0) and (.bytes > 0) and (.before_bytes <= 33554432) and (.bytes <= 33554432) and (.hash_failures == 0) and (.fallbacks == 0) and (.fallback_fields == 0))' <<< "$P3_INTEGRITY_JSON" >/dev/null \
  && pass=true || pass=false
add_check 'intégrité P3 technique' "$pass" "observations=$P3_INTEGRITY_JSON, plafond <= 33554432, hash/fallback = 0"
[ "${#P3_INTEGRITY_DIAGNOSTICS[@]}" -eq 18 ] \
  && jq -e --argjson target "$P3_INTEGRITY_TARGET_BYTES" 'all(.[]; (.before_bytes | type) == "number" and (.bytes | type) == "number" and (.before_bytes > 0) and (.bytes > 0) and (.before_bytes <= $target) and (.bytes <= $target))' <<< "$P3_INTEGRITY_JSON" >/dev/null \
  && pass=true || pass=false
add_check 'cible intégrité P3' "$pass" "observations=$P3_INTEGRITY_JSON, cible <= $P3_INTEGRITY_TARGET_BYTES (17 Mio)"
[ "${#P3_MEMORY_DERIVATIVES[@]}" -eq 3 ] \
  && jq -e 'all(.[]; (.compaction_directory_c_over_b | type) == "number" and (.recovery.rss | type) == "number" and (.recovery.rss_anon | type) == "number" and (.recovery.file | type) == "number" and ([.match_control.A,.match_control.B,.match_control.C] | all(type == "object")))' <<< "$P3_MEMORY_DERIVATIVES_JSON" >/dev/null \
  && pass=true || pass=false
add_check 'télémétrie P3 et dérivés match_control' "$pass" "triplets=$P3_MEMORY_DERIVATIVES_JSON"
numbers_all_le 0.0100 "${COMPACTION[@]}" && pass=true || pass=false
add_check 'compaction directory_bytes C/B' "$pass" "ratios=$COMPACTION_JSON, cible <= 0.0100"
number_le 0.90 "$MEDIAN_RSS_RECOVERY" && pass=true || pass=false
add_check 'récupération RSS médiane' "$pass" "médiane=$(printf '%.4f' "$MEDIAN_RSS_RECOVERY"), ratios=$RSS_RECOVERY_JSON, cible >= 0.90"
numbers_all_ge 0.80 "${RSS_RECOVERY[@]}" && pass=true || pass=false
add_check 'récupération RSS : aucune répétition < 0.80' "$pass" "ratios=$RSS_RECOVERY_JSON"
number_le 0.90 "$MEDIAN_RSS_ANON_RECOVERY" && pass=true || pass=false
add_check 'récupération RssAnon médiane' "$pass" "médiane=$(printf '%.4f' "$MEDIAN_RSS_ANON_RECOVERY"), ratios=$RSS_ANON_RECOVERY_JSON, cible >= 0.90"
numbers_all_ge 0.80 "${RSS_ANON_RECOVERY[@]}" && pass=true || pass=false
add_check 'récupération RssAnon : aucune répétition < 0.80' "$pass" "ratios=$RSS_ANON_RECOVERY_JSON"
number_le 0.90 "$MEDIAN_FILE_RECOVERY" && pass=true || pass=false
add_check 'récupération cache fichier médiane' "$pass" "médiane=$(printf '%.4f' "$MEDIAN_FILE_RECOVERY"), ratios=$FILE_RECOVERY_JSON, cible >= 0.90"
numbers_all_ge 0.80 "${FILE_RECOVERY[@]}" && pass=true || pass=false
add_check 'récupération cache fichier : aucune répétition < 0.80' "$pass" "ratios=$FILE_RECOVERY_JSON"

all_measurements_valid=true
[ "$all_valid" = true ] && [ "$baseline_valid" = true ] && [ "$cost_valid" = true ] || all_measurements_valid=false
if [ "$all_measurements_valid" != true ]; then
  VERDICT='INVALIDE P3'
elif [ "$REPLAY_REQUIRED" = true ]; then
  VERDICT='REJOUER P3'
elif [ "$ALL_PASSED" = true ]; then
  VERDICT='PASS P3'
else
  VERDICT='ÉCHEC P3'
fi

CHECKS_JSON=$(jq -s . "$CHECKS_JSONL")
jq -n \
  --arg verdict "$VERDICT" --argjson pair_directories "$(jq -Rn --args '$ARGS.positional' -- "${PAIR_DIRS[@]}")" \
  --argjson product_took "$(numbers_json "${PRODUCT_TOOK[@]}")" --argjson product_client "$(numbers_json "${PRODUCT_CLIENT[@]}")" \
  --argjson core95 "$(numbers_json "${CORE_TOOK95[@]}")" --argjson core99 "$(numbers_json "${CORE_TOOK99[@]}")" \
  --argjson fixed_match "$(numbers_json "${FIXED_MATCH[@]}")" --argjson random_match "$(numbers_json "${RANDOM_MATCH[@]}")" --argjson random_match_client "$(numbers_json "${RANDOM_MATCH_CLIENT[@]}")" \
  --argjson probe_delta "$(numbers_json "${PROBE_DELTA[@]}")" --argjson bootstrap_upper "$(numbers_json "${BOOTSTRAP_UPPER[@]}")" \
  --argjson median_product_took "$MEDIAN_PRODUCT_TOOK" --argjson median_product_client "$MEDIAN_PRODUCT_CLIENT" --argjson median_random_match_client "$MEDIAN_RANDOM_MATCH_CLIENT" \
  --argjson median_core95 "$MEDIAN_CORE95" --argjson median_core99 "$MEDIAN_CORE99" --argjson checks "$CHECKS_JSON" --argjson blocks_ratio_diagnostics "$BLOCK_RATIO_JSON" --argjson blocks_ratio_c_diagnostics "$BLOCK_RATIO_C_JSON" --argjson p3_integrity_diagnostics "$P3_INTEGRITY_JSON" --argjson p3_integrity_target "$P3_INTEGRITY_TARGET_BYTES" --argjson baseline_pair_directories "$(jq -Rn --args '$ARGS.positional' -- "${BASELINE_PAIR_DIRS[@]}")" --argjson p3_cost_took "$P3_COST_TOOK_JSON" --argjson p3_cost_size0_took "$P3_COST_SIZE0_TOOK_JSON" --argjson p3_cost_pair_directories "$(jq -Rn --args '$ARGS.positional' -- "${COST_PAIR_DIRS[@]}")" --argjson memory_derivatives "$P3_MEMORY_DERIVATIVES_JSON" --argjson compaction "$COMPACTION_JSON" --argjson rss_recovery "$RSS_RECOVERY_JSON" --argjson rss_anon_recovery "$RSS_ANON_RECOVERY_JSON" --argjson file_recovery "$FILE_RECOVERY_JSON" --argjson replay_required "$REPLAY_REQUIRED" \
  '{schema:"surch.bench.p3.campaign.v1", verdict:$verdict, replay_required:$replay_required,pair_directories:$pair_directories,baseline_pair_directories:$baseline_pair_directories,ratios:{bool_size10_took_p95:$product_took,bool_size10_client_p95:$product_client,bool_size0_took_p95:$core95,bool_size0_took_p99:$core99,fixed_martin_match_took_p95:$fixed_match,match_control_took_p95:$random_match,match_control_client_p95:$random_match_client,probe_p95_delta_ms:$probe_delta,bootstrap_primary_p95_took_ci95_upper:$bootstrap_upper},medians:{bool_size10_took_p95:$median_product_took,bool_size10_client_p95:$median_product_client,bool_size0_took_p95:$median_core95,bool_size0_took_p99:$median_core99,match_control_client_p95:$median_random_match_client},blocks_ratio:{target:0.25,B:$blocks_ratio_diagnostics,C:$blocks_ratio_c_diagnostics},p3_integrity:{target_bytes:$p3_integrity_target,observations:$p3_integrity_diagnostics,c_over_b_bool_size10_took_p95:$p3_cost_took,c_over_b_bool_size0_took_p95:$p3_cost_size0_took,pair_directories:$p3_cost_pair_directories},memory:{derivatives:$memory_derivatives,compaction_directory_c_over_b:$compaction,recovery:{rss:$rss_recovery,rss_anon:$rss_anon_recovery,file:$file_recovery}},checks:$checks}' \
  > "$CAMPAIGN/campaign-summary.json" || die 'écriture impossible de campaign-summary.json'

{
  printf '%s\n\n' '# P3 — verdict de campagne'
  printf 'Verdict: **%s**.\n\n' "$VERDICT"
  printf '%s\n%s\n' '| Gate | Verdict | Détail |' '|---|---|---|'
  jq -r '"| \(.name) | \(if .pass then "pass" else "fail" end) | \(.detail) |"' "$CHECKS_JSONL"
  printf '\n%s\n' 'Les nombres A/B, C/A et les coûts C/B sont conservés sous `pairs/*/`, `p3-primary-pairs/*/` et `p3-cost-pairs/*/`.'
} > "$CAMPAIGN/README.md" || die 'écriture impossible de README.md'

# Le statut du processus est le contrat d'automatisation : seul PASS P3 peut
# être vert. ÉCHEC, INVALIDE et REJOUER doivent arrêter le pilote et la CI.
[ "$VERDICT" = 'PASS P3' ]
