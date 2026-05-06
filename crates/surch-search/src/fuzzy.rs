//! Bounded fuzzy term distance primitives.

use thiserror::Error;

/// Lucene's fuzzy query implementation accepts edit distances from 0 to 2.
pub const MAX_SUPPORTED_EDITS: u8 = 2;

/// Fuzzy edit distance configuration compatible with Lucene/OpenSearch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fuzziness {
    /// Use a fixed edit distance.
    Edits(u8),
    /// Choose edit distance from the indexed term length.
    Auto { low: usize, high: usize },
}

/// Fuzzy query runtime configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FuzzyQueryConfig {
    pub fuzziness: Fuzziness,
    pub prefix_length: usize,
    pub max_expansions: usize,
    pub transpositions: bool,
}

/// Errors returned by fuzzy term helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FuzzyError {
    /// The requested edit distance exceeds Lucene's supported fuzzy distance.
    #[error("max_edits {max_edits} exceeds Lucene fuzzy limit of 2")]
    MaxEditsTooLarge { max_edits: u8 },
    /// A fuzziness string is not one of the supported OpenSearch forms.
    #[error("invalid fuzziness value")]
    InvalidFuzziness,
    /// AUTO fuzziness thresholds must be ordered.
    #[error("invalid AUTO fuzziness thresholds: low {low} is greater than high {high}")]
    InvalidAutoBounds { low: usize, high: usize },
    /// Lucene/OpenSearch require at least one fuzzy term expansion.
    #[error("max_expansions must be greater than 0")]
    MaxExpansionsZero,
}

impl FuzzyQueryConfig {
    /// Creates a fuzzy query configuration after validating Lucene limits.
    pub fn new(
        fuzziness: Fuzziness,
        prefix_length: usize,
        max_expansions: usize,
        transpositions: bool,
    ) -> Result<Self, FuzzyError> {
        validate_fuzziness(fuzziness)?;
        if max_expansions == 0 {
            return Err(FuzzyError::MaxExpansionsZero);
        }

        Ok(Self {
            fuzziness,
            prefix_length,
            max_expansions,
            transpositions,
        })
    }
}

/// Parses the P0 fuzzy forms used by OpenSearch/Lucene.
pub fn parse_fuzziness(value: &str) -> Result<Fuzziness, FuzzyError> {
    if value == "AUTO" {
        return Ok(Fuzziness::Auto { low: 3, high: 6 });
    }

    if let Some(bounds) = value.strip_prefix("AUTO:") {
        let (low, high) = bounds.split_once(',').ok_or(FuzzyError::InvalidFuzziness)?;
        let low = low
            .parse::<usize>()
            .map_err(|_| FuzzyError::InvalidFuzziness)?;
        let high = high
            .parse::<usize>()
            .map_err(|_| FuzzyError::InvalidFuzziness)?;
        let fuzziness = Fuzziness::Auto { low, high };
        validate_fuzziness(fuzziness)?;

        return Ok(fuzziness);
    }

    let max_edits = value
        .parse::<u8>()
        .map_err(|_| FuzzyError::InvalidFuzziness)?;
    validate_max_edits(max_edits)?;

    Ok(Fuzziness::Edits(max_edits))
}

/// Resolves a fuzzy edit distance for a term length.
pub fn edits_for_term_len(fuzziness: Fuzziness, len: usize) -> Result<u8, FuzzyError> {
    validate_fuzziness(fuzziness)?;

    match fuzziness {
        Fuzziness::Edits(max_edits) => Ok(max_edits),
        Fuzziness::Auto { low, .. } if len < low => Ok(0),
        Fuzziness::Auto { high, .. } if len < high => Ok(1),
        Fuzziness::Auto { .. } => Ok(2),
    }
}

fn validate_fuzziness(fuzziness: Fuzziness) -> Result<(), FuzzyError> {
    match fuzziness {
        Fuzziness::Edits(max_edits) => validate_max_edits(max_edits),
        Fuzziness::Auto { low, high } if low > high => {
            Err(FuzzyError::InvalidAutoBounds { low, high })
        }
        Fuzziness::Auto { .. } => Ok(()),
    }
}

fn validate_max_edits(max_edits: u8) -> Result<(), FuzzyError> {
    if max_edits > MAX_SUPPORTED_EDITS {
        return Err(FuzzyError::MaxEditsTooLarge { max_edits });
    }

    Ok(())
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
