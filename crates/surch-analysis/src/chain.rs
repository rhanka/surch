//! Configurable analysis chains: `char_filter*` -> `tokenizer` ->
//! `filter*`, in declaration order, as Elasticsearch builds a `custom`
//! analyzer or normalizer.

use std::collections::BTreeSet;

use crate::char_filter::{OffsetCorrection, PatternReplaceCharFilter};
use crate::standard::StandardTokenizer;
use crate::{
    asciifold_string, java_lowercase, java_uppercase, Analyzer, KeywordAnalyzer, SimpleAnalyzer,
    Token, WhitespaceAnalyzer, ENGLISH_STOP_WORDS,
};

/// A character filter of a chain.
#[derive(Clone, Debug)]
pub enum CharFilter {
    PatternReplace(PatternReplaceCharFilter),
}

impl CharFilter {
    fn filter(&self, input: &str) -> (String, OffsetCorrection) {
        match self {
            Self::PatternReplace(filter) => filter.filter(input),
        }
    }
}

/// ES `token_chars` classes of the `ngram` / `edge_ngram` tokenizers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokenChars {
    pub letter: bool,
    pub digit: bool,
    pub whitespace: bool,
    pub punctuation: bool,
    pub symbol: bool,
    pub custom: Vec<char>,
}

impl TokenChars {
    /// `true` when no class is selected: ES then keeps every character.
    pub fn is_empty(&self) -> bool {
        !(self.letter
            || self.digit
            || self.whitespace
            || self.punctuation
            || self.symbol
            || !self.custom.is_empty())
    }

    /// Approximates Java's `Character.isLetter` / `isDigit` /
    /// `isWhitespace` and the punctuation / symbol general categories with
    /// the std predicates (exact on ASCII and Latin letters).
    pub fn matches(&self, character: char) -> bool {
        if self.is_empty() {
            return true;
        }
        (self.letter && character.is_alphabetic())
            || (self.digit && character.is_numeric())
            || (self.whitespace && character.is_whitespace())
            || (self.punctuation && is_punctuation(character))
            || (self.symbol && is_symbol(character))
            || self.custom.contains(&character)
    }
}

fn is_symbol(character: char) -> bool {
    matches!(character, '$' | '+' | '<' | '=' | '>' | '|' | '~' | '^' | '`')
        || (!character.is_ascii()
            && !character.is_alphanumeric()
            && !character.is_whitespace()
            && !character.is_control()
            && !is_punctuation(character))
}

fn is_punctuation(character: char) -> bool {
    if character.is_ascii() {
        return character.is_ascii_punctuation() && !is_symbol_ascii(character);
    }
    matches!(
        character,
        '\u{00A1}' | '\u{00A7}' | '\u{00AB}' | '\u{00B6}' | '\u{00B7}' | '\u{00BB}' | '\u{00BF}'
    ) || ('\u{2010}'..='\u{2027}').contains(&character)
        || ('\u{2030}'..='\u{205E}').contains(&character)
        || ('\u{3001}'..='\u{3003}').contains(&character)
}

fn is_symbol_ascii(character: char) -> bool {
    matches!(character, '$' | '+' | '<' | '=' | '>' | '|' | '~' | '^' | '`')
}

/// The tokenizer of a chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tokenizer {
    /// Lucene `StandardTokenizer` (UAX#29).
    Standard { max_token_length: usize },
    /// The whole input as one token (Surch: none for empty input).
    Keyword,
    /// Splits on whitespace.
    Whitespace,
    /// Maximal runs of letters.
    Letter,
    /// `letter` + lowercase.
    Lowercase,
    /// Lucene `EdgeNGramTokenizer`: prefixes of each run of `token_chars`,
    /// lengths `min_gram..=max_gram` in code points.
    EdgeNgram {
        min_gram: usize,
        max_gram: usize,
        token_chars: TokenChars,
    },
}

impl Tokenizer {
    fn tokenize(&self, text: &str) -> Vec<Token> {
        match self {
            Self::Standard { max_token_length } => {
                StandardTokenizer::new(*max_token_length).tokenize(text)
            }
            Self::Keyword => KeywordAnalyzer.token_stream(text),
            Self::Whitespace => WhitespaceAnalyzer.token_stream(text),
            Self::Letter => letter_runs(text),
            Self::Lowercase => SimpleAnalyzer.token_stream(text),
            Self::EdgeNgram {
                min_gram,
                max_gram,
                token_chars,
            } => edge_ngrams(text, *min_gram, *max_gram, token_chars),
        }
    }
}

fn letter_runs(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (offset, character) in text.char_indices() {
        if character.is_alphabetic() {
            start.get_or_insert(offset);
        } else if let Some(begin) = start.take() {
            tokens.push(raw_token(text, begin, offset));
        }
    }
    if let Some(begin) = start {
        tokens.push(raw_token(text, begin, text.len()));
    }
    tokens
}

fn raw_token(text: &str, start: usize, end: usize) -> Token {
    Token {
        term: text[start..end].to_owned(),
        start_offset: start,
        end_offset: end,
        position_increment: 1,
    }
}

fn edge_ngrams(text: &str, min_gram: usize, max_gram: usize, chars: &TokenChars) -> Vec<Token> {
    let mut tokens = Vec::new();
    if min_gram == 0 || max_gram < min_gram {
        return tokens;
    }
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for (offset, character) in text.char_indices() {
        if chars.matches(character) {
            start.get_or_insert(offset);
        } else if let Some(begin) = start.take() {
            runs.push((begin, offset));
        }
    }
    if let Some(begin) = start {
        runs.push((begin, text.len()));
    }
    for (begin, end) in runs {
        let run = &text[begin..end];
        let boundaries: Vec<usize> = run
            .char_indices()
            .map(|(offset, _)| offset)
            .skip(1)
            .chain(std::iter::once(run.len()))
            .collect();
        for (length, &boundary) in boundaries.iter().enumerate().map(|(i, b)| (i + 1, b)) {
            if length < min_gram {
                continue;
            }
            if length > max_gram {
                break;
            }
            tokens.push(raw_token(text, begin, begin + boundary));
        }
    }
    tokens
}

/// A token filter of a chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenFilter {
    /// Lucene `LowerCaseFilter` (Java `Character.toLowerCase`).
    Lowercase,
    /// Lucene `UpperCaseFilter` (Java `Character.toUpperCase`).
    Uppercase,
    /// Lucene `ASCIIFoldingFilter`.
    AsciiFolding { preserve_original: bool },
    /// Lucene `TrimFilter`.
    Trim,
    /// Lucene `StopFilter`.
    Stop {
        words: BTreeSet<String>,
        ignore_case: bool,
    },
}

impl TokenFilter {
    /// ES `stop` filter defaults (`_english_`).
    pub fn english_stop() -> Self {
        Self::Stop {
            words: ENGLISH_STOP_WORDS.iter().map(|w| (*w).to_owned()).collect(),
            ignore_case: false,
        }
    }

    fn apply(&self, tokens: Vec<Token>) -> Vec<Token> {
        match self {
            Self::Lowercase => map_terms(tokens, java_lowercase),
            Self::Uppercase => map_terms(tokens, java_uppercase),
            Self::Trim => map_terms(tokens, |term| term.trim().to_owned()),
            Self::AsciiFolding { preserve_original } => {
                let mut out = Vec::with_capacity(tokens.len());
                for token in tokens {
                    let folded = asciifold_string(&token.term);
                    if *preserve_original && folded != token.term {
                        // Lucene emits the folded term first, then the
                        // original stacked on the same position.
                        out.push(Token {
                            term: folded,
                            ..token.clone()
                        });
                        out.push(Token {
                            position_increment: 0,
                            ..token
                        });
                    } else {
                        out.push(Token {
                            term: folded,
                            ..token
                        });
                    }
                }
                out
            }
            Self::Stop { words, ignore_case } => {
                let mut out = Vec::with_capacity(tokens.len());
                let mut skipped = 0;
                for mut token in tokens {
                    let probe = if *ignore_case {
                        java_lowercase(&token.term)
                    } else {
                        token.term.clone()
                    };
                    if words.contains(&probe) {
                        skipped += token.position_increment;
                    } else {
                        token.position_increment += skipped;
                        skipped = 0;
                        out.push(token);
                    }
                }
                out
            }
        }
    }
}

fn map_terms(tokens: Vec<Token>, map: impl Fn(&str) -> String) -> Vec<Token> {
    tokens
        .into_iter()
        .map(|token| Token {
            term: map(&token.term),
            ..token
        })
        .collect()
}

/// An ES `custom` analyzer (or normalizer, with [`Tokenizer::Keyword`]):
/// char filters, then the tokenizer, then token filters, in order.
#[derive(Clone, Debug)]
pub struct CustomAnalyzer {
    pub char_filters: Vec<CharFilter>,
    pub tokenizer: Tokenizer,
    pub filters: Vec<TokenFilter>,
}

impl CustomAnalyzer {
    pub fn new(char_filters: Vec<CharFilter>, tokenizer: Tokenizer, filters: Vec<TokenFilter>) -> Self {
        Self {
            char_filters,
            tokenizer,
            filters,
        }
    }

    /// ES `standard` analyzer: standard tokenizer + lowercase (+ optional
    /// stop words, none by default).
    pub fn standard(max_token_length: usize, stop: Option<TokenFilter>) -> Self {
        let mut filters = vec![TokenFilter::Lowercase];
        filters.extend(stop);
        Self::new(
            Vec::new(),
            Tokenizer::Standard { max_token_length },
            filters,
        )
    }
}

impl Analyzer for CustomAnalyzer {
    fn token_stream(&self, text: &str) -> Vec<Token> {
        let mut corrections: Vec<OffsetCorrection> = Vec::with_capacity(self.char_filters.len());
        let mut filtered: Option<String> = None;
        for char_filter in &self.char_filters {
            let (next, correction) = char_filter.filter(filtered.as_deref().unwrap_or(text));
            corrections.push(correction);
            filtered = Some(next);
        }
        let mut tokens = self
            .tokenizer
            .tokenize(filtered.as_deref().unwrap_or(text));
        if !corrections.is_empty() {
            for token in &mut tokens {
                for correction in corrections.iter().rev() {
                    token.start_offset = correction.correct(token.start_offset);
                    token.end_offset = correction.correct(token.end_offset);
                }
            }
        }
        for filter in &self.filters {
            tokens = filter.apply(tokens);
        }
        tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deces_alphanum() -> CharFilter {
        CharFilter::PatternReplace(
            PatternReplaceCharFilter::new(r"[^\p{L}\d]", " ", "").expect("pattern compiles"),
        )
    }

    fn terms(analyzer: &CustomAnalyzer, text: &str) -> Vec<String> {
        analyzer
            .token_stream(text)
            .into_iter()
            .map(|token| token.term)
            .collect()
    }

    #[test]
    fn deces_norm_analyzer_splits_hyphens_and_apostrophes() {
        let norm = CustomAnalyzer::new(
            vec![deces_alphanum()],
            Tokenizer::Standard {
                max_token_length: 255,
            },
            vec![
                TokenFilter::AsciiFolding {
                    preserve_original: false,
                },
                TokenFilter::Lowercase,
            ],
        );
        assert_eq!(
            terms(&norm, "L'HAŸ-LES-ROSES"),
            vec!["l", "hay", "les", "roses"]
        );
        let tokens = norm.token_stream("d’Arc");
        assert_eq!(tokens[1].term, "arc");
        assert_eq!((tokens[1].start_offset, tokens[1].end_offset), (4, 7));
    }

    #[test]
    fn deces_norm_normalizer_keeps_one_token() {
        let norm = CustomAnalyzer::new(
            vec![deces_alphanum()],
            Tokenizer::Keyword,
            vec![
                TokenFilter::AsciiFolding {
                    preserve_original: false,
                },
                TokenFilter::Lowercase,
            ],
        );
        assert_eq!(terms(&norm, "Saint-Étienne"), vec!["saint etienne"]);
    }

    #[test]
    fn edge_ngram_honours_token_chars_and_bounds() {
        let edge = CustomAnalyzer::new(
            Vec::new(),
            Tokenizer::EdgeNgram {
                min_gram: 2,
                max_gram: 10,
                token_chars: TokenChars {
                    letter: true,
                    digit: true,
                    ..TokenChars::default()
                },
            },
            Vec::new(),
        );
        assert_eq!(
            terms(&edge, "Jean-Baptistounet X"),
            vec![
                "Je", "Jea", "Jean", "Ba", "Bap", "Bapt", "Bapti", "Baptis", "Baptist",
                "Baptisto", "Baptistou", "Baptistoun"
            ]
        );
    }

    #[test]
    fn edge_ngram_without_token_chars_keeps_the_whole_input() {
        // ES reference: `edge_ngram` defaults (min 1, max 2) on "Quick Fox".
        let edge = CustomAnalyzer::new(
            Vec::new(),
            Tokenizer::EdgeNgram {
                min_gram: 1,
                max_gram: 2,
                token_chars: TokenChars::default(),
            },
            Vec::new(),
        );
        assert_eq!(terms(&edge, "Quick Fox"), vec!["Q", "Qu"]);
    }

    #[test]
    fn asciifolding_preserve_original_stacks_tokens() {
        let analyzer = CustomAnalyzer::new(
            Vec::new(),
            Tokenizer::Whitespace,
            vec![TokenFilter::AsciiFolding {
                preserve_original: true,
            }],
        );
        let tokens = analyzer.token_stream("Évry");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].term, "Evry");
        assert_eq!(tokens[1].term, "Évry");
        assert_eq!(tokens[1].position_increment, 0);
    }
}
