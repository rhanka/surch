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
