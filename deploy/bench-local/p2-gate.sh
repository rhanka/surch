#!/usr/bin/env bash
# p2-gate.sh — applique les gates P2 pré-engagées après trois paires valides.
# Les rapports restent JSON pour le pilote, mais les décisions sont prises en
# Bash et awk : aucun interpréteur Python n'est requis par le harnais local.
set -euo pipefail
export LC_ALL=C

CAMPAIGN=""
P2_REQUIRE_P3_INTEGRITY="${P2_REQUIRE_P3_INTEGRITY:-1}"

usage(){ printf 'usage: p2-gate.sh --campaign RÉPERTOIRE\n' >&2; }
die(){ printf '[p2-gate] %s\n' "$*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --campaign) CAMPAIGN=${2:-}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage; die "option inconnue: $1" ;;
  esac
done
[ -n "$CAMPAIGN" ] || { usage; die '--campaign est obligatoire'; }
[ -d "$CAMPAIGN" ] || die "campagne introuvable: $CAMPAIGN"
command -v jq >/dev/null 2>&1 || die 'commande requise absente: jq'
case "$P2_REQUIRE_P3_INTEGRITY" in 0|1) ;; *) die 'P2_REQUIRE_P3_INTEGRITY doit valoir 0 ou 1';; esac

phase_status_valid(){
  local run_dir="$1" require_p3="$2" score status
  score="$run_dir/surch.json"
  jq -e '.measurement_valid == true and .p2.hot_phase_records == 4 and (.p2.phase_records >= 4 and .p2.phase_records <= 5)' "$score" >/dev/null 2>&1 || return 1
  status=$(jq -er '.p2.phase_status_jsonl | strings' "$score" 2>/dev/null) || return 1
  [ -f "$status" ] || return 1
  # Les quatre phases chaudes portent la preuve A/B. Cold reste disponible
  # dans les scorecards, mais ses droits de reclaim ne peuvent pas invalider
  # le routage, les corps ou la parité déjà vérifiés sur ces quatre phases.
  jq -se --argjson require_p3 "$require_p3" '
    ([.[] | select(.phase == "warm" or .phase == "fixed" or .phase == "random" or .phase == "no_source")]) as $hot
    | ($hot | length == 4)
    and ([ $hot[] | .phase ] | sort == ["fixed", "no_source", "random", "warm"])
    and all($hot[]; .valid == true and .cpu_steal_within_limit == true)
    and (if $require_p3 then
      all($hot[];
        .integrity.required == true
        and (.integrity.bytes.before | type) == "number"
        and (.integrity.bytes.after | type) == "number"
        and (.integrity.hash_failures.before | type) == "number"
        and (.integrity.hash_failures.after | type) == "number"
        and (.integrity.fallback_fields.before | type) == "number"
        and (.integrity.fallback_fields.after | type) == "number"
        and .integrity.bytes.before <= 33554432
        and .integrity.bytes.after <= 33554432
        and .integrity.hash_failures.before == 0
        and .integrity.hash_failures.after == 0
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
    (first(.records[] | select(.phase == "random" and .kind == "bool" and .metric == "probe") | .b.p95)) as $b
    | (first(.records[] | select(.phase == "random" and .kind == "bool" and .metric == "probe") | .a.p95)) as $a
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
numbers_json(){ printf '%s\n' "$@" | jq -Rsc 'split("\n") | map(select(length > 0) | tonumber)'; }

BASELINE_PAIR_SUMMARIES=("$CAMPAIGN"/pairs/*/pair-summary.json)
[ -e "${BASELINE_PAIR_SUMMARIES[0]}" ] || BASELINE_PAIR_SUMMARIES=()
[ "${#BASELINE_PAIR_SUMMARIES[@]}" -eq 3 ] || die "P2 exige exactement trois paires A/B, trouvé ${#BASELINE_PAIR_SUMMARIES[@]}"
PAIR_SUMMARIES=("$CAMPAIGN"/p3-primary-pairs/*/pair-summary.json)
[ -e "${PAIR_SUMMARIES[0]}" ] || PAIR_SUMMARIES=()
[ "${#PAIR_SUMMARIES[@]}" -eq 3 ] || die "P3 exige exactement trois paires C/A, trouvé ${#PAIR_SUMMARIES[@]}"
COST_PAIR_SUMMARIES=("$CAMPAIGN"/p3-cost-pairs/*/pair-summary.json)
[ -e "${COST_PAIR_SUMMARIES[0]}" ] || COST_PAIR_SUMMARIES=()
[ "${#COST_PAIR_SUMMARIES[@]}" -eq 3 ] || die "P3 exige exactement trois paires C/B, trouvé ${#COST_PAIR_SUMMARIES[@]}"

VALIDITIES=()
PRODUCT_TOOK=()
PRODUCT_CLIENT=()
CORE_TOOK95=()
CORE_TOOK99=()
FIXED_MATCH=()
RANDOM_MATCH=()
PROBE_DELTA=()
BOOTSTRAP_UPPER=()
PAIR_DIRS=()
BLOCK_RATIO_DIAGNOSTICS=()
P3_INTEGRITY_DIAGNOSTICS=()
BASELINE_VALIDITIES=()
BASELINE_PAIR_DIRS=()
COST_VALIDITIES=()
COST_PAIR_DIRS=()
P3_COST_TOOK=()

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
  b_status=$(jq -er '.p2.phase_status_jsonl | strings' "$CAMPAIGN/runs/$b_run/surch.json") || die "statut P2 B absent: $b_run"
  [ -f "$b_status" ] || die "statut P2 B illisible: $b_status"
  while IFS= read -r diagnostic; do
    [ -n "$diagnostic" ] && BLOCK_RATIO_DIAGNOSTICS+=("$diagnostic")
  done < <(jq -c --arg pair "$pair" --arg run "$b_run" '
    select(.variant == "B" and .bool_requests > 0 and (.phase == "warm" or .phase == "random" or .phase == "no_source"))
    | {pair:$pair,run:$run,phase:$phase,ratio:.blocks_read_over_total,target:.blocks_ratio_target,pass:.blocks_ratio_target_pass,verdict:.blocks_ratio_verdict}
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
  PRODUCT_TOOK+=("$(record_ratio "$summary" random bool took p95)") || die "ratio C/A absent: $summary random/bool/took/p95"
  PRODUCT_CLIENT+=("$(record_ratio "$summary" random bool client p95)") || die "ratio C/A absent: $summary random/bool/client/p95"
  CORE_TOOK95+=("$(record_ratio "$summary" no_source bool took p95)") || die "ratio C/A absent: $summary no_source/bool/took/p95"
  CORE_TOOK99+=("$(record_ratio "$summary" no_source bool took p99)") || die "ratio C/A absent: $summary no_source/bool/took/p99"
  FIXED_MATCH+=("$(record_ratio "$summary" fixed match took p95)") || die "ratio C/A absent: $summary fixed/match/took/p95"
  RANDOM_MATCH+=("$(record_ratio "$summary" random match took p95)") || die "ratio C/A absent: $summary random/match/took/p95"
  PROBE_DELTA+=("$(record_probe_delta "$summary")") || die "sonde C/A absente: $summary"
  BOOTSTRAP_UPPER+=("$(jq -er '.primary_bootstrap.ci95_high | tonumber' "$summary")") || die "IC95 C/A absent: $summary"
  if [ "$P2_REQUIRE_P3_INTEGRITY" = "1" ]; then
    c_status=$(jq -er '.p2.phase_status_jsonl | strings' "$CAMPAIGN/runs/$c_run/surch.json") || die "statut P3 C absent: $c_run"
    [ -f "$c_status" ] || die "statut P3 C illisible: $c_status"
    while IFS= read -r diagnostic; do
      [ -n "$diagnostic" ] && P3_INTEGRITY_DIAGNOSTICS+=("$diagnostic")
    done < <(jq -c --arg pair "$pair" --arg run "$c_run" '
      select(.variant == "C" and (.phase == "warm" or .phase == "fixed" or .phase == "random" or .phase == "no_source"))
      | {pair:$pair,run:$run,phase:$phase,bytes:.integrity.bytes.after,hash_failures:.integrity.hash_failures.after,fallback_fields:.integrity.fallback_fields.after,verified_bytes:.integrity.verified_bytes.after}
    ' "$c_status")
  fi
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
  P3_COST_TOOK+=("$(record_ratio "$summary" random bool took p95)") || die "ratio C/B absent: $summary random/bool/took/p95"
done

MEDIAN_CORE95=$(median_three "${CORE_TOOK95[@]}")
MEDIAN_CORE99=$(median_three "${CORE_TOOK99[@]}")
MEDIAN_PRODUCT_TOOK=$(median_three "${PRODUCT_TOOK[@]}")
MEDIAN_PRODUCT_CLIENT=$(median_three "${PRODUCT_CLIENT[@]}")
VALIDITIES_JSON=$(printf '%s\n' "${VALIDITIES[@]}" | jq -Rsc 'split("\n") | map(select(length > 0) == "true")')
BASELINE_VALIDITIES_JSON=$(printf '%s\n' "${BASELINE_VALIDITIES[@]}" | jq -Rsc 'split("\n") | map(select(length > 0) == "true")')
COST_VALIDITIES_JSON=$(printf '%s\n' "${COST_VALIDITIES[@]}" | jq -Rsc 'split("\n") | map(select(length > 0) == "true")')
BLOCK_RATIO_JSON=$(printf '%s\n' "${BLOCK_RATIO_DIAGNOSTICS[@]}" | jq -sc '.')
P3_INTEGRITY_JSON=$(printf '%s\n' "${P3_INTEGRITY_DIAGNOSTICS[@]}" | jq -sc '.')
P3_COST_TOOK_JSON=$(numbers_json "${P3_COST_TOOK[@]}")
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
add_check 'noyau size:0 bool p95' "$pass" "médiane=$(printf '%.4f' "$MEDIAN_CORE95"), cible <= 0.50"
number_le "$MEDIAN_CORE99" 0.70 && pass=true || pass=false
add_check 'noyau size:0 bool p99' "$pass" "médiane=$(printf '%.4f' "$MEDIAN_CORE99"), cible <= 0.70"
number_le "$MEDIAN_PRODUCT_TOOK" 0.70 && pass=true || pass=false
add_check 'produit size:10 bool p95 took' "$pass" "médiane=$(printf '%.4f' "$MEDIAN_PRODUCT_TOOK"), cible <= 0.70"
number_le "$MEDIAN_PRODUCT_CLIENT" 0.70 && pass=true || pass=false
add_check 'produit size:10 bool p95 client' "$pass" "médiane=$(printf '%.4f' "$MEDIAN_PRODUCT_CLIENT"), cible <= 0.70"
numbers_all_le 0.80 "${PRODUCT_TOOK[@]}" && pass=true || pass=false
add_check 'trois paires produit même sens <= 0.80' "$pass" "ratios=$(numbers_json "${PRODUCT_TOOK[@]}")"
# La borne haute est stricte dans le protocole : 0,90 lui-même échoue.
strict_upper=true
for value in "${BOOTSTRAP_UPPER[@]}"; do
  awk -v value="$value" 'BEGIN { exit !((value + 0) < .90) }' || strict_upper=false
done
add_check 'IC95 bootstrap primaire' "$strict_upper" "bornes supérieures=$(numbers_json "${BOOTSTRAP_UPPER[@]}"), cible < 0.90"
numbers_all_between 0.95 1.05 "${FIXED_MATCH[@]}" && pass=true || pass=false
add_check 'témoin fixed match' "$pass" "ratios=$(numbers_json "${FIXED_MATCH[@]}")"
numbers_all_between 0.95 1.05 "${RANDOM_MATCH[@]}" && pass=true || pass=false
add_check 'témoin random match' "$pass" "ratios=$(numbers_json "${RANDOM_MATCH[@]}")"
numbers_all_le 2.0 "${PROBE_DELTA[@]}" && pass=true || pass=false
add_check 'écart sonde p95' "$pass" "écarts ms=$(numbers_json "${PROBE_DELTA[@]}"), cible <= 2"
# Le ratio indique si P2 a effectivement évité les lectures attendues. Il ne
# protège pas la comparabilité (déjà couverte par validité/parité) : un écart
# produit un ÉCHEC P2 lisible, jamais une campagne techniquement invalide.
[ "${#BLOCK_RATIO_DIAGNOSTICS[@]}" -eq 9 ] && jq -e 'all(.[]; .pass == true)' <<< "$BLOCK_RATIO_JSON" >/dev/null && pass=true || pass=false
add_check 'ratio de blocs P2 (résultat)' "$pass" "observations=$BLOCK_RATIO_JSON, cible <= 0.25"
numbers_all_le 1.05 "${P3_COST_TOOK[@]}" && pass=true || pass=false
add_check 'coût P3 C/B bool size:10 p95 took' "$pass" "ratios=$P3_COST_TOOK_JSON, cible <= 1.05"
if [ "$P2_REQUIRE_P3_INTEGRITY" = "1" ]; then
  [ "${#P3_INTEGRITY_DIAGNOSTICS[@]}" -eq 12 ] \
    && jq -e 'all(.[]; (.bytes | type) == "number" and (.hash_failures | type) == "number" and (.fallback_fields | type) == "number" and (.bytes <= 33554432) and (.hash_failures == 0) and (.fallback_fields == 0))' <<< "$P3_INTEGRITY_JSON" >/dev/null \
    && pass=true || pass=false
  add_check 'intégrité P3 agrégée (résultat)' "$pass" "observations=$P3_INTEGRITY_JSON, plafond <= 33554432, hash/fallback = 0"
fi

all_measurements_valid=true
[ "$all_valid" = true ] && [ "$baseline_valid" = true ] && [ "$cost_valid" = true ] || all_measurements_valid=false
if [ "$ALL_PASSED" = true ]; then
  VERDICT='PASS P2'
elif [ "$all_measurements_valid" = true ]; then
  VERDICT='ÉCHEC P2'
else
  VERDICT='INVALIDE P2'
fi

CHECKS_JSON=$(jq -s . "$CHECKS_JSONL")
jq -n \
  --arg verdict "$VERDICT" --argjson pair_directories "$(jq -Rn --args '$ARGS.positional' -- "${PAIR_DIRS[@]}")" \
  --argjson product_took "$(numbers_json "${PRODUCT_TOOK[@]}")" --argjson product_client "$(numbers_json "${PRODUCT_CLIENT[@]}")" \
  --argjson core95 "$(numbers_json "${CORE_TOOK95[@]}")" --argjson core99 "$(numbers_json "${CORE_TOOK99[@]}")" \
  --argjson fixed_match "$(numbers_json "${FIXED_MATCH[@]}")" --argjson random_match "$(numbers_json "${RANDOM_MATCH[@]}")" \
  --argjson probe_delta "$(numbers_json "${PROBE_DELTA[@]}")" --argjson bootstrap_upper "$(numbers_json "${BOOTSTRAP_UPPER[@]}")" \
  --argjson median_product_took "$MEDIAN_PRODUCT_TOOK" --argjson median_product_client "$MEDIAN_PRODUCT_CLIENT" \
  --argjson median_core95 "$MEDIAN_CORE95" --argjson median_core99 "$MEDIAN_CORE99" --argjson checks "$CHECKS_JSON" --argjson blocks_ratio_diagnostics "$BLOCK_RATIO_JSON" --argjson p3_integrity_diagnostics "$P3_INTEGRITY_JSON" --argjson p3_integrity_required "$P2_REQUIRE_P3_INTEGRITY" --argjson baseline_pair_directories "$(jq -Rn --args '$ARGS.positional' -- "${BASELINE_PAIR_DIRS[@]}")" --argjson p3_cost_took "$P3_COST_TOOK_JSON" --argjson p3_cost_pair_directories "$(jq -Rn --args '$ARGS.positional' -- "${COST_PAIR_DIRS[@]}")" \
  '{schema:"surch.bench.p2.campaign.v1", verdict:$verdict, pair_directories:$pair_directories, baseline_pair_directories:$baseline_pair_directories, ratios:{product_random_bool_took_p95:$product_took, product_random_bool_client_p95:$product_client, core_no_source_bool_took_p95:$core95, core_no_source_bool_took_p99:$core99, fixed_match_took_p95:$fixed_match, random_match_took_p95:$random_match, probe_p95_delta_ms:$probe_delta, bootstrap_primary_p95_took_ci95_upper:$bootstrap_upper}, medians:{product_random_bool_took_p95:$median_product_took, product_random_bool_client_p95:$median_product_client, core_no_source_bool_took_p95:$median_core95, core_no_source_bool_took_p99:$median_core99}, blocks_ratio:{target:0.25,observations:$blocks_ratio_diagnostics}, p3_integrity:{required:($p3_integrity_required == 1),observations:$p3_integrity_diagnostics,c_over_b_random_bool_took_p95:$p3_cost_took,pair_directories:$p3_cost_pair_directories}, checks:$checks}' \
  > "$CAMPAIGN/campaign-summary.json" || die 'écriture impossible de campaign-summary.json'

{
  printf '%s\n\n' '# P2 — verdict de campagne'
  printf 'Verdict: **%s**.\n\n' "$VERDICT"
  printf '%s\n%s\n' '| Gate | Verdict | Détail |' '|---|---|---|'
  jq -r '"| \(.name) | \(if .pass then "pass" else "fail" end) | \(.detail) |"' "$CHECKS_JSONL"
  printf '\n%s\n' 'Les nombres A/B, C/A et les coûts C/B sont conservés sous `pairs/*/`, `p3-primary-pairs/*/` et `p3-cost-pairs/*/`.'
} > "$CAMPAIGN/README.md" || die 'écriture impossible de README.md'

# Une campagne causalement valide doit être livrée même si P2 échoue son
# objectif : son verdict et tous les ratios guident alors le prochain lot.
# Seule une invalidité de mesure conserve un code non nul pour le pilote.
[ "$all_measurements_valid" = true ]
