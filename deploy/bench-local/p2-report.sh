#!/usr/bin/env bash
# p2-report.sh — agrège une paire P2 A/B sans masquer les séries appariées.
#
# Les distributions restent calculées avec le rang le plus proche. Le
# bootstrap emploie le même LCG que fair-ab.sh afin que les IC soient
# déterministes, auditables et rejouables sur une VM sans Python.
set -euo pipefail
export LC_ALL=C

usage(){
  printf 'usage: %s --a RUN_A --b RUN_B --out RAPPORT [--bootstrap 10000] [--seed ENTIER]\n' "$0" >&2
}
die(){ printf '[p2-report] %s\n' "$*" >&2; exit 1; }

a_dir=""
b_dir=""
out_dir=""
bootstrap_samples=10000
bootstrap_seed=20260726
while [ "$#" -gt 0 ]; do
  case "$1" in
    --a) [ "$#" -ge 2 ] || { usage; exit 2; }; a_dir=$2; shift 2 ;;
    --b) [ "$#" -ge 2 ] || { usage; exit 2; }; b_dir=$2; shift 2 ;;
    --out) [ "$#" -ge 2 ] || { usage; exit 2; }; out_dir=$2; shift 2 ;;
    --bootstrap) [ "$#" -ge 2 ] || { usage; exit 2; }; bootstrap_samples=$2; shift 2 ;;
    --seed) [ "$#" -ge 2 ] || { usage; exit 2; }; bootstrap_seed=$2; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage; die "option inconnue: $1" ;;
  esac
done

[ -n "$a_dir" ] && [ -n "$b_dir" ] && [ -n "$out_dir" ] || { usage; exit 2; }
case "$bootstrap_samples" in ''|*[!0-9]*) die 'bootstrap invalide';; esac
[ "$bootstrap_samples" -eq 10000 ] || die 'P2 exige exactement 10 000 rééchantillonnages bootstrap'
case "$bootstrap_seed" in ''|-[!0-9]*|*[!0-9-]*|*--*) die 'graine bootstrap invalide';; esac

for command in awk jq sort mktemp; do
  command -v "$command" >/dev/null 2>&1 || die "commande requise absente: $command"
done

mkdir -p "$out_dir/paired-ratios" || die "création impossible: $out_dir"
tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/p2-report.XXXXXX")
trap 'rm -rf "$tmp_dir"' EXIT
records_file="$tmp_dir/records.jsonl"
: > "$records_file"

# Une taille d'hôte différente ne fausse pas une paire P2 : le moteur et la
# sonde sont pinnés. En revanche, comparer deux cpusets ou deux hôtes
# différents casserait l'appariement A/B. La scorecard porte donc la
# configuration observée, et le rapport la valide avant toute statistique.
p2_cpu_configuration(){
  local scorecard="$1"
  jq -ce '
    .p2.observed_cpu_configuration as $cpu
    | if (
        ($cpu | type) == "object"
        and ($cpu.nproc | type == "number" and . >= 2 and floor == .)
        and ($cpu.engine_cpuset | type == "string" and length > 0)
        and ($cpu.probe_cpuset | type == "string" and length > 0)
        and .probe_cpu_count == $cpu.nproc
        and .cpuset == $cpu.engine_cpuset
        and .probe_cpuset == $cpu.probe_cpuset
      ) then $cpu else error("configuration CPU P2 absente ou incohérente") end
  ' "$scorecard"
}

a_scorecard="$a_dir/surch.json"
b_scorecard="$b_dir/surch.json"
[ -r "$a_scorecard" ] || die "scorecard A illisible: $a_scorecard"
[ -r "$b_scorecard" ] || die "scorecard B illisible: $b_scorecard"
a_cpu_configuration=$(p2_cpu_configuration "$a_scorecard") \
  || die "P2 invalide: configuration CPU A absente ou incohérente"
b_cpu_configuration=$(p2_cpu_configuration "$b_scorecard") \
  || die "P2 invalide: configuration CPU B absente ou incohérente"
[ "$a_cpu_configuration" = "$b_cpu_configuration" ] \
  || die "P2 invalide: configuration CPU A/B différente (nproc, cpuset moteur ou cpuset sonde)"

# Refuse les entrées non numériques/non finies. Les lignes vides sont
# volontairement ignorées comme dans l'ancien lecteur Python.
series_count(){
  local path=$1
  [ -r "$path" ] || die "série illisible $path"
  awk '
    function finite(value, rendered) {
      rendered = tolower(sprintf("%.17g", value + 0))
      return rendered != "inf" && rendered != "-inf" && rendered != "nan"
    }
    $0 == "" { next }
    {
      value = $0
      sub(/^[[:space:]]+/, "", value); sub(/[[:space:]]+$/, "", value)
    }
    value !~ /^[-+]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][-+]?[0-9]+)?$/ || !finite(value) {
      printf "série non finie ou illisible %s ligne %d\n", FILENAME, FNR > "/dev/stderr"
      exit 1
    }
    { count++ }
    END { if (count == 0) exit 1; print count }
  ' "$path" || die "série vide ou illisible $path"
}

# Retourne p50, p95, p99 et max en rang le plus proche, dans l'unité finale.
series_stats(){
  local path=$1 scale=$2
  sort -g "$path" | awk -v scale="$scale" '
    function rank_nearest(n, q, raw, rank) {
      raw = n * q; rank = int(raw); if (rank < raw) rank++
      return rank < 1 ? 1 : rank
    }
    $0 != "" { values[++count] = $1 }
    END {
      if (count == 0) exit 1
      printf "%.12g %.12g %.12g %.12g\n", \
        values[rank_nearest(count, .50)] * scale, \
        values[rank_nearest(count, .95)] * scale, \
        values[rank_nearest(count, .99)] * scale, values[count] * scale
    }
  '
}

ratio_or_null(){
  awk -v a="$1" -v b="$2" 'BEGIN { if (a == 0) print "null"; else printf "%.12g\n", b / a }'
}

# Préserve tous les ratios corps à corps, dont les dénominateurs nuls restent
# explicitement NA. Les valeurs client restent en secondes dans ce TSV, comme
# auparavant, tandis que le résumé est exprimé en millisecondes.
write_ratios(){
  local a_file=$1 b_file=$2 output=$3
  awk '
    NR == FNR { if ($0 != "") a[++count_a] = $1; next }
    $0 != "" { b[++count_b] = $1 }
    END {
      if (count_a != count_b) exit 2
      print "sequence\ta\tb\tb_over_a"
      for (i = 1; i <= count_a; i++) {
        if (a[i] == 0) { print i "\t" sprintf("%.12g", a[i]) "\t" sprintf("%.12g", b[i]) "\tNA"; zero++ }
        else print i "\t" sprintf("%.12g", a[i]) "\t" sprintf("%.12g", b[i]) "\t" sprintf("%.12g", b[i] / a[i])
      }
      print zero + 0 > "/dev/stderr"
    }
  ' "$a_file" "$b_file" > "$output" 2> "$tmp_dir/zero" || die "ratios A/B impossibles: $a_file / $b_file"
  cat "$tmp_dir/zero"
}

# Bootstrap apparié : l'état du LCG est mis à jour à chaque tirage de corps.
# Les constantes sont identiques à fair-ab.sh, donc l'échantillonnage est
# reproductible d'une exécution à l'autre sur le même awk.
bootstrap_primary(){
  local a_file=$1 b_file=$2 output=$3 values=$4
  awk -v samples="$bootstrap_samples" -v initial_seed="$bootstrap_seed" -v output="$output" '
    function rank_nearest(n, q, raw, rank) {
      raw = n * q; rank = int(raw); if (rank < raw) rank++
      return rank < 1 ? 1 : rank
    }
    function nth(values, n, wanted, left, right, pivot, i, j, swap) {
      left = 1; right = n
      while (left < right) {
        pivot = values[int((left + right) / 2)]
        i = left; j = right
        while (i <= j) {
          while (values[i] < pivot) i++
          while (values[j] > pivot) j--
          if (i <= j) {
            swap = values[i]; values[i] = values[j]; values[j] = swap
            i++; j--
          }
        }
        if (wanted <= j) right = j
        else if (wanted >= i) left = i
        else return values[wanted]
      }
      return values[left]
    }
    NR == FNR { if ($0 != "") a[++count_a] = $1; next }
    $0 != "" { b[++count_b] = $1 }
    END {
      if (count_a != count_b || count_a == 0) {
        print "bootstrap: séries A/B de longueurs différentes" > "/dev/stderr"; exit 2
      }
      seed = initial_seed % 2147483648
      if (seed < 0) seed += 2147483648
      rank95 = rank_nearest(count_a, .95)
      print "resample\tp95_a_ms\tp95_b_ms\tb_over_a" > output
      for (resample = 1; resample <= samples; resample++) {
        for (i = 1; i <= count_a; i++) {
          # Même LCG que fair-ab.sh : seed = (seed*1103515245+12345) % 2^31.
          seed = (seed * 1103515245 + 12345) % 2147483648
          sample_index = (seed % count_a) + 1
          sample_a[i] = a[sample_index]; sample_b[i] = b[sample_index]
        }
        p95_a = nth(sample_a, count_a, rank95)
        p95_b = nth(sample_b, count_a, rank95)
        if (p95_a == 0) {
          print "bootstrap: p95 A nul, ratio indéfini" > "/dev/stderr"; exit 3
        }
        ratios[resample] = p95_b / p95_a
        printf "%d\t%.12g\t%.12g\t%.12g\n", resample, p95_a, p95_b, ratios[resample] >> output
      }
      median = nth(ratios, samples, rank_nearest(samples, .50))
      low = nth(ratios, samples, rank_nearest(samples, .025))
      high = nth(ratios, samples, rank_nearest(samples, .975))
      printf "%.12g %.12g %.12g\n", median, low, high
    }
  ' "$a_file" "$b_file" > "$values" || die 'bootstrap primaire invalide'
}

primary_a=""
primary_b=""
cold_records_available(){
  local dir kind metric suffix
  for dir in "$a_dir" "$b_dir"; do
    for kind in bool; do
      for metric in client took probe; do
        case "$metric" in client) suffix=_s ;; *) suffix=_ms ;; esac
        [ -s "$dir/surch.p2.cold.$kind.$metric$suffix" ] || return 1
      done
    done
  done
}

replay_records_available(){
  local dir kind metric suffix
  for dir in "$a_dir" "$b_dir"; do
    for kind in bool match; do
      for metric in client took probe; do
        case "$metric" in client) suffix=_s ;; *) suffix=_ms ;; esac
        [ -s "$dir/surch.p2.replay_mix_5050.$kind.$metric$suffix" ] || return 1
      done
    done
  done
}

# Cold demeure dans les scorecards comme diagnostic. Si les deux côtés ont une
# série complète, ses records restent présents pour les lecteurs existants ;
# sinon il est omis sans bloquer les phases causales qui établissent la
# validité P2 (corps gelés, routage et parité A/B).
phases=(warm_match match_control warm_bool bool_size10 bool_size0 fixed_martin cold)
replay_records_available && phases+=(replay_mix_5050)
for phase in "${phases[@]}"; do
  if [ "$phase" = cold ] && ! cold_records_available; then
    continue
  fi
  case "$phase" in
    warm_match|match_control|fixed_martin) kinds='match' ;;
    warm_bool|bool_size10|bool_size0|cold) kinds='bool' ;;
    replay_mix_5050) kinds='bool match' ;;
  esac
  for kind in $kinds; do
    for metric in client took probe; do
      case "$metric" in client) suffix=_s; scale=1000 ;; *) suffix=_ms; scale=1 ;; esac
      a_file="$a_dir/surch.p2.$phase.$kind.$metric$suffix"
      b_file="$b_dir/surch.p2.$phase.$kind.$metric$suffix"
      count_a=$(series_count "$a_file")
      count_b=$(series_count "$b_file")
      [ "$count_a" -eq "$count_b" ] || die "P2 invalide: longueurs A/B différentes pour $phase/$kind/$metric: $count_a != $count_b"
      read -r a_p50 a_p95 a_p99 a_max < <(series_stats "$a_file" "$scale")
      read -r b_p50 b_p95 b_p99 b_max < <(series_stats "$b_file" "$scale")
      ratio_file="$out_dir/paired-ratios/$phase.$kind.$metric.tsv"
      zero_count=$(write_ratios "$a_file" "$b_file" "$ratio_file")
      a_r50=$(ratio_or_null "$a_p50" "$b_p50")
      a_r95=$(ratio_or_null "$a_p95" "$b_p95")
      a_r99=$(ratio_or_null "$a_p99" "$b_p99")
      a_rmax=$(ratio_or_null "$a_max" "$b_max")
      jq -n \
        --arg phase "$phase" --arg kind "$kind" --arg metric "$metric" --arg ratio_file "$ratio_file" \
        --argjson n "$count_a" \
        --argjson a_p50 "$a_p50" --argjson a_p95 "$a_p95" --argjson a_p99 "$a_p99" --argjson a_max "$a_max" \
        --argjson b_p50 "$b_p50" --argjson b_p95 "$b_p95" --argjson b_p99 "$b_p99" --argjson b_max "$b_max" \
        --argjson r_p50 "$a_r50" --argjson r_p95 "$a_r95" --argjson r_p99 "$a_r99" --argjson r_max "$a_rmax" \
        --argjson zero "$zero_count" \
        '{phase:$phase,kind:$kind,metric:$metric,n:$n,unit:"ms",
          a:{p50:$a_p50,p95:$a_p95,p99:$a_p99,max:$a_max},
          b:{p50:$b_p50,p95:$b_p95,p99:$b_p99,max:$b_max},
          b_over_a:{p50:$r_p50,p95:$r_p95,p99:$r_p99,max:$r_max},
          paired_ratios_file:$ratio_file,paired_ratios_zero_denominator:$zero}' >> "$records_file"
      if [ "$phase/$kind/$metric" = 'bool_size10/bool/took' ]; then
        primary_a=$a_file
        primary_b=$b_file
        primary_a_p95=$a_p95
        primary_b_p95=$b_p95
        primary_ratio_p95=$a_r95
      fi
    done
  done
done

[ -n "$primary_a" ] && [ -n "$primary_b" ] || die 'P2 invalide: série primaire bool_size10/bool/took absente'
bootstrap_tsv="$out_dir/bootstrap-bool-size10-bool-took-p95.tsv"
bootstrap_values="$tmp_dir/bootstrap-values"
bootstrap_primary "$primary_a" "$primary_b" "$bootstrap_tsv" "$bootstrap_values"
read -r bootstrap_median bootstrap_low bootstrap_high < "$bootstrap_values"
bootstrap_json=$(jq -n \
  --arg raw_file "$bootstrap_tsv" --argjson samples "$bootstrap_samples" --argjson seed "$bootstrap_seed" \
  --argjson median "$bootstrap_median" --argjson low "$bootstrap_low" --argjson high "$bootstrap_high" \
  '{metric:"took",phase:"bool_size10",kind:"bool",quantile:"p95",resamples:$samples,seed:$seed,
    ratio_median:$median,ci95_low:$low,ci95_high:$high,raw_file:$raw_file}')
jq -s --arg a_dir "$a_dir" --arg b_dir "$b_dir" --argjson bootstrap "$bootstrap_json" \
  --argjson observed_cpu_configuration "$a_cpu_configuration" \
  '{schema:"surch.bench.p2.pair.v1",a_dir:$a_dir,b_dir:$b_dir,nearest_rank:true,observed_cpu_configuration:$observed_cpu_configuration,records:.,primary_bootstrap:$bootstrap}' \
  "$records_file" > "$out_dir/pair-summary.json"

awk -v a="$primary_a_p95" -v b="$primary_b_p95" -v ratio="$primary_ratio_p95" -v low="$bootstrap_low" -v high="$bootstrap_high" '
  BEGIN { printf "| bool_size10 / bool / took | %.2f ms | %.2f ms | %.4f | [%.4f; %.4f] |\n", a, b, ratio, low, high }
' > "$tmp_dir/primary-row"
{
  printf '# P2 — paire A/B\n\n'
  printf 'La parité des réponses doit déjà avoir été validée par le pilote avant cette lecture.\n\n'
  printf 'Configuration CPU observée et identique A/B : `nproc=%s`, moteur `%s`, sonde `%s`.\n\n' \
    "$(jq -r .nproc <<< "$a_cpu_configuration")" \
    "$(jq -r .engine_cpuset <<< "$a_cpu_configuration")" \
    "$(jq -r .probe_cpuset <<< "$a_cpu_configuration")"
  printf '| Série primaire | A p95 took | B p95 took | B/A | IC95 bootstrap |\n'
  printf '|---|---:|---:|---:|---:|\n'
  cat "$tmp_dir/primary-row"
  printf '\nLes statistiques complètes et les ratios par corps sont dans `pair-summary.json` et `paired-ratios/`.\n'
} > "$out_dir/README.md"
