use super::{Analyzer, Token};
use unicode_segmentation::UnicodeSegmentation;

pub struct SimpleAnalyzer;

impl SimpleAnalyzer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SimpleAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer for SimpleAnalyzer {
    fn name(&self) -> &str {
        "simple"
    }

    fn analyze(&self, text: &str) -> Vec<Token> {
        text.unicode_words()
            .enumerate()
            .map(|(i, word)| {
                let word_lower = word.to_lowercase();
                Token {
                    text: word_lower,
                    field: String::new(),
                    start_offset: 0,
                    end_offset: word.len(),
                    position: i,
                    term_freq: 1,
                    pos_increment: 1,
                }
            })
            .collect()
    }
}
