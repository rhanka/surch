//! Bounded fuzzy term distance primitives.

use thiserror::Error;

/// Lucene's fuzzy query implementation accepts edit distances from 0 to 2.
pub const MAX_SUPPORTED_EDITS: u8 = 2;

/// Errors returned by fuzzy term helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FuzzyError {
    /// The requested edit distance exceeds Lucene's supported fuzzy distance.
    #[error("max_edits {max_edits} exceeds Lucene fuzzy limit of 2")]
    MaxEditsTooLarge { max_edits: u8 },
}

/// Computes a bounded Damerau-Levenshtein distance for Rust `char` values.
///
/// Returns `Ok(None)` when the distance is greater than `max_edits`.
/// Adjacent transpositions cost one edit when `transpositions` is enabled;
/// otherwise they are counted as the two substitutions required by plain
/// Levenshtein distance.
pub fn bounded_damerau_levenshtein(
    left: &str,
    right: &str,
    max_edits: u8,
    transpositions: bool,
) -> Result<Option<u8>, FuzzyError> {
    if max_edits > MAX_SUPPORTED_EDITS {
        return Err(FuzzyError::MaxEditsTooLarge { max_edits });
    }

    if left == right {
        return Ok(Some(0));
    }

    let left_chars: Vec<char> = left.chars().collect();
    let right_chars: Vec<char> = right.chars().collect();
    let max_edits = usize::from(max_edits);

    if left_chars.len().abs_diff(right_chars.len()) > max_edits {
        return Ok(None);
    }

    let distance = damerau_levenshtein_chars(&left_chars, &right_chars, transpositions);
    if distance <= max_edits {
        Ok(Some(distance as u8))
    } else {
        Ok(None)
    }
}

fn damerau_levenshtein_chars(left: &[char], right: &[char], transpositions: bool) -> usize {
    let right_len = right.len();
    let mut rows = Vec::with_capacity(left.len() + 1);
    rows.push((0..=right_len).collect::<Vec<_>>());

    for i in 1..=left.len() {
        let mut row = Vec::with_capacity(right_len + 1);
        row.push(i);

        for j in 1..=right_len {
            let substitution_cost = usize::from(left[i - 1] != right[j - 1]);
            let deletion = rows[i - 1][j] + 1;
            let insertion = row[j - 1] + 1;
            let substitution = rows[i - 1][j - 1] + substitution_cost;
            let mut distance = deletion.min(insertion).min(substitution);

            if transpositions
                && i > 1
                && j > 1
                && left[i - 1] == right[j - 2]
                && left[i - 2] == right[j - 1]
            {
                distance = distance.min(rows[i - 2][j - 2] + 1);
            }

            row.push(distance);
        }

        rows.push(row);
    }

    rows[left.len()][right_len]
}
