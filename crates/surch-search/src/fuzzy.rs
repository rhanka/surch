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

/// A term accepted by fuzzy expansion, with its edit distance from the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyTermMatch {
    pub term: String,
    pub edits: u8,
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
///
/// `AUTO` and `AUTO:<low>,<high>` are case-insensitive — ES 7.x accepts
/// both `"AUTO"` and `"auto"`; matchID's deces-backend uses lowercase
/// in production. Numeric edits (`"0"`, `"1"`, `"2"`) are parsed as-is.
pub fn parse_fuzziness(value: &str) -> Result<Fuzziness, FuzzyError> {
    if value.eq_ignore_ascii_case("AUTO") {
        return Ok(Fuzziness::Auto { low: 3, high: 6 });
    }

    let auto_prefixed = value
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("AUTO:"));
    if auto_prefixed {
        let bounds = &value[5..];
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

/// Expands a fuzzy query against a term list using Lucene/OpenSearch P0 rules.
pub fn expand_fuzzy_terms<'a>(
    query: &str,
    terms: impl IntoIterator<Item = &'a str>,
    config: FuzzyQueryConfig,
) -> Result<Vec<FuzzyTermMatch>, FuzzyError> {
    if config.max_expansions == 0 {
        return Err(FuzzyError::MaxExpansionsZero);
    }

    let max_edits = edits_for_term_len(config.fuzziness, query.chars().count())?;
    let mut matches = Vec::new();

    for term in terms {
        if !has_exact_char_prefix(query, term, config.prefix_length) {
            continue;
        }

        if let Some(edits) =
            bounded_damerau_levenshtein(query, term, max_edits, config.transpositions)?
        {
            matches.push(FuzzyTermMatch {
                term: term.to_string(),
                edits,
            });
        }
    }

    matches.sort_by(|left, right| {
        left.edits
            .cmp(&right.edits)
            .then_with(|| left.term.cmp(&right.term))
    });
    matches.truncate(config.max_expansions);

    Ok(matches)
}

fn has_exact_char_prefix(query: &str, term: &str, prefix_length: usize) -> bool {
    if prefix_length == 0 {
        return true;
    }

    let mut query_chars = query.chars();
    let mut term_chars = term.chars();

    for _ in 0..prefix_length {
        match (query_chars.next(), term_chars.next()) {
            (Some(query_char), Some(term_char)) if query_char == term_char => {}
            _ => return false,
        }
    }

    true
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

/// MVP L1: effective per-token fuzzy plan, following Lucene `FuzzyQuery`:
/// the edit budget comes from the token length (AUTO) or the fixed
/// fuzziness; when it is 0, or when `prefix_length` covers the whole token,
/// Lucene degrades to an exact single-term lookup (`SingleTermsEnum`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FuzzyTokenPlan {
    /// Maximum edit distance for this token (0 = exact term only).
    pub max_edits: u8,
    /// Leading chars that must match exactly (0 when `max_edits == 0`).
    pub prefix_chars: usize,
}

/// Resolves the [`FuzzyTokenPlan`] of one analysed query token.
pub fn fuzzy_token_plan(
    token: &str,
    fuzziness: Fuzziness,
    prefix_length: usize,
) -> Result<FuzzyTokenPlan, FuzzyError> {
    let len = token.chars().count();
    let max_edits = edits_for_term_len(fuzziness, len)?;
    if max_edits == 0 || prefix_length >= len {
        return Ok(FuzzyTokenPlan {
            max_edits: 0,
            prefix_chars: 0,
        });
    }
    Ok(FuzzyTokenPlan {
        max_edits,
        prefix_chars: prefix_length,
    })
}

/// Lucene `FuzzyTermsEnum` boost of an expanded term: `1.0` for the exact
/// term, otherwise `1 - edits / min(term_chars, query_chars)` (computed in
/// `f32`, like Lucene, so tie-breaking on equal boosts is reproducible).
///
/// L7: verified against Lucene 9.4.2 `FuzzyTermsEnum.next` (l.248-255):
/// `ed == 0 ? 1.0F : 1.0f - (float) ed / (float) Math.min(codePointCount(term), termLength)`
/// where `termLength` is the code-point length of the WHOLE query term,
/// prefix included (`FuzzyAutomatonBuilder` l.50-51). The value may be
/// negative (`ed > min length`): `TopTermsRewrite` selects on the raw value
/// and only clamps it when building the clause (`Math.max(0.0f, st.boost)`,
/// TopTermsRewrite.java l.171-172) — the scorer applies that clamp.
pub fn lucene_fuzzy_similarity(query_chars: usize, term: &str, edits: u8) -> f32 {
    if edits == 0 {
        return 1.0;
    }
    let min_len = term.chars().count().min(query_chars);
    if min_len == 0 {
        return 0.0;
    }
    1.0_f32 - f32::from(edits) / min_len as f32
}

/// An expanded term kept by [`select_top_fuzzy_terms`], with the Lucene
/// boost it carries into scoring.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredFuzzyTerm {
    pub term: String,
    pub edits: u8,
    pub boost: f32,
}

/// Keeps the `max_expansions` best fuzzy expansions the way Lucene's
/// `TopTermsRewrite` (ES default `top_terms_blended_freqs_N` for `fuzzy`
/// and `match`+`fuzziness`) does: highest [`lucene_fuzzy_similarity`]
/// first, ties broken by ascending term bytes (UTF-8 byte order equals
/// `str` order). The selection is global over the whole index (callers
/// pass the union of every segment's matches). Returned in that rank
/// order.
///
/// Note: this differs from [`expand_fuzzy_terms`], which orders by raw
/// edit count then term and is kept as the plain-list oracle.
pub fn select_top_fuzzy_terms(
    query: &str,
    hits: impl IntoIterator<Item = FuzzyTermMatch>,
    max_expansions: usize,
) -> Vec<ScoredFuzzyTerm> {
    let query_chars = query.chars().count();
    let mut scored: Vec<ScoredFuzzyTerm> = hits
        .into_iter()
        .map(|hit| ScoredFuzzyTerm {
            boost: lucene_fuzzy_similarity(query_chars, &hit.term, hit.edits),
            term: hit.term,
            edits: hit.edits,
        })
        .collect();
    scored.sort_by(|left, right| {
        right
            .boost
            .total_cmp(&left.boost)
            .then_with(|| left.term.cmp(&right.term))
    });
    scored.truncate(max_expansions);
    scored
}

/// ES / Lucene default `max_expansions` for `fuzzy` and `match`+`fuzziness`.
pub const DEFAULT_MAX_EXPANSIONS: usize = 50;
