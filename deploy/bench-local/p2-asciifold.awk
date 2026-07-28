# p2-asciifold.awk — clé de sélection P3 partagée avec les validateurs AWK.
#
# L'entrée est volontairement limitée au domaine mono-token ASCII accepté par
# le protocole. La table couvre les caractères latins que ce domaine peut
# rendre admissibles et est comparée au NormAnalyzer Rust par le test CI
# `p3_asciifold_oracle`.
function p2_asciifold(value) {
  gsub(/À|Á|Â|Ã|Ä|Å|Ā|Ă|Ą/, "A", value); gsub(/à|á|â|ã|ä|å|ā|ă|ą/, "a", value)
  gsub(/Æ/, "AE", value); gsub(/æ/, "ae", value); gsub(/Ç|Ć|Ĉ|Ċ|Č/, "C", value); gsub(/ç|ć|ĉ|ċ|č/, "c", value)
  gsub(/Ð|Ď|Đ/, "D", value); gsub(/ð|ď|đ/, "d", value); gsub(/È|É|Ê|Ë|Ē|Ĕ|Ė|Ę|Ě/, "E", value); gsub(/è|é|ê|ë|ē|ĕ|ė|ę|ě/, "e", value)
  gsub(/Ĝ|Ğ|Ġ|Ģ/, "G", value); gsub(/ĝ|ğ|ġ|ģ/, "g", value); gsub(/Ĥ|Ħ/, "H", value); gsub(/ĥ|ħ/, "h", value)
  gsub(/Ì|Í|Î|Ï|Ĩ|Ī|Ĭ|Į|İ/, "I", value); gsub(/ì|í|î|ï|ĩ|ī|ĭ|į|ı/, "i", value); gsub(/Ĳ/, "IJ", value); gsub(/ĳ/, "ij", value); gsub(/Ĵ/, "J", value); gsub(/ĵ/, "j", value)
  gsub(/Ķ/, "K", value); gsub(/ķ/, "k", value); gsub(/Ĺ|Ļ|Ľ|Ŀ|Ł/, "L", value); gsub(/ĺ|ļ|ľ|ŀ|ł/, "l", value)
  gsub(/Ñ|Ń|Ņ|Ň|Ŋ/, "N", value); gsub(/ñ|ń|ņ|ň|ŋ/, "n", value); gsub(/Ò|Ó|Ô|Õ|Ö|Ø|Ō|Ŏ|Ő/, "O", value); gsub(/ò|ó|ô|õ|ö|ø|ō|ŏ|ő/, "o", value); gsub(/Œ/, "OE", value); gsub(/œ/, "oe", value)
  gsub(/Ŕ|Ŗ|Ř/, "R", value); gsub(/ŕ|ŗ|ř/, "r", value); gsub(/Ś|Ŝ|Ş|Š/, "S", value); gsub(/ś|ŝ|ş|š/, "s", value); gsub(/ß/, "ss", value); gsub(/ẞ/, "SS", value)
  gsub(/Ţ|Ť|Ŧ/, "T", value); gsub(/Þ/, "TH", value); gsub(/ţ|ť|ŧ/, "t", value); gsub(/þ/, "th", value); gsub(/Ù|Ú|Û|Ü|Ũ|Ū|Ŭ|Ů|Ű|Ų/, "U", value); gsub(/ù|ú|û|ü|ũ|ū|ŭ|ů|ű|ų/, "u", value)
  gsub(/Ŵ/, "W", value); gsub(/ŵ/, "w", value); gsub(/Ý|Ÿ|Ŷ/, "Y", value); gsub(/ý|ÿ|ŷ/, "y", value); gsub(/Ź|Ż|Ž/, "Z", value); gsub(/ź|ż|ž/, "z", value)
  gsub(/Ə/, "E", value); gsub(/ə/, "e", value)
  return value
}

function p2_analysed_nom(value) { return tolower(p2_asciifold(value)) }

# Mode oracle : le même fichier est exécutable seul sans ajouter une seconde
# implémentation de la table dans le test.
p2_asciifold_emit { print p2_analysed_nom($0) }
