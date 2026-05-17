#![forbid(unsafe_code)]
//! Lucene-compatible analyzer and token stream primitives.

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "Lucene-compatible analysis";

/// A normalized term emitted by an analyzer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub term: String,
    pub start_offset: usize,
    pub end_offset: usize,
    pub position_increment: u32,
}

/// Converts input text into a Lucene-like token stream.
pub trait Analyzer {
    fn token_stream(&self, text: &str) -> Vec<Token>;
}

/// Minimal English stop word list used by [`StopAnalyzer`].
pub const ENGLISH_STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "if", "in", "into", "is", "it",
    "no", "not", "of", "on", "or", "such", "that", "the", "their", "then", "there", "these",
    "they", "this", "to", "was", "will", "with",
];

/// Lowercases token terms while preserving positional metadata.
pub fn lowercase_tokens(tokens: Vec<Token>) -> Vec<Token> {
    tokens
        .into_iter()
        .map(|token| Token {
            term: token.term.to_lowercase(),
            start_offset: token.start_offset,
            end_offset: token.end_offset,
            position_increment: token.position_increment,
        })
        .collect()
}

/// Removes stop words while preserving offsets and accumulating skipped positions.
pub fn stop_filter<I, S>(tokens: Vec<Token>, stop_words: I) -> Vec<Token>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let stop_words: Vec<String> = stop_words
        .into_iter()
        .map(|word| word.as_ref().to_owned())
        .collect();
    let mut filtered = Vec::new();
    let mut skipped_positions = 0;

    for mut token in tokens {
        if stop_words.iter().any(|word| word == &token.term) {
            skipped_positions += token.position_increment;
        } else {
            token.position_increment += skipped_positions;
            skipped_positions = 0;
            filtered.push(token);
        }
    }

    filtered
}

/// Analyzer that keeps the full input as a single token.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeywordAnalyzer;

/// Analyzer that keeps alphanumeric sequences as lowercased tokens.
#[derive(Clone, Copy, Debug, Default)]
pub struct StandardAnalyzer;

impl Analyzer for KeywordAnalyzer {
    fn token_stream(&self, text: &str) -> Vec<Token> {
        if text.is_empty() {
            return Vec::new();
        }

        vec![Token {
            term: text.to_owned(),
            start_offset: 0,
            end_offset: text.len(),
            position_increment: 1,
        }]
    }
}

impl Analyzer for StandardAnalyzer {
    fn token_stream(&self, text: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut token_start = None;

        for (offset, character) in text.char_indices() {
            if character.is_alphanumeric() {
                if token_start.is_none() {
                    token_start = Some(offset);
                }
            } else if let Some(start_offset) = token_start.take() {
                tokens.push(Token {
                    term: text[start_offset..offset].to_lowercase(),
                    start_offset,
                    end_offset: offset,
                    position_increment: 1,
                });
            }
        }

        if let Some(start_offset) = token_start {
            tokens.push(Token {
                term: text[start_offset..].to_lowercase(),
                start_offset,
                end_offset: text.len(),
                position_increment: 1,
            });
        }

        tokens
    }
}

/// Analyzer that splits terms on Rust Unicode whitespace.
#[derive(Clone, Copy, Debug, Default)]
pub struct WhitespaceAnalyzer;

impl Analyzer for WhitespaceAnalyzer {
    fn token_stream(&self, text: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut token_start = None;

        for (offset, character) in text.char_indices() {
            if character.is_whitespace() {
                if let Some(start_offset) = token_start.take() {
                    tokens.push(Token {
                        term: text[start_offset..offset].to_owned(),
                        start_offset,
                        end_offset: offset,
                        position_increment: 1,
                    });
                }
            } else if token_start.is_none() {
                token_start = Some(offset);
            }
        }

        if let Some(start_offset) = token_start {
            tokens.push(Token {
                term: text[start_offset..].to_owned(),
                start_offset,
                end_offset: text.len(),
                position_increment: 1,
            });
        }

        tokens
    }
}

/// Analyzer that emits lowercased alphabetic Unicode sequences.
#[derive(Clone, Copy, Debug, Default)]
pub struct SimpleAnalyzer;

impl Analyzer for SimpleAnalyzer {
    fn token_stream(&self, text: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut token_start = None;

        for (offset, character) in text.char_indices() {
            if character.is_alphabetic() {
                if token_start.is_none() {
                    token_start = Some(offset);
                }
            } else if let Some(start_offset) = token_start.take() {
                tokens.push(Token {
                    term: text[start_offset..offset].to_lowercase(),
                    start_offset,
                    end_offset: offset,
                    position_increment: 1,
                });
            }
        }

        if let Some(start_offset) = token_start {
            tokens.push(Token {
                term: text[start_offset..].to_lowercase(),
                start_offset,
                end_offset: text.len(),
                position_increment: 1,
            });
        }

        tokens
    }
}

/// Analyzer that emits simple tokens after removing minimal English stop words.
#[derive(Clone, Copy, Debug, Default)]
pub struct StopAnalyzer;

impl Analyzer for StopAnalyzer {
    fn token_stream(&self, text: &str) -> Vec<Token> {
        stop_filter(SimpleAnalyzer.token_stream(text), ENGLISH_STOP_WORDS)
    }
}

/// Folds a single character to its closest ASCII equivalent.
///
/// Kept for backwards compatibility with callers that need a 1:1 char map
/// on the French diacritic set. Multi-character expansions and barred /
/// stroked letters that NFD cannot decompose (e.g. `Ł`, `Đ`, `Ø`) live in
/// [`asciifold_override`] / [`asciifold_string`]. Characters without an
/// ASCII fold mapping are returned unchanged.
pub fn asciifold_char(input: char) -> char {
    match input {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'A',
        'ç' => 'c',
        'Ç' => 'C',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'È' | 'É' | 'Ê' | 'Ë' => 'E',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'Ì' | 'Í' | 'Î' | 'Ï' => 'I',
        'ñ' => 'n',
        'Ñ' => 'N',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => 'O',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'Ù' | 'Ú' | 'Û' | 'Ü' => 'U',
        'ý' | 'ÿ' => 'y',
        'Ý' | 'Ÿ' => 'Y',
        other => other,
    }
}

/// Override table for characters NFD cannot decompose to ASCII + combining
/// marks. Covers stroked / barred letters and historical ligatures used
/// across Western and Central European names (Polish `Ł`, Croatian `Đ`,
/// Danish/Norwegian `Ø` `Æ`, French `Œ`, German `ß`, Icelandic `Þ` `Ð`,
/// Turkish dotless `ı`, etc.). Returned values are pushed onto the output
/// buffer verbatim — `None` means "fall through to NFD + strip-marks".
///
/// Mirrors the subset of Lucene's `ASCIIFoldingFilter` that targets the
/// Latin-supplement, Latin Extended-A, and Latin Extended-B code blocks
/// most commonly seen in INSEE/Etat-civil and broader European corpora.
fn asciifold_override(input: char) -> Option<&'static str> {
    match input {
        'Æ' => Some("AE"),
        'æ' => Some("ae"),
        'Œ' => Some("OE"),
        'œ' => Some("oe"),
        'Ĳ' => Some("IJ"),
        'ĳ' => Some("ij"),
        'ß' => Some("ss"),
        'ẞ' => Some("SS"),
        'Ł' => Some("L"),
        'ł' => Some("l"),
        'Đ' | 'Ð' => Some("D"),
        'đ' | 'ð' => Some("d"),
        'Ø' => Some("O"),
        'ø' => Some("o"),
        'Þ' => Some("TH"),
        'þ' => Some("th"),
        'Ħ' => Some("H"),
        'ħ' => Some("h"),
        'Ŧ' => Some("T"),
        'ŧ' => Some("t"),
        'Ŀ' => Some("L"),
        'ŀ' => Some("l"),
        'ı' => Some("i"),
        'Ŋ' => Some("N"),
        'ŋ' => Some("n"),
        'Ə' => Some("E"),
        'ə' => Some("e"),
        '«' | '»' => Some("\""),
        '“' | '”' | '„' => Some("\""),
        '‘' | '’' | '‚' => Some("'"),
        '–' | '—' => Some("-"),
        _ => None,
    }
}

/// Folds `input` to ASCII via Unicode NFD decomposition plus an override
/// table for non-decomposable letters and ligatures.
///
/// Pipeline (mirrors Lucene's `ASCIIFoldingFilter` for the Latin blocks):
///
/// 1. Apply [`asciifold_override`] to handle ligatures (`Æ → AE`,
///    `Œ → OE`, `ß → ss`), barred / stroked letters (`Ł → L`, `Đ → D`,
///    `Ø → O`), and a handful of typographic quotes / dashes.
/// 2. For every remaining character, run NFD canonical decomposition and
///    drop combining marks. This covers virtually all Latin / Greek /
///    Cyrillic diacritics (`é → e`, `ñ → n`, `ć → c`, `ỹ → y`, …).
/// 3. Characters that survive both steps are passed through unchanged.
pub fn asciifold_string(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for character in input.chars() {
        if let Some(replacement) = asciifold_override(character) {
            output.push_str(replacement);
            continue;
        }
        for decomposed in character.nfd() {
            if !is_combining_mark(decomposed) {
                output.push(decomposed);
            }
        }
    }
    output
}

/// Emits every prefix of each [`StandardAnalyzer`] token whose length lies
/// in `min_gram..=max_gram` (inclusive), measured in Unicode characters.
///
/// Mirrors OpenSearch's `edge_ngram` tokenizer in the simplified shape
/// emitted by `deces_index.yml::edge_ngram_tokenizer` (matchID workload):
/// `{ type: edge_ngram, min_gram: 2, max_gram: 20, token_chars: [letter, digit] }`.
/// The `token_chars` constraint reduces to standard ascii tokenisation for
/// the MVP — this is documented in `docs/wp-d-matchid/gap-analysis.md`
/// under A13.
#[derive(Clone, Copy, Debug)]
pub struct EdgeNgramAnalyzer {
    pub min_gram: usize,
    pub max_gram: usize,
}

impl EdgeNgramAnalyzer {
    pub fn new(min_gram: usize, max_gram: usize) -> Self {
        Self { min_gram, max_gram }
    }
}

impl Default for EdgeNgramAnalyzer {
    fn default() -> Self {
        Self::new(2, 20)
    }
}

impl Analyzer for EdgeNgramAnalyzer {
    fn token_stream(&self, text: &str) -> Vec<Token> {
        if self.min_gram == 0 || self.max_gram < self.min_gram {
            return Vec::new();
        }

        let mut tokens = Vec::new();

        for base in raw_alphanumeric_tokens(text) {
            let char_offsets: Vec<usize> = base
                .term
                .char_indices()
                .map(|(byte_offset, _)| byte_offset)
                .chain(std::iter::once(base.term.len()))
                .collect();
            let char_count = char_offsets.len().saturating_sub(1);

            let upper = self.max_gram.min(char_count);
            for &byte_end in char_offsets.iter().take(upper + 1).skip(self.min_gram) {
                tokens.push(Token {
                    term: base.term[..byte_end].to_owned(),
                    start_offset: base.start_offset,
                    end_offset: base.start_offset + byte_end,
                    position_increment: 1,
                });
            }
        }

        tokens
    }
}

/// Returns alphanumeric token slices of `text` **without** lowercasing.
///
/// Used internally by [`EdgeNgramAnalyzer`] which must preserve case so
/// callers can layer their own `lowercase`/`asciifolding` filter
/// downstream (matching the deces-backend `autocomplete_analyzer` chain).
fn raw_alphanumeric_tokens(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut token_start = None;

    for (offset, character) in text.char_indices() {
        if character.is_alphanumeric() {
            if token_start.is_none() {
                token_start = Some(offset);
            }
        } else if let Some(start_offset) = token_start.take() {
            tokens.push(Token {
                term: text[start_offset..offset].to_owned(),
                start_offset,
                end_offset: offset,
                position_increment: 1,
            });
        }
    }

    if let Some(start_offset) = token_start {
        tokens.push(Token {
            term: text[start_offset..].to_owned(),
            start_offset,
            end_offset: text.len(),
            position_increment: 1,
        });
    }

    tokens
}

/// `analyzer.norm` from `deces_index.yml`: standard tokenizer, then
/// lowercase + asciifolding on every emitted token. Matches the Lucene
/// chain `tokenizer: standard → filter: [lowercase, asciifolding]`.
#[derive(Clone, Copy, Debug, Default)]
pub struct NormAnalyzer;

impl Analyzer for NormAnalyzer {
    fn token_stream(&self, text: &str) -> Vec<Token> {
        StandardAnalyzer
            .token_stream(text)
            .into_iter()
            .map(|token| Token {
                term: asciifold_string(&token.term),
                start_offset: token.start_offset,
                end_offset: token.end_offset,
                position_increment: token.position_increment,
            })
            .collect()
    }
}

/// `normalizer.norm` from `deces_index.yml`: lowercase + asciifolding
/// applied to the **entire** input as one keyword token (no tokenisation).
///
/// Used as the keyword sub-field normalizer for `NOM.raw` / `PRENOMS.raw`:
/// the wire shape relies on `term`/`terms` queries matching the whole
/// folded string. Empty input yields no tokens, consistent with
/// [`KeywordAnalyzer`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Normalizer;

impl Analyzer for Normalizer {
    fn token_stream(&self, text: &str) -> Vec<Token> {
        if text.is_empty() {
            return Vec::new();
        }

        let folded = asciifold_string(text).to_lowercase();
        vec![Token {
            term: folded,
            start_offset: 0,
            end_offset: text.len(),
            position_increment: 1,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_ngram_analyzer_emits_prefixes_within_bounds() {
        let analyzer = EdgeNgramAnalyzer::new(2, 4);
        let terms: Vec<String> = analyzer
            .token_stream("Jean")
            .into_iter()
            .map(|token| token.term)
            .collect();
        assert_eq!(terms, vec!["Je", "Jea", "Jean"]);
    }

    #[test]
    fn edge_ngram_analyzer_caps_at_token_length() {
        let analyzer = EdgeNgramAnalyzer::new(2, 20);
        let terms: Vec<String> = analyzer
            .token_stream("Jo")
            .into_iter()
            .map(|token| token.term)
            .collect();
        assert_eq!(terms, vec!["Jo"]);
    }

    #[test]
    fn norm_analyzer_lowercases_and_strips_french_accents() {
        let analyzer = NormAnalyzer;
        let terms: Vec<String> = analyzer
            .token_stream("Dupond Élève")
            .into_iter()
            .map(|token| token.term)
            .collect();
        // Note: NormAnalyzer keeps the standard-tokenizer's lowercase pass,
        // then folds accents on the already-lowercased term.
        assert_eq!(terms, vec!["dupond", "eleve"]);
    }

    #[test]
    fn normalizer_emits_one_folded_lowercase_token() {
        let analyzer = Normalizer;
        let tokens = analyzer.token_stream("Pré FONTAINE");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].term, "pre fontaine");
    }

    #[test]
    fn normalizer_emits_no_tokens_for_empty_input() {
        let analyzer = Normalizer;
        assert!(analyzer.token_stream("").is_empty());
    }

    #[test]
    fn asciifold_string_handles_multi_char_expansions() {
        assert_eq!(asciifold_string("Œuvre cœur"), "OEuvre coeur");
        assert_eq!(asciifold_string("Straße"), "Strasse");
    }

    #[test]
    fn asciifold_string_folds_french_diacritics_via_nfd() {
        // Élève — historical phase-1 anchor.
        assert_eq!(asciifold_string("Élève"), "Eleve");
    }

    #[test]
    fn asciifold_string_folds_spanish_tilde_via_nfd() {
        // Ñoño — Spanish n-tilde, both cases.
        assert_eq!(asciifold_string("Ñoño"), "Nono");
    }

    #[test]
    fn asciifold_string_folds_croatian_acute_via_nfd() {
        // Ćelo — Croatian c-acute.
        assert_eq!(asciifold_string("Ćelo"), "Celo");
    }

    #[test]
    fn asciifold_string_folds_polish_stroked_letters_via_override() {
        // Łódź — Polish: Ł and ł must come from the override table since
        // NFD cannot decompose stroked letters; ó / ź decompose via NFD.
        assert_eq!(asciifold_string("Łódź"), "Lodz");
    }

    #[test]
    fn asciifold_string_folds_danish_ligature_and_slashed_o() {
        // Ærø — Danish: Æ via override (ligature), ø via override
        // (slashed-o, non-decomposable).
        assert_eq!(asciifold_string("Ærø"), "AEro");
    }

    #[test]
    fn asciifold_string_folds_vietnamese_via_nfd() {
        // Mỹ — Vietnamese: y with tilde above (U+1EF9) decomposes via NFD.
        assert_eq!(asciifold_string("Mỹ"), "My");
    }

    #[test]
    fn asciifold_string_passes_ascii_through_unchanged() {
        // SciFact-style payloads must round-trip byte-for-byte.
        assert_eq!(asciifold_string("Hello, World! 123"), "Hello, World! 123");
    }

    #[test]
    fn asciifold_string_handles_icelandic_thorn_and_eth() {
        assert_eq!(asciifold_string("Þór Eð"), "THor Ed");
    }
}
