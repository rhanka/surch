//! MVP L1: fuzzy term expansion over the per-field FST term dictionary.
//!
//! [`DamerauLevenshteinAutomaton`] is an [`fst::Automaton`] that accepts
//! every term whose *optimal string alignment* (restricted
//! Damerau-Levenshtein, Lucene's `transpositions: true` default) distance
//! to the query is at most `max_edits` (0..=2, Lucene's limit). The `fst`
//! crate's own `levenshtein` feature has no transpositions, hence this
//! hand-written automaton.
//!
//! Design:
//!
//! * distances are counted in Unicode scalar values (`char`), not bytes —
//!   the FST walks UTF-8 bytes, so the state carries a tiny UTF-8 decoder
//!   and only advances the DP row once a full `char` has been consumed;
//! * the state is a banded DP row: cell `D[i][j]` (term prefix of `i`
//!   chars vs query prefix of `j` chars) can only be `<= k` when
//!   `|i - j| <= k`, so each row stores the `2k + 1 <= 5` cells of that
//!   diagonal band, clamped to `k + 1`. The previous row is kept for the
//!   transposition rule. The whole state is `Copy` and allocation-free;
//! * `can_match` prunes a branch as soon as every band cell exceeds `k`;
//! * `prefix_chars` (Lucene `prefix_length`) requires the first chars of
//!   the term to equal the query's exactly.
//!
//! The reference oracle is `surch_search::fuzzy::bounded_damerau_levenshtein`
//! (property-tested against this automaton in
//! `crates/surch-search/tests/fuzzy_automaton.rs`).

use fst::Automaton;

/// Lucene's fuzzy query accepts edit distances from 0 to 2.
pub const MAX_AUTOMATON_EDITS: u8 = 2;

const BAND_CAPACITY: usize = 2 * MAX_AUTOMATON_EDITS as usize + 1;

/// A term of the dictionary accepted by a fuzzy expansion, with its exact
/// (bounded) edit distance to the query term.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FuzzyTermHit {
    pub term: String,
    pub edits: u8,
}

/// Optimal-string-alignment Levenshtein automaton over UTF-8 bytes.
#[derive(Debug, Clone)]
pub struct DamerauLevenshteinAutomaton {
    query: Vec<char>,
    max_edits: u8,
    prefix_chars: usize,
    transpositions: bool,
}

/// Automaton state: banded DP rows plus an in-flight UTF-8 decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamerauLevenshteinState {
    /// Number of complete chars of the term consumed so far (`i`).
    chars: u32,
    /// Band of row `i`: `row[d]` is `D[i][i + d - k]`, clamped to `k + 1`.
    row: [u8; BAND_CAPACITY],
    /// Band of row `i - 1` (for the transposition rule).
    prev_row: [u8; BAND_CAPACITY],
    /// Last complete char of the term (term char `i - 1`).
    last_char: char,
    /// UTF-8 decoder: partial code point and continuation bytes left.
    pending: u32,
    remaining: u8,
    dead: bool,
}

impl DamerauLevenshteinAutomaton {
    /// Builds the automaton. `max_edits` is clamped to
    /// [`MAX_AUTOMATON_EDITS`]; `prefix_chars` is clamped to the query
    /// length (callers implementing Lucene semantics switch to an exact
    /// lookup when `prefix_chars >= query length`, see
    /// `surch_search::fuzzy`).
    pub fn new(query: &str, max_edits: u8, prefix_chars: usize, transpositions: bool) -> Self {
        let query: Vec<char> = query.chars().collect();
        let prefix_chars = prefix_chars.min(query.len());
        Self {
            query,
            max_edits: max_edits.min(MAX_AUTOMATON_EDITS),
            prefix_chars,
            transpositions,
        }
    }

    /// The (clamped) maximum edit distance this automaton accepts.
    pub fn max_edits(&self) -> u8 {
        self.max_edits
    }

    /// The query's leading chars that every accepted term must share, as a
    /// UTF-8 string (empty when `prefix_chars == 0`). Lets callers bound
    /// the FST walk with a range.
    pub fn required_prefix(&self) -> String {
        self.query[..self.prefix_chars].iter().collect()
    }

    fn cap(&self) -> u8 {
        self.max_edits + 1
    }

    fn width(&self) -> usize {
        2 * usize::from(self.max_edits) + 1
    }

    /// Edit distance of an accepted state, `None` when the state does not
    /// match (partial char, dead branch, or distance above `max_edits`).
    pub fn distance(&self, state: &DamerauLevenshteinState) -> Option<u8> {
        if state.dead || state.remaining != 0 || (state.chars as usize) < self.prefix_chars {
            return None;
        }
        let k = isize::from(self.max_edits);
        let idx = self.query.len() as isize + k - state.chars as isize;
        if idx < 0 || idx > 2 * k {
            return None;
        }
        let value = state.row[idx as usize];
        (value <= self.max_edits).then_some(value)
    }

    fn dead_state(state: &DamerauLevenshteinState) -> DamerauLevenshteinState {
        DamerauLevenshteinState {
            dead: true,
            ..*state
        }
    }

    /// Advances the DP by one complete term char.
    fn step_char(&self, state: &DamerauLevenshteinState, c: char) -> DamerauLevenshteinState {
        let i = state.chars as usize;
        if i < self.prefix_chars && self.query.get(i) != Some(&c) {
            return Self::dead_state(state);
        }
        let k = isize::from(self.max_edits);
        let cap = self.cap();
        let width = self.width();
        let n = self.query.len() as isize;
        let i1 = i as isize + 1;
        let mut next = [cap; BAND_CAPACITY];
        for idx in 0..width {
            let j = i1 + idx as isize - k;
            if j < 0 || j > n {
                continue;
            }
            if j == 0 {
                next[idx] = u8::try_from(i1).unwrap_or(u8::MAX).min(cap);
                continue;
            }
            let ju = j as usize;
            // Deletion of the term char: D[i][j] + 1 (old band idx + 1).
            let deletion = if idx + 1 < width {
                state.row[idx + 1]
            } else {
                cap
            };
            // Insertion of the query char: D[i+1][j-1] + 1 (new band idx - 1).
            let insertion = if idx == 0 { cap } else { next[idx - 1] };
            // Substitution / match: D[i][j-1] + cost (old band idx).
            let substitution = state.row[idx].saturating_add(u8::from(self.query[ju - 1] != c));
            let mut value = deletion
                .saturating_add(1)
                .min(insertion.saturating_add(1))
                .min(substitution);
            if self.transpositions
                && i1 >= 2
                && ju >= 2
                && c == self.query[ju - 2]
                && state.last_char == self.query[ju - 1]
            {
                // D[i-1][j-2] + 1 sits at the same band idx in row i - 1.
                value = value.min(state.prev_row[idx].saturating_add(1));
            }
            next[idx] = value.min(cap);
        }
        DamerauLevenshteinState {
            chars: state.chars.saturating_add(1),
            row: next,
            prev_row: state.row,
            last_char: c,
            pending: 0,
            remaining: 0,
            dead: false,
        }
    }
}

impl Automaton for DamerauLevenshteinAutomaton {
    type State = DamerauLevenshteinState;

    fn start(&self) -> Self::State {
        let k = isize::from(self.max_edits);
        let cap = self.cap();
        let n = self.query.len() as isize;
        let mut row = [cap; BAND_CAPACITY];
        for (idx, cell) in row.iter_mut().enumerate().take(self.width()) {
            let j = idx as isize - k;
            if (0..=n).contains(&j) {
                *cell = u8::try_from(j).unwrap_or(u8::MAX).min(cap);
            }
        }
        DamerauLevenshteinState {
            chars: 0,
            row,
            prev_row: [cap; BAND_CAPACITY],
            last_char: '\0',
            pending: 0,
            remaining: 0,
            dead: false,
        }
    }

    fn is_match(&self, state: &Self::State) -> bool {
        self.distance(state).is_some()
    }

    fn can_match(&self, state: &Self::State) -> bool {
        !state.dead
            && state.row[..self.width()]
                .iter()
                .any(|&cell| cell <= self.max_edits)
    }

    fn accept(&self, state: &Self::State, byte: u8) -> Self::State {
        if state.dead {
            return *state;
        }
        if state.remaining == 0 {
            let (pending, remaining) = match byte {
                0x00..=0x7F => return self.step_char(state, char::from(byte)),
                0xC0..=0xDF => (u32::from(byte & 0x1F), 1),
                0xE0..=0xEF => (u32::from(byte & 0x0F), 2),
                0xF0..=0xF7 => (u32::from(byte & 0x07), 3),
                _ => return Self::dead_state(state),
            };
            return DamerauLevenshteinState {
                pending,
                remaining,
                ..*state
            };
        }
        if byte & 0xC0 != 0x80 {
            return Self::dead_state(state);
        }
        let pending = (state.pending << 6) | u32::from(byte & 0x3F);
        let remaining = state.remaining - 1;
        if remaining > 0 {
            return DamerauLevenshteinState {
                pending,
                remaining,
                ..*state
            };
        }
        match char::from_u32(pending) {
            Some(c) => self.step_char(state, c),
            None => Self::dead_state(state),
        }
    }
}

/// Runs the automaton over a whole string (test/diagnostic helper and the
/// reference for per-term distance checks): `Some(distance)` when `term`
/// is accepted.
pub fn automaton_distance(automaton: &DamerauLevenshteinAutomaton, term: &str) -> Option<u8> {
    let mut state = automaton.start();
    for &byte in term.as_bytes() {
        if !automaton.can_match(&state) {
            return None;
        }
        state = automaton.accept(&state, byte);
    }
    automaton.distance(&state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(query: &str, term: &str, k: u8) -> Option<u8> {
        automaton_distance(&DamerauLevenshteinAutomaton::new(query, k, 0, true), term)
    }

    #[test]
    fn exact_and_single_edits() {
        assert_eq!(dist("dupont", "dupont", 0), Some(0));
        assert_eq!(dist("dupont", "dupond", 0), None);
        assert_eq!(dist("dupont", "dupond", 1), Some(1));
        assert_eq!(dist("dupont", "dupontt", 1), Some(1));
        assert_eq!(dist("dupont", "dupon", 1), Some(1));
        assert_eq!(dist("dupont", "dpuont", 1), Some(1));
        assert_eq!(dist("dupont", "dpuont", 2), Some(1));
        assert_eq!(dist("dupont", "martin", 2), None);
    }

    #[test]
    fn transpositions_can_be_disabled() {
        let plain = DamerauLevenshteinAutomaton::new("jean", 1, 0, false);
        assert_eq!(automaton_distance(&plain, "jaen"), None);
        let plain2 = DamerauLevenshteinAutomaton::new("jean", 2, 0, false);
        assert_eq!(automaton_distance(&plain2, "jaen"), Some(2));
    }

    #[test]
    fn distances_count_chars_not_bytes() {
        assert_eq!(dist("helene", "hélène", 2), Some(2));
        assert_eq!(dist("hélène", "hélèn", 1), Some(1));
        assert_eq!(dist("éloïse", "éolïse", 1), Some(1));
    }

    #[test]
    fn prefix_chars_must_match_exactly() {
        let automaton = DamerauLevenshteinAutomaton::new("martin", 1, 2, true);
        assert_eq!(automaton_distance(&automaton, "martyn"), Some(1));
        assert_eq!(automaton_distance(&automaton, "mratin"), None);
        assert_eq!(automaton_distance(&automaton, "m"), None);
    }

    #[test]
    fn empty_query_accepts_short_terms() {
        assert_eq!(dist("", "", 1), Some(0));
        assert_eq!(dist("", "a", 1), Some(1));
        assert_eq!(dist("", "ab", 1), None);
    }
}
