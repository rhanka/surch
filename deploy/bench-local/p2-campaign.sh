#!/usr/bin/env bash
# p2-campaign.sh — pilote fermé de la campagne A/B/C P2/P3 sur la VM dédiée.
#
# Le même fair-ab.sh pilote les trois images : A avant P2, B P2, C P3.
# Les images sont reconstruites depuis les trois SHA dans cette même
# session Docker ; les corps P2 vivent dans un répertoire partagé, gelé par
# SHA-256 dès la première exécution.
set -euo pipefail
export LC_ALL=C

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
FAIR_AB="${FAIR_AB:-$ROOT_DIR/deploy/bench-local/fair-ab.sh}"
PAIR_REPORT="${PAIR_REPORT:-$ROOT_DIR/deploy/bench-local/p2-report.sh}"
GATE_REPORT="${GATE_REPORT:-$ROOT_DIR/deploy/bench-local/p2-gate.sh}"
P3_A_SHA="961ade10ffb74d78156aee8148f1e5c6bbbe6ba2"
P3_B_SHA="6ce390e55da3593242ec11e2b09d4dee1057726d"
P3_C_SHA="d0accd6e4809bc7340a6cd55cef0a94fcb6c062d"
P3_PROTOCOL_VERSION="p2-segmented-postings-v4-termes-analyses"
# Les trois variantes sont celles du protocole pré-engagé. Une surcharge qui
# change un SHA est un autre protocole, donc un refus plutôt qu'un replay
# silencieusement différent.
P2_A_SHA="${P2_A_SHA:-$P3_A_SHA}"
P2_B_SHA="${P2_B_SHA:-$P3_B_SHA}"
P2_C_SHA="${P2_C_SHA:-$P3_C_SHA}"
P2_MODE="${P2_MODE:-full}"             # full ou smoke
P2_CAMPAIGN_DIR="${P2_CAMPAIGN_DIR:-$HOME/p2-campaign-$(date -u +%Y%m%dT%H%M%SZ)}"
P2_SMOKE_DIR="${P2_SMOKE_DIR:-}"       # obligatoire avant le full
P2_DOCKER_ROOT="${P2_DOCKER_ROOT:-/var/lib/docker}"
P2_DOCKER_CLASSIC_SOURCE="${P2_DOCKER_CLASSIC_SOURCE:-}"
P2_REST_SECONDS="${P2_REST_SECONDS:-300}"
P2_RECOVERY_MEM_TOLERANCE_MIB="${P2_RECOVERY_MEM_TOLERANCE_MIB:-512}"
P2_RECOVERY_DISK_TOLERANCE_MIB="${P2_RECOVERY_DISK_TOLERANCE_MIB:-128}"
P2_RECOVERY_LOAD_TOLERANCE="${P2_RECOVERY_LOAD_TOLERANCE:-0.25}"
P2_REPLAY_MIX_5050="${P2_REPLAY_MIX_5050:-0}"
BULK_FILE="${BULK_FILE:-}"
MAPPING_FILE="${MAPPING_FILE:-}"

log(){ printf '\033[1;35m[p2-campaign]\033[0m %s\n' "$*"; }
err(){ printf '\033[1;31m[p2-campaign]\033[0m %s\n' "$*" >&2; }
die(){ err "$*"; exit 1; }

verify_smoke_prerequisite(){
  local smoke="$1" proof provenance metadata manifest manifest_sha formula formula_sha score variant expected
  proof="$smoke/smoke-proof.json"
  [ -s "$proof" ] || return 1
  jq -e --arg protocol "$P3_PROTOCOL_VERSION" --arg a "$P3_A_SHA" --arg b "$P3_B_SHA" --arg c "$P3_C_SHA" '
    .schema == "surch.bench.p3.smoke.v1" and .verdict == "PASS SMOKE P3"
    and .protocol == $protocol
    and .variants.A.commit == $a and .variants.B.commit == $b and .variants.C.commit == $c
    and ([.variants[] | .image, .image_id, .digest] | all(type == "string" and length > 0))
    and (.inputs.manifest | type == "string" and length > 0)
    and (.inputs.manifest_sha256 | type == "string" and test("^[0-9a-f]{64}$"))
    and (.formula_fixture.path | type == "string" and length > 0)
    and (.formula_fixture.sha256 | type == "string" and test("^[0-9a-f]{64}$"))
  ' "$proof" >/dev/null || return 1
  provenance="$smoke/campaign-provenance.json"
  [ -s "$provenance" ] || return 1
  jq -e --slurpfile proof "$proof" '
    .schema == "surch.bench.p3.provenance.v1"
    and .protocol == "p3-campagne-plan-v1"
    and .variants == $proof[0].variants
  ' "$provenance" >/dev/null || return 1
  manifest=$(jq -er '.inputs.manifest | strings' "$proof") || return 1
  manifest_sha=$(jq -er '.inputs.manifest_sha256 | strings' "$proof") || return 1
  [ -r "$manifest" ] && [ "$(sha256sum "$manifest" | awk '{print $1}')" = "$manifest_sha" ] || return 1
  formula=$(jq -er '.formula_fixture.path | strings' "$proof") || return 1
  formula_sha=$(jq -er '.formula_fixture.sha256 | strings' "$proof") || return 1
  [ -r "$formula" ] && [ "$(sha256sum "$formula" | awk '{print $1}')" = "$formula_sha" ] || return 1
  jq -e '.compaction_directory_c_over_b == 0.01 and .recovery.rss == 0.9 and .recovery.rss_anon == 0.9 and .recovery.file == 0.9 and .undefined_denominator_rejected == true' "$formula" >/dev/null || return 1
  for variant in A B C; do
    score="$smoke/runs/smoke-$variant/surch.json"
    [ -s "$score" ] || return 1
    expected=$(jq -c --arg variant "$variant" '.variants[$variant]' "$proof") || return 1
    metadata="$smoke/image-$variant.json"
    [ -s "$metadata" ] || return 1
    jq -e --argjson expected "$expected" '. == $expected' "$metadata" >/dev/null || return 1
    jq -e --arg variant "$variant" --arg protocol "$P3_PROTOCOL_VERSION" --arg manifest "$(readlink -f -- "$manifest")" --arg sha "$manifest_sha" --argjson expected "$expected" '
      .measurement_valid == true and .p2.variant == $variant and .p2.protocol == $protocol
      and .p2.input_manifest == $manifest and .p2.input_manifest_sha256 == $sha
      and .p2.image == $expected.image and .p2.image_id == $expected.image_id and .p2.image_digest == $expected.digest
      and .p2.causal_phase_records == 5
      and ((.p2.replay_mix_5050 == 0 and .p2.phase_records == 6 and .p2.telemetry_records == 13) or (.p2.replay_mix_5050 == 1 and .p2.phase_records == 7 and .p2.telemetry_records == 15))
    ' "$score" >/dev/null || return 1
  done
  grep -qx 'SMOKE P3 valide : protocole v4, images, manifeste et formule vérifiés ; aucune conclusion de latence.' "$smoke/README.md"
}

for command in docker git jq findmnt sha256sum sync; do
  command -v "$command" >/dev/null 2>&1 || die "commande requise absente: $command"
done
[ -x "$FAIR_AB" ] || die "fair-ab.sh introuvable/exécutable: $FAIR_AB"
[ -x "$PAIR_REPORT" ] || die "rapport de paire introuvable/exécutable: $PAIR_REPORT"
[ -x "$GATE_REPORT" ] || die "rapport de gates introuvable/exécutable: $GATE_REPORT"
[ -s "$BULK_FILE" ] || die "BULK_FILE obligatoire et non vide"
[ -s "$MAPPING_FILE" ] || die "MAPPING_FILE obligatoire et non vide"
case "$P2_MODE" in full|smoke) ;; *) die "P2_MODE doit valoir full ou smoke";; esac
case "$P2_REST_SECONDS:$P2_RECOVERY_MEM_TOLERANCE_MIB:$P2_RECOVERY_DISK_TOLERANCE_MIB" in
  *[!0-9:]*|:*|*::*) die "paramètres P2 de récupération invalides";;
esac
case "$P2_REPLAY_MIX_5050" in 0|1) ;; *) die "P2_REPLAY_MIX_5050 doit valoir 0 ou 1";; esac
[ "$P2_A_SHA" = "$P3_A_SHA" ] || die "P2_A_SHA diffère du SHA pré-engagé P3"
[ "$P2_B_SHA" = "$P3_B_SHA" ] || die "P2_B_SHA diffère du SHA pré-engagé P3"
[ "$P2_C_SHA" = "$P3_C_SHA" ] || die "P2_C_SHA diffère du SHA pré-engagé P3"
[ "$P2_REST_SECONDS" -eq 300 ] || die "P2_REST_SECONDS doit rester 300 secondes"
[ -n "$P2_DOCKER_CLASSIC_SOURCE" ] || die "P2_DOCKER_CLASSIC_SOURCE est obligatoire (ex. /dev/sdb du volume classic)"
if [ "$P2_MODE" = "full" ]; then
  [ -n "$P2_SMOKE_DIR" ] || die "P2_SMOKE_DIR est obligatoire : exécuter et conserver le smoke avant le full"
  verify_smoke_prerequisite "$P2_SMOKE_DIR" \
    || die "smoke P3 v4 absent ou invalide (pins, images, manifeste ou formule): $P2_SMOKE_DIR"
fi

mkdir -p "$P2_CAMPAIGN_DIR" || die "création impossible: $P2_CAMPAIGN_DIR"
P2_CAMPAIGN_DIR=$(readlink -f "$P2_CAMPAIGN_DIR")
P2_INPUT_DIR="${P2_INPUT_DIR:-$P2_CAMPAIGN_DIR/inputs}"
P2_INPUT_DIR=$(readlink -m "$P2_INPUT_DIR")

docker_root_actual=$(docker info --format '{{.DockerRootDir}}' 2>/dev/null) || die "docker info impossible"
[ "$docker_root_actual" = "$P2_DOCKER_ROOT" ] || die "DockerRootDir=$docker_root_actual, attendu $P2_DOCKER_ROOT"
mount_source=$(findmnt -T "$P2_DOCKER_ROOT" -n -o SOURCE 2>/dev/null) || die "findmnt impossible pour $P2_DOCKER_ROOT"
[ "$mount_source" = "$P2_DOCKER_CLASSIC_SOURCE" ] || die "data-root Docker sur $mount_source, attendu volume classic $P2_DOCKER_CLASSIC_SOURCE"
free_kib=$(df -Pk "$P2_DOCKER_ROOT" | awk 'NR == 2 { print $4 }')
[ "${free_kib:-0}" -ge $(( 18 * 1024 * 1024 )) ] || die "volume Docker < 18 Gio libres ($free_kib KiB)"
findmnt -T "$P2_DOCKER_ROOT" > "$P2_CAMPAIGN_DIR/docker-data-root.findmnt.txt"

mem_available_mib(){ awk '/^MemAvailable:/{print int($2 / 1024)}' /proc/meminfo; }
load_one(){ awk '{print $1}' /proc/loadavg; }
disk_free_mib(){ df -Pm "$P2_DOCKER_ROOT" | awk 'NR == 2 {print $4}'; }
campaign_artifacts_on_docker_fs(){
  local docker_fs campaign_fs
  docker_fs=$(df -Pk "$P2_DOCKER_ROOT" | awk 'NR == 2 {print $1}')
  campaign_fs=$(df -Pk "$P2_CAMPAIGN_DIR" | awk 'NR == 2 {print $1}')
  [ -n "$docker_fs" ] && [ "$docker_fs" = "$campaign_fs" ]
}
campaign_artifacts_mib(){
  if campaign_artifacts_on_docker_fs; then
    du -sm -- "$P2_CAMPAIGN_DIR" | awk 'NR == 1 {print $1}'
  else
    printf '0'
  fi
}
disk_free_effective_mib(){
  local free artifacts
  free=$(disk_free_mib); artifacts=$(campaign_artifacts_mib)
  printf '%s' "$(( free + artifacts ))"
}
write_host_state(){
  local path="$1" artifacts_on_docker_fs=false
  campaign_artifacts_on_docker_fs && artifacts_on_docker_fs=true
  jq -n \
    --arg timestamp "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg mount_source "$mount_source" \
    --arg findmnt "$(findmnt -T "$P2_DOCKER_ROOT" -n -o SOURCE,TARGET,FSTYPE)" \
    --argjson mem_available_mib "$(mem_available_mib)" \
    --argjson disk_free_mib "$(disk_free_mib)" \
    --argjson campaign_artifacts_mib "$(campaign_artifacts_mib)" \
    --argjson disk_free_effective_mib "$(disk_free_effective_mib)" \
    --argjson campaign_artifacts_on_docker_filesystem "$artifacts_on_docker_fs" \
    --argjson load1 "$(load_one)" \
    '{timestamp:$timestamp,mount_source:$mount_source,findmnt:$findmnt,mem_available_mib:$mem_available_mib,disk_free_mib:$disk_free_mib,campaign_artifacts_mib:$campaign_artifacts_mib,disk_free_effective_mib:$disk_free_effective_mib,campaign_artifacts_on_docker_filesystem:$campaign_artifacts_on_docker_filesystem,load1:$load1}' \
    > "$path"
}

image_metadata(){
  local image="$1" sha="$2" out="$3" image_id digest
  image_id=$(docker image inspect -f '{{.Id}}' "$image") || return 1
  digest=$(docker image inspect -f '{{index .RepoDigests 0}}' "$image" 2>/dev/null || true)
  [ -n "$digest" ] || digest="$image_id"
  jq -n --arg commit "$sha" --arg image "$image" --arg image_id "$image_id" --arg digest "$digest" \
    '{commit:$commit,image:$image,image_id:$image_id,digest:$digest}' > "$out"
}

build_image(){
  local variant="$1" sha="$2" image="surch-p2-${variant,,}:${sha:0:12}"
  git -C "$ROOT_DIR" cat-file -e "${sha}^{commit}" || die "SHA absent du clone local: $sha"
  log "construction image $variant depuis $sha (sans pull, session courante)"
  if ! git -C "$ROOT_DIR" archive --format=tar "$sha" | docker build --pull=false -t "$image" -; then
    die "build Docker échoué pour $variant/$sha"
  fi
  image_metadata "$image" "$sha" "$P2_CAMPAIGN_DIR/image-${variant}.json" || die "inspection image $variant impossible"
  BUILT_IMAGE="$image"
}

build_image A "$P2_A_SHA"; IMAGE_A="$BUILT_IMAGE"
build_image B "$P2_B_SHA"; IMAGE_B="$BUILT_IMAGE"
build_image C "$P2_C_SHA"; IMAGE_C="$BUILT_IMAGE"
jq -n \
  --arg protocol 'p3-campagne-plan-v1' \
  --slurpfile image_a "$P2_CAMPAIGN_DIR/image-A.json" \
  --slurpfile image_b "$P2_CAMPAIGN_DIR/image-B.json" \
  --slurpfile image_c "$P2_CAMPAIGN_DIR/image-C.json" \
  '{schema:"surch.bench.p3.provenance.v1",protocol:$protocol,variants:{A:$image_a[0],B:$image_b[0],C:$image_c[0]}}' \
  > "$P2_CAMPAIGN_DIR/campaign-provenance.json" \
  || die 'écriture impossible de la provenance de campagne'
jq -e --arg a "$P3_A_SHA" --arg b "$P3_B_SHA" --arg c "$P3_C_SHA" '
  .variants.A.commit == $a and .variants.B.commit == $b and .variants.C.commit == $c
  and ([.variants[] | .image, .image_id, .digest] | all(type == "string" and length > 0))
' "$P2_CAMPAIGN_DIR/campaign-provenance.json" >/dev/null \
  || die 'provenance image incomplète ou SHA non pré-engagé'

# Le baseline est pris après les builds : leurs couches et cache vivent sous
# Docker, ne sont pas des artefacts de campagne et ne doivent pas être pris
# pour une fuite lors du premier contrôle de récupération.
write_host_state "$P2_CAMPAIGN_DIR/host-baseline.json"
baseline_mem=$(jq -r .mem_available_mib "$P2_CAMPAIGN_DIR/host-baseline.json")
baseline_disk=$(jq -r .disk_free_mib "$P2_CAMPAIGN_DIR/host-baseline.json")
baseline_disk_effective=$(jq -r .disk_free_effective_mib "$P2_CAMPAIGN_DIR/host-baseline.json")
baseline_load=$(jq -r .load1 "$P2_CAMPAIGN_DIR/host-baseline.json")

if [ "$P2_MODE" = "full" ]; then
  PAIR_COUNT=1000
  PROBE_REQUESTS=2000
  EXPECTED_DOCS=28917511
  REQUIRED_SEGMENTS=12
  SEGMENT_GATE=exact
  FLUSH_BUDGET=268435456
  MERGE_FANIN=8
  # Ordre latin pré-engagé : C commence le premier triplet afin qu'un échec
  # mémoire P3 coupe la dépense avant A1/B1.
  SCHEDULE=("A1,B1,C1:C:A:B" "A2,B2,C2:A:B:C" "A3,B3,C3:B:C:A")
else
  PAIR_COUNT=100
  PROBE_REQUESTS=200
  EXPECTED_DOCS=$(( $(wc -l < "$BULK_FILE") / 2 ))
  REQUIRED_SEGMENTS=3
  SEGMENT_GATE=minimum
  FLUSH_BUDGET=33554432
  MERGE_FANIN=64
  SCHEDULE=("smoke-A,smoke-B,smoke-C:A:B:C")
fi
[ "$EXPECTED_DOCS" -gt 0 ] || die "corpus smoke vide ou NDJSON invalide"

assert_scorecard(){
  local out="$1" variant="$2" image_metadata_file="$3" execution_id="$4" score="$out/surch.json"
  [ -s "$score" ] || die "scorecard absente: $score"
  jq -e \
    --arg variant "$variant" \
    --argjson docs "$EXPECTED_DOCS" \
    --argjson segments "$REQUIRED_SEGMENTS" '
      .measurement_valid == true
      and .count == $docs and .indexed == $docs and .item_errors == 0
      and .p2.variant == $variant and .p2.expected_docs == $docs
      and .p2.required_segments == $segments and .p2.causal_phase_records == 5
      and ((.p2.replay_mix_5050 == 0 and .p2.phase_records == 6 and .p2.telemetry_records == 13) or (.p2.replay_mix_5050 == 1 and .p2.phase_records == 7 and .p2.telemetry_records == 15))
      and (.p2.observed_cpu_configuration.nproc == .probe_cpu_count)
      and (.p2.observed_cpu_configuration.engine_cpuset == .cpuset)
      and (.p2.observed_cpu_configuration.probe_cpuset == .probe_cpuset)
      and (.p2.image | strings | length > 0)
      and (.p2.image_id | strings | length > 0)
      and (.p2.image_digest | strings | length > 0)
    ' "$score" >/dev/null || die "scorecard P2 invalide: $score"
  jq -e --arg execution_id "$execution_id" '
    .p2.execution_id == $execution_id
    and ($execution_id | test("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"))
  ' "$score" >/dev/null || die "identifiant d'exécution P2 incohérent: $score"
  jq -e --slurpfile metadata "$image_metadata_file" '
    ($metadata | length) == 1
    and .p2.image == $metadata[0].image
    and .p2.image_id == $metadata[0].image_id
    and .p2.image_digest == $metadata[0].digest
  ' "$score" >/dev/null || die "provenance image incohérente: $score"
}

new_execution_id(){
  local execution_id
  [ -r /proc/sys/kernel/random/uuid ] || return 1
  execution_id=$(cat /proc/sys/kernel/random/uuid) || return 1
  [[ "$execution_id" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]] || return 1
  printf '%s' "$execution_id"
}

declare -A P2_RUN_EXECUTION_IDS=()

run_variant(){
  local name="$1" variant="$2" image="$3" metadata="$4" require_p3=0 execution_id out="$P2_CAMPAIGN_DIR/runs/$name"
  [ "$variant" = "C" ] && require_p3=1
  [ -z "${P2_RUN_EXECUTION_IDS[$name]:-}" ] || die "identifiant d'exécution déjà attribué à $name"
  execution_id=$(new_execution_id) || die "UUID d'exécution impossible pour $name"
  P2_RUN_EXECUTION_IDS[$name]="$execution_id"
  mkdir -p "$out" || die "création run impossible: $out"
  log "run $name ($variant, image=$image)"
  if ! env \
    "BULK_FILE=$BULK_FILE" "MAPPING_FILE=$MAPPING_FILE" \
    "PROBE_FIELD_NOM=NOM" "PROBE_FIELD_PRENOMS=PRENOMS" "PROBE_FIXED_TERM=MARTIN" \
    "CPUSET=0-2" "MEM_LIMIT=6g" "HARNESS_MEM_MAX=3G" "PREFLIGHT_MARGIN_MIB=2048" "AUX_MEM=512m" \
    "POSTINGS_DISK=1" "SURCH_SOURCE_COMPRESS=1" \
    "SURCH_FLUSH_BUDGET_BYTES=$FLUSH_BUDGET" "SURCH_MERGE_FANIN=$MERGE_FANIN" \
    "SURCH_DENSIFY_BUDGET_DOCS=1000000" "SURCH_MERGE_MAX_DOCS=7000000" \
    "SURCH_SOURCE_FETCH_PROFILE=0" "PREFLIGHT_FORCE=0" \
    "PROBE_REQUESTS=$PROBE_REQUESTS" "COLD_PROBE_REQUESTS=50" "COLD_PROBE=1" \
    "P2_MEASURE=1" "P2_VARIANT=$variant" "P2_EXECUTION_ID=$execution_id" "P2_REQUIRE_P3_INTEGRITY=$require_p3" "P2_PAIR_COUNT=$PAIR_COUNT" "P2_WARM_TERM_COUNT=200" "P2_REPLAY_MIX_5050=$P2_REPLAY_MIX_5050" \
    "P2_EXPECTED_DOCS=$EXPECTED_DOCS" "P2_REQUIRED_SEGMENTS=$REQUIRED_SEGMENTS" "P2_SEGMENT_GATE=$SEGMENT_GATE" \
    "P2_INPUT_DIR=$P2_INPUT_DIR" "SURCH_IMAGE=$image" "ENGINES=surch" "OUT_DIR=$out" \
    "$FAIR_AB" > "$out/fair-ab.log" 2>&1; then
    die "run $name invalide, voir $out/fair-ab.log"
  fi
  assert_scorecard "$out" "$variant" "$metadata" "$execution_id"
}

compare_parity(){
  local pair="$1" a_name="$2" b_name="$3" report_group="$4" report="$P2_CAMPAIGN_DIR/$report_group/$pair" a_execution_id b_execution_id
  local phase a_file b_file phases=(warm_match match_control warm_bool bool_size10 bool_size0 fixed_martin)
  mkdir -p "$report" || die "création rapport impossible: $report"
  [ "$P2_REPLAY_MIX_5050" = "0" ] || phases+=(replay_mix_5050)
  for phase in "${phases[@]}"; do
    a_file="$P2_CAMPAIGN_DIR/runs/$a_name/surch.p2.responses.${phase}.canonical.ndjson"
    b_file="$P2_CAMPAIGN_DIR/runs/$b_name/surch.p2.responses.${phase}.canonical.ndjson"
    if ! cmp -s "$a_file" "$b_file"; then
      diff -u "$a_file" "$b_file" > "$report/parity-${phase}.diff" || true
      die "parité divergente pour $pair/$phase (diff: $report/parity-${phase}.diff)"
    fi
  done
  a_execution_id=$(jq -er '.p2.execution_id | strings' "$P2_CAMPAIGN_DIR/runs/$a_name/surch.json") || die "UUID A absent pour $pair"
  b_execution_id=$(jq -er '.p2.execution_id | strings' "$P2_CAMPAIGN_DIR/runs/$b_name/surch.json") || die "UUID B absent pour $pair"
  [ "$a_execution_id" != "$b_execution_id" ] || die "UUID identique pour $pair"
  jq -n \
    --arg pair "$pair" --arg a_run "$a_name" --arg b_run "$b_name" \
    --arg a_execution_id "$a_execution_id" --arg b_execution_id "$b_execution_id" \
    --arg a_manifest "$(jq -r '.p2.input_manifest_sha256' "$P2_CAMPAIGN_DIR/runs/$a_name/surch.json")" \
    --arg b_manifest "$(jq -r '.p2.input_manifest_sha256' "$P2_CAMPAIGN_DIR/runs/$b_name/surch.json")" \
    '{pair:$pair,a_run:$a_run,b_run:$b_run,a_execution_id:$a_execution_id,b_execution_id:$b_execution_id,parity:true,a_manifest_sha256:$a_manifest,b_manifest_sha256:$b_manifest}' \
    > "$report/parity.json"
  [ "$(jq -r .a_manifest_sha256 "$report/parity.json")" = "$(jq -r .b_manifest_sha256 "$report/parity.json")" ] \
    || die "manifestes différents pour $pair"
  # Cold est conservé comme diagnostic quand les deux côtés l'ont produit,
  # mais son reclaim dépend de droits hôte qui ne modifient pas les quatre
  # phases chaudes, leurs corps gelés, leur routage ni leur parité A/B.
  # Une divergence cold est donc rapportée sans annuler la comparaison P2.
  local cold_a_ok cold_b_ok cold_a_file cold_b_file cold_parity=false cold_status
  cold_a_ok=$(jq -r '.cold_probe_ok == true' "$P2_CAMPAIGN_DIR/runs/$a_name/surch.json")
  cold_b_ok=$(jq -r '.cold_probe_ok == true' "$P2_CAMPAIGN_DIR/runs/$b_name/surch.json")
  cold_a_file="$P2_CAMPAIGN_DIR/runs/$a_name/surch.p2.responses.cold.canonical.ndjson"
  cold_b_file="$P2_CAMPAIGN_DIR/runs/$b_name/surch.p2.responses.cold.canonical.ndjson"
  if [ "$cold_a_ok" = true ] && [ "$cold_b_ok" = true ] && [ -s "$cold_a_file" ] && [ -s "$cold_b_file" ]; then
    if cmp -s "$cold_a_file" "$cold_b_file"; then
      cold_parity=true; cold_status="available_and_equal"
    else
      diff -u "$cold_a_file" "$cold_b_file" > "$report/parity-cold-diagnostic.diff" || true
      cold_status="available_but_different"
    fi
  else
    cold_status="unavailable_or_incomplete"
  fi
  jq -n \
    --arg pair "$pair" --arg a_run "$a_name" --arg b_run "$b_name" --arg status "$cold_status" \
    --argjson a_cold_ok "$cold_a_ok" --argjson b_cold_ok "$cold_b_ok" --argjson parity "$cold_parity" \
    '{pair:$pair,a_run:$a_run,b_run:$b_run,optional:true,status:$status,a_cold_ok:$a_cold_ok,b_cold_ok:$b_cold_ok,parity:$parity}' \
    > "$report/cold-diagnostic.json"
  "$PAIR_REPORT" --a "$P2_CAMPAIGN_DIR/runs/$a_name" --b "$P2_CAMPAIGN_DIR/runs/$b_name" --out "$report" \
    || die "rapport statistique invalide pour $pair"
}

recover_host(){
  local name="$1" state="$P2_CAMPAIGN_DIR/recovery-$name.json" mem disk disk_effective artifacts load containers volumes
  containers=$(docker ps -a --format '{{.Names}}') || die 'docker ps impossible pendant la récupération'
  if grep -qx 'fairab-surch' <<< "$containers"; then
    die 'teardown incomplet: fairab-surch existe'
  fi
  volumes=$(docker volume ls --format '{{.Name}}') || die 'docker volume ls impossible pendant la récupération'
  if grep -qx 'fairab-vol-surch' <<< "$volumes"; then
    die 'teardown incomplet: fairab-vol-surch existe'
  fi
  sync
  if [ -w /proc/sys/vm/drop_caches ]; then
    printf '3\n' > /proc/sys/vm/drop_caches
  else
    sudo -n sh -c 'printf "3\n" > /proc/sys/vm/drop_caches' >/dev/null 2>&1 \
      || die "drop_caches impossible (root/sudo non interactif requis)"
  fi
  log "attente de récupération hôte: $P2_REST_SECONDS s"
  sleep "$P2_REST_SECONDS"
  write_host_state "$state"
  mem=$(jq -r .mem_available_mib "$state")
  disk=$(jq -r .disk_free_mib "$state")
  disk_effective=$(jq -r .disk_free_effective_mib "$state")
  artifacts=$(jq -r .campaign_artifacts_mib "$state")
  load=$(jq -r .load1 "$state")
  [ "$mem" -ge $(( baseline_mem - P2_RECOVERY_MEM_TOLERANCE_MIB )) ] \
    || die "mémoire non revenue: $mem MiB vs baseline $baseline_mem MiB"
  # Sur un disque unique, les rapports et captures P2 volontairement conservés
  # consomment le même FS que Docker. Les ajouter à l'espace libre sépare cette
  # croissance d'artefacts traçables d'une fuite de volume/conteneur, sans
  # relâcher le contrôle de teardown ou la marge disque réellement récupérée.
  [ "$disk_effective" -ge $(( baseline_disk_effective - P2_RECOVERY_DISK_TOLERANCE_MIB )) ] \
    || die "espace disque non revenu hors artefacts: libre=$disk MiB + artefacts=$artifacts MiB = $disk_effective MiB vs baseline_effective=$baseline_disk_effective MiB (brut baseline=$baseline_disk MiB)"
  awk -v after="$load" -v before="$baseline_load" -v tolerance="$P2_RECOVERY_LOAD_TOLERANCE" \
    'BEGIN { exit !(after <= before + tolerance) }' \
    || die "charge non revenue: $load vs baseline $baseline_load"
}

p3_ratio(){
  local summary="$1" phase="$2" metric="$3" quantile="$4"
  jq -er --arg phase "$phase" --arg metric "$metric" --arg quantile "$quantile" '
    first(.records[] | select(.phase == $phase and .kind == "bool" and .metric == $metric) | .b_over_a[$quantile])
    | if type == "number" then . else error("ratio P3 absent") end
  ' "$summary"
}

p3_match_ratio(){
  local summary="$1"
  jq -er '
    first(.records[] | select(.phase == "match_control" and .kind == "match" and .metric == "took") | .b_over_a.p95)
    | if type == "number" then . else error("ratio témoin P3 absent") end
  ' "$summary"
}

p3_index_telemetry_value(){
  local score="$1" filter="$2" telemetry
  telemetry=$(jq -er '.p2.telemetry_jsonl | strings' "$score") || return 1
  [ -r "$telemetry" ] || return 1
  jq -ser --arg filter "$filter" '
    def safe_path($keys):
      reduce $keys[] as $key (.;
        if type == "object" and ($key | type) == "string" and has($key) then .[$key] else null end);
    first(.[] | select(.phase == "index_ready" and .boundary == "snapshot") | safe_path($filter | split(".")))
    | if type == "number" then . else error("télémétrie index_ready absente") end
  ' "$telemetry"
}

p3_recovery_ratio(){
  local a="$1" b="$2" c="$3" mode="$4"
  awk -v a="$a" -v b="$b" -v c="$c" -v mode="$mode" '
    BEGIN {
      if (mode == "rss") denominator = b - a
      else if (mode == "file") denominator = a - b
      else exit 1
      if (denominator <= 0) exit 1
      if (mode == "rss") numerator = b - c
      else numerator = c - b
      printf "%.12g", numerator / denominator
    }
  '
}

# Le smoke ne publie aucune latence, mais il doit exercer les mêmes formules
# de compaction/récupération que le gate final. Les valeurs sont synthétiques,
# déterministes et le dénominateur indéfini doit être rejeté.
p3_smoke_formula_fixture(){
  local compaction rss anon file
  compaction=$(awk 'BEGIN { printf "%.12g", 10 / 1000 }') || return 1
  rss=$(p3_recovery_ratio 100 200 110 rss) || return 1
  anon=$(p3_recovery_ratio 100 200 110 rss) || return 1
  file=$(p3_recovery_ratio 200 100 190 file) || return 1
  if p3_recovery_ratio 100 100 100 rss >/dev/null 2>&1; then
    return 1
  fi
  jq -n --argjson compaction "$compaction" --argjson rss "$rss" --argjson anon "$anon" --argjson file "$file" \
    '{schema:"surch.bench.p3.smoke-formulas.v1",compaction_directory_c_over_b:$compaction,recovery:{rss:$rss,rss_anon:$anon,file:$file},undefined_denominator_rejected:true}' \
    > "$P2_CAMPAIGN_DIR/smoke-formulas.json" || return 1
  jq -e '.compaction_directory_c_over_b == 0.01 and .recovery.rss == 0.9 and .recovery.rss_anon == 0.9 and .recovery.file == 0.9 and .undefined_denominator_rejected == true' "$P2_CAMPAIGN_DIR/smoke-formulas.json" >/dev/null
}

write_smoke_proof(){
  local manifest="" manifest_sha="" candidate_sha variant score candidate_manifest formula formula_sha
  [ "$P2_MODE" = smoke ] || return 1
  p3_smoke_formula_fixture || return 1
  for variant in A B C; do
    score="$P2_CAMPAIGN_DIR/runs/smoke-$variant/surch.json"
    [ -s "$score" ] || return 1
    candidate_manifest=$(jq -er '.p2.input_manifest | strings' "$score") || return 1
    candidate_manifest=$(readlink -f -- "$candidate_manifest") || return 1
    candidate_sha=$(jq -er '.p2.input_manifest_sha256 | strings' "$score") || return 1
    [ -r "$candidate_manifest" ] && [ "$(sha256sum "$candidate_manifest" | awk '{print $1}')" = "$candidate_sha" ] || return 1
    if [ -z "$manifest" ]; then
      manifest="$candidate_manifest"; manifest_sha="$candidate_sha"
    elif [ "$candidate_manifest" != "$manifest" ] || [ "$candidate_sha" != "$manifest_sha" ]; then
      return 1
    fi
  done
  formula="$P2_CAMPAIGN_DIR/smoke-formulas.json"
  formula_sha=$(sha256sum "$formula" | awk '{print $1}') || return 1
  jq -n --arg protocol "$P3_PROTOCOL_VERSION" --arg manifest "$manifest" --arg manifest_sha "$manifest_sha" \
    --arg formula "$formula" --arg formula_sha "$formula_sha" \
    --slurpfile provenance "$P2_CAMPAIGN_DIR/campaign-provenance.json" \
    '{schema:"surch.bench.p3.smoke.v1",verdict:"PASS SMOKE P3",protocol:$protocol,variants:$provenance[0].variants,inputs:{manifest:$manifest,manifest_sha256:$manifest_sha},formula_fixture:{path:$formula,sha256:$formula_sha}}' \
    > "$P2_CAMPAIGN_DIR/smoke-proof.json" || return 1
}

p3_c1_hard_stop(){
  local score="$P2_CAMPAIGN_DIR/runs/C1/surch.json" status
  [ -s "$score" ] || die 'C1: scorecard introuvable'
  status=$(jq -er '.p2.phase_status_jsonl | strings' "$score") || die 'C1: statut de phases absent'
  [ -r "$status" ] || die 'C1: statut de phases illisible'
  jq -se --argjson target 17825792 '
    ([.[] | select(.phase == "warm_match" or .phase == "match_control" or .phase == "warm_bool" or .phase == "bool_size10" or .phase == "bool_size0" or .phase == "fixed_martin")]) as $phases
    | ($phases | length == 6)
    and all($phases[];
      .variant == "C" and .valid == true and .integrity.required == true
      and (.integrity.bytes.before | type) == "number" and (.integrity.bytes.after | type) == "number"
      and .integrity.bytes.before > 0 and .integrity.bytes.after > 0
      and .integrity.bytes.before <= $target and .integrity.bytes.after <= $target
      and .integrity.hash_failures.before == 0 and .integrity.hash_failures.after == 0
      and .integrity.fallbacks.before == 0 and .integrity.fallbacks.after == 0
      and .integrity.fallback_fields.before == 0 and .integrity.fallback_fields.after == 0
    )
  ' "$status" | grep -qx true \
    || die 'hard-stop C1: intégrité P3, count/segments ou validité interne hors contrat (cible <= 17 Mio)'
  jq -n --arg run C1 --arg status "$status" --argjson target 17825792 \
    '{run:$run,hard_stop:"passed",integrity_target_bytes:$target,phase_status_jsonl:$status}' \
    > "$P2_CAMPAIGN_DIR/preselection-c1.json" || die 'écriture impossible du hard-stop C1'
}

p3_first_triplet_hard_stop(){
  local a="$1" b="$2" c="$3" primary cost c_a c_b_10 c_b_0 match a_rss b_rss c_rss a_anon b_anon c_anon a_file b_file c_file rss_recovery anon_recovery file_recovery
  primary="$P2_CAMPAIGN_DIR/p3-primary-pairs/$a-$c/pair-summary.json"
  cost="$P2_CAMPAIGN_DIR/p3-cost-pairs/$b-$c/pair-summary.json"
  [ -r "$primary" ] && [ -r "$cost" ] || die 'hard-stop premier triplet: rapports de paires absents'
  c_a=$(p3_ratio "$primary" bool_size10 took p95) || die 'hard-stop premier triplet: C/A produit absent'
  c_b_10=$(p3_ratio "$cost" bool_size10 took p95) || die 'hard-stop premier triplet: C/B size:10 absent'
  c_b_0=$(p3_ratio "$cost" bool_size0 took p95) || die 'hard-stop premier triplet: C/B size:0 absent'
  match=$(p3_match_ratio "$primary") || die 'hard-stop premier triplet: témoin C/A absent'
  a_rss=$(p3_index_telemetry_value "$P2_CAMPAIGN_DIR/runs/$a/surch.json" process.rss_bytes) || die 'hard-stop premier triplet: RSS A absent'
  b_rss=$(p3_index_telemetry_value "$P2_CAMPAIGN_DIR/runs/$b/surch.json" process.rss_bytes) || die 'hard-stop premier triplet: RSS B absent'
  c_rss=$(p3_index_telemetry_value "$P2_CAMPAIGN_DIR/runs/$c/surch.json" process.rss_bytes) || die 'hard-stop premier triplet: RSS C absent'
  a_anon=$(p3_index_telemetry_value "$P2_CAMPAIGN_DIR/runs/$a/surch.json" process.rss_anon_bytes) || die 'hard-stop premier triplet: RssAnon A absent'
  b_anon=$(p3_index_telemetry_value "$P2_CAMPAIGN_DIR/runs/$b/surch.json" process.rss_anon_bytes) || die 'hard-stop premier triplet: RssAnon B absent'
  c_anon=$(p3_index_telemetry_value "$P2_CAMPAIGN_DIR/runs/$c/surch.json" process.rss_anon_bytes) || die 'hard-stop premier triplet: RssAnon C absent'
  a_file=$(p3_index_telemetry_value "$P2_CAMPAIGN_DIR/runs/$a/surch.json" cgroup.memory_stat.file) || die 'hard-stop premier triplet: cache fichier A absent'
  b_file=$(p3_index_telemetry_value "$P2_CAMPAIGN_DIR/runs/$b/surch.json" cgroup.memory_stat.file) || die 'hard-stop premier triplet: cache fichier B absent'
  c_file=$(p3_index_telemetry_value "$P2_CAMPAIGN_DIR/runs/$c/surch.json" cgroup.memory_stat.file) || die 'hard-stop premier triplet: cache fichier C absent'
  rss_recovery=$(p3_recovery_ratio "$a_rss" "$b_rss" "$c_rss" rss) || die 'hard-stop premier triplet: formule RSS indéfinie'
  anon_recovery=$(p3_recovery_ratio "$a_anon" "$b_anon" "$c_anon" rss) || die 'hard-stop premier triplet: formule RssAnon indéfinie'
  file_recovery=$(p3_recovery_ratio "$a_file" "$b_file" "$c_file" file) || die 'hard-stop premier triplet: formule cache fichier indéfinie'
  jq -n --argjson c_a "$c_a" --argjson c_b_size10 "$c_b_10" --argjson c_b_size0 "$c_b_0" --argjson match "$match" \
    --argjson rss_recovery "$rss_recovery" --argjson rss_anon_recovery "$anon_recovery" --argjson file_recovery "$file_recovery" \
    '{c_over_a_bool_size10_took_p95:$c_a,c_over_b_bool_size10_took_p95:$c_b_size10,c_over_b_bool_size0_took_p95:$c_b_size0,match_control_c_over_a_took_p95:$match,recovery:{rss:$rss_recovery,rss_anon:$rss_anon_recovery,file:$file_recovery}}' \
    > "$P2_CAMPAIGN_DIR/preselection-triplet-1.json" || die 'écriture impossible du hard-stop premier triplet'
  awk -v c_a="$c_a" -v c_b_10="$c_b_10" -v c_b_0="$c_b_0" -v match="$match" -v rss="$rss_recovery" -v anon="$anon_recovery" -v file="$file_recovery" '
    BEGIN { exit !(c_a <= .80 && c_b_10 <= 1.10 && c_b_0 <= 1.10 && match <= 1.10 && rss >= .80 && anon >= .80 && file >= .80) }
  ' || die 'hard-stop premier triplet: présélection P3 rouge; campagne arrêtée avant les six runs restants'
}

triplet_number=0
for scheduled in "${SCHEDULE[@]}"; do
  triplet_number=$(( triplet_number + 1 ))
  IFS=: read -r names first second third <<< "$scheduled"
  IFS=, read -r a_name b_name c_name <<< "$names"
  for variant in "$first" "$second" "$third"; do
    case "$variant" in
      A) run_name="$a_name"; image="$IMAGE_A"; metadata="$P2_CAMPAIGN_DIR/image-A.json" ;;
      B) run_name="$b_name"; image="$IMAGE_B"; metadata="$P2_CAMPAIGN_DIR/image-B.json" ;;
      C) run_name="$c_name"; image="$IMAGE_C"; metadata="$P2_CAMPAIGN_DIR/image-C.json" ;;
      *) die "variante planifiée inconnue: $variant" ;;
    esac
    run_variant "$run_name" "$variant" "$image" "$metadata"
    if [ "$P2_MODE" = full ] && [ "$run_name" = C1 ]; then
      p3_c1_hard_stop
    fi
    [ "$variant" = "$third" ] || recover_host "$run_name"
  done
  compare_parity "$a_name-$b_name" "$a_name" "$b_name" pairs
  compare_parity "$b_name-$c_name" "$b_name" "$c_name" p3-cost-pairs
  compare_parity "$a_name-$c_name" "$a_name" "$c_name" p3-primary-pairs
  recover_host "$a_name-$b_name-$c_name"
  if [ "$P2_MODE" = full ] && [ "$triplet_number" -eq 1 ]; then
    p3_first_triplet_hard_stop "$a_name" "$b_name" "$c_name"
  fi
done

if [ "$P2_MODE" = "full" ]; then
  "$GATE_REPORT" --campaign "$P2_CAMPAIGN_DIR" || die "gates P2 non satisfaites (voir $P2_CAMPAIGN_DIR/README.md)"
else
  # README fait partie du contrat de reprise : ni son écriture ni son verdict
  # ne peuvent être avalés par le dernier `log` du script.
  # Le proof porte aussi les identités image et manifeste ; l'écrire avant le
  # README rend un disque plein fail-closed et exerce la fixture de formules.
  write_smoke_proof || die 'preuve smoke P3 impossible à écrire ou invalide'
  printf 'SMOKE P3 valide : protocole v4, images, manifeste et formule vérifiés ; aucune conclusion de latence.\n' \
    > "$P2_CAMPAIGN_DIR/README.md" || die 'écriture impossible du verdict smoke P3'
  verify_smoke_prerequisite "$P2_CAMPAIGN_DIR" || die 'verdict smoke P3 non rejouable après écriture'
fi
log "campagne $P2_MODE terminée: $P2_CAMPAIGN_DIR"
