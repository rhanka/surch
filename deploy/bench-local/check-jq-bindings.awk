# Audit mécanisé des liaisons jq --arg/--argjson/--slurpfile/--rawfile dans
# les scripts du harnais bench-local. Pour chaque invocation `jq` détectée,
# ce programme compare :
#   - l'ensemble des noms déclarés via --arg/--argjson/--slurpfile/--rawfile ;
#   - l'ensemble des $identifiants réellement référencés dans le texte du
#     filtre (uniquement le contenu entre apostrophes, jamais la valeur d'un
#     --arg qui est en général entre guillemets doubles bash).
# Il signale dans les deux sens : une variable déclarée jamais utilisée dans
# le filtre (DECLARED_NOT_USED) et une variable $x référencée dans le filtre
# jamais déclarée (USED_NOT_DECLARED, la classe de bug qui a invalidé le
# smoke2 : --argjson directory_bytes déclaré, $directory référencé).
#
# Portée et limite assumée (documentée, pas cachée) : ce scanner ne modélise
# pas la réinitialisation de contexte de citation bash à l'intérieur d'un
# $(...) imbriqué. Quand un appel jq est imbriqué dans la valeur --argjson
# d'un appel jq englobant (ex. --argjson x "$(jq ... '...')"), les noms
# déclarés et utilisés des deux appels sont regroupés dans le même bassin
# plutôt que d'être distingués. Ce cas est rare (grep dans le rapport) et
# reste à la charge d'une revue manuelle ; il n'invalide pas la détection du
# bug réel (variable utilisée mais jamais déclarée nulle part dans le
# bassin), seule la granularité du rapport en pâtit.
#
# Compatible awk POSIX (testé sous mawk 1.3.4 et attendu sous gawk 5.1 en
# CI) : aucune construction gawk-only (pas de gensub, pas de asort).

function flush_word(   i) {
  if (word == "") return
  if (in_invocation) {
    if (awaiting) {
      declared[word] = 1
      declared_order[++declared_n] = word
      awaiting = 0
    } else if (word == "--arg" || word == "--argjson" || word == "--slurpfile" || word == "--rawfile") {
      awaiting = 1
    }
  } else {
    if (word == "jq") {
      in_invocation = 1
      inv_start_line = FNR
      paren_depth = 0
      quoted_buf = ""
      awaiting = 0
      delete declared
      declared_n = 0
      delete declared_order
    }
  }
  word = ""
}

# Extrait tous les $identifiants d'une chaîne donnée dans le tableau out[],
# et retourne le nombre trouvé. Ne fait aucune distinction sémantique : les
# liaisons locales (as $x) sont retirées séparément par l'appelant.
function extract_dollar_vars(text, out,    n, rest, pos, id) {
  n = 0
  rest = text
  while ((pos = match(rest, /\$[A-Za-z_][A-Za-z_0-9]*/)) > 0) {
    id = substr(rest, pos + 1, RLENGTH - 1)
    out[++n] = id
    rest = substr(rest, pos + RLENGTH)
  }
  return n
}

# Retire de used[] toute variable locale liée sans passer par
# --arg/--argjson :
#   - "as $x", "as [$a,$b]", "as {k:$a}" (liaison de valeur intermédiaire) ;
#   - "def f($keys): ..." (paramètre de valeur d'une définition de fonction
#     jq : sucre syntaxique pour "def f(keys): keys as $keys | ...").
# Approximation assumée : une liaison "def f($x): ..." est traitée comme
# locale sur TOUT le texte du filtre, pas seulement le corps de la fonction
# (portée réelle plus étroite). Sur-approximation volontaire et documentée :
# elle ne peut que masquer un faux positif rarissime (même nom réutilisé par
# ailleurs comme --arg dans le même appel), jamais un vrai bug de la classe
# qui a invalidé le smoke2 (variable utilisée nulle part ailleurs que dans le
# filtre et jamais déclarée du tout).
function strip_as_bindings(text, used, used_n,    rest, pos, span, close_paren, close_pipe, close_len, locals, locals_n, i, j, k, is_local, span_vars, span_n) {
  rest = text
  locals_n = 0
  while ((pos = match(rest, /as[ \t\n]+/)) > 0) {
    rest = substr(rest, pos + RLENGTH)
    close_paren = index(rest, "(")
    close_pipe = index(rest, "|")
    if (close_paren == 0 && close_pipe == 0) {
      span = rest
    } else if (close_paren == 0) {
      span = substr(rest, 1, close_pipe - 1)
    } else if (close_pipe == 0) {
      span = substr(rest, 1, close_paren - 1)
    } else if (close_paren < close_pipe) {
      span = substr(rest, 1, close_paren - 1)
    } else {
      span = substr(rest, 1, close_pipe - 1)
    }
    delete span_vars
    span_n = extract_dollar_vars(span, span_vars)
    for (i = 1; i <= span_n; i++) locals[++locals_n] = span_vars[i]
  }
  rest = text
  while ((pos = match(rest, /def[ \t\n]+[A-Za-z_][A-Za-z_0-9]*[ \t\n]*\(/)) > 0) {
    rest = substr(rest, pos + RLENGTH)
    close_paren = index(rest, ")")
    span = (close_paren == 0) ? rest : substr(rest, 1, close_paren - 1)
    delete span_vars
    span_n = extract_dollar_vars(span, span_vars)
    for (i = 1; i <= span_n; i++) locals[++locals_n] = span_vars[i]
  }
  j = 0
  for (i = 1; i <= used_n; i++) {
    is_local = 0
    for (k = 1; k <= locals_n; k++) if (used[i] == locals[k]) { is_local = 1; break }
    if (!is_local) used[++j] = used[i]
  }
  return j
}

function is_builtin(name) {
  return name == "ENV" || name == "__loc__" || name == "__prog_name__" || name == "ARGS"
}

function finalize_invocation(    used_all, used_n, kept_n, i, j, name, undeclared_n, unused_n, undeclared_list, unused_list, seen, found) {
  flush_word()
  used_n = extract_dollar_vars(quoted_buf, used_all)
  kept_n = strip_as_bindings(quoted_buf, used_all, used_n)

  delete seen
  undeclared_n = 0
  for (i = 1; i <= kept_n; i++) {
    name = used_all[i]
    if (is_builtin(name)) continue
    if (name in declared) continue
    if (name in seen) continue
    seen[name] = 1
    undeclared_list[++undeclared_n] = name
  }

  delete seen
  unused_n = 0
  for (i = 1; i <= declared_n; i++) {
    name = declared_order[i]
    if (name in seen) continue
    seen[name] = 1
    found = 0
    for (j = 1; j <= kept_n; j++) if (used_all[j] == name) { found = 1; break }
    if (!found) unused_list[++unused_n] = name
  }

  total_invocations++
  if (undeclared_n == 0 && unused_n == 0) {
    clean_invocations++
    printf "OK\t%s:%d\tdeclared=%d used=%d\n", FILENAME, inv_start_line, declared_n, kept_n
  } else {
    for (i = 1; i <= undeclared_n; i++) {
      printf "USED_NOT_DECLARED\t%s:%d\t$%s\n", FILENAME, inv_start_line, undeclared_list[i]
      problems++
    }
    for (i = 1; i <= unused_n; i++) {
      printf "DECLARED_NOT_USED\t%s:%d\t%s\n", FILENAME, inv_start_line, unused_list[i]
      problems++
    }
  }
  in_invocation = 0
}

BEGIN {
  squote = sprintf("%c", 39)
  seen_any_file = 0
}

# mawk n'a pas ENDFILE (extension gawk) : la frontière de fichier est
# détectée ici, au FNR==1 du fichier SUIVANT, en utilisant le nom du fichier
# PRÉCÉDENT mémorisé dans last_filename. Le tout dernier fichier de la liste
# est traité séparément dans le bloc END.
FNR == 1 {
  if (seen_any_file && in_invocation) {
    printf "PARSE_INCOMPLETE\t%s:%d\tinvocation jamais terminée avant fin de fichier (guillemets déséquilibrés ?)\n", last_filename, inv_start_line
    problems++
  }
  seen_any_file = 1
  last_filename = FILENAME
  state = 0        # 0=NONE 1=SQUOTE 2=DQUOTE
  in_invocation = 0
  paren_depth = 0
  word = ""
  quoted_buf = ""
  awaiting = 0
  delete declared
  declared_n = 0
  delete declared_order
}

{
  line = $0
  # Continuation de ligne bash : un antislash isolé en fin de ligne joint la
  # ligne suivante. On le retire avant le scan pour ne pas le traiter comme
  # un caractère ordinaire ni comme un échec de terminaison.
  continuation = 0
  if (state == 0 && match(line, /\\$/) > 0) {
    # un antislash final n'est une continuation que si le nombre
    # d'antislashs consécutifs en fin de ligne est impair (une paire
    # d'antislashs = un antislash littéral, pas une continuation).
    tail = line
    bscount = 0
    while (bscount < length(tail) && substr(tail, length(tail) - bscount, 1) == "\\") bscount++
    if (bscount % 2 == 1) {
      continuation = 1
      line = substr(line, 1, length(line) - 1)
    }
  }

  n = length(line)
  i = 1
  while (i <= n) {
    c = substr(line, i, 1)
    if (state == 1) {                       # SQUOTE
      if (c == squote) {
        state = 0
      } else if (in_invocation) {
        quoted_buf = quoted_buf c
      }
      i++
      continue
    }
    if (state == 2) {                       # DQUOTE
      if (c == "\\") {
        i += 2
        continue
      } else if (c == "\"") {
        state = 0
      }
      i++
      continue
    }
    # state == NONE
    if (c == "#" && word == "") {
      break                                 # commentaire jusqu'à fin de ligne
    }
    if (c == squote) {
      flush_word()
      state = 1
      i++
      continue
    }
    if (c == "\"") {
      flush_word()
      state = 2
      i++
      continue
    }
    if (c == " " || c == "\t") {
      flush_word()
      i++
      continue
    }
    # Ces métacaractères bash terminent toujours un mot, même hors
    # invocation (sinon "$(jq" reste collé en un seul mot et "jq" n'est
    # jamais reconnu : c'est ce qui a fait disparaître les deux appels
    # p2_metric_bundle_json lors du premier essai de cet audit). La
    # logique de fin d'invocation (paren_depth, finalize) ne s'applique
    # elle que si une invocation est active.
    if (c == "(") {
      flush_word()
      if (in_invocation) paren_depth++
      i++
      continue
    }
    if (c == ")") {
      flush_word()
      if (in_invocation) {
        paren_depth--
        if (paren_depth < 0) finalize_invocation()
      }
      i++
      continue
    }
    if (c == ";") {
      flush_word()
      if (in_invocation) finalize_invocation()
      i++
      continue
    }
    if (c == "|") {
      flush_word()
      if (in_invocation) finalize_invocation()
      if (substr(line, i + 1, 1) == "|") i++
      i++
      continue
    }
    if (c == "&") {
      flush_word()
      if (in_invocation) finalize_invocation()
      if (substr(line, i + 1, 1) == "&") i++
      i++
      continue
    }
    word = word c
    i++
  }

  if (state == 1 && in_invocation) quoted_buf = quoted_buf "\n"

  # Une fin de ligne NONE (hors continuation d'antislash) termine un mot,
  # que l'on soit ou non dans une invocation jq : sans ce flush, le dernier
  # mot d'une ligne (ex. "fi", "}") fusionne avec le premier mot de la ligne
  # suivante et casse notamment la détection des commentaires "#", ce qui a
  # fait basculer le scanner en état guillemet-double bloqué sur une simple
  # apostrophe française de commentaire lors du premier essai de cet audit.
  if (state == 0 && !continuation) {
    if (in_invocation) {
      finalize_invocation()
    } else {
      flush_word()
    }
  }
}

END {
  # Le tout dernier fichier de la liste ne déclenche jamais de FNR==1
  # suivant : sa frontière de fin est donc vérifiée ici, avec last_filename
  # (pas ENDFILE, non portable sous mawk).
  if (in_invocation) {
    printf "PARSE_INCOMPLETE\t%s:%d\tinvocation jamais terminée avant fin de fichier (guillemets déséquilibrés ?)\n", last_filename, inv_start_line
    problems++
  }
  printf "---\ninvocations jq analysées: %d, saines: %d, problèmes: %d\n", total_invocations, clean_invocations, problems+0 > "/dev/stderr"
  if (problems + 0 > 0) exit 1
}
