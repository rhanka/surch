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

# Lot « harnais 4 axes » : ventilation par forme de requête (besoin 1) et
# ventilation disque par composant (besoin 4). Ce sont des fonctions pures
# (awk + fichiers), donc exerçables sans Docker ni moteur.
awk '
  /^probe_quantiles\(\)\{/ { capture = 1 }
  /^p2_index_ready_metric_names=\(/ { capture = 0 }
  capture { print }
' "$HARNESS" > "$TMP_DIR/form-helpers.sh"
for helper in probe_quantiles probe_form_vector probe_form_count probe_split_series_by_form \
  probe_mono_token_match_count probe_publish_form_stats by_form_ratios disk_ventilation_classify \
  by_form_pairing_report by_form_common_phases by_form_filter_phases by_form_ratios_publish \
  by_form_ratios_status cold_reclaim_evaluate cold_reclaim_audit_summary cold_axis_symmetry; do
  grep -q "^$helper(){" "$TMP_DIR/form-helpers.sh" \
    || fail "$helper non extrait : le bornage awk a dérivé"
done

# p2_write_phase_status : la vraie porte de validité par phase du protocole P2,
# où atterrit la preuve d'activation C2 (besoin 2). Elle ne lit que des
# fichiers Prometheus et écrit un JSONL : entièrement exerçable ici.
awk '
  /^p2_write_phase_status\(\)\{/ { capture = 1 }
  /^p2_extract_warm_bodies\(\)\{/ { capture = 0 }
  capture { print }
' "$HARNESS" > "$TMP_DIR/phase-status-helper.sh"
grep -q '^p2_write_phase_status(){' "$TMP_DIR/phase-status-helper.sh" \
  || fail 'p2_write_phase_status non extrait : le bornage awk a dérivé'

source "$TMP_DIR/err-helper.sh"
source "$TMP_DIR/metric-helpers.sh"
source "$TMP_DIR/bundle-helpers.sh"
source "$TMP_DIR/validation-helpers.sh"
source "$TMP_DIR/cpu-steal-helper.sh"
source "$TMP_DIR/form-helpers.sh"
source "$TMP_DIR/phase-status-helper.sh"
for helper in c2_expected_stream_delta c2_stream_phase_verdict c2_stream_phase_verdict_bounded \
  c1_metric_status c1_phase_record; do
  grep -q "^$helper(){" "$TMP_DIR/validation-helpers.sh" \
    || fail "$helper non extrait : le bornage awk a dérivé"
done

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

# ============================================================================
# Lot « harnais 4 axes » — les quatre besoins, exercés sur le VRAI code.
# ============================================================================

ENGINE=surch
OUT_DIR="$TMP_DIR/out"
P2_ASCIIFOLD_AWK="$ROOT_DIR/deploy/bench-local/p2-asciifold.awk"
[ -r "$P2_ASCIIFOLD_AWK" ] || fail 'table asciifolding introuvable'
DISK_VENTILATION_TOLERANCE_MIB=16
DISK_VENTILATION_TOLERANCE_PERCENT=1

# ==== T9 : besoin 1 — la forme est lue DANS LE CORPS, jamais déduite d'une
#           parité de ligne, et un corps inconnu est fail-closed. ====
set_step 'T9 — vecteur de formes lu dans les corps'
MIX="$TMP_DIR/mix.ndjson"
{
  printf '{"query":{"bool":{"must":[{"match":{"NOM":"MARTIN"}},{"match":{"PRENOMS":"JEAN"}}]}},"size":10}\n'
  printf '{"query":{"match":{"NOM":"MARTIN"}},"size":10}\n'
  printf '{"query":{"bool":{"must":[{"match":{"NOM":"DURAND"}},{"match":{"PRENOMS":"PAUL"}}]}},"size":10}\n'
  printf '{"query":{"match":{"NOM":"DURAND"}},"size":10}\n'
} > "$MIX"
FORMS="$TMP_DIR/mix.forms"
probe_form_vector "$MIX" "$FORMS" 4 || fail 'T9: vecteur de formes refusé sur un mix valide'
[ "$(probe_form_count "$FORMS" match)" = 2 ] || fail 'T9: comptage match incorrect'
[ "$(probe_form_count "$FORMS" bool)" = 2 ] || fail 'T9: comptage bool incorrect'
[ "$(head -n 1 "$FORMS")" = bool ] || fail 'T9: la première ligne du mix est un bool'
BAD="$TMP_DIR/bad.ndjson"
printf '{"query":{"match_all":{}},"size":10}\n' > "$BAD"
if probe_form_vector "$BAD" "$TMP_DIR/bad.forms" 1; then
  fail 'T9: un corps ni match ni bool.must doit être refusé, jamais rangé dans un fourre-tout'
fi
if probe_form_vector "$MIX" "$TMP_DIR/mix.forms.bad" 5; then
  fail 'T9: un nombre de corps différent de l attendu doit être refusé'
fi

# ==== T10 : besoin 1 — découpe alignée, et piège maison du `NR == FNR` avec un
#            premier fichier VIDE (qui consommerait un échantillon en silence). ====
set_step 'T10 — découpe des séries par forme, fichier de formes vide refusé'
SERIES="$TMP_DIR/series"
printf '1\n2\n3\n4\n' > "$SERIES"
probe_split_series_by_form "$SERIES" "$FORMS" match "$TMP_DIR/series.match" 2 \
  || fail 'T10: découpe match refusée'
[ "$(tr '\n' ' ' < "$TMP_DIR/series.match")" = '2 4 ' ] \
  || fail 'T10: la découpe match ne rend pas les lignes 2 et 4'
probe_split_series_by_form "$SERIES" "$FORMS" bool "$TMP_DIR/series.bool" 2 \
  || fail 'T10: découpe bool refusée'
[ "$(tr '\n' ' ' < "$TMP_DIR/series.bool")" = '1 3 ' ] \
  || fail 'T10: la découpe bool ne rend pas les lignes 1 et 3'
if probe_split_series_by_form "$SERIES" "$FORMS" match "$TMP_DIR/series.match" 3; then
  fail 'T10: un compte attendu faux doit être refusé'
fi
: > "$TMP_DIR/forms.empty"
if probe_split_series_by_form "$SERIES" "$TMP_DIR/forms.empty" match "$TMP_DIR/series.x" 0; then
  fail 'T10: un vecteur de formes vide doit être refusé (piège NR == FNR)'
fi

# ==== T11 : besoin 2 — borne BASSE des corps `match` mono-token. Elle doit
#            rester une SOUS-estimation : accents pliés = mono, espace/tiret =
#            multi. Une surestimation invaliderait à tort un run. ====
set_step 'T11 — borne basse mono-token des corps match'
MONO="$TMP_DIR/mono.ndjson"
{
  printf '{"query":{"match":{"NOM":"MARTIN"}},"size":10}\n'
  printf '{"query":{"match":{"NOM":"ÉVRARD"}},"size":10}\n'
  printf '{"query":{"match":{"NOM":"LE GALL"}},"size":10}\n'
  printf '{"query":{"match":{"NOM":"MARTIN-DUPONT"}},"size":10}\n'
  printf '{"query":{"bool":{"must":[{"match":{"NOM":"DURAND"}},{"match":{"PRENOMS":"PAUL"}}]}},"size":0}\n'
} > "$MONO"
[ "$(probe_mono_token_match_count "$MONO" NOM)" = 2 ] \
  || fail 'T11: seuls MARTIN et ÉVRARD doivent compter comme mono-token'

# ==== T12 : besoin 1 — publication effective des quantiles PAR FORME, et
#            invalidité si une série est incomplète. ====
set_step 'T12 — publication des quantiles par forme'
mkdir -p "$OUT_DIR"
PROBE_BY_FORM_JSONL="$OUT_DIR/by-form.jsonl"
: > "$PROBE_BY_FORM_JSONL"
printf '0.010\n0.020\n0.030\n0.040\n' > "$OUT_DIR/client_s"
printf '10\n20\n30\n40\n' > "$OUT_DIR/took_ms"
printf '1\n2\n3\n4\n' > "$OUT_DIR/probe_ms"
probe_publish_form_stats mixte "$MIX" "$OUT_DIR/client_s" "$OUT_DIR/took_ms" "$OUT_DIR/probe_ms" 4 \
  || fail "T12: publication par forme refusée (raison=$PROBE_FORM_STATS_REASON)"
[ "$(wc -l < "$PROBE_BY_FORM_JSONL")" -eq 6 ] \
  || fail 'T12: 2 formes × 3 métriques = 6 enregistrements attendus'
jq -se '
  ([.[] | .form] | sort | unique) == ["bool","match"]
  and all(.[]; .n == 2 and .phase == "mixte" and .engine == "surch")
  and (first(.[] | select(.form == "match" and .metric == "took")) | .p50 == 20 and .p95 == 40)
  and (first(.[] | select(.form == "bool" and .metric == "took")) | .p50 == 10 and .p95 == 30)
  and (first(.[] | select(.form == "match" and .metric == "client")) | .p50 == 20)
' "$PROBE_BY_FORM_JSONL" | grep -qx true \
  || { cat "$PROBE_BY_FORM_JSONL" >&2; fail 'T12: les quantiles par forme ne sont ni séparés ni corrects'; }
printf '10\n20\n30\n' > "$OUT_DIR/took_court"
if probe_publish_form_stats tronque "$MIX" "$OUT_DIR/client_s" "$OUT_DIR/took_court" "$OUT_DIR/probe_ms" 4; then
  fail 'T12: une série tronquée doit rendre la ventilation invalide'
fi

# ==== T12bis : besoin 1 — ratios surch/ES PAR FORME, appariés, sans agrégat.
#               Un ratio calculé entre deux populations différentes serait
#               exactement la faute du « 4,13× ». ====
set_step 'T12bis — ratios par forme appariés, jamais agrégés'
ES_FORMS="$OUT_DIR/es.by-form.jsonl"
SURCH_FORMS="$OUT_DIR/surch.by-form.jsonl"
{
  printf '{"engine":"es","phase":"random","form":"match","metric":"took","unit":"ms","n":500,"raw_file":"/e/m","p50":10,"p95":50,"p99":100}\n'
  printf '{"engine":"es","phase":"random","form":"bool","metric":"took","unit":"ms","n":500,"raw_file":"/e/b","p50":20,"p95":60,"p99":120}\n'
} > "$ES_FORMS"
{
  printf '{"engine":"surch","phase":"random","form":"match","metric":"took","unit":"ms","n":500,"raw_file":"/s/m","p50":25,"p95":200,"p99":300}\n'
  printf '{"engine":"surch","phase":"random","form":"bool","metric":"took","unit":"ms","n":500,"raw_file":"/s/b","p50":10,"p95":30,"p99":60}\n'
} > "$SURCH_FORMS"
by_form_ratios "$ES_FORMS" "$SURCH_FORMS" "$OUT_DIR/by-form-ratios.jsonl" \
  || fail "T12bis: ratios par forme refusés (raison=$BY_FORM_RATIOS_REASON)"
jq -se '
  length == 2
  and all(.[]; .aggregate == false and .n == 500 and .phase == "random")
  and (first(.[] | select(.form == "match")) | .surch_over_es.p95 == 4 and .surch_over_es.p50 == 2.5)
  and (first(.[] | select(.form == "bool")) | .surch_over_es.p95 == 0.5)
' "$OUT_DIR/by-form-ratios.jsonl" | grep -qx true \
  || { cat "$OUT_DIR/by-form-ratios.jsonl" >&2; fail 'T12bis: les ratios par forme sont faux ou agrégés'; }
# Effectifs différents entre moteurs : refus, jamais un ratio entre populations
# distinctes.
jq -c '.n = 400' "$SURCH_FORMS" > "$OUT_DIR/surch.by-form.desapparie.jsonl"
if by_form_ratios "$ES_FORMS" "$OUT_DIR/surch.by-form.desapparie.jsonl" "$OUT_DIR/ratios-desapparies.jsonl" 2>/dev/null; then
  fail 'T12bis: des effectifs différents doivent refuser le ratio'
fi
case "$BY_FORM_RATIOS_REASON" in
  by_form_population_mismatch_only_es_0_only_surch_0_n_mismatch_2_*) ;;
  *) fail "T12bis: raison inattendue pour un appariement rompu : $BY_FORM_RATIOS_REASON";;
esac
[ ! -e "$OUT_DIR/ratios-desapparies.jsonl" ] \
  || fail 'T12bis: un appariement rompu ne doit laisser AUCUN fichier de ratios (même vide)'
# Forme absente d'un côté : refus également.
head -n 1 "$ES_FORMS" > "$OUT_DIR/es.by-form.partiel.jsonl"
if by_form_ratios "$OUT_DIR/es.by-form.partiel.jsonl" "$SURCH_FORMS" "$OUT_DIR/ratios-partiels.jsonl" 2>/dev/null; then
  fail 'T12bis: une forme absente côté ES doit refuser le ratio'
fi

# ==== T13 : besoin 4 — ventilation disque par composant, fail-closed. ====
set_step 'T13 — ventilation disque par composant et réconciliation avec du'
NOMINAL_LISTING=$'104857600 surch-postings-0\n52428800 surch-postings-1\n20971520 surch-subfields-0\n314572800 surch-source-0\n1048576 autre-fichier'
disk_ventilation_classify "$NOMINAL_LISTING" 471 \
  || fail "T13: ventilation nominale refusée (raison=$DISK_VENT_REASON)"
[ "$DISK_VENT_VALID" = true ] || fail 'T13: la ventilation nominale doit être valide'
[ "$DISK_VENT_POSTINGS" = 157286400 ] || fail 'T13: total postings incorrect'
[ "$DISK_VENT_POSTINGS_FILES" = 2 ] || fail 'T13: nombre de fichiers postings incorrect'
[ "$DISK_VENT_SOURCE" = 314572800 ] || fail 'T13: total _source incorrect'
[ "$DISK_VENT_SUBFIELDS" = 20971520 ] || fail 'T13: total subfields incorrect'
[ "$DISK_VENT_FST_MERGE" = 0 ] || fail 'T13: fst/merge doit rester nul sur ce jeu'
[ "$DISK_VENT_OTHER" = 1048576 ] || fail 'T13: le reste non classé doit être compté, pas ignoré'
[ "$DISK_VENT_FILES" = 5 ] || fail 'T13: nombre de fichiers incorrect'
if disk_ventilation_classify "" 471; then fail 'T13: une liste vide doit être invalide'; fi
[ "$DISK_VENT_REASON" = disk_ventilation_listing_empty ] || fail 'T13: motif liste vide incorrect'
if disk_ventilation_classify $'314572800 surch-source-0' 300; then
  fail 'T13: une ventilation sans postings doit être invalide'
fi
[ "$DISK_VENT_REASON" = disk_ventilation_postings_zero ] || fail 'T13: motif postings nuls incorrect'
if disk_ventilation_classify $'104857600 surch-postings-0' 100; then
  fail 'T13: une ventilation sans _source doit être invalide'
fi
[ "$DISK_VENT_REASON" = disk_ventilation_source_zero ] || fail 'T13: motif _source nul incorrect'
if disk_ventilation_classify "$NOMINAL_LISTING" '?'; then
  fail 'T13: un du illisible doit être invalide, pas une réconciliation implicite'
fi
case "$DISK_VENT_REASON" in disk_ventilation_du_unreadable_*) ;; *) fail 'T13: motif du illisible incorrect';; esac
if disk_ventilation_classify "$NOMINAL_LISTING" 1000; then
  fail 'T13: une somme qui ne boucle pas avec du doit être invalide'
fi
case "$DISK_VENT_REASON" in disk_ventilation_sum_*) ;; *) fail 'T13: motif de réconciliation incorrect';; esac
# Bornes de tolérance : 471 Mio de composants, tolérance = max(1 % de du, 16 Mio).
disk_ventilation_classify "$NOMINAL_LISTING" 487 \
  || fail 'T13: un écart exactement à la tolérance doit rester valide'
if disk_ventilation_classify "$NOMINAL_LISTING" 488; then
  fail 'T13: un écart d un Mio au-delà de la tolérance doit être invalide'
fi
if disk_ventilation_classify $'abc surch-postings-0\n1 surch-source-0' 1; then
  fail 'T13: une taille non numérique doit être invalide'
fi
[ "$DISK_VENT_REASON" = disk_ventilation_size_non_numeric ] || fail 'T13: motif taille non numérique incorrect'

# ==== T14 : besoin 2 — attente EXACTE du compteur C2, dans les deux sens. ====
set_step 'T14 — preuve C2, attente exacte'
SURCH_C2_STREAM_METRIC=surch_dbg_c2_single_term_stream_total
write_c2_snapshot(){
  local file="$1"
  local value="$2"
  printf 'surch_index_segment_count{index="deces_bench"} 12\n' > "$file"
  [ "$value" = absent ] || printf '%s %s\n' "$SURCH_C2_STREAM_METRIC" "$value" >> "$file"
}
C2_BEFORE="$TMP_DIR/c2.before.prom"
C2_AFTER="$TMP_DIR/c2.after.prom"
SURCH_C2_STREAM_EXPECT=stream
write_c2_snapshot "$C2_BEFORE" 1000
write_c2_snapshot "$C2_AFTER" 1100
c2_stream_phase_verdict "$C2_BEFORE" "$C2_AFTER" 100 \
  || fail "T14: delta exact de 100 refusé (raison=$C2_STREAM_REASON)"
[ "$C2_STREAM_DELTA" = 100 ] || fail 'T14: delta calculé incorrect'
[ "$C2_STREAM_EXPECTED" = 100 ] || fail 'T14: attente calculée incorrecte'
write_c2_snapshot "$C2_AFTER" 1099
if c2_stream_phase_verdict "$C2_BEFORE" "$C2_AFTER" 100; then
  fail 'T14: un déclin d une seule requête doit rendre la phase invalide'
fi
case "$C2_STREAM_REASON" in c2_stream_delta_99_expected_100) ;; *) fail "T14: motif de delta incorrect : $C2_STREAM_REASON";; esac
# Déclin SILENCIEUX intégral : la série n'existe même pas après la phase. Le
# zéro implicite de `p2_counter_value` ne doit jamais devenir un succès.
write_c2_snapshot "$C2_BEFORE" absent
write_c2_snapshot "$C2_AFTER" absent
if c2_stream_phase_verdict "$C2_BEFORE" "$C2_AFTER" 100; then
  fail 'T14: compteur absent après une phase match doit être INVALIDE, pas un zéro silencieux'
fi
case "$C2_STREAM_REASON" in c2_stream_metric_absent_after_100_match_requests) ;; *) fail "T14: motif d absence incorrect : $C2_STREAM_REASON";; esac
# Phase bool : rien ne doit streamer, et un compteur qui bouge est une anomalie.
write_c2_snapshot "$C2_BEFORE" 1000
write_c2_snapshot "$C2_AFTER" 1000
c2_stream_phase_verdict "$C2_BEFORE" "$C2_AFTER" 0 \
  || fail 'T14: une phase bool sans delta doit être valide'
write_c2_snapshot "$C2_AFTER" 1001
if c2_stream_phase_verdict "$C2_BEFORE" "$C2_AFTER" 0; then
  fail 'T14: une phase bool qui incrémente le compteur C2 doit être invalide'
fi
# Mode témoin : l'image antérieure à C2 ne doit rien streamer du tout.
SURCH_C2_STREAM_EXPECT=reference
write_c2_snapshot "$C2_BEFORE" absent
write_c2_snapshot "$C2_AFTER" absent
c2_stream_phase_verdict "$C2_BEFORE" "$C2_AFTER" 100 \
  || fail 'T14: en mode reference, un compteur absent est le comportement attendu'
write_c2_snapshot "$C2_AFTER" 1
if c2_stream_phase_verdict "$C2_BEFORE" "$C2_AFTER" 100; then
  fail 'T14: en mode reference, un compteur qui bouge doit être invalide'
fi
SURCH_C2_STREAM_EXPECT=off
if c2_stream_phase_verdict "$C2_BEFORE" "$C2_AFTER" 100; then
  fail 'T14: sans attente déclarée, la vérification ne doit jamais réussir'
fi
[ "$C2_STREAM_REASON" = c2_stream_expectation_off ] || fail 'T14: motif « attente absente » incorrect'

# ==== T15 : besoin 2 — attente ENCADRÉE hors protocole P2 (corps du corpus
#            brut, proportion mono-token non connue exactement). ====
set_step 'T15 — preuve C2, attente encadrée'
SURCH_C2_STREAM_EXPECT=stream
write_c2_snapshot "$C2_BEFORE" 0
write_c2_snapshot "$C2_AFTER" 420
c2_stream_phase_verdict_bounded "$C2_BEFORE" "$C2_AFTER" 500 400 \
  || fail "T15: un delta dans [400;500] doit être valide (raison=$C2_STREAM_REASON)"
write_c2_snapshot "$C2_AFTER" 399
if c2_stream_phase_verdict_bounded "$C2_BEFORE" "$C2_AFTER" 500 400; then
  fail 'T15: un delta sous la borne basse mono-token doit être invalide'
fi
write_c2_snapshot "$C2_AFTER" 501
if c2_stream_phase_verdict_bounded "$C2_BEFORE" "$C2_AFTER" 500 400; then
  fail 'T15: un delta au-dessus du nombre de match doit être invalide'
fi
# Déclin global : zéro requête streamée alors que la borne basse est > 0.
write_c2_snapshot "$C2_BEFORE" absent
write_c2_snapshot "$C2_AFTER" absent
if c2_stream_phase_verdict_bounded "$C2_BEFORE" "$C2_AFTER" 500 400; then
  fail 'T15: un déclin global doit être invalide même en attente encadrée'
fi

# ==== T16 : besoin 2 de bout en bout — la preuve C2 traverse RÉELLEMENT
#            `p2_write_phase_status`, la porte de validité par phase du
#            protocole P2, et se retrouve dans le JSONL de statut. ====
set_step 'T16 — p2_write_phase_status porte la preuve C2'
P2_VARIANT=B
P2_REQUIRE_P3_INTEGRITY=0
P2_SEGMENT_GATE=exact
P2_REQUIRED_SEGMENTS=12
P2_INTEGRITY_MAX_BYTES=$((32 * 1024 * 1024))
P2_EXECUTION_ID=00000000-0000-4000-8000-000000000042
P2_CPU_STEAL_MAX_PERCENT=1
P2_PHASE_STATUS="$OUT_DIR/phase-status.jsonl"
SURCH_C2_STREAM_EXPECT=stream
: > "$P2_PHASE_STATUS"
write_c2_snapshot "$C2_BEFORE" 1000
write_c2_snapshot "$C2_AFTER" 1100
p2_write_phase_status match_control 0 100 "$C2_BEFORE" "$C2_AFTER" 0 true '' \
  || fail 'T16: une phase match_control conforme doit rester valide'
jq -se '
  length == 1
  and (.[0] | .valid == true and .phase == "match_control"
       and .c2_stream.expect == "stream" and .c2_stream.checked == true
       and .c2_stream.delta == 100 and .c2_stream.expected_delta == 100
       and .c2_stream.metric_present_after == true
       and .c2_stream.metric == "surch_dbg_c2_single_term_stream_total")
' "$P2_PHASE_STATUS" | grep -qx true \
  || { cat "$P2_PHASE_STATUS" >&2; fail 'T16: le statut de phase ne porte pas la preuve C2 attendue'; }
: > "$P2_PHASE_STATUS"
write_c2_snapshot "$C2_AFTER" 1000
if p2_write_phase_status match_control 0 100 "$C2_BEFORE" "$C2_AFTER" 0 true ''; then
  fail 'T16: un chemin C2 décliné doit rendre la phase INVALIDE, jamais performante'
fi
jq -se '.[0] | .valid == false and (.reason | test("^c2_stream_delta_0_expected_100$"))' \
  "$P2_PHASE_STATUS" | grep -qx true \
  || { cat "$P2_PHASE_STATUS" >&2; fail 'T16: le motif du déclin C2 n est pas consigné'; }
# `off` : rien n'est vérifié, mais le statut le DIT — pas de preuve implicite.
: > "$P2_PHASE_STATUS"
SURCH_C2_STREAM_EXPECT=off
p2_write_phase_status match_control 0 100 "$C2_BEFORE" "$C2_AFTER" 0 true '' \
  || fail 'T16: sans attente déclarée, la phase reste valide sur les autres critères'
jq -se '.[0] | .c2_stream.checked == false and .c2_stream.expect == "off" and .c2_stream.delta == null' \
  "$P2_PHASE_STATUS" | grep -qx true \
  || { cat "$P2_PHASE_STATUS" >&2; fail 'T16: l absence de vérification C2 doit être explicite dans le statut'; }

# ============================================================================
# Lot « sonde froide réalisable » : le régime froid est le seul qui teste le
# comportement disk-backed. Il n'est pas supprimé — il est rendu RÉALISABLE
# (cible sur le cache de pages, fraction évincée mesurée) et HONNÊTE (seuil
# pré-engagé, symétrie entre moteurs, invalidités nommées).
# ============================================================================

# ==== T17 : le verdict d'une tentative de reclaim porte sur la FRACTION
#            RÉELLEMENT ÉVINCÉE, pas sur le code de retour de l'écriture.
#            Le cas PARTIEL est celui qui bloquait la campagne 28 M. ====
set_step 'T17 — reclaim partiel : accepté au-dessus du seuil, nommé en dessous'
COLD_FILE_CACHE_FLOOR_BYTES=4194304
COLD_RECLAIM_MIN_EVICTED_PERCENT=80
COLD_RECLAIM_REQUEST_PERCENT=90

# Éviction quasi totale : froid, évidemment.
cold_reclaim_evaluate 1000000000 50000000 \
  || fail "T17: 95 % d'éviction doit être accepté (verdict=$COLD_RECLAIM_VERDICT)"
[ "$COLD_RECLAIM_EVICTED_PERCENT" = 95 ] || fail "T17: fraction évincée incorrecte ($COLD_RECLAIM_EVICTED_PERCENT)"
[ "$COLD_RECLAIM_VERDICT" = ok ] || fail "T17: verdict incorrect ($COLD_RECLAIM_VERDICT)"

# Reclaim PARTIEL exactement au seuil pré-engagé : accepté, et c'est tout le
# point du lot — le noyau rend EAGAIN bien avant 100 % sur un hôte sans swap.
cold_reclaim_evaluate 1000000000 200000000 \
  || fail 'T17: une éviction exactement au seuil pré-engagé doit être acceptée'
[ "$COLD_RECLAIM_EVICTED_PERCENT" = 80 ] || fail 'T17: fraction au seuil incorrecte'
[ "$COLD_RECLAIM_GAP_PERCENT" = 0 ] || fail 'T17: écart au seuil non nul alors que le seuil est atteint'

# Reclaim PARTIEL un point sous le seuil : REFUS nommé, avec l'écart exact.
if cold_reclaim_evaluate 1000000000 201000000; then
  fail 'T17: une éviction sous le seuil pré-engagé doit être refusée'
fi
[ "$COLD_RECLAIM_VERDICT" = below_floor ] || fail "T17: verdict de refus incorrect ($COLD_RECLAIM_VERDICT)"
[ "$COLD_RECLAIM_EVICTED_PERCENT" = 79 ] || fail 'T17: la troncature entière doit rester conservatrice (79, pas 80)'
[ "$COLD_RECLAIM_GAP_PERCENT" = 1 ] || fail "T17: l'écart exact au seuil doit être journalisé (reçu $COLD_RECLAIM_GAP_PERCENT)"

# Reclaim franchement partiel : refus, écart de 20 points.
if cold_reclaim_evaluate 1000000000 400000000; then
  fail 'T17: 60 % d éviction doit être refusé à seuil 80'
fi
[ "$COLD_RECLAIM_GAP_PERCENT" = 20 ] || fail 'T17: écart au seuil incorrect pour 60 %'

# Plancher absolu : quelques Mio résiduels sont froids quelle que soit la
# fraction relative (sinon un cgroup déjà froid serait déclaré chaud).
cold_reclaim_evaluate 3000000 3000000 \
  || fail 'T17: un résidu sous le plancher absolu doit rester froid'
[ "$COLD_RECLAIM_VERDICT" = ok_residual_under_floor ] || fail 'T17: verdict de plancher absolu incorrect'

# Entrées non numériques : fail-closed, jamais une fraction inventée.
if cold_reclaim_evaluate '' 10; then fail 'T17: un file_before illisible doit être invalide'; fi
[ "$COLD_RECLAIM_VERDICT" = file_before_invalid ] || fail 'T17: motif file_before invalide incorrect'
if cold_reclaim_evaluate 100 'x'; then fail 'T17: un file_after illisible doit être invalide'; fi
[ "$COLD_RECLAIM_VERDICT" = file_after_invalid ] || fail 'T17: motif file_after invalide incorrect'

# Le seuil est un PARAMÈTRE PRÉ-ENGAGÉ, pas une constante cachée : le desserrer
# doit visiblement changer le verdict (et c'est pour cela qu'il est publié dans
# la scorecard et figé sous protocole P2).
COLD_RECLAIM_MIN_EVICTED_PERCENT=60
cold_reclaim_evaluate 1000000000 400000000 \
  || fail 'T17: à seuil 60, 60 % d éviction doit être accepté'
COLD_RECLAIM_MIN_EVICTED_PERCENT=80

# ==== T18 : le degré de froideur obtenu est MESURÉ et RÉCAPITULÉ. Une sonde
#            froide dont on ignore la fraction évincée ne vaut rien. ====
set_step 'T18 — synthèse de l audit de reclaim (min/p50/max, tentatives sous le seuil)'
COLD_AUDIT="$OUT_DIR/cold_reclaim.tsv"
{
  printf '1\t1000000000\t50000000\t200000000\t4000000000\t900000000\t0\t950000000\t95\t80\tok\n'
  printf '2\t1000000000\t200000000\t200000000\t4000000000\t900000000\t1\t800000000\t80\t80\tok\n'
  printf '3\t1000000000\t400000000\t200000000\t4000000000\t900000000\t1\t600000000\t60\t80\tbelow_floor\n'
} > "$COLD_AUDIT"
cold_reclaim_audit_summary "$COLD_AUDIT" || fail 'T18: la synthèse de l audit doit aboutir'
[ "$COLD_AUDIT_RECORDS" = 3 ] || fail "T18: effectif d audit incorrect ($COLD_AUDIT_RECORDS)"
[ "$COLD_AUDIT_EVICTED_MIN_PERCENT" = 60 ] || fail 'T18: minimum d éviction incorrect'
[ "$COLD_AUDIT_EVICTED_MAX_PERCENT" = 95 ] || fail 'T18: maximum d éviction incorrect'
[ "$COLD_AUDIT_EVICTED_P50_PERCENT" = 80 ] || fail 'T18: médiane d éviction incorrecte'
[ "$COLD_AUDIT_BELOW_FLOOR" = 1 ] || fail 'T18: le décompte des tentatives sous le seuil est faux'
if cold_reclaim_audit_summary "$OUT_DIR/audit-absent.tsv"; then
  fail 'T18: un audit absent ne doit pas produire de synthèse'
fi
[ "$COLD_AUDIT_EVICTED_MIN_PERCENT" = null ] \
  || fail 'T18: sans audit, la fraction publiée doit être null — jamais 0'

# ==== T19 : SYMÉTRIE entre moteurs. Un moteur qui obtient sa série froide et
#            pas l'autre rend l'axe froid INVALIDE, nommément. ====
set_step 'T19 — symétrie de l axe froid entre moteurs'
COLD_RECLAIM_REQUEST_PERCENT=90
printf '{"engine":"es","cold_axis_valid":true}\n' > "$OUT_DIR/es.json"
printf '{"engine":"surch","cold_axis_valid":true}\n' > "$OUT_DIR/surch.json"
cold_axis_symmetry "$OUT_DIR/es.json" "$OUT_DIR/surch.json" "$OUT_DIR/cold-axis.json" \
  || fail "T19: deux séries froides complètes doivent être symétriques ($COLD_SYMMETRY_STATUS)"
[ "$COLD_SYMMETRY_STATUS" = symmetric_valid ] || fail 'T19: statut de symétrie incorrect'
jq -e '.cold_axis_valid == true and .status == "symmetric_valid" and .min_evicted_percent_engaged == 80' \
  "$OUT_DIR/cold-axis.json" > /dev/null \
  || { cat "$OUT_DIR/cold-axis.json" >&2; fail 'T19: le seuil pré-engagé doit être publié avec le verdict'; }

printf '{"engine":"surch","cold_axis_valid":false,"cold_axis_invalid_reason":"reclaim_count_27_of_50"}\n' > "$OUT_DIR/surch.json"
if cold_axis_symmetry "$OUT_DIR/es.json" "$OUT_DIR/surch.json" "$OUT_DIR/cold-axis.json"; then
  fail 'T19: une série froide obtenue par un seul moteur ne peut pas être valide'
fi
[ "$COLD_SYMMETRY_STATUS" = asymmetric ] || fail "T19: statut asymétrique attendu ($COLD_SYMMETRY_STATUS)"
[ "$COLD_SYMMETRY_REASON" = cold_asymmetry_es_valid_surch_invalid ] \
  || fail "T19: motif d asymétrie non diagnosticable : $COLD_SYMMETRY_REASON"
jq -e '.cold_axis_valid == false and .status == "asymmetric"' "$OUT_DIR/cold-axis.json" > /dev/null \
  || fail 'T19: l artefact de symétrie ne porte pas l invalidité'

# Absence des DEUX côtés : symétrique, donc pas un biais de comparaison — mais
# l'axe froid reste invalide et le dit.
printf '{"engine":"es","cold_axis_valid":false}\n' > "$OUT_DIR/es.json"
if cold_axis_symmetry "$OUT_DIR/es.json" "$OUT_DIR/surch.json" "$OUT_DIR/cold-axis.json"; then
  fail 'T19: un axe froid absent des deux côtés ne peut pas être déclaré valide'
fi
[ "$COLD_SYMMETRY_STATUS" = symmetric_absent ] || fail 'T19: statut d absence symétrique incorrect'
case "$COLD_SYMMETRY_REASON" in
  cold_axis_unmeasured_on_both_engines_*) ;;
  *) fail "T19: motif d absence symétrique non nommé : $COLD_SYMMETRY_REASON";;
esac

# ==== T20 : effectifs par forme DIVERGENTS — le cas exact qui a produit un
#            `by-form-ratios.jsonl` de 0 octet. Invalidité nommée, aucun
#            fichier vide, et repli EXPLICITE sur les phases communes. ====
set_step 'T20 — effectifs divergents : invalidité nommée, jamais un fichier de ratios vide'
ES_COLD="$OUT_DIR/es.cold-asym.jsonl"
SURCH_COLD="$OUT_DIR/surch.cold-asym.jsonl"
{
  printf '{"engine":"es","phase":"random","form":"match","metric":"took","unit":"ms","n":500,"raw_file":"/e/m","p50":10,"p95":50,"p99":100}\n'
  printf '{"engine":"es","phase":"random","form":"bool","metric":"took","unit":"ms","n":500,"raw_file":"/e/b","p50":20,"p95":60,"p99":120}\n'
} > "$ES_COLD"
{
  printf '{"engine":"surch","phase":"random","form":"match","metric":"took","unit":"ms","n":500,"raw_file":"/s/m","p50":25,"p95":200,"p99":300}\n'
  printf '{"engine":"surch","phase":"random","form":"bool","metric":"took","unit":"ms","n":500,"raw_file":"/s/b","p50":10,"p95":30,"p99":60}\n'
  printf '{"engine":"surch","phase":"cold","form":"match","metric":"took","unit":"ms","n":50,"raw_file":"/s/c","p50":90,"p95":120,"p99":150}\n'
} > "$SURCH_COLD"
RATIOS_ASYM="$OUT_DIR/ratios-cold-asym.jsonl"
if by_form_ratios "$ES_COLD" "$SURCH_COLD" "$RATIOS_ASYM" 2>/dev/null; then
  fail 'T20: des populations divergentes ne doivent JAMAIS produire de ratios'
fi
case "$BY_FORM_RATIOS_REASON" in
  by_form_population_mismatch_only_es_0_only_surch_1_*) ;;
  *) fail "T20: raison non diagnosticable pour une divergence d effectifs : $BY_FORM_RATIOS_REASON";;
esac
[ ! -e "$RATIOS_ASYM" ] || fail 'T20: aucun fichier de ratios ne doit subsister (0 octet compris)'
[ -s "$RATIOS_ASYM.pairing.json" ] || fail 'T20: le rapport d appariement doit nommer les clés fautives'
jq -e '.only_surch == ["[\"cold\",\"match\",\"took\"]"] and (.only_es | length) == 0' \
  "$RATIOS_ASYM.pairing.json" > /dev/null \
  || { cat "$RATIOS_ASYM.pairing.json" >&2; fail 'T20: le rapport d appariement ne désigne pas la clé fautive'; }

# Repli : les ratios CHAUDS restent calculables sur les phases strictement
# appariées, la phase froide est NOMMÉMENT écartée, et l'état est écrit.
STATUS_ASYM="$OUT_DIR/ratios-cold-asym.status.json"
by_form_ratios_publish "$ES_COLD" "$SURCH_COLD" "$RATIOS_ASYM" "$STATUS_ASYM" 2>/dev/null \
  || fail "T20: le repli sur les phases communes doit aboutir ($BY_FORM_PUBLISH_REASON)"
[ "$BY_FORM_PUBLISH_SCOPE" = common_phases ] || fail 'T20: le périmètre publié doit être explicitement restreint'
jq -e '
  .scope == "common_phases" and .valid == true
  and .reason == "cold_phase_excluded_from_ratios"
  and (.included_phases | test("random"))
  and (.excluded_phases | test("cold"))
  and .ratio_records == 2
' "$STATUS_ASYM" > /dev/null \
  || { cat "$STATUS_ASYM" >&2; fail 'T20: l état de publication ne nomme pas la phase écartée'; }
jq -se 'length == 2 and all(.[]; .phase == "random")' "$RATIOS_ASYM" | grep -qx true \
  || { cat "$RATIOS_ASYM" >&2; fail 'T20: les ratios publiés doivent porter uniquement sur les phases appariées'; }

# Une divergence HORS phase froide n'est pas l'asymétrie connue : elle est un
# défaut, et le repli doit la refuser.
ES_BAD="$OUT_DIR/es.random-bad.jsonl"
head -n 1 "$ES_COLD" > "$ES_BAD"
head -n 2 "$SURCH_COLD" > "$OUT_DIR/surch.random-bad.jsonl"
if by_form_ratios_publish "$ES_BAD" "$OUT_DIR/surch.random-bad.jsonl" \
     "$OUT_DIR/ratios-bad.jsonl" "$OUT_DIR/ratios-bad.status.json" 2>/dev/null; then
  fail 'T20: une divergence hors phase froide ne doit pas être repliée en silence'
fi
case "$BY_FORM_PUBLISH_REASON" in
  by_form_no_common_phase|by_form_population_mismatch_outside_cold_phase_*) ;;
  *) fail "T20: motif de refus hors phase froide incorrect : $BY_FORM_PUBLISH_REASON";;
esac
[ -s "$OUT_DIR/ratios-bad.status.json" ] \
  || fail 'T20: même en échec, l état de publication doit exister — rien ne passe sous silence'
jq -e '.valid == false and .ratio_records == 0 and .ratios_file == null' \
  "$OUT_DIR/ratios-bad.status.json" > /dev/null \
  || { cat "$OUT_DIR/ratios-bad.status.json" >&2; fail 'T20: l état d échec doit dire qu aucun ratio n existe'; }

# ==== T21 : compteurs C1 — les NOMS doivent être ceux du moteur, et un
#            compteur ATTENDU mais absent doit produire une invalidité nommée,
#            jamais un zéro silencieux. ====
set_step 'T21 — compteurs C1 : noms réels, absence != zéro'
C1_REQUIRED_DEFAULT=$(awk -F'-' '
  /^SURCH_C1_METRICS_REQUIRED=/ { sub(/^[^-]*-/, ""); sub(/}"$/, ""); print; exit }
' "$HARNESS")
C1_OPTIONAL_DEFAULT=$(awk -F'-' '
  /^SURCH_C1_METRICS_OPTIONAL=/ { sub(/^[^-]*-/, ""); sub(/}"$/, ""); print; exit }
' "$HARNESS")
[ -n "$C1_REQUIRED_DEFAULT" ] && [ -n "$C1_OPTIONAL_DEFAULT" ] \
  || fail 'T21: les listes de compteurs C1 par défaut sont introuvables dans fair-ab.sh'
# Garde-fou mécanisé contre le bug d origine : deux des quatre noms cités dans
# les briefs (`surch_dbg_c1_scored_total`, `surch_dbg_c1_early_stop_total`)
# n existent pas dans le moteur. Un nom que le moteur n émet pas se scraperait à
# vide et rendrait « 0 » — un faux vert.
for metric in $C1_REQUIRED_DEFAULT $C1_OPTIONAL_DEFAULT "$SURCH_C2_STREAM_METRIC"; do
  grep -rq "$metric" "$ROOT_DIR/crates/surch-api/src" \
    || fail "T21: le compteur $metric scrapé par le harnais n existe pas dans le moteur"
done
for absent in surch_dbg_c1_scored_total surch_dbg_c1_early_stop_total; do
  if grep -rq "$absent" "$ROOT_DIR/crates/surch-api/src"; then
    fail "T21: $absent existe désormais dans le moteur — la table de correspondance des noms est à revoir"
  fi
done

ENGINE=surch
SURCH_C1_EXPECT=require
SURCH_C1_METRICS_REQUIRED="$C1_REQUIRED_DEFAULT"
SURCH_C1_METRICS_OPTIONAL="$C1_OPTIONAL_DEFAULT"
C1_ACTIVATION_JSONL="$OUT_DIR/c1-activation.jsonl"
C1_ACTIVATION_RUN_VALID=true
C1_ACTIVATION_RUN_REASON=""
: > "$C1_ACTIVATION_JSONL"
C1_BEFORE="$OUT_DIR/c1.before.prom"
C1_AFTER="$OUT_DIR/c1.after.prom"
# Valeurs relevées au smoke 1,36 M (fin `fixed` -> fin `random`) : élagage
# massif, saut de blocs rare, terminaison anticipée JAMAIS déclenchée.
printf 'surch_dbg_c1_stream_docs_scored_total 17000\nsurch_dbg_c1_stream_docs_pruned_total 4811000\n' > "$C1_BEFORE"
printf 'surch_dbg_c1_stream_docs_scored_total 20746\nsurch_dbg_c1_stream_docs_pruned_total 4876841\nsurch_dbg_c1_maxscore_blocks_skipped_total 227\n' > "$C1_AFTER"
c1_phase_record random "$C1_BEFORE" "$C1_AFTER" \
  || fail "T21: des compteurs requis présents et croissants doivent rester valides ($C1_ACTIVATION_RUN_REASON)"
[ "$C1_ACTIVATION_RUN_VALID" = true ] || fail 'T21: la phase C1 nominale ne doit pas être invalidée'
jq -se '
  length == 4
  and (first(.[] | select(.metric == "surch_dbg_c1_stream_docs_scored_total"))
       | .status == "increased" and .delta == 3746 and .class == "required")
  and (first(.[] | select(.metric == "surch_dbg_c1_maxscore_blocks_skipped_total"))
       | .status == "increased" and .present_before == false and .value_before == null and .delta == 227)
  and (first(.[] | select(.metric == "surch_dbg_c1_maxscore_early_stop_total"))
       | .status == "absent" and .value_after == null and .delta == null and .class == "optional")
' "$C1_ACTIVATION_JSONL" | grep -qx true \
  || { cat "$C1_ACTIVATION_JSONL" >&2; fail 'T21: un compteur absent doit être publié `absent`/null, jamais 0'; }

# Un compteur REQUIS absent (nom faux ou chemin jamais emprunté) : invalidité
# nommée. C est exactement ce qu un scrape à vide rendait « 0 » auparavant.
: > "$C1_ACTIVATION_JSONL"
C1_ACTIVATION_RUN_VALID=true
C1_ACTIVATION_RUN_REASON=""
printf 'surch_dbg_c1_stream_docs_pruned_total 4876841\n' > "$C1_AFTER"
if c1_phase_record random "$C1_BEFORE" "$C1_AFTER"; then
  fail 'T21: un compteur REQUIS absent doit invalider la phase'
fi
[ "$C1_ACTIVATION_RUN_VALID" = false ] || fail 'T21: le run doit être marqué invalide'
[ "$C1_ACTIVATION_RUN_REASON" = 'c1_metric_absent_after_surch_dbg_c1_stream_docs_scored_total_phase_random' ] \
  || fail "T21: motif d absence non diagnosticable : $C1_ACTIVATION_RUN_REASON"
jq -se 'first(.[] | select(.metric == "surch_dbg_c1_stream_docs_scored_total"))
        | .status == "absent" and .value_after == null' \
  "$C1_ACTIVATION_JSONL" | grep -qx true \
  || { cat "$C1_ACTIVATION_JSONL" >&2; fail 'T21: un requis absent doit rester `absent`, jamais 0'; }

# Sous `observe`, la même absence est publiée mais n invalide rien : l opérateur
# choisit ce qu il exige, le harnais ne l invente pas.
: > "$C1_ACTIVATION_JSONL"
C1_ACTIVATION_RUN_VALID=true
C1_ACTIVATION_RUN_REASON=""
SURCH_C1_EXPECT=observe
c1_phase_record random "$C1_BEFORE" "$C1_AFTER" \
  || fail 'T21: sous observe, une absence ne doit pas faire échouer la phase'
[ "$C1_ACTIVATION_RUN_VALID" = true ] || fail 'T21: observe ne doit pas invalider le run'
jq -se 'first(.[] | select(.metric == "surch_dbg_c1_stream_docs_scored_total")) | .status == "absent"' \
  "$C1_ACTIVATION_JSONL" | grep -qx true \
  || fail 'T21: sous observe, l absence doit rester publiée'

# Compteur présent mais immobile : `flat`, ni absent ni erreur — l anomalie
# observée entre `random` et `no_source` au smoke doit rester descriptible.
SURCH_C1_EXPECT=require
: > "$C1_ACTIVATION_JSONL"
C1_ACTIVATION_RUN_VALID=true
C1_ACTIVATION_RUN_REASON=""
printf 'surch_dbg_c1_stream_docs_scored_total 20746\nsurch_dbg_c1_stream_docs_pruned_total 4876841\n' > "$C1_BEFORE"
cp "$C1_BEFORE" "$C1_AFTER"
c1_phase_record no_source "$C1_BEFORE" "$C1_AFTER" \
  || fail 'T21: un compteur présent mais immobile ne doit pas invalider la phase'
jq -se 'all(.[] | select(.class == "required"); .status == "flat" and .delta == 0)' \
  "$C1_ACTIVATION_JSONL" | grep -qx true \
  || { cat "$C1_ACTIVATION_JSONL" >&2; fail 'T21: un compteur immobile doit être publié `flat`'; }

# Compteur qui recule : impossible pour un compteur Prometheus, donc invalide.
: > "$C1_ACTIVATION_JSONL"
C1_ACTIVATION_RUN_VALID=true
C1_ACTIVATION_RUN_REASON=""
printf 'surch_dbg_c1_stream_docs_scored_total 10\nsurch_dbg_c1_stream_docs_pruned_total 4876841\n' > "$C1_AFTER"
if c1_phase_record cold "$C1_BEFORE" "$C1_AFTER"; then
  fail 'T21: un compteur en recul doit invalider la phase'
fi
case "$C1_ACTIVATION_RUN_REASON" in
  c1_metric_decreased_surch_dbg_c1_stream_docs_scored_total_phase_cold) ;;
  *) fail "T21: motif de recul incorrect : $C1_ACTIVATION_RUN_REASON";;
esac

printf '[test-p3-telemetry] OK — toutes les assertions sur le VRAI code fair-ab.sh sont passées\n' >&2
