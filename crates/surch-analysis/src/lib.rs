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
