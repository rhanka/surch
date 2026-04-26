use super::{Analyzer, Token};

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
        let mut tokens = Vec::new();
        let mut start = None;

        for (offset, ch) in text.char_indices() {
            if ch.is_alphabetic() {
                start.get_or_insert(offset);
                continue;
            }

            if let Some(token_start) = start.take() {
                let token_text = text[token_start..offset].to_lowercase();
                tokens.push(Token::new(token_text, String::new(), tokens.len()));
            }
        }

        if let Some(token_start) = start {
            let token_text = text[token_start..].to_lowercase();
            tokens.push(Token::new(token_text, String::new(), tokens.len()));
        }

        tokens
    }
}
