#![forbid(unsafe_code)]
//! Lucene-compatible analyzer and token stream primitives.

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
/// Covers the French diacritic set (`éèêëàâäîïôöùûüç` + uppercase) plus the
/// most common Western-European letters seen in INSEE/Etat-civil data
/// (`ñõåæœÿ`). Characters that do not have an ASCII fold mapping are
/// returned unchanged. This is the inline fallback used while a full
/// Lucene `ASCIIFoldingFilter` port is out of scope for the WP-D MVP.
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

/// Folds every character of `input` through [`asciifold_char`].
///
/// Multi-character expansions (`æ → ae`, `œ → oe`, `ß → ss`) are handled
/// here so the function returns an owned `String` rather than a `char`.
pub fn asciifold_string(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            'æ' => output.push_str("ae"),
            'Æ' => output.push_str("AE"),
            'œ' => output.push_str("oe"),
            'Œ' => output.push_str("OE"),
            'ß' => output.push_str("ss"),
            other => output.push(asciifold_char(other)),
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
        Self {
            min_gram,
            max_gram,
        }
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
            for size in self.min_gram..=upper {
                let byte_end = char_offsets[size];
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
}
