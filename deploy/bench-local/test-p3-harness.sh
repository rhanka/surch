#!/usr/bin/env bash
# Régressions unitaires sans Docker ni charge pour les garde-fous P3.
set -euo pipefail
export LC_ALL=C

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
HARNESS="$ROOT_DIR/deploy/bench-local/fair-ab.sh"
TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/surch-p3-harness.XXXXXX")
if [ "${KEEP_P3_FIXTURES:-0}" = 1 ]; then
  trap 'printf "test-p3-harness: fixtures conservées dans %s\n" "$TMP_DIR" >&2' EXIT
else
  trap 'rm -rf -- "$TMP_DIR"' EXIT
fi

fail(){ printf '[test-p3-harness] %s\n' "$*" >&2; exit 1; }

# Charge seulement les helpers purs : sourcer fair-ab.sh exécuterait un run.
awk '
  /^p2_validate_pairs\(\)\{/ { capture = 1 }
  /^p2_validate_body_files\(\)\{/ { capture = 0 }
  capture { print }
' "$HARNESS" > "$TMP_DIR/term-helpers.sh"
awk '
  /^p2_metric_present\(\)\{/ { capture = 1 }
  /^p2_proc_status_bytes\(\)\{/ { capture = 0 }
  capture { print }
' "$HARNESS" > "$TMP_DIR/metric-helpers.sh"
awk '
  /^p2_cgroup_io_json\(\)\{/ { capture = 1 }
  capture { print }
  capture && /^}$/ { exit }
' "$HARNESS" > "$TMP_DIR/io-helpers.sh"

# B1 : la forme cgroup v2 minimale doit être un JSON consommable par jq.
printf '%s\n' '8:0 rbytes=1 wbytes=2 rios=3 wios=4' > "$TMP_DIR/io.stat"
source "$TMP_DIR/io-helpers.sh"
p2_cgroup_io_json "$TMP_DIR/io.stat" > "$TMP_DIR/io.json"
jq -e 'type == "array" and length == 4 and .[0] == {device:"8:0",metric:"rbytes",value:1}' "$TMP_DIR/io.json" >/dev/null \
  || fail 'B1: io.stat n est pas sérialisé en JSON valide attendu'

# B2 : Dupont et DUPONT désignent le même posting ASCII après lowercase ; le
# validateur indépendant doit refuser leur placement dans deux ensembles.
source "$TMP_DIR/term-helpers.sh"
P2_PAIR_COUNT=1
P2_WARM_TERM_COUNT=1
PROBE_FIXED_TERM=MARTIN
printf '%s\n' $'1\tDupont\tJean' > "$TMP_DIR/bool.tsv"
printf '%s\n' $'1\tDUPONT' > "$TMP_DIR/control.tsv"
printf '%s\n' $'1\tDurand\tJules' > "$TMP_DIR/warm.tsv"
if p2_validate_term_sets "$TMP_DIR/bool.tsv" "$TMP_DIR/control.tsv" "$TMP_DIR/warm.tsv"; then
  fail 'B2: collision analysée Dupont/DUPONT acceptée'
fi
printf '%s\n' $'1\tMartin\tJean' > "$TMP_DIR/fixed.tsv"
if p2_validate_pairs "$TMP_DIR/fixed.tsv" 1; then
  fail 'B2: le terme fixe Martin n est pas exclu après lowercase'
fi
printf '%s\n' $'1\tÉvrard\tNoël' > "$TMP_DIR/accented.tsv"
p2_validate_pairs "$TMP_DIR/accented.tsv" 1 \
  || fail 'N2: Évrard/Noël doit être accepté après asciifolding réel'
printf '%s\n' $'1\tÞór\tEð' > "$TMP_DIR/asciifold-extensions.tsv"
p2_validate_pairs "$TMP_DIR/asciifold-extensions.tsv" 1 \
  || fail 'N2: Þ/ð doivent suivre les expansions TH/D du moteur'
printf '%s\n' $'1\tEvrard' > "$TMP_DIR/accented-control.tsv"
if p2_validate_term_sets "$TMP_DIR/accented.tsv" "$TMP_DIR/accented-control.tsv" "$TMP_DIR/warm.tsv"; then
  fail 'N2: collision Évrard/Evrard après asciifolding acceptée'
fi

# M4 : une jauge non finie n est jamais convertie silencieusement en zéro.
source "$TMP_DIR/metric-helpers.sh"
printf '%s\n' 'surch_jemalloc_resident_bytes NaN' > "$TMP_DIR/prometheus.bad"
if p2_metric_value surch_jemalloc_resident_bytes "$TMP_DIR/prometheus.bad" >/dev/null; then
  fail 'M4: NaN Prometheus a été accepté'
fi
printf '%s\n' 'surch_jemalloc_resident_bytes 0' > "$TMP_DIR/prometheus.zero"
[ "$(p2_metric_value surch_jemalloc_resident_bytes "$TMP_DIR/prometheus.zero")" = 0 ] \
  || fail 'M4: la valeur numérique zéro n est pas préservée'

# N8 : matrice versionnée du gate. Elle ne dépend ni de Docker ni du corpus :
# les neuf scorecards, les trois familles de paires et leurs télémétries sont
# synthétiques, mais ont exactement le schéma de la campagne P3.
GATE="$ROOT_DIR/deploy/bench-local/p2-gate.sh"

make_gate_fixture(){
  local campaign="$1" run variant status telemetry value directory rss anon file p3 phase boundary delta
  local manifest="$campaign/inputs/probe_p2_inputs.manifest" manifest_sha
  mkdir -p "$campaign/inputs" "$campaign/runs"
  printf '%s\n' 'protocol=p2-segmented-postings-v4-termes-analyses' > "$manifest"
  manifest_sha=$(sha256sum "$manifest" | awk '{print $1}')
  jq -n \
    --arg a 961ade10ffb74d78156aee8148f1e5c6bbbe6ba2 \
    --arg b 6ce390e55da3593242ec11e2b09d4dee1057726d \
    --arg c d0accd6e4809bc7340a6cd55cef0a94fcb6c062d \
    '{schema:"surch.bench.p3.provenance.v1",protocol:"p3-campagne-plan-v1",variants:{A:{commit:$a,image:"fixture:A",image_id:"sha256:fixture-a",digest:"sha256:fixture-a"},B:{commit:$b,image:"fixture:B",image_id:"sha256:fixture-b",digest:"sha256:fixture-b"},C:{commit:$c,image:"fixture:C",image_id:"sha256:fixture-c",digest:"sha256:fixture-c"}}}' \
    > "$campaign/campaign-provenance.json"
  for variant in A B C; do
    jq ".variants.$variant" "$campaign/campaign-provenance.json" > "$campaign/image-$variant.json"
    for number in 1 2 3; do
      run="$variant$number"
      mkdir -p "$campaign/runs/$run"
      status="$campaign/runs/$run/status.jsonl"
      telemetry="$campaign/runs/$run/telemetry.jsonl"
      case "$variant" in
        A) directory=900; rss=100; anon=100; file=200; value=100 ;;
        B) directory=1000; rss=200; anon=200; file=100; value=200 ;;
        C) directory=10; rss=110; anon=110; file=190; value=110 ;;
      esac
      : > "$status"; : > "$telemetry"
      for phase in warm_match match_control warm_bool bool_size10 bool_size0 fixed_martin; do
        if [ "$variant" = C ]; then
          p3='{"required":true,"bytes":{"before":10000000,"after":10000000},"hash_failures":{"before":0,"after":0},"fallbacks":{"before":0,"after":0},"fallback_fields":{"before":0,"after":0},"verified_bytes":{"before":1,"after":2}}'
        else
          p3='{"required":false,"bytes":{"before":null,"after":null},"hash_failures":{"before":null,"after":null},"fallbacks":{"before":null,"after":null},"fallback_fields":{"before":null,"after":null},"verified_bytes":{"before":null,"after":null}}'
        fi
        case "$phase" in warm_bool|bool_size10|bool_size0) bool_requests=100; ratio=0.24 ;; *) bool_requests=0; ratio=null ;; esac
        jq -nc --arg phase "$phase" --arg variant "$variant" --argjson bool_requests "$bool_requests" --argjson ratio "$ratio" --argjson integrity "$p3" \
          '{phase:$phase,variant:$variant,bool_requests:$bool_requests,match_requests:100,request_cache:false,hits_total_positive:true,valid:true,cpu_steal_within_limit:true,blocks_read_over_total:$ratio,blocks_ratio_target:0.25,blocks_ratio_target_pass:true,blocks_ratio_verdict:"pass",integrity:$integrity}' \
          >> "$status"
      done
      for phase in index_ready warm_match match_control warm_bool bool_size10 bool_size0 fixed_martin; do
        if [ "$phase" = index_ready ]; then
          boundary=snapshot
          jq -nc --arg phase "$phase" --arg boundary "$boundary" --argjson directory "$directory" --argjson rss "$rss" --argjson anon "$anon" --argjson file "$file" --argjson p3 "$( [ "$variant" = C ] && printf '{"bytes":10000000,"pages":1,"verified_bytes":1,"hash_failures":0,"fallbacks":0,"fallback_fields":0,"term_occurrences":1,"blocks":1,"fields":1,"term_payload_bytes":1,"csr_bytes":1,"directory_bytes":10}' || printf null )" \
            '{phase:$phase,boundary:$boundary,metrics:{index:{postings_directory_bytes:$directory,total_bytes:$directory},jemalloc:{allocated:$rss,active:$rss,resident:$rss,retained:$rss},p3_integrity:$p3},process:{rss_bytes:$rss,rss_anon_bytes:$anon,vmhwm_bytes:$rss},cgroup:{memory_current:$rss,memory_stat:{anon:$anon,file:$file,workingset_refault_file:0,workingset_activate_file:0,pgmajfault:0},io_stat:[{device:"8:0",metric:"rbytes",value:1}],io_stat_delta_from_before:null,memory_psi:{some:{avg10:0,avg60:0,avg300:0,total:0},full:{avg10:0,avg60:0,avg300:0,total:0}},io_psi:{some:{avg10:0,avg60:0,avg300:0,total:0},full:{avg10:0,avg60:0,avg300:0,total:0}},cpu_stat:{nr_throttled:0,throttled_usec:0}}}' >> "$telemetry"
        else
          for boundary in before after; do
            if [ "$boundary" = after ]; then delta='[{"device":"8:0","metric":"rbytes","delta":0}]'; else delta=null; fi
            jq -nc --arg phase "$phase" --arg boundary "$boundary" --argjson directory "$directory" --argjson rss "$rss" --argjson anon "$anon" --argjson file "$file" --argjson delta "$delta" --argjson p3 "$( [ "$variant" = C ] && printf '{"bytes":10000000,"pages":1,"verified_bytes":1,"hash_failures":0,"fallbacks":0,"fallback_fields":0,"term_occurrences":1,"blocks":1,"fields":1,"term_payload_bytes":1,"csr_bytes":1,"directory_bytes":10}' || printf null )" \
              '{phase:$phase,boundary:$boundary,metrics:{index:{postings_directory_bytes:$directory,total_bytes:$directory},jemalloc:{allocated:$rss,active:$rss,resident:$rss,retained:$rss},p3_integrity:$p3},process:{rss_bytes:$rss,rss_anon_bytes:$anon,vmhwm_bytes:$rss},cgroup:{memory_current:$rss,memory_stat:{anon:$anon,file:$file,workingset_refault_file:0,workingset_activate_file:0,pgmajfault:0},io_stat:[{device:"8:0",metric:"rbytes",value:1}],io_stat_delta_from_before:$delta,memory_psi:{some:{avg10:0,avg60:0,avg300:0,total:0},full:{avg10:0,avg60:0,avg300:0,total:0}},io_psi:{some:{avg10:0,avg60:0,avg300:0,total:0},full:{avg10:0,avg60:0,avg300:0,total:0}},cpu_stat:{nr_throttled:0,throttled_usec:0}}}' >> "$telemetry"
          done
        fi
      done
      jq -n --arg variant "$variant" --arg manifest "$manifest" --arg sha "$manifest_sha" --arg status "$status" --arg telemetry "$telemetry" --argjson image "$(jq ".variants.$variant" "$campaign/campaign-provenance.json")" \
        '{measurement_valid:true,p2:{protocol:"p2-segmented-postings-v4-termes-analyses",variant:$variant,input_manifest:$manifest,input_manifest_sha256:$sha,phase_status_jsonl:$status,telemetry_jsonl:$telemetry,image:$image.image,image_id:$image.image_id,image_digest:$image.digest,causal_phase_records:5,replay_mix_5050:0,phase_records:6,telemetry_records:13}}' \
        > "$campaign/runs/$run/surch.json"
    done
  done
  write_pair(){
    local group="$1" pair="$2" a="$3" b="$4" out
    out="$campaign/$group/$pair"
    mkdir -p "$out"
    jq -n --arg pair "$pair" --arg a "$a" --arg b "$b" --arg ma "$manifest_sha" \
      '{pair:$pair,a_run:$a,b_run:$b,parity:true,a_manifest_sha256:$ma,b_manifest_sha256:$ma}' > "$out/parity.json"
    jq -n --arg a "$campaign/runs/$a" --arg b "$campaign/runs/$b" '
      def r($phase;$kind;$metric;$ratio): {phase:$phase,kind:$kind,metric:$metric,a:{p95:10},b:{p95:(10 * $ratio)},b_over_a:{p95:$ratio,p99:$ratio}};
      {schema:"surch.bench.p2.pair.v1",a_dir:$a,b_dir:$b,records:[r("bool_size10";"bool";"took";0.69),r("bool_size10";"bool";"client";0.69),r("bool_size0";"bool";"took";0.49),r("fixed_martin";"match";"took";0.96),r("match_control";"match";"took";0.96),r("match_control";"match";"client";1.04),{phase:"bool_size10",kind:"bool",metric:"probe",a:{p95:10},b:{p95:11},b_over_a:{p95:1.1,p99:1.1}}],primary_bootstrap:{ci95_high:0.89}}' > "$out/pair-summary.json"
  }
  for number in 1 2 3; do
    write_pair pairs "A$number-B$number" "A$number" "B$number"
    write_pair p3-primary-pairs "A$number-C$number" "A$number" "C$number"
    write_pair p3-cost-pairs "B$number-C$number" "B$number" "C$number"
  done
}

copy_fixture(){
  local target="$1" file
  cp -a "$TMP_DIR/gate-base" "$target"
  # pair-summary.json conserve des chemins absolus : les réancrer dans la
  # copie est indispensable pour tester la liaison répertoire/run du gate.
  for file in "$target"/pairs/*/pair-summary.json "$target"/p3-primary-pairs/*/pair-summary.json "$target"/p3-cost-pairs/*/pair-summary.json; do
    sed -i "s|$TMP_DIR/gate-base|$target|g" "$file"
  done
  for file in "$target"/runs/*/surch.json; do
    sed -i "s|$TMP_DIR/gate-base|$target|g" "$file"
  done
}

set_ratio(){
  local campaign="$1" group="$2" phase="$3" kind="$4" metric="$5" quantile="$6" value="$7" pair file tmp
  for pair in "$campaign/$group"/*; do
    file="$pair/pair-summary.json"; tmp="$file.tmp"
    jq --arg phase "$phase" --arg kind "$kind" --arg metric "$metric" --arg quantile "$quantile" --argjson value "$value" '
      .records |= map(if .phase == $phase and .kind == $kind and .metric == $metric then .b_over_a[$quantile] = $value | .b.p95 = (.a.p95 * $value) else . end)
    ' "$file" > "$tmp" && mv "$tmp" "$file"
  done
}

set_ratio_one(){
  local campaign="$1" group="$2" pair="$3" phase="$4" kind="$5" metric="$6" quantile="$7" value="$8" file tmp
  file="$campaign/$group/$pair/pair-summary.json"; tmp="$campaign/$group/$pair/pair-summary.tmp"
  jq --arg phase "$phase" --arg kind "$kind" --arg metric "$metric" --arg quantile "$quantile" --argjson value "$value" '
    .records |= map(if .phase == $phase and .kind == $kind and .metric == $metric then .b_over_a[$quantile] = $value | .b.p95 = (.a.p95 * $value) else . end)
  ' "$file" > "$tmp" && mv "$tmp" "$file"
}

set_bootstrap(){
  local campaign="$1" value="$2" file tmp
  for file in "$campaign/p3-primary-pairs"/*/pair-summary.json; do
    tmp="$file.tmp"; jq --argjson value "$value" '.primary_bootstrap.ci95_high = $value' "$file" > "$tmp" && mv "$tmp" "$file"
  done
}

set_status_value(){
  local campaign="$1" variant="$2" filter="$3" value="$4" run file tmp
  for run in "$campaign/runs/$variant"[123]; do
    file="$run/status.jsonl"; tmp="$file.tmp"
    jq -c --arg filter "$filter" --argjson value "$value" '
      if $filter == "blocks" then
        .blocks_read_over_total = $value | .blocks_ratio_target_pass = ($value <= 0.25)
      else
        .integrity.bytes.before = $value | .integrity.bytes.after = $value
      end
    ' "$file" > "$tmp" && mv "$tmp" "$file"
  done
}

set_telemetry_value(){
  local campaign="$1" variant="$2" path="$3" value="$4" run file tmp
  for run in "$campaign/runs/$variant"[123]; do
    file="$run/telemetry.jsonl"; tmp="$file.tmp"
    jq -c --arg path "$path" --argjson value "$value" 'setpath($path | split("."); $value)' "$file" > "$tmp" && mv "$tmp" "$file"
  done
}

set_telemetry_one(){
  local campaign="$1" run="$2" path="$3" value="$4" file tmp
  file="$campaign/runs/$run/telemetry.jsonl"; tmp="$campaign/runs/$run/telemetry.tmp"
  jq -c --arg path "$path" --argjson value "$value" 'setpath($path | split("."); $value)' "$file" > "$tmp" && mv "$tmp" "$file"
}

run_gate(){
  local campaign="$1" verdict="$2" expected_code="$3" code
  set +e
  "$GATE" --campaign "$campaign" > "$campaign/gate.stdout" 2> "$campaign/gate.stderr"
  code=$?
  set -e
  if [ "$code" -ne "$expected_code" ]; then
    sed -n '1,80p' "$campaign/gate.stderr" >&2
    fail "N8: code gate $code, attendu $expected_code pour $verdict ($campaign)"
  fi
  jq -e --arg verdict "$verdict" '.verdict == $verdict' "$campaign/campaign-summary.json" >/dev/null \
    || fail "N8: verdict gate absent ou différent de $verdict"
}

matrix_upper(){
  local name="$1" group="$2" phase="$3" kind="$4" metric="$5" quantile="$6" below="$7" above="$8" ok bad
  ok="$TMP_DIR/$name-ok"; bad="$TMP_DIR/$name-bad"
  copy_fixture "$ok"; set_ratio "$ok" "$group" "$phase" "$kind" "$metric" "$quantile" "$below"; run_gate "$ok" 'PASS P3' 0
  copy_fixture "$bad"; set_ratio "$bad" "$group" "$phase" "$kind" "$metric" "$quantile" "$above"; run_gate "$bad" 'ÉCHEC P3' 1
}

matrix_lower(){
  local name="$1" group="$2" phase="$3" kind="$4" metric="$5" quantile="$6" above="$7" below="$8" bad_verdict="$9" ok bad
  ok="$TMP_DIR/$name-ok"; bad="$TMP_DIR/$name-bad"
  copy_fixture "$ok"; set_ratio "$ok" "$group" "$phase" "$kind" "$metric" "$quantile" "$above"; run_gate "$ok" 'PASS P3' 0
  copy_fixture "$bad"; set_ratio "$bad" "$group" "$phase" "$kind" "$metric" "$quantile" "$below"; run_gate "$bad" "$bad_verdict" 1
}

matrix_repetition_upper(){
  local name="$1" group="$2" phase="$3" kind="$4" metric="$5" quantile="$6" below="$7" above="$8" ok bad pair=A1-C1
  ok="$TMP_DIR/$name-ok"; bad="$TMP_DIR/$name-bad"
  [ "$group" = p3-cost-pairs ] && pair=B1-C1
  copy_fixture "$ok"; set_ratio_one "$ok" "$group" "$pair" "$phase" "$kind" "$metric" "$quantile" "$below"; run_gate "$ok" 'PASS P3' 0
  copy_fixture "$bad"; set_ratio_one "$bad" "$group" "$pair" "$phase" "$kind" "$metric" "$quantile" "$above"; run_gate "$bad" 'ÉCHEC P3' 1
}

make_gate_fixture "$TMP_DIR/gate-base"
run_gate "$TMP_DIR/gate-base" 'PASS P3' 0

# La version exhaustive isole chaque borne dans son propre répertoire. Le cas
# compact ci-dessous reste celui exécuté en CI : il porte, pour chaque seuil,
# une observation juste de chaque côté tout en gardant trois runs distincts.
if [ "${P3_MATRIX_EXHAUSTIVE:-0}" = 1 ]; then
# Seuils C/A, bootstrap et témoins : un point juste de chaque côté de chaque borne.
matrix_upper core-p95 p3-primary-pairs bool_size0 bool took p95 0.49 0.51
matrix_upper core-p99 p3-primary-pairs bool_size0 bool took p99 0.69 0.71
matrix_upper product-took p3-primary-pairs bool_size10 bool took p95 0.69 0.71
matrix_upper product-client p3-primary-pairs bool_size10 bool client p95 0.69 0.71
matrix_repetition_upper product-repetition p3-primary-pairs bool_size10 bool took p95 0.79 0.81
ok="$TMP_DIR/bootstrap-ok"; bad="$TMP_DIR/bootstrap-bad"; copy_fixture "$ok"; set_bootstrap "$ok" 0.89; run_gate "$ok" 'PASS P3' 0; copy_fixture "$bad"; set_bootstrap "$bad" 0.90; run_gate "$bad" 'ÉCHEC P3' 1
matrix_lower fixed-lower p3-primary-pairs fixed_martin match took p95 0.96 0.94 'REJOUER P3'
matrix_upper fixed-upper p3-primary-pairs fixed_martin match took p95 1.04 1.06
matrix_lower match-lower p3-primary-pairs match_control match took p95 0.96 0.94 'REJOUER P3'
matrix_upper match-upper p3-primary-pairs match_control match took p95 1.04 1.06
matrix_upper match-client-median p3-primary-pairs match_control match client p95 1.04 1.06
matrix_repetition_upper match-client-repetition p3-primary-pairs match_control match client p95 1.09 1.11

# Seuils de sonde, blocs et coût C/B (médiane puis répétition).
probe_ok="$TMP_DIR/probe-ok"; probe_bad="$TMP_DIR/probe-bad"; copy_fixture "$probe_ok"; copy_fixture "$probe_bad"
for pair in "$probe_ok"/p3-primary-pairs/*; do jq '.records |= map(if .metric == "probe" then .b.p95 = 11.9 else . end)' "$pair/pair-summary.json" > "$pair/x" && mv "$pair/x" "$pair/pair-summary.json"; done
for pair in "$probe_bad"/p3-primary-pairs/*; do jq '.records |= map(if .metric == "probe" then .b.p95 = 12.1 else . end)' "$pair/pair-summary.json" > "$pair/x" && mv "$pair/x" "$pair/pair-summary.json"; done
run_gate "$probe_ok" 'PASS P3' 0; run_gate "$probe_bad" 'ÉCHEC P3' 1
blocks_ok="$TMP_DIR/blocks-ok"; blocks_bad="$TMP_DIR/blocks-bad"; copy_fixture "$blocks_ok"; set_status_value "$blocks_ok" B blocks 0.24; set_status_value "$blocks_ok" C blocks 0.24; run_gate "$blocks_ok" 'PASS P3' 0; copy_fixture "$blocks_bad"; set_status_value "$blocks_bad" B blocks 0.26; run_gate "$blocks_bad" 'ÉCHEC P3' 1
matrix_upper cost10-median p3-cost-pairs bool_size10 bool took p95 1.04 1.06
matrix_repetition_upper cost10-repetition p3-cost-pairs bool_size10 bool took p95 1.09 1.11
matrix_upper cost0-median p3-cost-pairs bool_size0 bool took p95 1.04 1.06
matrix_repetition_upper cost0-repetition p3-cost-pairs bool_size0 bool took p95 1.09 1.11

# Intégrité, compaction et récupérations : les valeurs restent sur trois runs distincts.
integrity_ok="$TMP_DIR/integrity-ok"; integrity_bad="$TMP_DIR/integrity-bad"; copy_fixture "$integrity_ok"; set_status_value "$integrity_ok" C integrity 17825791; run_gate "$integrity_ok" 'PASS P3' 0; copy_fixture "$integrity_bad"; set_status_value "$integrity_bad" C integrity 17825793; run_gate "$integrity_bad" 'ÉCHEC P3' 1
technical="$TMP_DIR/integrity-technical-invalid"; copy_fixture "$technical"; set_status_value "$technical" C integrity 33554433; run_gate "$technical" 'INVALIDE P3' 1
compaction_ok="$TMP_DIR/compaction-ok"; compaction_bad="$TMP_DIR/compaction-bad"; copy_fixture "$compaction_ok"; set_telemetry_value "$compaction_ok" C metrics.index.postings_directory_bytes 9.9; run_gate "$compaction_ok" 'PASS P3' 0; copy_fixture "$compaction_bad"; set_telemetry_value "$compaction_bad" C metrics.index.postings_directory_bytes 10.1; run_gate "$compaction_bad" 'ÉCHEC P3' 1
for kind in rss anon file; do
  ok="$TMP_DIR/recovery-$kind-ok"; bad="$TMP_DIR/recovery-$kind-bad"; copy_fixture "$ok"; copy_fixture "$bad"
  case "$kind" in rss) path=process.rss_bytes; good=109; poor=111 ;; anon) path=process.rss_anon_bytes; good=109; poor=111 ;; file) path=cgroup.memory_stat.file; good=191; poor=179 ;; esac
  set_telemetry_value "$ok" C "$path" "$good"; run_gate "$ok" 'PASS P3' 0
  set_telemetry_value "$bad" C "$path" "$poor"; run_gate "$bad" 'ÉCHEC P3' 1
  repetition_ok="$TMP_DIR/recovery-$kind-repetition-ok"; repetition_bad="$TMP_DIR/recovery-$kind-repetition-bad"; copy_fixture "$repetition_ok"; copy_fixture "$repetition_bad"
  case "$kind" in rss|anon) repeat_good=119; repeat_poor=121 ;; file) repeat_good=181; repeat_poor=179 ;; esac
  set_telemetry_one "$repetition_ok" C1 "$path" "$repeat_good"; run_gate "$repetition_ok" 'PASS P3' 0
  set_telemetry_one "$repetition_bad" C1 "$path" "$repeat_poor"; run_gate "$repetition_bad" 'ÉCHEC P3' 1
done
fi

checks_at_boundary(){
  local campaign="$1" expected="$2"
  jq -e --argjson expected "$expected" '
    ([.checks[] | select(.pass == false) | .name]) as $failed
    | all($expected[]; . as $name | ($failed | index($name)) != null)
  ' "$campaign/campaign-summary.json" >/dev/null || fail 'N8: une borne rouge attendue n est pas évaluée par le gate'
}

# Côté accepté : les trois répétitions demeurent distinctes, avec une valeur
# près de chaque borne (médiane et borne par répétition incluses).
boundary_ok="$TMP_DIR/matrice-bornes-acceptees"; copy_fixture "$boundary_ok"
set_ratio_one "$boundary_ok" p3-primary-pairs A1-C1 bool_size10 bool took p95 0.79
set_ratio_one "$boundary_ok" p3-primary-pairs A2-C2 fixed_martin match took p95 1.04
set_ratio_one "$boundary_ok" p3-primary-pairs A2-C2 match_control match took p95 1.04
set_ratio_one "$boundary_ok" p3-primary-pairs A2-C2 match_control match client p95 1.09
set_ratio "$boundary_ok" p3-cost-pairs bool_size10 bool took p95 1.04
set_ratio "$boundary_ok" p3-cost-pairs bool_size0 bool took p95 1.04
set_ratio_one "$boundary_ok" p3-cost-pairs B1-C1 bool_size10 bool took p95 1.09
set_ratio_one "$boundary_ok" p3-cost-pairs B1-C1 bool_size0 bool took p95 1.09
set_status_value "$boundary_ok" C integrity 17825791
set_telemetry_value "$boundary_ok" C metrics.index.postings_directory_bytes 9.9
for kind in rss anon file; do
  case "$kind" in rss) path=process.rss_bytes; middle=109; edge=119 ;; anon) path=process.rss_anon_bytes; middle=109; edge=119 ;; file) path=cgroup.memory_stat.file; middle=191; edge=181 ;; esac
  set_telemetry_value "$boundary_ok" C "$path" "$middle"; set_telemetry_one "$boundary_ok" C1 "$path" "$edge"
done
for pair in "$boundary_ok"/p3-primary-pairs/*; do
  jq '.records |= map(if .metric == "probe" then .b.p95 = 11.9 else . end)' "$pair/pair-summary.json" > "$pair/boundary.tmp" && mv "$pair/boundary.tmp" "$pair/pair-summary.json"
done
run_gate "$boundary_ok" 'PASS P3' 0

# Côté rouge : même matrice, avec chaque limite haute/franchie ou récupération
# sous le contrat. Le tableau de checks doit citer chaque seuil concerné.
boundary_bad="$TMP_DIR/matrice-bornes-rouges"; copy_fixture "$boundary_bad"
set_ratio "$boundary_bad" p3-primary-pairs bool_size0 bool took p95 0.51
set_ratio "$boundary_bad" p3-primary-pairs bool_size0 bool took p99 0.71
set_ratio "$boundary_bad" p3-primary-pairs bool_size10 bool took p95 0.71
set_ratio "$boundary_bad" p3-primary-pairs bool_size10 bool client p95 0.71
set_ratio_one "$boundary_bad" p3-primary-pairs A1-C1 bool_size10 bool took p95 0.81
set_bootstrap "$boundary_bad" 0.90
set_ratio "$boundary_bad" p3-primary-pairs fixed_martin match took p95 1.06
set_ratio "$boundary_bad" p3-primary-pairs match_control match took p95 1.06
set_ratio "$boundary_bad" p3-primary-pairs match_control match client p95 1.06
set_ratio_one "$boundary_bad" p3-primary-pairs A1-C1 match_control match client p95 1.11
for pair in "$boundary_bad"/p3-primary-pairs/*; do
  jq '.records |= map(if .metric == "probe" then .b.p95 = 12.1 else . end)' "$pair/pair-summary.json" > "$pair/boundary.tmp" && mv "$pair/boundary.tmp" "$pair/pair-summary.json"
done
set_status_value "$boundary_bad" B blocks 0.26; set_status_value "$boundary_bad" C blocks 0.26
set_ratio "$boundary_bad" p3-cost-pairs bool_size10 bool took p95 1.06
set_ratio "$boundary_bad" p3-cost-pairs bool_size0 bool took p95 1.06
set_ratio_one "$boundary_bad" p3-cost-pairs B1-C1 bool_size10 bool took p95 1.11
set_ratio_one "$boundary_bad" p3-cost-pairs B1-C1 bool_size0 bool took p95 1.11
set_status_value "$boundary_bad" C integrity 17825793
set_telemetry_value "$boundary_bad" C metrics.index.postings_directory_bytes 10.1
for kind in rss anon file; do
  case "$kind" in rss) path=process.rss_bytes; poor=111; repeat_poor=121 ;; anon) path=process.rss_anon_bytes; poor=111; repeat_poor=121 ;; file) path=cgroup.memory_stat.file; poor=189; repeat_poor=179 ;; esac
  set_telemetry_value "$boundary_bad" C "$path" "$poor"; set_telemetry_one "$boundary_bad" C1 "$path" "$repeat_poor"
done
run_gate "$boundary_bad" 'ÉCHEC P3' 1
checks_at_boundary "$boundary_bad" '["bool size:0 p95 took","bool size:0 p99 took","bool size:10 p95 took","bool size:10 p95 client","trois paires bool size:10 <= 0.80","IC95 bootstrap primaire","témoin fixed match : absence de régression <= 1.05","témoin match autonome : took <= 1.05","témoin match autonome : médiane p95 client <= 1.05","témoin match autonome : aucune p95 client > 1.10","écart sonde p95","ratio de blocs B (résultat)","ratio de blocs C (résultat)","coût P3 C/B bool size:10 : médiane p95 took <= 1.05","coût P3 C/B bool size:10 : aucune répétition > 1.10","coût P3 C/B bool size:0 : médiane p95 took <= 1.05","coût P3 C/B bool size:0 : aucune répétition > 1.10","cible intégrité P3","compaction directory_bytes C/B","récupération RSS médiane","récupération RSS : aucune répétition < 0.80","récupération RssAnon médiane","récupération RssAnon : aucune répétition < 0.80","récupération cache fichier médiane","récupération cache fichier : aucune répétition < 0.80"]'

# Une borne basse témoin est un REJOUER, jamais un gain silencieusement crédité.
replay="$TMP_DIR/matrice-rejouer"; copy_fixture "$replay"
set_ratio "$replay" p3-primary-pairs fixed_martin match took p95 0.94
set_ratio "$replay" p3-primary-pairs match_control match took p95 0.94
run_gate "$replay" 'REJOUER P3' 1
checks_at_boundary "$replay" '["témoin fixed match : comparabilité >= 0.95","témoin match autonome : comparabilité >= 0.95"]'

# Le plafond logiciel de 32 Mio reste une invalidité technique distincte de
# l échec de cible 17 Mio testé dans la matrice rouge.
technical="$TMP_DIR/integrite-technique-invalide"; copy_fixture "$technical"
set_status_value "$technical" C integrity 33554433
run_gate "$technical" 'INVALIDE P3' 1

# N1 : trois paires par famille qui réemploient A1/B1/C1 restent des fichiers
# bien formés, mais doivent désormais obtenir INVALIDE et jamais PASS.
duplicate="$TMP_DIR/n1-runs-dupliques"; copy_fixture "$duplicate"
for number in 2 3; do
  for spec in "pairs:A$number-B$number:A1:B1" "p3-primary-pairs:A$number-C$number:A1:C1" "p3-cost-pairs:B$number-C$number:B1:C1"; do
    IFS=: read -r group pair a b <<EOF
$spec
EOF
    jq --arg a "$a" --arg b "$b" '.a_run = $a | .b_run = $b' "$duplicate/$group/$pair/parity.json" > "$duplicate/$group/$pair/parity.tmp" && mv "$duplicate/$group/$pair/parity.tmp" "$duplicate/$group/$pair/parity.json"
    jq --arg a "$duplicate/runs/$a" --arg b "$duplicate/runs/$b" '.a_dir = $a | .b_dir = $b' "$duplicate/$group/$pair/pair-summary.json" > "$duplicate/$group/$pair/summary.tmp" && mv "$duplicate/$group/$pair/summary.tmp" "$duplicate/$group/$pair/pair-summary.json"
  done
done
run_gate "$duplicate" 'INVALIDE P3' 1

# M2/N6 : chacun des champs P3 obligatoires invalide indépendamment des
# autres gates ; le schéma ne peut plus perdre une preuve sans le dire.
for field in bytes pages verified_bytes hash_failures fallbacks fallback_fields \
  term_occurrences blocks fields term_payload_bytes csr_bytes directory_bytes; do
  schema_missing="$TMP_DIR/schema-p3-$field-absent"; copy_fixture "$schema_missing"
  for file in "$schema_missing"/runs/C[123]/telemetry.jsonl; do
    jq -c --arg field "$field" 'del(.metrics.p3_integrity[$field])' "$file" > "$file.tmp" && mv "$file.tmp" "$file"
  done
  run_gate "$schema_missing" 'INVALIDE P3' 1
done

printf 'test-p3-harness: PASS\n'
