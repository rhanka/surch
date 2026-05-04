use super::{Analyzer, Token};

pub struct KeywordAnalyzer;

impl KeywordAnalyzer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for KeywordAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer for KeywordAnalyzer {
    fn name(&self) -> &str {
        "keyword"
    }

    fn analyze(&self, text: &str) -> Vec<Token> {
        vec![Token {
            text: text.to_string(),
            field: String::new(),
            start_offset: 0,
            end_offset: text.len(),
            position: 0,
            term_freq: 1,
            pos_increment: 1,
        }]
    }
}
