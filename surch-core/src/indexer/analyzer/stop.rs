use super::{Analyzer, Token};
use unicode_segmentation::UnicodeSegmentation;

pub struct StopAnalyzer {
    stop_words: Vec<&'static str>,
}

impl StopAnalyzer {
    pub fn new() -> Self {
        Self {
            stop_words: vec![
                "a", "an", "and", "are", "as", "at", "be", "but", "by", "for",
                "if", "in", "into", "is", "it", "no", "not", "of", "on", "or",
                "such", "that", "the", "their", "then", "there", "these", "they",
                "this", "to", "was", "will", "with",
            ],
        }
    }

    fn is_stop_word(&self, word: &str) -> bool {
        self.stop_words.contains(&word.to_lowercase().as_str())
    }
}

impl Default for StopAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer for StopAnalyzer {
    fn name(&self) -> &str {
        "stop"
    }

    fn analyze(&self, text: &str) -> Vec<Token> {
        text.unicode_words()
            .filter(|word| !self.is_stop_word(word))
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
