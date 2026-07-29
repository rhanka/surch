# D2 — ventilation reelle des postings de surch sur le corpus deces 28,9 M.
#
# Simule, octet pour octet, l'encodeur `encode_postings_blocked` ANTERIEUR a D2
# (delta-varint + freq varint, aucun en-tete de bloc) sur UN segment de
# production, et compare a plusieurs variantes de format. Sert a produire la
# ventilation du rapport `.remote/d2-bitpacking-postings.md` §2.
#
# Aucun Python. Aucune dependance hors mawk/awk.
#
# INVOCATION EXACTE utilisee pour le rapport (segment median, 1/12 du corpus) :
#
#   cd ~/surch-bench-data
#   head -4819584 deces-28M.ndjson \
#     | mawk -v DOCBASE=14458752 -v MAXDOCS=2409792 -f d2-ventilation.awk
#
# (4 819 584 lignes = 2 409 792 documents au format bulk, une ligne d'action
#  suivie d'une ligne de document. DOCBASE place le segment au milieu du
#  corpus pour que le cout du premier doc_id ABSOLU de chaque bloc soit
#  representatif : au-dela de 2^21, un varint u32 coute 4 octets.)
#
# Duree observee : ~7 min, ~3 Gio de RSS mawk (4,8 M de termes distincts).
#
# La chaine d'analyse reproduite ici suit :
#   - indexed_fields_for_document        crates/surch-api/src/state.rs:4362
#   - analyze_document                   crates/surch-index/src/document_index.rs:3442
#   - AnalyzerName::default_for          crates/surch-index/src/mapping.rs:141
#   - StandardAnalyzer / SimpleAnalyzer / KeywordAnalyzer / Normalizer
#                                        crates/surch-analysis/src/lib.rs:72-455
# Le corpus deces est purement ASCII, donc l'asciifolding est un no-op ici.
#
# F0 = actuel (delta-varint + freq varint, aucun en-tete)
# F1 = 1 o d en-tete + FoR PUR (aucun repli varint) + freqs omises si constantes
# F4 = 1 o d en-tete + min(varint, FoR pur) par bloc + freqs omises si constantes
# F5 = 1 o d'en-tete + min(varint, FoR, PFor 1 exception, PFor 2 exceptions) + freqs omises
# Exception PFor = 1 o de position + varint de la valeur complete.

function vb(x) { if (x<128) return 1; if (x<16384) return 2; if (x<2097152) return 3; if (x<268435456) return 4; return 5 }
function bits(x,   b) { b=0; while (x>0) { b++; x=int(x/2) } return b }
function getstr(line, key,   re, s, p) {
  re = "\"" key "\":\""; p = index(line, re); if (p==0) return ""
  s = substr(line, p+length(re)); p = index(s, "\""); if (p==0) return ""
  return substr(s, 1, p-1)
}
function getnum(line, key,   re, s, p, i, c, out) {
  re = "\"" key "\":"; p = index(line, re); if (p==0) return ""
  s = substr(line, p+length(re)); if (substr(s,1,1)=="\"") return ""
  out=""
  for (i=1; i<=length(s); i++) { c=substr(s,i,1); if (c==","||c=="}") break; out = out c }
  return out
}
function emit(field, term,   k) { k = field SUBSEP term; tf[k]++; if (!(k in seen)) { seen[k]=1; order[++nseen]=k } }

function close_block(k, f,   n, m, body, fb, first, cvar, cfor, cp1, cp2, best) {
  n = bn[k]; if (n==0) return
  blocks[f]++
  bdhist[bits(bmaxd[k])]++
  if (n == 128) fullblk[f]++
  first = bfirst[k]
  # --- canal freq (identique pour toutes les variantes packees) ---
  if (bfmin[k] == bfmax[k]) {
    if (bfmin[k] == 1) { fb = 0; cblk1[f]++ } else { fb = vb(bfmin[k]); cblkc[f]++ }
  } else { fb = int((n*bits(bfmax[k]) + 7) / 8); vblk[f]++ }
  newfrq[f] += fb
  # --- canal doc_id ---
  cvar = bvar[k]                                   # deltas en varint (hors premier)
  cfor = int(((n-1)*bits(bmaxd[k]) + 7) / 8)       # FoR pur
  cp1  = 999999999; cp2 = 999999999
  if (n >= 2) cp1 = int(((n-1)*bits(bmaxd2[k]) + 7) / 8) + 1 + vb(bmaxd[k])
  if (n >= 3) cp2 = int(((n-1)*bits(bmaxd3[k]) + 7) / 8) + 2 + vb(bmaxd[k]) + vb(bmaxd2[k])
  best = cvar; mode = 0
  if (cfor < best) { best = cfor; mode = 1 }
  f1doc[f] += 1 + vb(first) + cfor
  f4doc[f] += 1 + vb(first) + ((cfor < cvar) ? cfor : cvar)
  if (cp1 < best) { best = cp1; mode = 2 }
  if (cp2 < best) { best = cp2; mode = 3 }
  f5doc[f] += 1 + vb(first) + best
  modecnt[mode]++
  f0hdr[f] += vb(first) + cvar                     # rappel : cout doc du format actuel
  bn[k]=0; bmaxd[k]=0; bmaxd2[k]=0; bmaxd3[k]=0; bfmax[k]=0; bfmin[k]=0; bvar[k]=0
}
function add_posting(k, f, doc, freq,   d) {
  if (!(k in nterm)) { nterm[k]=1; terms[f]++ }
  if (bn[k]==0) {
    f0doc[f] += vb(doc); bfirst[k]=doc; bmaxd[k]=0; bmaxd2[k]=0; bmaxd3[k]=0
    bvar[k]=0; bfmax[k]=freq; bfmin[k]=freq
  } else {
    d = doc - prev[k]; f0doc[f] += vb(d); bvar[k] += vb(d)
    if (d > bmaxd[k]) { bmaxd3[k]=bmaxd2[k]; bmaxd2[k]=bmaxd[k]; bmaxd[k]=d }
    else if (d > bmaxd2[k]) { bmaxd3[k]=bmaxd2[k]; bmaxd2[k]=d }
    else if (d > bmaxd3[k]) { bmaxd3[k]=d }
    if (freq>bfmax[k]) bfmax[k]=freq
    if (freq<bfmin[k]) bfmin[k]=freq
  }
  f0frq[f] += vb(freq)
  prev[k]=doc; bn[k]++; nb[k]++; postings[f]++
  if (bn[k]==128) close_block(k, f)
}
function text_tokens(f, v,   n, i, t) { if (v=="") return; n=split(tolower(v), TOK, /[^a-z0-9]+/); for (i=1;i<=n;i++) { t=TOK[i]; if (t!="") emit(f,t) } }
function simple_tokens(f, v,   n, i, t) { if (v=="") return; n=split(tolower(v), TOK, /[^a-z]+/); for (i=1;i<=n;i++) { t=TOK[i]; if (t!="") emit(f,t) } }
function kw_norm(f, v) { if (v!="") emit(f, tolower(v)) }
function kw_raw(f, v)  { if (v!="") emit(f, v) }

BEGIN { docid = DOCBASE + 0; ndocs = 0 }
NR % 2 == 1 { next }
{
  if (ndocs >= MAXDOCS) exit
  for (i=1;i<=nseen;i++) { delete tf[order[i]]; delete seen[order[i]] }
  nseen = 0
  v=getstr($0,"UID");                   kw_raw("UID",v)
  v=getstr($0,"SOURCE");                kw_norm("SOURCE",v)
  v=getnum($0,"SOURCE_LINE");           kw_raw("SOURCE_LINE",v)
  v=getstr($0,"PRENOMS_NOM");           text_tokens("PRENOMS_NOM",v)
  v=getstr($0,"PRENOM_NOM");            text_tokens("PRENOM_NOM",v)
  v=getstr($0,"NOM");                   text_tokens("NOM",v); kw_norm("NOM.raw",v)
  v=getstr($0,"PRENOM");                text_tokens("PRENOM",v); kw_norm("PRENOM.raw",v)
  v=getstr($0,"PRENOMS");               text_tokens("PRENOMS",v); kw_norm("PRENOMS.raw",v)
  v=getstr($0,"SEXE");                  kw_raw("SEXE",v)
  v=getstr($0,"DATE_NAISSANCE");        simple_tokens("DATE_NAISSANCE",v); kw_raw("DATE_NAISSANCE.raw",v)
  v=getstr($0,"DATE_NAISSANCE_NORM");   kw_raw("DATE_NAISSANCE_NORM",v)
  v=getstr($0,"COMMUNE_NAISSANCE");     text_tokens("COMMUNE_NAISSANCE",v); kw_norm("COMMUNE_NAISSANCE.raw",v)
  v=getstr($0,"CODE_INSEE_NAISSANCE");  kw_norm("CODE_INSEE_NAISSANCE",v)
  v=getstr($0,"CODE_POSTAL_NAISSANCE"); kw_norm("CODE_POSTAL_NAISSANCE",v)
  v=getstr($0,"DEPARTEMENT_NAISSANCE"); kw_norm("DEPARTEMENT_NAISSANCE",v)
  v=getstr($0,"PAYS_NAISSANCE");        text_tokens("PAYS_NAISSANCE",v); kw_norm("PAYS_NAISSANCE.raw",v)
  v=getstr($0,"DATE_DECES");            simple_tokens("DATE_DECES",v); kw_raw("DATE_DECES.raw",v)
  v=getstr($0,"DATE_DECES_NORM");       kw_raw("DATE_DECES_NORM",v)
  v=getnum($0,"AGE_DECES");             kw_raw("AGE_DECES",v)
  v=getstr($0,"COMMUNE_DECES");         text_tokens("COMMUNE_DECES",v); kw_norm("COMMUNE_DECES.raw",v)
  v=getstr($0,"CODE_INSEE_DECES");      kw_norm("CODE_INSEE_DECES",v)
  v=getstr($0,"CODE_POSTAL_DECES");     kw_norm("CODE_POSTAL_DECES",v)
  v=getstr($0,"DEPARTEMENT_DECES");     kw_norm("DEPARTEMENT_DECES",v)
  v=getstr($0,"PAYS_DECES");            text_tokens("PAYS_DECES",v); kw_norm("PAYS_DECES.raw",v)
  for (i=1;i<=nseen;i++) { k=order[i]; split(k,KF,SUBSEP); add_posting(k, KF[1], docid, tf[k]) }
  docid++; ndocs++
}
END {
  for (k in nterm) {
    split(k,KF,SUBSEP); close_block(k, KF[1])
    if (nb[k] <= 128) mono[KF[1]]++
  }
  printf "docs=%d docbase=%d\n", ndocs, DOCBASE+0
  printf "%-24s %9s %11s %9s %9s %11s %11s %11s %11s %11s %9s\n", \
    "field","terms","postings","blocks","monoterm","F0_doc","F0_frq","F1_doc","F4_doc","F5_doc","new_frq"
  for (f in terms) {
    printf "%-24s %9d %11d %9d %9d %11d %11d %11d %11d %11d %9d\n", \
      f, terms[f], postings[f], blocks[f], mono[f], f0doc[f], f0frq[f], f1doc[f], f4doc[f], f5doc[f], newfrq[f]
    T+=terms[f]; P+=postings[f]; B+=blocks[f]; M+=mono[f]; FB+=fullblk[f]
    A0+=f0doc[f]; A0F+=f0frq[f]; A1+=f1doc[f]; A4+=f4doc[f]; A5+=f5doc[f]; ANF+=newfrq[f]
  }
  printf "\nTOTAL terms=%d postings=%d blocks=%d fullblocks=%d monoblock_terms=%d\n", T, P, B, FB, M
  printf "F0 doc=%d freq=%d payload=%d\n", A0, A0F, A0+A0F
  printf "F1 doc=%d freq=%d payload=%d   (FoR PUR, aucun repli varint)\n", A1, ANF, A1+ANF
  printf "F4 doc=%d freq=%d payload=%d\n", A4, ANF, A4+ANF
  printf "F5 doc=%d freq=%d payload=%d\n", A5, ANF, A5+ANF
  printf "META term_entries(28B)=%d block_dir(10B)=%d block_dir_sans_monoblocs(10B)=%d\n", T*28, B*10, (B-M)*10
  printf "SEGMENT_TOTAL F0=%d F4=%d F5=%d F5_sans_dir_monobloc=%d\n", \
    A0+A0F+T*28+B*10, A4+ANF+T*28+B*10, A5+ANF+T*28+B*10, A5+ANF+T*28+(B-M)*10
  printf "modes retenus (0=varint 1=FoR 2=PFor1 3=PFor2) :"
  for (m in modecnt) printf " %d=%d", m, modecnt[m]
  printf "\n"
  printf "histogramme des largeurs FoR (bits necessaires au max delta du bloc) :\n"
  for (b = 0; b <= 32; b++) if (bdhist[b] > 0) printf "  bits=%-3d blocs=%d\n", b, bdhist[b]
}
