#!/usr/bin/env bash
# Casse le motif des trois échecs précédents : jusqu'ici, chaque test P3
# simulait précisément la frontière où le bug se trouvait (test-p3-campaign.sh
# remplace fair-ab.sh entier par fake-fair-ab ; test-p3-harness.sh n'extrait
# jamais les fonctions de construction du bundle P3). Ce test source le VRAI
# code de fair-ab.sh — p2_metric_bundle_json et ses dépendances directes — et
# l'exécute contre une sortie Prometheus synthétique, sans Docker, sans
# moteur, sans VM. Il exerce spécifiquement la branche C qui a invalidé le
# smoke2 : `--argjson directory_bytes "$directory"` déclaré mais `$directory`
# référencé dans le filtre jq (au lieu de `$directory_bytes`).
set -Eeuo pipefail
export LC_ALL=C

CURRENT_STEP='initialisation du test'
TMP_DIR=''

fail(){ printf '[test-p3-telemetry] ECHEC (%s) : %s\n' "$CURRENT_STEP" "$*" >&2; exit 1; }
set_step(){ CURRENT_STEP="$1"; }

cleanup(){
  local status=$?
  if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
    rm -rf -- "$TMP_DIR"
  fi
  exit "$status"
}
trap cleanup EXIT

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
HARNESS="$ROOT_DIR/deploy/bench-local/fair-ab.sh"
TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/surch-p3-telemetry.XXXXXX")
OUT_DIR="$TMP_DIR/out"
mkdir -p "$OUT_DIR"

printf 'test-p3-telemetry: versions bash=%s jq=%s awk=%s\n' \
  "$BASH_VERSION" "$(jq --version 2>&1)" "$(awk -W version 2>&1 | head -n1)" >&2

# ---- extraction des VRAIES fonctions depuis fair-ab.sh (pas de mock) ----
set_step 'extraction des helpers réels'
grep '^err(){' "$HARNESS" > "$TMP_DIR/err-helper.sh" \
  || fail 'la fonction err() est introuvable dans fair-ab.sh'

# p2_metric_present, p2_snapshot_metrics, p2_metric_value : parsing des
# métriques Prometheus (même bornage que test-p3-harness.sh).
awk '
  /^p2_metric_present\(\)\{/ { capture = 1 }
  /^p2_proc_status_bytes\(\)\{/ { capture = 0 }
  capture { print }
' "$HARNESS" > "$TMP_DIR/metric-helpers.sh"
grep -q '^p2_metric_value(){' "$TMP_DIR/metric-helpers.sh" \
  || fail 'p2_metric_value non extrait : le bornage awk a dérivé'

# p2_cgroup_directory, p2_cgroup_stat_value, p2_cgroup_io_json,
# p2_cgroup_io_delta_json, p2_psi_json, p2_metric_bundle_json : construction
# du bundle métrique index_ready, y compris le bundle p3_integrity — la
# branche qui a échoué en smoke2.
awk '
  /^p2_cgroup_directory\(\)\{/ { capture = 1 }
  /^p2_capture_telemetry\(\)\{/ { capture = 0 }
  capture { print }
' "$HARNESS" > "$TMP_DIR/bundle-helpers.sh"
grep -q '^p2_metric_bundle_json(){' "$TMP_DIR/bundle-helpers.sh" \
  || fail 'p2_metric_bundle_json non extrait : le bornage awk a dérivé'

# p2_counter_value, p2_number_equal, p2_number_le, p2_segment_value_valid :
# validations fail-closed et petits calculs numériques atteignables sans
# Docker.
awk '
  /^p2_counter_value\(\)\{/ { capture = 1 }
  /^p2_write_phase_status\(\)\{/ { capture = 0 }
  capture { print }
' "$HARNESS" > "$TMP_DIR/validation-helpers.sh"
grep -q '^p2_segment_value_valid(){' "$TMP_DIR/validation-helpers.sh" \
  || fail 'p2_segment_value_valid non extrait : le bornage awk a dérivé'

# p2_cpu_steal_percent : calcul de delta pur (pas de dépendance /proc/stat).
awk '
  /^p2_cpu_steal_percent\(\)\{/ { capture = 1 }
  /^p2_metric_present\(\)\{/ { capture = 0 }
  capture { print }
' "$HARNESS" > "$TMP_DIR/cpu-steal-helper.sh"
grep -q '^p2_cpu_steal_percent(){' "$TMP_DIR/cpu-steal-helper.sh" \
  || fail 'p2_cpu_steal_percent non extrait : le bornage awk a dérivé'

source "$TMP_DIR/err-helper.sh"
source "$TMP_DIR/metric-helpers.sh"
source "$TMP_DIR/bundle-helpers.sh"
source "$TMP_DIR/validation-helpers.sh"
source "$TMP_DIR/cpu-steal-helper.sh"

# ---- fixture : sortie Prometheus synthétique, valeurs réelles du smoke2 ----
# Les valeurs P3 reprennent exactement celles consignées dans
# .remote/p3-smoke2-verdict.md pour la variante C qui a échoué : le snapshot
# contenait bien ces métriques, seule la construction jq du bundle a échoué.
write_snapshot(){
  local file="$1"
  cat > "$file" <<'PROM'
surch_index_postings_directory_bytes 103278432
surch_index_total_bytes 210000000
surch_jemalloc_allocated_bytes 190000000
surch_jemalloc_active_bytes 195000000
surch_jemalloc_resident_bytes 200000000
surch_jemalloc_retained_bytes 5000000
surch_postings_p2_integrity_bytes 2592256
surch_postings_p2_integrity_pages 79648
surch_postings_p2_verified_bytes 0
surch_postings_p2_hash_failures 0
surch_postings_p2_fallbacks 0
surch_postings_p2_fallback_fields 0
surch_postings_p2_term_occurrences 1360000
surch_postings_p2_blocks 500000
surch_postings_p2_fields 3
surch_postings_p2_term_payload_bytes 45000000
surch_postings_p2_csr_bytes 12000000
surch_postings_p2_directory_bytes 103278432
PROM
}

SNAPSHOT="$TMP_DIR/snapshot.prom"
write_snapshot "$SNAPSHOT"

# ==== T1 : variante C nominale — le VRAI bundle p3_integrity doit se
#           construire, avec la valeur directory_bytes correcte. C'est le
#           chemin qui était invalide en smoke2 avant le correctif. ====
set_step 'T1 — bundle p3_integrity, variante C nominale'
P2_VARIANT=C
P2_REQUIRE_P3_INTEGRITY=1
if ! p2_metric_bundle_json "$SNAPSHOT"; then
  fail "T1: p2_metric_bundle_json a échoué en nominal (raison=$P2_METRIC_BUNDLE_REASON)"
fi
jq -e . >/dev/null 2>&1 <<< "$P2_METRIC_BUNDLE_JSON" \
  || fail 'T1: P2_METRIC_BUNDLE_JSON n est pas un JSON valide'
[ "$(jq -r '.p3_integrity.directory_bytes' <<< "$P2_METRIC_BUNDLE_JSON")" = 103278432 ] \
  || fail 'T1: p3_integrity.directory_bytes ne vaut pas la valeur du snapshot (103278432)'
[ "$(jq -r '.p3_integrity.bytes' <<< "$P2_METRIC_BUNDLE_JSON")" = 2592256 ] \
  || fail 'T1: p3_integrity.bytes incorrect'
[ "$(jq -r '.p3_integrity.pages' <<< "$P2_METRIC_BUNDLE_JSON")" = 79648 ] \
  || fail 'T1: p3_integrity.pages incorrect'
[ "$(jq -r '.p3_integrity.hash_failures' <<< "$P2_METRIC_BUNDLE_JSON")" = 0 ] \
  || fail 'T1: p3_integrity.hash_failures incorrect'
[ "$(jq -r '.index.postings_directory_bytes' <<< "$P2_METRIC_BUNDLE_JSON")" = 103278432 ] \
  || fail 'T1: index.postings_directory_bytes incorrect'

# ==== T2 : variante A/B — p3_integrity doit rester null même si les
#           métriques P3 sont absentes du snapshot (elles ne sont jamais
#           publiées par A/B). ====
set_step 'T2 — bundle A/B, p3_integrity=null sans exiger les métriques P3'
SNAPSHOT_NO_P3="$TMP_DIR/snapshot-no-p3.prom"
head -n 6 "$SNAPSHOT" > "$SNAPSHOT_NO_P3"
P2_VARIANT=A
P2_REQUIRE_P3_INTEGRITY=0
if ! p2_metric_bundle_json "$SNAPSHOT_NO_P3"; then
  fail "T2: p2_metric_bundle_json a échoué pour A sans métriques P3 (raison=$P2_METRIC_BUNDLE_REASON)"
fi
[ "$(jq -r '.p3_integrity' <<< "$P2_METRIC_BUNDLE_JSON")" = null ] \
  || fail 'T2: p3_integrity doit être null pour la variante A'

# ==== T3 : métrique réellement absente (pas un bug jq) — le motif de
#           raison doit nommer la métrique manquante, jamais
#           prometheus_bundle_jq_error. C'est l'exigence de la tâche 4 :
#           distinguer une vraie absence d'une erreur de construction. ====
set_step 'T3 — métrique P3 réellement absente : raison metric_missing'
SNAPSHOT_MISSING="$TMP_DIR/snapshot-missing-directory.prom"
grep -v '^surch_postings_p2_directory_bytes ' "$SNAPSHOT" > "$SNAPSHOT_MISSING"
P2_VARIANT=C
P2_REQUIRE_P3_INTEGRITY=1
if p2_metric_bundle_json "$SNAPSHOT_MISSING"; then
  fail 'T3: p2_metric_bundle_json aurait dû échouer, la métrique directory_bytes est absente'
fi
[ "$P2_METRIC_BUNDLE_REASON" = 'prometheus_metric_missing_surch_postings_p2_directory_bytes' ] \
  || fail "T3: raison inattendue pour une métrique réellement absente : $P2_METRIC_BUNDLE_REASON"

# ==== T4 : réintroduction du bug historique — mutation d'une COPIE des
#           helpers extraits (jamais du fair-ab.sh commité) pour vérifier
#           mécaniquement que le motif exact du smoke2 est bien détecté et
#           bien classifié comme erreur jq, PAS comme métrique manquante.
#           Cette assertion mord : elle échoue si quelqu'un restaure
#           `directory_bytes:$directory` dans le futur. ====
set_step 'T4 — réintroduction mécanisée du bug smoke2 ($directory au lieu de $directory_bytes)'
grep -q 'directory_bytes:\$directory_bytes}' "$TMP_DIR/bundle-helpers.sh" \
  || fail 'T4: le motif corrigé directory_bytes:$directory_bytes est absent des helpers extraits — la correction a-t-elle régressé ?'
sed 's/directory_bytes:\$directory_bytes}/directory_bytes:$directory}/' \
  "$TMP_DIR/bundle-helpers.sh" > "$TMP_DIR/bundle-helpers-buggy.sh"
grep -q 'directory_bytes:\$directory}' "$TMP_DIR/bundle-helpers-buggy.sh" \
  || fail 'T4: la mutation sed n a pas pris, le test ne prouve rien'

(
  set -Eeuo pipefail
  source "$TMP_DIR/err-helper.sh"
  source "$TMP_DIR/metric-helpers.sh"
  source "$TMP_DIR/bundle-helpers-buggy.sh"
  P2_VARIANT=C
  P2_REQUIRE_P3_INTEGRITY=1
  if p2_metric_bundle_json "$SNAPSHOT" 2> "$TMP_DIR/t4-stderr.log"; then
    printf 'T4: BUG REINTRODUIT MAIS NON DETECTE\n' >&2
    exit 1
  fi
  if [ "$P2_METRIC_BUNDLE_REASON" != 'prometheus_bundle_jq_error' ]; then
    printf 'T4: raison incorrecte avec le bug réintroduit : attendu prometheus_bundle_jq_error, obtenu %s\n' \
      "$P2_METRIC_BUNDLE_REASON" >&2
    exit 1
  fi
  grep -q 'is not defined' "$TMP_DIR/t4-stderr.log" \
    || { printf 'T4: la sortie d erreur jq brute n a pas été journalisée\n' >&2; exit 1; }
) || fail 'T4: le bug historique réintroduit n a pas été détecté et classifié correctement — voir ci-dessus'
grep -q 'is not defined' "$TMP_DIR/t4-stderr.log" \
  || fail 'T4: le message err() doit contenir la sortie jq brute ("is not defined")'

# ==== T5 : p2_counter_value — absence de série = zéro implicite documenté,
#           présence = valeur réelle (pas de faux zéro qui masquerait un
#           vrai compteur). ====
set_step 'T5 — p2_counter_value : absence de série = zéro, présence = valeur réelle'
[ "$(p2_counter_value surch_absent_counter_total "$SNAPSHOT")" = '0' ] \
  || fail 'T5: une série absente doit rendre 0 (initialisation de compteur Prometheus)'
[ "$(p2_counter_value surch_postings_p2_blocks "$SNAPSHOT")" = '500000' ] \
  || fail 'T5: une série présente doit rendre sa vraie valeur, pas 0'

# ==== T6 : p2_number_equal / p2_number_le — piège maison "0 est vrai en
#           jq" : ici ce sont des awk, mais 0 doit rester un 0 numérique
#           valide, pas une fausse absence. ====
set_step 'T6 — comparaisons numériques fail-closed, y compris zéro'
p2_number_equal 0 0 || fail 'T6: 0 == 0 doit être vrai'
p2_number_equal 5 5 || fail 'T6: 5 == 5 doit être vrai'
if p2_number_equal 5 6; then fail 'T6: 5 == 6 ne doit pas être vrai'; fi
p2_number_le 0 0 || fail 'T6: 0 <= 0 doit être vrai'
p2_number_le 4 5 || fail 'T6: 4 <= 5 doit être vrai'
if p2_number_le 5 4; then fail 'T6: 5 <= 4 ne doit pas être vrai'; fi

# ==== T7 : p2_segment_value_valid — gate exact vs minimum. ====
set_step 'T7 — p2_segment_value_valid : gate exact et minimum'
P2_SEGMENT_GATE=exact
P2_REQUIRED_SEGMENTS=12
p2_segment_value_valid 12 || fail 'T7: gate exact, 12 segments doit passer'
if p2_segment_value_valid 13; then fail 'T7: gate exact, 13 segments ne doit pas passer'; fi
P2_SEGMENT_GATE=minimum
P2_REQUIRED_SEGMENTS=3
p2_segment_value_valid 3 || fail 'T7: gate minimum, exactement 3 doit passer'
p2_segment_value_valid 5 || fail 'T7: gate minimum, plus que 3 doit passer'
if p2_segment_value_valid 2; then fail 'T7: gate minimum, 2 (< 3) ne doit pas passer'; fi

# ==== T8 : p2_cpu_steal_percent — delta de compteurs cumulatifs /proc/stat,
#           et rejet fail-closed d un delta négatif (compteur qui régresse
#           = série invalide, jamais un steal négatif silencieux). ====
set_step 'T8 — p2_cpu_steal_percent : delta correct et rejet du delta négatif'
result=$(p2_cpu_steal_percent "1000 10" "2000 15") || fail 'T8: calcul nominal a échoué'
[ "$result" = '0.500000' ] \
  || fail "T8: 5/1000*100 attendu 0.500000, obtenu $result"
if p2_cpu_steal_percent "2000 15" "1000 10" 2>/dev/null; then
  fail 'T8: un total négatif (after < before) doit être rejeté fail-closed'
fi

printf '[test-p3-telemetry] OK — toutes les assertions sur le VRAI code fair-ab.sh sont passées\n' >&2
