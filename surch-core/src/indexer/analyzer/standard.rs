use super::{Analyzer, Token};
use unicode_segmentation::UnicodeSegmentation;

pub struct StandardAnalyzer {
    stop_words: Vec<&'static str>,
}

impl StandardAnalyzer {
    pub fn new() -> Self {
        Self {
            stop_words: vec![
                "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "if", "in", "into",
                "is", "it", "no", "not", "of", "on", "or", "such", "that", "the", "their", "then",
                "there", "these", "they", "this", "to", "was", "will", "with",
            ],
        }
    }

    fn is_stop_word(&self, word: &str) -> bool {
        self.stop_words.contains(&word.to_lowercase().as_str())
    }
}

impl Default for StandardAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer for StandardAnalyzer {
    fn name(&self) -> &str {
        "standard"
    }

    fn analyze(&self, text: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut position = 0;
        let mut char_offset = 0;

        for word in text.unicode_words() {
            let word_lower = word.to_lowercase();

            if !self.is_stop_word(&word_lower) && word_lower.len() > 1 {
                let start = char_offset;
                let end = start + word.len();

                tokens.push(Token {
                    text: word_lower,
                    field: String::new(),
                    start_offset: start,
                    end_offset: end,
                    position,
                    term_freq: 1,
                    pos_increment: 1,
                });

                position += 1;
            }

            char_offset += word.len();
        }

        tokens
    }
}
