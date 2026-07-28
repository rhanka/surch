#!/usr/bin/env bash
# Test versionné de l'audit mécanisé check-jq-bindings.awk (tâche : détecter
# --arg/--argjson/--slurpfile/--rawfile déclarés mais non référencés dans le
# filtre jq, et réciproquement des $variables référencées mais jamais
# déclarées — la classe de bug qui a invalidé le smoke2 P3
# : --argjson directory_bytes déclaré, $directory référencé dans le filtre).
#
# Chaque scénario vit dans son propre fichier fixture, pour ne jamais
# dépendre d'un numéro de ligne précis (fragile face à un futur réagencement
# de ce test) : on vérifie seulement que le fichier entier est rapporté sain
# (0 problème) ou en défaut (>=1 problème), et le cas échéant la nature
# exacte du défaut.
set -Eeuo pipefail
export LC_ALL=C

CURRENT_STEP='initialisation'
TMP_DIR=''
fail(){ printf '[test-jq-bindings] ECHEC (%s) : %s\n' "$CURRENT_STEP" "$*" >&2; exit 1; }
set_step(){ CURRENT_STEP="$1"; }
cleanup(){ local status=$?; [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ] && rm -rf -- "$TMP_DIR"; exit "$status"; }
trap cleanup EXIT

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
BENCH_DIR="$ROOT_DIR/deploy/bench-local"
CHECKER="$BENCH_DIR/check-jq-bindings.awk"
TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/surch-jq-bindings-test.XXXXXX")

[ -r "$CHECKER" ] || fail "checker introuvable : $CHECKER"

printf 'test-jq-bindings: versions bash=%s awk=%s\n' "$BASH_VERSION" "$(awk -W version 2>&1 | head -n1)" >&2

# ==== 1. Les six scripts réels du harnais ne doivent lever aucun problème
#         à date. Une régression ici est soit un vrai bug de liaison jq
#         (à corriger), soit une dérive du scanner (faux positif à
#         diagnostiquer avant de l'ignorer). ====
set_step '1 — audit des scripts réels du harnais bench-local'
REAL_SCRIPTS=(
  "$BENCH_DIR/fair-ab.sh"
  "$BENCH_DIR/p2-campaign.sh"
  "$BENCH_DIR/p2-gate.sh"
  "$BENCH_DIR/p2-report.sh"
  "$BENCH_DIR/test-p3-harness.sh"
  "$BENCH_DIR/test-p3-campaign.sh"
)
for f in "${REAL_SCRIPTS[@]}"; do
  [ -r "$f" ] || fail "script réel introuvable : $f"
done
REAL_OUT="$TMP_DIR/real.out"
if ! awk -f "$CHECKER" "${REAL_SCRIPTS[@]}" > "$REAL_OUT" 2>&1; then
  cat "$REAL_OUT" >&2
  fail '1: au moins une invocation jq réelle est en défaut de liaison — voir le détail ci-dessus (nouveau bug ou faux positif du scanner à trancher avant de continuer)'
fi
invocation_count=$(awk -F'[ ,]+' '/invocations jq analysées/ { for (i=1;i<=NF;i++) if ($i ~ /^[0-9]+$/) { print $i; exit } }' "$REAL_OUT")
[ -n "$invocation_count" ] && [ "$invocation_count" -ge 100 ] \
  || fail "1: nombre d'invocations détectées suspect (${invocation_count:-vide}, attendu >= 100) — le scanner a-t-il régressé (mots collés entre lignes, \$(jq non détecté) ?"

# ---- helper : audit d'une fixture à un seul scénario ----
run_checker(){
  local fixture="$1"
  local out="$2"
  awk -f "$CHECKER" "$fixture" > "$out" 2>&1
}

# ==== 2. Bug historique : --argjson directory_bytes déclaré, $directory
#         référencé dans le filtre (motif exact du smoke2). ====
set_step '2 — bug historique : variable déclarée sous un nom, utilisée sous un autre'
F2="$TMP_DIR/bug.sh"
cat > "$F2" <<'EOF'
bug(){
  local directory="103278432"
  jq -cn --argjson directory_bytes "$directory" '{directory_bytes:$directory}'
}
EOF
O2="$TMP_DIR/bug.out"
if run_checker "$F2" "$O2"; then
  cat "$O2" >&2
  fail '2: le bug historique aurait dû être détecté (audit resté silencieux)'
fi
grep -q "USED_NOT_DECLARED	$F2:.*\$directory\$" "$O2" \
  || { cat "$O2" >&2; fail '2: $directory (utilisé, non déclaré) non signalé'; }
grep -q "DECLARED_NOT_USED	$F2:.*directory_bytes\$" "$O2" \
  || { cat "$O2" >&2; fail '2: directory_bytes (déclaré, non utilisé) non signalé'; }

# ==== 3. Cas sain : --arg/--argjson tous utilisés sous leur propre nom. ====
set_step '3 — cas sain : déclarations toutes utilisées'
F3="$TMP_DIR/clean.sh"
cat > "$F3" <<'EOF'
clean(){
  local a="1"
  local b="2"
  jq -cn --arg a "$a" --argjson b "$b" '{a:$a,b:$b}'
}
EOF
O3="$TMP_DIR/clean.out"
run_checker "$F3" "$O3" || { cat "$O3" >&2; fail '3: un cas sain a été signalé en défaut'; }
grep -q "^OK	$F3:" "$O3" || { cat "$O3" >&2; fail '3: le cas sain aurait dû apparaître comme OK'; }

# ==== 4. Déclaration inutile : --arg jamais référencé dans le filtre. ====
set_step '4 — déclaration --arg jamais utilisée dans le filtre'
F4="$TMP_DIR/unused.sh"
cat > "$F4" <<'EOF'
unused_decl(){
  local x="1"
  jq -n --arg x "$x" '{}'
}
EOF
O4="$TMP_DIR/unused.out"
if run_checker "$F4" "$O4"; then
  cat "$O4" >&2
  fail '4: --arg x jamais utilisé dans le filtre aurait dû être signalé'
fi
grep -q "DECLARED_NOT_USED	$F4:.*x\$" "$O4" \
  || { cat "$O4" >&2; fail '4: motif DECLARED_NOT_USED absent pour x'; }

# ==== 5. Liaison locale "as $x" (avec destructuration implicite via un
#         binding simple) : ne doit jamais être confondue avec un --arg
#         manquant. ====
set_step '5 — liaison locale "reduce ... as $key" non confondue avec --arg manquant'
F5="$TMP_DIR/as_binding.sh"
cat > "$F5" <<'EOF'
as_binding(){
  jq -n --arg keys_source "$KEYS" '
    reduce ($keys_source | fromjson)[] as $key (.;
      . + {($key): 1})
  '
}
EOF
O5="$TMP_DIR/as_binding.out"
run_checker "$F5" "$O5" || { cat "$O5" >&2; fail "5: la liaison locale 'as \$key' a été faussement signalée en défaut"; }
grep -q "^OK	$F5:" "$O5" || { cat "$O5" >&2; fail '5: as_binding aurait dû apparaître comme OK'; }

# ==== 6. Paramètre de fonction "def f($keys): ..." (sucre syntaxique jq
#         pour une liaison implicite) : même exigence que le cas 5. ====
set_step '6 — paramètre "def f($keys)" non confondu avec --arg manquant'
F6="$TMP_DIR/def_param.sh"
cat > "$F6" <<'EOF'
def_param(){
  jq -ser --arg filter "$filter" '
    def safe_path($keys):
      reduce $keys[] as $key (.;
        if type == "object" and ($key | type) == "string" and has($key) then .[$key] else null end);
    first(.[] | select(.phase == "index_ready") | safe_path($filter | split(".")))
  ' "$telemetry"
}
EOF
O6="$TMP_DIR/def_param.out"
run_checker "$F6" "$O6" || { cat "$O6" >&2; fail "6: def safe_path(\$keys) a été faussement signalée en défaut"; }
grep -q "^OK	$F6:" "$O6" || { cat "$O6" >&2; fail '6: def_param aurait dû apparaître comme OK'; }

# ==== 7. Filtre multi-ligne avec argument positionnel de fichier après le
#         filtre : le positional ne doit pas être confondu avec le texte du
#         filtre (motif réel de p3_ratio dans p2-campaign.sh). ====
set_step '7 — filtre multi-ligne avec positional de fichier après le filtre'
F7="$TMP_DIR/multiline.sh"
cat > "$F7" <<'EOF'
multiline(){
  jq -e --arg protocol "$PROTOCOL" --arg a "$A" '
    .protocol == $protocol and .a == $a
  ' "$file"
}
EOF
O7="$TMP_DIR/multiline.out"
run_checker "$F7" "$O7" || { cat "$O7" >&2; fail '7: le filtre multi-ligne a été faussement signalé en défaut'; }
grep -q "^OK	$F7:" "$O7" || { cat "$O7" >&2; fail '7: multiline aurait dû apparaître comme OK'; }

# ==== 8. $(jq ...) imbriqué dans la valeur --argjson d'un appel englobant
#         (motif réel de p2-gate.sh:767, p2-gate.sh:773,
#         test-p3-harness.sh:225) : limite assumée du scanner (bassin
#         commun), documentée — ne doit pas planter ni fausser le résultat
#         sur un cas par ailleurs cohérent. ====
set_step '8 — $(jq ...) imbriqué dans --argjson : ne doit pas casser l’audit'
F8="$TMP_DIR/nested.sh"
cat > "$F8" <<'EOF'
nested(){
  jq -n --argjson pair_directories "$(jq -Rn --args '$ARGS.positional' -- "${PAIR_DIRS[@]}")" \
    '{pair_directories:$pair_directories}'
}
EOF
O8="$TMP_DIR/nested.out"
run_checker "$F8" "$O8" || { cat "$O8" >&2; fail '8: le $(jq ...) imbriqué a été faussement signalé en défaut'; }
grep -q "^OK	$F8:" "$O8" || { cat "$O8" >&2; fail '8: nested aurait dû apparaître comme OK'; }

# ==== 9. Non-régression du bug de fusion de mots en fin de ligne : sans le
#         flush_word() inconditionnel en fin de ligne NONE, un commentaire
#         français avec apostrophe précédant un appel jq basculait le
#         scanner en état guillemet-double bloqué (panne réelle rencontrée
#         en construisant cet audit — cf. rapport). ====
set_step '9 — non-régression : apostrophe de commentaire avant un appel jq'
F9="$TMP_DIR/lineend.sh"
cat > "$F9" <<'EOF'
f(){
  local x
  x=1
}
# Un commentaire avec une apostrophe française : l'espace insuffisant.
g(){
  jq -n --arg y "$y" '{y:$y}'
}
EOF
O9="$TMP_DIR/lineend.out"
run_checker "$F9" "$O9" || { cat "$O9" >&2; fail '9: régression du bug de fusion de mots — g() non détectée ou en défaut'; }
grep -q "invocations jq analysées: 1, saines: 1" "$O9" \
  || { cat "$O9" >&2; fail '9: g() aurait dû être la seule invocation détectée, saine'; }

# ==== 10. Guillemet simple déséquilibré : doit produire PARSE_INCOMPLETE et
#          échouer fail-closed, jamais un silence. ====
set_step '10 — guillemet simple déséquilibré : PARSE_INCOMPLETE fail-closed'
F10="$TMP_DIR/unbalanced.sh"
printf 'h(){\n  jq -n --arg z "$z" '"'"'{z:$z}\n}\n' > "$F10"
O10="$TMP_DIR/unbalanced.out"
if run_checker "$F10" "$O10"; then
  cat "$O10" >&2
  fail '10: un guillemet simple déséquilibré doit faire échouer l’audit (PARSE_INCOMPLETE), pas passer silencieusement'
fi
grep -q 'PARSE_INCOMPLETE' "$O10" \
  || { cat "$O10" >&2; fail '10: motif PARSE_INCOMPLETE absent pour un guillemet déséquilibré'; }

# ==== 11. Réintroduction mécanisée du bug réel dans une COPIE de
#          fair-ab.sh (jamais le fichier commité) : preuve que l'audit mord
#          sur le fichier réel, pas seulement sur une fixture ad hoc. ====
set_step '11 — réintroduction du bug réel sur une copie de fair-ab.sh'
REAL_COPY="$TMP_DIR/fair-ab-mutated.sh"
cp "$BENCH_DIR/fair-ab.sh" "$REAL_COPY"
grep -q 'directory_bytes:\$directory_bytes}' "$REAL_COPY" \
  || fail '11: le motif corrigé est absent de fair-ab.sh — la correction a-t-elle régressé ?'
sed -i 's/directory_bytes:\$directory_bytes}/directory_bytes:$directory}/' "$REAL_COPY"
O11="$TMP_DIR/real-mutated.out"
if run_checker "$REAL_COPY" "$O11"; then
  cat "$O11" >&2
  fail '11: le bug réintroduit dans une copie de fair-ab.sh aurait dû être détecté'
fi
grep -q "USED_NOT_DECLARED	$REAL_COPY:.*\$directory\$" "$O11" \
  || { cat "$O11" >&2; fail '11: $directory non détecté sur la copie mutée de fair-ab.sh'; }

printf '[test-jq-bindings] OK — audit mécanisé validé sur le réel et sur 10 scénarios synthétiques\n' >&2
