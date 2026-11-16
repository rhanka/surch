//! Lucene 9.4.2 / Elasticsearch 8.6.1 scoring primitives, bit-exact in `f32`.
//!
//! # Which similarity ES 8.6.1 actually runs
//!
//! ES 8.6.1 bundles Lucene 9.4.2 (`build-tools-internal/version.properties`:
//! `lucene = 9.4.2`) but its default `BM25` similarity is NOT Lucene's
//! `BM25Similarity` as-is. `SimilarityService` (v8.6.1,
//! `server/.../index/similarity/SimilarityService.java:48-50`) registers
//! `SimilarityProviders.createBM25Similarity` (`SimilarityProviders.java:254-261`),
//! which returns `org.elasticsearch.lucene.similarity.LegacyBM25Similarity`.
//! That class delegates to Lucene's `BM25Similarity` but re-introduces the
//! classic `(k1 + 1)` numerator factor through the boost
//! (`LegacyBM25Similarity.java:66-67`):
//!
//! ```java
//! public SimScorer scorer(float boost, CollectionStatistics collectionStats, TermStatistics... termStats) {
//!     return bm25Similarity.scorer(boost * (1 + bm25Similarity.getK1()), collectionStats, termStats);
//! }
//! ```
//!
//! So an ES 8.6.1 `_score` keeps the `(k1 + 1) = 2.2` factor; only the
//! arithmetic (f32, Lucene 9 rewrite of the tf term) differs from the
//! classic f64 formula Surch used before lot L7.
//!
//! # Lucene 9.4.2 `BM25Similarity` (lucene/core/.../similarities/BM25Similarity.java)
//!
//! * `idf` (l.104-106):
//!   `(float) Math.log(1 + (docCount - docFreq + 0.5D) / (docFreq + 0.5D))`
//! * `avgFieldLength` (l.109-111):
//!   `(float) (collectionStats.sumTotalTermFreq() / (double) collectionStats.docCount())`
//! * `LENGTH_TABLE[i] = SmallFloat.byte4ToInt((byte) i)` (l.123-129)
//! * per-norm cache in `scorer` (l.205-208):
//!   `cache[i] = 1f / (k1 * ((1 - b) + b * LENGTH_TABLE[i] / avgdl))`
//! * `BM25Scorer` (l.229-252): `weight = boost * idf`, and
//!   `score(freq, encodedNorm) = weight - weight / (1f + freq * cache[norm & 0xFF])`
//!
//! Every operation below is an IEEE-754 binary32 operation in the same
//! evaluation order as the Java source (Java 17+ float arithmetic is always
//! strict; Rust never contracts `a * b + c` into an FMA without an explicit
//! `mul_add`). `Math.log` is the only `f64` transcendental: Rust's `f64::ln`
//! (glibc `log`, < 1 ulp) and HotSpot's `Math.log` intrinsic can in theory
//! differ by one `f64` ulp, which changes the `(float)` cast only when the
//! value sits on an f32 rounding boundary (probability ~2^-29 per idf);
//! this residual is documented in `docs/wp-d-matchid/l7-score-parity-gate.md`.
//!
//! Norms disabled (keyword fields): `LeafSimScorer.getNormValue` returns `1L`
//! when the field has no norms (Lucene 9.4.2 `LeafSimScorer.java`), so the
//! cache entry used is `cache[1]`, i.e. `doc_len = LENGTH_TABLE[1] = 1`.

use thiserror::Error;

use crate::small_float::byte4_to_int;

/// BM25 configuration compatible with Lucene's BM25Similarity defaults.
///
/// Stored as `f64` for API stability; the Lucene kernel runs on the `f32`
/// values (`BM25Similarity` holds `float k1, b`; ES passes `1.2f`, `0.75f`,
/// `SimilarityProviders.java:257-258`). `1.2_f64 as f32 == 1.2f32`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bm25Config {
    pub k1: f64,
    pub b: f64,
}

/// Errors returned by BM25 scoring helpers.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum Bm25Error {
    /// The corpus must contain at least one document.
    #[error("doc_count must be greater than 0")]
    DocCountZero,
    /// The term must appear in at least one document.
    #[error("doc_freq must be greater than 0")]
    DocFreqZero,
    /// The term document frequency cannot exceed the corpus document count.
    #[error("doc_freq {doc_freq} exceeds doc_count {doc_count}")]
    DocFreqExceedsDocCount { doc_freq: u64, doc_count: u64 },
    /// The term must occur at least once in the scored document.
    #[error("term_freq must be greater than 0")]
    TermFreqZero,
    /// The scored document length must be positive.
    #[error("doc_len must be greater than 0")]
    DocLenZero,
    /// Average document length must be a positive finite value.
    #[error("avg_doc_len must be greater than 0")]
    AvgDocLenNotPositive,
    /// Lucene BM25 accepts non-negative k1 values.
    #[error("k1 {k1} must be greater than or equal to 0")]
    NegativeK1 { k1: f64 },
    /// k1 must be finite.
    #[error("k1 must be finite")]
    NonFiniteK1,
    /// Lucene BM25 requires b to be in the inclusive range [0, 1].
    #[error("b {b} must be in the inclusive range [0, 1]")]
    BOutOfRange { b: f64 },
    /// b must be finite.
    #[error("b must be finite")]
    NonFiniteB,
}

impl Bm25Config {
    /// Creates a BM25 configuration after validating Lucene parameter bounds.
    pub fn new(k1: f64, b: f64) -> Result<Self, Bm25Error> {
        if !k1.is_finite() {
            return Err(Bm25Error::NonFiniteK1);
        }

        if k1 < 0.0 {
            return Err(Bm25Error::NegativeK1 { k1 });
        }

        if !b.is_finite() {
            return Err(Bm25Error::NonFiniteB);
        }

        if !(0.0..=1.0).contains(&b) {
            return Err(Bm25Error::BOutOfRange { b });
        }

        Ok(Self { k1, b })
    }

    /// `BM25Similarity.k1` as the Java `float` field.
    #[inline]
    pub fn k1_f32(&self) -> f32 {
        self.k1 as f32
    }

    /// `BM25Similarity.b` as the Java `float` field.
    #[inline]
    pub fn b_f32(&self) -> f32 {
        self.b as f32
    }
}

impl Default for Bm25Config {
    fn default() -> Self {
        Self { k1: 1.2, b: 0.75 }
    }
}

/// Lucene 9.4.2 `BM25Similarity.idf(long docFreq, long docCount)`
/// (BM25Similarity.java:104-106):
/// `(float) Math.log(1 + (docCount - docFreq + 0.5D) / (docFreq + 0.5D))`.
///
/// `docCount - docFreq` is a Java `long` subtraction, then widened to
/// `double`. No validation: callers guarantee `doc_freq <= doc_count`.
#[inline]
pub fn lucene_idf(doc_freq: u64, doc_count: u64) -> f32 {
    let numerator = doc_count.saturating_sub(doc_freq) as f64 + 0.5;
    let denominator = doc_freq as f64 + 0.5;
    (1.0_f64 + numerator / denominator).ln() as f32
}

/// Lucene 9.4.2 `BM25Similarity.avgFieldLength` (BM25Similarity.java:109-111):
/// `(float) (collectionStats.sumTotalTermFreq() / (double) collectionStats.docCount())`.
#[inline]
pub fn lucene_avg_field_length(sum_total_term_freq: u64, doc_count: u64) -> f32 {
    (sum_total_term_freq as f64 / doc_count as f64) as f32
}

/// Lucene 9.4.2 `BM25Similarity.LENGTH_TABLE[norm & 0xFF]`
/// (BM25Similarity.java:123-129): `(float) SmallFloat.byte4ToInt((byte) i)`.
#[inline]
pub fn lucene_length_table(encoded_norm: u8) -> f32 {
    byte4_to_int(encoded_norm) as f32
}

/// One entry of the `BM25Similarity.scorer` per-norm cache
/// (BM25Similarity.java:205-208):
/// `1f / (k1 * ((1 - b) + b * LENGTH_TABLE[i] / avgdl))`.
///
/// Java evaluation order: `b * LENGTH_TABLE[i]` then `/ avgdl` (left
/// associative), `(1 - b)` is `int - float -> float`.
#[inline]
pub fn lucene_norm_inverse(k1: f32, b: f32, doc_len: f32, avgdl: f32) -> f32 {
    1.0_f32 / (k1 * ((1.0_f32 - b) + b * doc_len / avgdl))
}

/// ES 8.6.1 `LegacyBM25Similarity.scorer` boost (LegacyBM25Similarity.java:67):
/// `boost * (1 + bm25Similarity.getK1())`, all `float`.
#[inline]
pub fn es_legacy_bm25_boost(query_boost: f32, k1: f32) -> f32 {
    query_boost * (1.0_f32 + k1)
}

/// Lucene 9.4.2 `BM25Scorer` constructor (BM25Similarity.java:236):
/// `this.weight = boost * idf.getValue().floatValue()`.
#[inline]
pub fn lucene_bm25_weight(boost: f32, idf: f32) -> f32 {
    boost * idf
}

/// Lucene 9.4.2 `BM25Scorer.score(float freq, long encodedNorm)`
/// (BM25Similarity.java:240-252):
/// `weight - weight / (1f + freq * normInverse)`.
#[inline]
pub fn lucene_bm25_score(weight: f32, freq: f32, norm_inverse: f32) -> f32 {
    weight - weight / (1.0_f32 + freq * norm_inverse)
}

/// Lucene's `(float) doubleSum` combination used by every scorer that sums
/// sub-scores into a `double` before casting (Lucene 9.4.2
/// `DisjunctionSumScorer.score` l.38-43, `ConjunctionScorer.score` l.58-63,
/// `BlockMaxConjunctionScorer.score` l.206-211, `WANDScorer.score`
/// l.522-529, `BooleanScorer` bucket `double score` l.40/138/187).
///
/// The order of `scores` is the clause order. Because every summand is an
/// `f32` widened to `f64`, the `f64` sum of a handful of BM25 scores is exact
/// (their exponents span far less than 53 - 24 bits), so the result does not
/// depend on the order Lucene's heap visits the sub-scorers in.
#[inline]
pub fn lucene_double_sum<I: IntoIterator<Item = f32>>(scores: I) -> f32 {
    let mut sum = 0.0_f64;
    for score in scores {
        sum += f64::from(score);
    }
    sum as f32
}

/// Lucene 9.4.2 `ReqOptSumScorer.score` (ReqOptSumScorer.java:260-276):
/// `float score = reqScorer.score(); ... score += optScorer.score();` — a
/// plain `float` addition of the required and the optional sub-scores.
#[inline]
pub fn lucene_req_opt_sum(required: f32, optional: Option<f32>) -> f32 {
    match optional {
        Some(optional) => required + optional,
        None => required,
    }
}

/// Computes the BM25 inverse document frequency (Lucene `BM25Similarity.idf`,
/// `f32`), widened to `f64` (exact) for API compatibility.
pub fn bm25_idf(doc_count: u64, doc_freq: u64) -> Result<f64, Bm25Error> {
    validate_corpus_stats(doc_count, doc_freq)?;

    Ok(f64::from(lucene_idf(doc_freq, doc_count)))
}

/// A BM25 scorer with the per-term constants precomputed ONCE, so the
/// per-document hot loop is branch-free and `ln()`-free.
///
/// Mirrors ES 8.6.1 `LegacyBM25Similarity.scorer` -> Lucene 9.4.2
/// `BM25Similarity.scorer` -> `BM25Scorer`: `weight = (boost * (1 + k1)) *
/// idf` in `f32`, and per document `weight - weight / (1f + freq *
/// normInverse)` with `normInverse` the cache entry of the document's
/// (already `SmallFloat`-reconstructed) length.
///
/// [`Self::score`] returns the `f32` score widened to `f64` (exact): callers
/// that sum several term scores in `f64` then cast to `f32` reproduce
/// Lucene's `double` accumulation exactly (see [`lucene_double_sum`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bm25TermScorer {
    weight: f32,
    k1: f32,
    b: f32,
    avgdl: f32,
}

impl Bm25TermScorer {
    /// Unboosted (`boost = 1f`) scorer. Returns the same errors
    /// [`bm25_score`] would for invalid config / corpus stats / `avg_doc_len`.
    ///
    /// `avg_doc_len` must be `sumTotalTermFreq as f64 / docCount as f64`
    /// (Surch's `FieldLengthStats::avg_doc_len`): it is cast to `f32` exactly
    /// like `BM25Similarity.avgFieldLength`.
    pub fn new(
        config: Bm25Config,
        doc_count: u64,
        doc_freq: u64,
        avg_doc_len: f64,
    ) -> Result<Self, Bm25Error> {
        Self::with_boost(config, doc_count, doc_freq, avg_doc_len, 1.0)
    }

    /// Scorer for a term whose query boost (the product of every enclosing
    /// `BoostQuery` / clause boost, as Lucene's `createWeight` threads it) is
    /// `boost`. ES's legacy `(1 + k1)` factor is applied on top.
    pub fn with_boost(
        config: Bm25Config,
        doc_count: u64,
        doc_freq: u64,
        avg_doc_len: f64,
        boost: f32,
    ) -> Result<Self, Bm25Error> {
        let config = Bm25Config::new(config.k1, config.b)?;
        validate_corpus_stats(doc_count, doc_freq)?;
        if !avg_doc_len.is_finite() || avg_doc_len <= 0.0 {
            return Err(Bm25Error::AvgDocLenNotPositive);
        }
        let k1 = config.k1_f32();
        let idf = lucene_idf(doc_freq, doc_count);
        Ok(Self {
            weight: lucene_bm25_weight(es_legacy_bm25_boost(boost, k1), idf),
            k1,
            b: config.b_f32(),
            avgdl: avg_doc_len as f32,
        })
    }

    /// `BM25Scorer.weight` (`boost * (1 + k1) * idf`, `f32`).
    #[inline]
    pub fn weight(&self) -> f32 {
        self.weight
    }

    /// Per-document BM25 score in `f32` (`BM25Scorer.score`). `doc_len` is the
    /// reconstructed `LENGTH_TABLE` value (1 when norms are disabled).
    #[inline]
    pub fn score_f32(&self, term_freq: u64, doc_len: u64) -> f32 {
        let norm_inverse = lucene_norm_inverse(self.k1, self.b, doc_len as f32, self.avgdl);
        lucene_bm25_score(self.weight, term_freq as f32, norm_inverse)
    }

    /// [`Self::score_f32`] widened to `f64` (exact).
    #[inline]
    pub fn score(&self, term_freq: u64, doc_len: u64) -> f64 {
        f64::from(self.score_f32(term_freq, doc_len))
    }
}

/// Computes a BM25 term score (ES 8.6.1 legacy BM25 over Lucene 9.4.2, `f32`
/// arithmetic, widened to `f64`) using explicit corpus and document
/// statistics. `doc_len` must be the reconstructed `SmallFloat` length.
pub fn bm25_score(
    config: Bm25Config,
    doc_count: u64,
    doc_freq: u64,
    term_freq: u64,
    doc_len: u64,
    avg_doc_len: f64,
) -> Result<f64, Bm25Error> {
    validate_score_inputs(doc_count, doc_freq, term_freq, doc_len, avg_doc_len)?;
    let scorer = Bm25TermScorer::new(config, doc_count, doc_freq, avg_doc_len)?;
    Ok(scorer.score(term_freq, doc_len))
}

fn validate_corpus_stats(doc_count: u64, doc_freq: u64) -> Result<(), Bm25Error> {
    if doc_count == 0 {
        return Err(Bm25Error::DocCountZero);
    }

    if doc_freq == 0 {
        return Err(Bm25Error::DocFreqZero);
    }

    if doc_freq > doc_count {
        return Err(Bm25Error::DocFreqExceedsDocCount {
            doc_freq,
            doc_count,
        });
    }

    Ok(())
}

fn validate_score_inputs(
    doc_count: u64,
    doc_freq: u64,
    term_freq: u64,
    doc_len: u64,
    avg_doc_len: f64,
) -> Result<(), Bm25Error> {
    validate_corpus_stats(doc_count, doc_freq)?;

    if term_freq == 0 {
        return Err(Bm25Error::TermFreqZero);
    }

    if doc_len == 0 {
        return Err(Bm25Error::DocLenZero);
    }

    if !avg_doc_len.is_finite() || avg_doc_len <= 0.0 {
        return Err(Bm25Error::AvgDocLenNotPositive);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent re-derivation of every Java `float` operation through
    /// `f64` followed by an explicit `as f32` rounding. For `+ - * /` on
    /// binary32 inputs, computing in binary64 then rounding to binary32 is
    /// exactly the correctly rounded binary32 result (53 >= 2 * 24 + 2), so
    /// this is an oracle that shares no code with the kernel above.
    fn oracle_score(
        doc_count: u64,
        doc_freq: u64,
        sum_ttf: u64,
        freq: u64,
        norm: u8,
        boost: f32,
    ) -> f32 {
        let r = |x: f64| x as f32;
        let k1 = 1.2_f32;
        let b = 0.75_f32;
        let idf = r((1.0 + ((doc_count - doc_freq) as f64 + 0.5) / (doc_freq as f64 + 0.5)).ln());
        let avgdl = r(sum_ttf as f64 / doc_count as f64);
        let len = byte4_to_int(norm) as f32;
        let one_minus_b = r(1.0 - f64::from(b));
        let b_len = r(f64::from(b) * f64::from(len));
        let b_len_avg = r(f64::from(b_len) / f64::from(avgdl));
        let inner = r(f64::from(one_minus_b) + f64::from(b_len_avg));
        let k1_inner = r(f64::from(k1) * f64::from(inner));
        let cache = r(1.0 / f64::from(k1_inner));
        let legacy = r(f64::from(boost) * f64::from(r(1.0 + f64::from(k1))));
        let weight = r(f64::from(legacy) * f64::from(idf));
        let freq_norm = r(freq as f64 * f64::from(cache));
        let denom = r(1.0 + f64::from(freq_norm));
        let quotient = r(f64::from(weight) / f64::from(denom));
        r(f64::from(weight) - f64::from(quotient))
    }

    fn kernel_score(
        doc_count: u64,
        doc_freq: u64,
        sum_ttf: u64,
        freq: u64,
        norm: u8,
        boost: f32,
    ) -> f32 {
        let avg = sum_ttf as f64 / doc_count as f64;
        Bm25TermScorer::with_boost(Bm25Config::default(), doc_count, doc_freq, avg, boost)
            .expect("valid stats")
            .score_f32(freq, u64::from(byte4_to_int(norm)))
    }

    /// `lucene-source-derived`: expected `f32` bit patterns of ES 8.6.1
    /// legacy BM25 for representative inputs, derived from the Java
    /// expressions (BM25Similarity.java:104-111/205-208/236/251,
    /// LegacyBM25Similarity.java:67) through [`oracle_score`]. Not yet
    /// confirmed by an ES 8.6.1 run (see the L7 differential gate spec).
    /// Columns: doc_count, doc_freq, sumTotalTermFreq, freq, norm byte,
    /// boost, expected bits.
    const LUCENE_SOURCE_DERIVED_CASES: [(u64, u64, u64, u64, u8, f32, u32); 8] = [
        // deces-like keyword: norms disabled -> norm byte 1, avgdl 1.
        (1_000_000, 1_234, 1_000_000, 1, 1, 1.0, 0x40D6_4E90),
        // text field, single token doc, rare term.
        (1_360_000, 3, 2_720_000, 1, 1, 1.0, 0x4181_7012),
        // text field, two tokens, common term.
        (1_360_000, 120_000, 2_720_000, 1, 2, 1.0, 0x401B_602B),
        // tf 3, long doc (quantized bucket 1500 -> byte 91).
        (1_000_000, 5_000, 320_000_000, 3, 91, 1.0, 0x4098_B5CD),
        // boosted clause (queries.ts `boost: 4`).
        (10, 2, 1_000, 3, 52, 4.0, 0x411E_84A7),
        // boost 0.5 (queries.ts) and df == docCount.
        (50, 50, 75, 2, 2, 0.5, 0x3BCA_ED95),
        // fuzzy similarity boost 1 - 1/5 = 0.8 (float).
        (2_000, 7, 3_100, 1, 1, 0.8, 0x40A7_4CC2),
        // largest SmallFloat length byte: `1f + freq * normInverse` rounds
        // to `1f`, so Lucene returns exactly `+0.0`.
        (10_000, 9_999, 40_000, 7, 255, 1.0, 0x0000_0000),
    ];

    #[test]
    fn kernel_matches_lucene_source_derived_bits() {
        let mut mismatches = Vec::new();
        for (doc_count, doc_freq, sum_ttf, freq, norm, boost, expected) in
            LUCENE_SOURCE_DERIVED_CASES
        {
            let oracle = oracle_score(doc_count, doc_freq, sum_ttf, freq, norm, boost);
            let kernel = kernel_score(doc_count, doc_freq, sum_ttf, freq, norm, boost);
            assert_eq!(
                kernel.to_bits(),
                oracle.to_bits(),
                "kernel {kernel} vs oracle {oracle} for {doc_count}/{doc_freq}/{sum_ttf}/{freq}/{norm}/{boost}"
            );
            if kernel.to_bits() != expected {
                mismatches.push(format!(
                    "{doc_count}/{doc_freq}/{sum_ttf}/{freq}/{norm}/{boost}: got {:#010X} ({kernel}), pinned {expected:#010X}",
                    kernel.to_bits()
                ));
            }
        }
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[test]
    fn legacy_factor_is_two_point_two_float() {
        // `1 + 1.2f` is exactly the float nearest to 2.2.
        assert_eq!(es_legacy_bm25_boost(1.0, 1.2), 2.2_f32);
        assert_eq!((1.0_f32 + 1.2_f32).to_bits(), 2.2_f32.to_bits());
    }

    #[test]
    fn idf_matches_java_expression() {
        // lucene-source-derived: (float) Math.log(1 + (10 - 2 + 0.5) / (2 + 0.5))
        let idf = lucene_idf(2, 10);
        assert_eq!(idf, (1.0_f64 + 8.5 / 2.5).ln() as f32);
        assert_eq!(bm25_idf(10, 2), Ok(f64::from(idf)));
    }

    #[test]
    fn double_sum_rounds_once() {
        // Three floats whose float-by-float sum differs from the
        // double-accumulated-then-cast sum.
        let a = 1.0_f32;
        let b = f32::EPSILON / 2.0 * 0.75;
        let c = f32::EPSILON / 2.0 * 0.75;
        let float_chain = (a + b) + c;
        let lucene = lucene_double_sum([a, b, c]);
        assert_eq!(float_chain, 1.0);
        assert_eq!(lucene, 1.0 + f32::EPSILON);
    }

    /// Property: the f32 path is deterministic and monotone (the property the
    /// Lucene 9 rewrite of the tf term guarantees and that WAND/MaxScore
    /// upper bounds rely on): non-decreasing in `freq`, non-increasing in the
    /// document length. Pseudo-random inputs from a fixed xorshift seed.
    #[test]
    fn f32_path_is_deterministic_and_monotone() {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            let doc_count = 1 + next() % 5_000_000;
            let doc_freq = 1 + next() % doc_count;
            let sum_ttf = doc_count + next() % (doc_count * 40);
            let freq = 1 + next() % 50;
            let norm = (next() % 256) as u8;
            let boost = [0.5_f32, 1.0, 2.0, 4.0, 0.8, 0.6666667][(next() % 6) as usize];
            let first = kernel_score(doc_count, doc_freq, sum_ttf, freq, norm.max(1), boost);
            let second = kernel_score(doc_count, doc_freq, sum_ttf, freq, norm.max(1), boost);
            assert_eq!(first.to_bits(), second.to_bits());
            assert_eq!(
                first.to_bits(),
                oracle_score(doc_count, doc_freq, sum_ttf, freq, norm.max(1), boost).to_bits()
            );
            let more_freq =
                kernel_score(doc_count, doc_freq, sum_ttf, freq + 1, norm.max(1), boost);
            assert!(more_freq >= first, "freq monotonicity");
            if norm > 1 {
                let shorter = kernel_score(doc_count, doc_freq, sum_ttf, freq, norm - 1, boost);
                assert!(shorter >= first, "length monotonicity");
            }
        }
    }
}
