//! Lucene `StandardTokenizer` semantics (Unicode text segmentation, UAX#29
//! word boundaries), as used by Elasticsearch's `standard` tokenizer and
//! `standard` analyzer.
//!
//! What is reproduced:
//! - word boundaries from UAX#29 (`unicode-segmentation`): letters joined
//!   across `'` / `.` / `:` (MidLetter, MidNumLet, Single_Quote), digits
//!   across `.` / `,` / `;` (MidNum), letter-digit runs joined, `_`
//!   (ExtendNumLet) joining, hyphens and whitespace breaking;
//! - only segments holding at least one letter or digit become tokens
//!   (punctuation and whitespace segments are dropped);
//! - `max_token_length` (ES default 255): a longer token is split at
//!   `max_token_length` intervals, counted in UTF-16 code units like Java.
//!
//! Known deviations from Lucene 9.4 (ES 8.6.1), none hit by the matchID
//! `deces` corpus (Latin script, digits, punctuation):
//! - emoji sequences are dropped (Lucene emits `<EMOJI>` tokens);
//! - South-East Asian scripts (Thai, Lao, Myanmar, Khmer) are split per
//!   UAX#29 default rules (Lucene emits whole `<SOUTHEAST_ASIAN>` runs);
//! - the Unicode version is the one of `unicode-segmentation` (16.0), not
//!   the one Lucene's JFlex grammar was generated from;
//! - a split long token resumes as a fresh chunk of the same segment
//!   instead of re-running the grammar on the remainder.

use unicode_segmentation::UnicodeSegmentation;

use crate::Token;

/// ES default `max_token_length` for the `standard` tokenizer.
pub const DEFAULT_MAX_TOKEN_LENGTH: usize = 255;

/// UAX#29 word tokenizer. Emits tokens verbatim (no case folding).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StandardTokenizer {
    pub max_token_length: usize,
}

impl Default for StandardTokenizer {
    fn default() -> Self {
        Self {
            max_token_length: DEFAULT_MAX_TOKEN_LENGTH,
        }
    }
}

fn is_word_char(character: char) -> bool {
    character.is_alphanumeric()
}

impl StandardTokenizer {
    pub fn new(max_token_length: usize) -> Self {
        Self {
            max_token_length: max_token_length.max(1),
        }
    }

    /// Tokenizes `text`; offsets are byte offsets into `text`.
    pub fn tokenize(&self, text: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let max = self.max_token_length.max(1);
        for (start, segment) in text.split_word_bound_indices() {
            if !segment.chars().any(is_word_char) {
                continue;
            }
            let mut chunk_start = 0_usize;
            let mut chunk_units = 0_usize;
            for (offset, character) in segment.char_indices() {
                let units = character.len_utf16();
                if chunk_units + units > max && offset > chunk_start {
                    push_chunk(&mut tokens, segment, start, chunk_start, offset);
                    chunk_start = offset;
                    chunk_units = 0;
                }
                chunk_units += units;
            }
            push_chunk(&mut tokens, segment, start, chunk_start, segment.len());
        }
        tokens
    }
}

fn push_chunk(tokens: &mut Vec<Token>, segment: &str, base: usize, from: usize, to: usize) {
    let chunk = &segment[from..to];
    if chunk.is_empty() || !chunk.chars().any(is_word_char) {
        return;
    }
    tokens.push(Token {
        term: chunk.to_owned(),
        start_offset: base + from,
        end_offset: base + to,
        position_increment: 1,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(tokenizer: StandardTokenizer, text: &str) -> Vec<String> {
        tokenizer
            .tokenize(text)
            .into_iter()
            .map(|token| token.term)
            .collect()
    }

    #[test]
    fn es_reference_example_default_length() {
        // ES reference, `standard` tokenizer example (tokenizer only, no
        // lowercase).
        assert_eq!(
            terms(
                StandardTokenizer::default(),
                "The 2 QUICK Brown-Foxes jumped over the lazy dog's bone."
            ),
            vec![
                "The", "2", "QUICK", "Brown", "Foxes", "jumped", "over", "the", "lazy", "dog's",
                "bone"
            ]
        );
    }

    #[test]
    fn es_reference_example_max_token_length_5() {
        assert_eq!(
            terms(
                StandardTokenizer::new(5),
                "The 2 QUICK Brown-Foxes jumped over the lazy dog's bone."
            ),
            vec![
                "The", "2", "QUICK", "Brown", "Foxes", "jumpe", "d", "over", "the", "lazy",
                "dog's", "bone"
            ]
        );
    }

    #[test]
    fn keeps_digits_and_byte_offsets() {
        let tokens = StandardTokenizer::default().tokenize("né le 19290107");
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].term, "né");
        assert_eq!((tokens[0].start_offset, tokens[0].end_offset), (0, 3));
        assert_eq!(tokens[2].term, "19290107");
        assert_eq!((tokens[2].start_offset, tokens[2].end_offset), (7, 15));
    }

    #[test]
    fn splits_ideographs_one_per_token() {
        assert_eq!(
            terms(StandardTokenizer::default(), "北京"),
            vec!["北", "京"]
        );
    }
}
