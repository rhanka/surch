#!/usr/bin/env bash
# p2-campaign.sh — pilote fermé de la campagne A/B P2 sur la VM OVH dédiée.
#
# Le même fair-ab.sh (celui de 6ce390e enrichi par ce lot) pilote les deux
# images. Les images sont reconstruites depuis les deux SHA dans cette même
# session Docker ; les corps P2 vivent dans un répertoire partagé, gelé par
# SHA-256 dès la première exécution.
set -uo pipefail
export LC_ALL=C

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
FAIR_AB="${FAIR_AB:-$ROOT_DIR/deploy/bench-local/fair-ab.sh}"
PAIR_REPORT="${PAIR_REPORT:-$ROOT_DIR/deploy/bench-local/p2-report.sh}"
GATE_REPORT="${GATE_REPORT:-$ROOT_DIR/deploy/bench-local/p2-gate.sh}"
P2_A_SHA="${P2_A_SHA:-961ade10ffb74d78156aee8148f1e5c6bbbe6ba2}"
P2_B_SHA="${P2_B_SHA:-6ce390e55da3593242ec11e2b09d4dee1057726d}"
P2_MODE="${P2_MODE:-full}"             # full ou smoke
P2_CAMPAIGN_DIR="${P2_CAMPAIGN_DIR:-$HOME/p2-campaign-$(date -u +%Y%m%dT%H%M%SZ)}"
P2_SMOKE_DIR="${P2_SMOKE_DIR:-}"       # obligatoire avant le full
P2_DOCKER_ROOT="${P2_DOCKER_ROOT:-/var/lib/docker}"
P2_DOCKER_CLASSIC_SOURCE="${P2_DOCKER_CLASSIC_SOURCE:-}"
P2_REST_SECONDS="${P2_REST_SECONDS:-300}"
P2_RECOVERY_MEM_TOLERANCE_MIB="${P2_RECOVERY_MEM_TOLERANCE_MIB:-512}"
P2_RECOVERY_DISK_TOLERANCE_MIB="${P2_RECOVERY_DISK_TOLERANCE_MIB:-128}"
P2_RECOVERY_LOAD_TOLERANCE="${P2_RECOVERY_LOAD_TOLERANCE:-0.25}"
BULK_FILE="${BULK_FILE:-}"
MAPPING_FILE="${MAPPING_FILE:-}"

log(){ printf '\033[1;35m[p2-campaign]\033[0m %s\n' "$*"; }
err(){ printf '\033[1;31m[p2-campaign]\033[0m %s\n' "$*" >&2; }
die(){ err "$*"; exit 1; }

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
[ "$P2_REST_SECONDS" -eq 300 ] || die "P2_REST_SECONDS doit rester 300 secondes"
[ -n "$P2_DOCKER_CLASSIC_SOURCE" ] || die "P2_DOCKER_CLASSIC_SOURCE est obligatoire (ex. /dev/sdb du volume classic)"
if [ "$P2_MODE" = "full" ]; then
  [ -n "$P2_SMOKE_DIR" ] || die "P2_SMOKE_DIR est obligatoire : exécuter et conserver le smoke avant le full"
  [ -s "$P2_SMOKE_DIR/README.md" ] \
    && grep -q '^SMOKE P2 valide' "$P2_SMOKE_DIR/README.md" \
    && jq -e '.measurement_valid == true and .p2.phase_records == 5' "$P2_SMOKE_DIR/runs/smoke-A/surch.json" >/dev/null \
    && jq -e '.measurement_valid == true and .p2.phase_records == 5' "$P2_SMOKE_DIR/runs/smoke-B/surch.json" >/dev/null \
    || die "smoke P2 absent ou invalide: $P2_SMOKE_DIR"
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
write_host_state(){
  local path="$1"
  jq -n \
    --arg timestamp "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg mount_source "$mount_source" \
    --arg findmnt "$(findmnt -T "$P2_DOCKER_ROOT" -n -o SOURCE,TARGET,FSTYPE)" \
    --argjson mem_available_mib "$(mem_available_mib)" \
    --argjson disk_free_mib "$(disk_free_mib)" \
    --argjson load1 "$(load_one)" > "$path"
}

write_host_state "$P2_CAMPAIGN_DIR/host-baseline.json"
baseline_mem=$(jq -r .mem_available_mib "$P2_CAMPAIGN_DIR/host-baseline.json")
baseline_disk=$(jq -r .disk_free_mib "$P2_CAMPAIGN_DIR/host-baseline.json")
baseline_load=$(jq -r .load1 "$P2_CAMPAIGN_DIR/host-baseline.json")

image_metadata(){
  local image="$1" sha="$2" out="$3" image_id digest
  image_id=$(docker image inspect -f '{{.Id}}' "$image") || return 1
  digest=$(docker image inspect -f '{{index .RepoDigests 0}}' "$image" 2>/dev/null || true)
  [ -n "$digest" ] || digest="$image_id"
  jq -n --arg commit "$sha" --arg image "$image" --arg image_id "$image_id" --arg digest "$digest" > "$out"
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

if [ "$P2_MODE" = "full" ]; then
  PAIR_COUNT=1000
  PROBE_REQUESTS=2000
  EXPECTED_DOCS=28917511
  REQUIRED_SEGMENTS=12
  SEGMENT_GATE=exact
  FLUSH_BUDGET=268435456
  MERGE_FANIN=8
  SCHEDULE=("A1-B1:A:B" "B2-A2:B:A" "A3-B3:A:B")
else
  PAIR_COUNT=100
  PROBE_REQUESTS=200
  EXPECTED_DOCS=$(( $(wc -l < "$BULK_FILE") / 2 ))
  REQUIRED_SEGMENTS=3
  SEGMENT_GATE=minimum
  FLUSH_BUDGET=33554432
  MERGE_FANIN=64
  SCHEDULE=("smoke:A:B")
fi
[ "$EXPECTED_DOCS" -gt 0 ] || die "corpus smoke vide ou NDJSON invalide"

assert_scorecard(){
  local out="$1" variant="$2" score="$out/surch.json"
  [ -s "$score" ] || die "scorecard absente: $score"
  jq -e \
    --arg variant "$variant" \
    --argjson docs "$EXPECTED_DOCS" \
    --argjson segments "$REQUIRED_SEGMENTS" '
      .measurement_valid == true
      and .count == $docs and .indexed == $docs and .item_errors == 0
      and .p2.variant == $variant and .p2.expected_docs == $docs
      and .p2.required_segments == $segments and .p2.phase_records == 5
      and (.p2.observed_cpu_configuration.nproc == .probe_cpu_count)
      and (.p2.observed_cpu_configuration.engine_cpuset == .cpuset)
      and (.p2.observed_cpu_configuration.probe_cpuset == .probe_cpuset)
    ' "$score" >/dev/null || die "scorecard P2 invalide: $score"
}

run_variant(){
  local name="$1" variant="$2" image="$3" out="$P2_CAMPAIGN_DIR/runs/$name"
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
    "P2_MEASURE=1" "P2_VARIANT=$variant" "P2_PAIR_COUNT=$PAIR_COUNT" \
    "P2_EXPECTED_DOCS=$EXPECTED_DOCS" "P2_REQUIRED_SEGMENTS=$REQUIRED_SEGMENTS" "P2_SEGMENT_GATE=$SEGMENT_GATE" \
    "P2_INPUT_DIR=$P2_INPUT_DIR" "SURCH_IMAGE=$image" "ENGINES=surch" "OUT_DIR=$out" \
    "$FAIR_AB" > "$out/fair-ab.log" 2>&1; then
    die "run $name invalide, voir $out/fair-ab.log"
  fi
  assert_scorecard "$out" "$variant"
}

compare_parity(){
  local pair="$1" a_name="$2" b_name="$3" report="$P2_CAMPAIGN_DIR/pairs/$pair"
  local phase a_file b_file
  mkdir -p "$report" || die "création rapport impossible: $report"
  for phase in warm fixed random no_source cold; do
    a_file="$P2_CAMPAIGN_DIR/runs/$a_name/surch.p2.responses.${phase}.canonical.ndjson"
    b_file="$P2_CAMPAIGN_DIR/runs/$b_name/surch.p2.responses.${phase}.canonical.ndjson"
    if ! cmp -s "$a_file" "$b_file"; then
      diff -u "$a_file" "$b_file" > "$report/parity-${phase}.diff" || true
      die "parité A/B divergente pour $pair/$phase (diff: $report/parity-${phase}.diff)"
    fi
  done
  jq -n \
    --arg pair "$pair" --arg a_run "$a_name" --arg b_run "$b_name" \
    --arg a_manifest "$(jq -r '.p2.input_manifest_sha256' "$P2_CAMPAIGN_DIR/runs/$a_name/surch.json")" \
    --arg b_manifest "$(jq -r '.p2.input_manifest_sha256' "$P2_CAMPAIGN_DIR/runs/$b_name/surch.json")" \
    '{pair:$pair,a_run:$a_run,b_run:$b_run,parity:true,a_manifest_sha256:$a_manifest,b_manifest_sha256:$b_manifest}' \
    > "$report/parity.json"
  [ "$(jq -r .a_manifest_sha256 "$report/parity.json")" = "$(jq -r .b_manifest_sha256 "$report/parity.json")" ] \
    || die "manifestes différents pour $pair"
  "$PAIR_REPORT" --a "$P2_CAMPAIGN_DIR/runs/$a_name" --b "$P2_CAMPAIGN_DIR/runs/$b_name" --out "$report" \
    || die "rapport statistique invalide pour $pair"
}

recover_host(){
  local name="$1" state="$P2_CAMPAIGN_DIR/recovery-$name.json" mem disk load
  docker ps -a --format '{{.Names}}' | grep -qx 'fairab-surch' && die "teardown incomplet: fairab-surch existe"
  docker volume inspect fairab-vol-surch >/dev/null 2>&1 && die "teardown incomplet: fairab-vol-surch existe"
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
  load=$(jq -r .load1 "$state")
  [ "$mem" -ge $(( baseline_mem - P2_RECOVERY_MEM_TOLERANCE_MIB )) ] \
    || die "mémoire non revenue: $mem MiB vs baseline $baseline_mem MiB"
  [ "$disk" -ge $(( baseline_disk - P2_RECOVERY_DISK_TOLERANCE_MIB )) ] \
    || die "espace disque non revenu: $disk MiB vs baseline $baseline_disk MiB"
  awk -v after="$load" -v before="$baseline_load" -v tolerance="$P2_RECOVERY_LOAD_TOLERANCE" \
    'BEGIN { exit !(after <= before + tolerance) }' \
    || die "charge non revenue: $load vs baseline $baseline_load"
}

for scheduled in "${SCHEDULE[@]}"; do
  IFS=: read -r pair first second <<< "$scheduled"
  if [ "$first" = "A" ]; then
    a_name="${pair%%-*}"; [ "$a_name" = "$pair" ] && a_name="$pair-A"
    b_name="${pair##*-}"; [ "$b_name" = "$pair" ] && b_name="$pair-B"
    run_variant "$a_name" A "$IMAGE_A"
    recover_host "$a_name"
    run_variant "$b_name" B "$IMAGE_B"
  else
    b_name="${pair%%-*}"; [ "$b_name" = "$pair" ] && b_name="$pair-B"
    a_name="${pair##*-}"; [ "$a_name" = "$pair" ] && a_name="$pair-A"
    run_variant "$b_name" B "$IMAGE_B"
    recover_host "$b_name"
    run_variant "$a_name" A "$IMAGE_A"
  fi
  compare_parity "$pair" "$a_name" "$b_name"
  recover_host "$pair"
done

if [ "$P2_MODE" = "full" ]; then
  "$GATE_REPORT" --campaign "$P2_CAMPAIGN_DIR" || die "gates P2 non satisfaites (voir $P2_CAMPAIGN_DIR/README.md)"
else
  printf 'SMOKE P2 valide : routage, réponses, métriques et corps vérifiés ; aucune conclusion de latence.\n' \
    > "$P2_CAMPAIGN_DIR/README.md"
fi
log "campagne $P2_MODE terminée: $P2_CAMPAIGN_DIR"
