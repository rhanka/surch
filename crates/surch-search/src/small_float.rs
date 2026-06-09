//! Lucene-compatible 1-byte length quantization (`SmallFloat`).
//!
//! Bit-for-bit port of `org.apache.lucene.util.SmallFloat.intToByte4` /
//! `byte4ToInt` (the encoding `BM25Similarity` uses to store per-document
//! field lengths as 1 byte/doc). The encoding has two pieces:
//!
//! * a "free" passthrough range `[0..NUM_FREE_VALUES)` (24 values) where the
//!   byte equals the input;
//! * a logarithmic 4-bit mantissa + exponent encoding for any value `>=
//!   NUM_FREE_VALUES`. Decoding is lossy by design: each byte represents a
//!   bucket of input lengths.
//!
//! This module is the canonical reference for the Surch codebase. A
//! private mirror lives in `surch-index::document_index` (used by the
//! indexer, which cannot depend on `surch-search`); a CI parity test in
//! `crates/surch-index/tests` asserts the two implementations are
//! byte-identical for the full `0..=u16::MAX as u32` domain.
//!
//! ## Why parity matters
//!
//! See `docs/paper/ndcg-trec-covid-rootcause-22.md`. The TREC-COVID
//! NDCG@10 gap (Surch 0.4750 vs OS 0.4902, −0.0152) is fully explained
//! by Surch scoring with exact `doc_len` while Lucene scores with the
//! quantized bucket. Adopting the same quantization closes the gap and
//! incidentally drops `field_stats_bytes` from 8 B/doc to 1 B/doc
//! (~65 MiB saved on the deces 1.36 M × ~6 indexed fields corpus).

/// Lucene's `NUM_FREE_VALUES = 255 - MAX_INT4`, where
/// `MAX_INT4 = longToInt4(Integer.MAX_VALUE) = 231`. The first 24
/// non-negative integers encode to themselves (round-trip lossless).
pub const NUM_FREE_VALUES: u32 = 24;

/// Lucene `SmallFloat.longToInt4` — internal 4-bit encoder (3-bit
/// mantissa + 5-bit exponent) used by [`int_to_byte4`] above the free
/// range. Public so test harnesses can assert intermediate parity.
#[inline]
pub fn long_to_int4(value: u64) -> u32 {
    // 64 - Long.numberOfLeadingZeros(value)
    let num_bits = 64 - value.leading_zeros();
    if num_bits < 4 {
        // Fits in the mantissa, no exponent. Java's
        // `Math.toIntExact(i)` cannot overflow here because num_bits<4
        // ⇒ value < 16.
        return value as u32;
    }
    let shift = num_bits - 4;
    let mantissa = (value >> shift) as u32 & 0x07;
    let exponent = shift + 1;
    mantissa | (exponent << 3)
}

/// Lucene `SmallFloat.int4ToLong` — inverse of [`long_to_int4`].
#[inline]
pub fn int4_to_long(encoded: u32) -> u64 {
    let bits = (encoded & 0x07) as u64;
    // Lucene's `shift = (i >>> 3) - 1`. When the high 5 bits are zero
    // (encoded < 8), `shift` becomes -1 and Lucene returns just the
    // mantissa. We model that with `Option<u32>` of the actual shift.
    let exp = encoded >> 3;
    if exp == 0 {
        bits
    } else {
        (bits | 0x08) << (exp - 1)
    }
}

/// Lucene `SmallFloat.intToByte4`. Quantizes a non-negative `u32`
/// length to 1 byte; the lower 24 values pass through unchanged and
/// larger values go through the log-mantissa encoder.
#[inline]
pub fn int_to_byte4(value: u32) -> u8 {
    if value < NUM_FREE_VALUES {
        return value as u8;
    }
    let offset = (value - NUM_FREE_VALUES) as u64;
    let encoded = long_to_int4(offset);
    (NUM_FREE_VALUES + encoded) as u8
}

/// Lucene `SmallFloat.byte4ToInt`. Inverse of [`int_to_byte4`] (lossy
/// for `value >= NUM_FREE_VALUES`).
#[inline]
pub fn byte4_to_int(byte: u8) -> u32 {
    let i = byte as u32;
    if i < NUM_FREE_VALUES {
        return i;
    }
    let decoded = NUM_FREE_VALUES as u64 + int4_to_long(i - NUM_FREE_VALUES);
    // Saturate to `u32::MAX` instead of Java's `Math.toIntExact` panic:
    // a deliberately-tolerant API for ports of legacy norms. In
    // practice the encoder side never produces a byte whose decoded
    // value overflows `u32` (the input was a `u32` already).
    decoded.min(u32::MAX as u64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lucene's first 24 values are reserved as identity passthrough.
    #[test]
    fn free_range_is_identity() {
        for raw in 0..NUM_FREE_VALUES {
            let byte = int_to_byte4(raw);
            assert_eq!(byte as u32, raw, "free range encoder identity at {raw}");
            assert_eq!(
                byte4_to_int(byte),
                raw,
                "free range decoder identity at {raw}"
            );
        }
    }

    /// Canonical Lucene reference vectors (hand-computed from the
    /// Java source). Locking these down guarantees bit-identity.
    #[test]
    fn critical_values_match_lucene_reference() {
        // (raw, expected_byte, expected_reconstructed)
        let cases = [
            (0u32, 0u8, 0u32),
            (1, 1, 1),
            (23, 23, 23),
            (24, 24, 24),    // boundary: first non-free, encoder forwards 0.
            (25, 25, 25),    // 24 + longToInt4(1) = 24 + 1
            (31, 31, 31),    // 24 + longToInt4(7) = 24 + 7
            (32, 32, 32),    // 24 + longToInt4(8) = 24 + 8
            (255, 70, 248),  // hand-traced in docs/paper #22 root-cause.
            (1000, 87, 984),
            (1500, 91, 1432),
            (65535, 135, 61464),
        ];
        for (raw, want_byte, want_reconstructed) in cases {
            let byte = int_to_byte4(raw);
            assert_eq!(byte, want_byte, "encoder({raw})");
            assert_eq!(
                byte4_to_int(byte),
                want_reconstructed,
                "decoder(encoder({raw}))"
            );
        }
    }

    /// `byte4ToInt(intToByte4(x))` is monotone non-decreasing in `x`
    /// and never exceeds `x` (the bucket is rounded down).
    #[test]
    fn round_trip_monotone_and_lossy_below_input() {
        let mut last = 0u32;
        for x in 0..=10_000u32 {
            let round = byte4_to_int(int_to_byte4(x));
            assert!(round <= x, "round({x}) = {round} exceeds input");
            assert!(round >= last, "round({x}) = {round} < previous {last}");
            last = round;
        }
    }

    /// Every byte decodes to a strictly monotone non-decreasing `u32`
    /// (so the dense `doc_len_dense` slice, sorted by byte, would also
    /// be sorted by reconstructed length — the property the WAND
    /// `min_doc_len` optimisation relies on).
    #[test]
    fn decoder_is_monotone_over_full_byte_domain() {
        let mut previous = 0u32;
        for b in 0u32..=255u32 {
            let decoded = byte4_to_int(b as u8);
            assert!(
                decoded >= previous,
                "byte {b} decodes to {decoded} < previous {previous}"
            );
            previous = decoded;
        }
    }

    /// Bit-identity sanity: two inputs in the same Lucene bucket must
    /// decode to the same `doc_len`, otherwise the scorer would
    /// disagree with Lucene on tie-breaking. We use the documented
    /// 1500 bucket from the rootcause doc.
    #[test]
    fn same_bucket_decodes_identical() {
        // 1500 → byte 91 → 1432. 1499 and 1501 share the same byte.
        let b = int_to_byte4(1500);
        // Probe a small neighbourhood; any input mapping to byte 91
        // must round-trip to 1432.
        for delta in 0u32..=64 {
            let lo = 1500u32.saturating_sub(delta);
            let hi = 1500u32 + delta;
            if int_to_byte4(lo) == b {
                assert_eq!(byte4_to_int(int_to_byte4(lo)), 1432);
            }
            if int_to_byte4(hi) == b {
                assert_eq!(byte4_to_int(int_to_byte4(hi)), 1432);
            }
        }
    }

    /// BM25 parity sanity: a Surch score computed against the
    /// Lucene-quantized `doc_len` must equal the score Lucene itself
    /// would compute (modulo float epsilon) when fed the same raw
    /// length. The test feeds raw doc_len=1500 to the encoder and
    /// applies the BM25 kernel from `scoring.rs` on the reconstructed
    /// length (1432), then compares to the manual reference formula.
    #[test]
    fn bm25_score_matches_lucene_quantized_reference() {
        use crate::scoring::{Bm25Config, Bm25TermScorer};

        let config = Bm25Config::default();
        let doc_count = 1_000_000u64;
        let doc_freq = 5_000u64;
        let avg_doc_len = 320.0f64;
        let raw_doc_len = 1500u64;
        let term_freq = 3u64;

        let quantized = byte4_to_int(int_to_byte4(raw_doc_len as u32)) as u64;
        assert_eq!(quantized, 1432, "Lucene bucket reconstruction");

        let scorer = Bm25TermScorer::new(config, doc_count, doc_freq, avg_doc_len)
            .expect("scorer constructs");
        let surch_score = scorer.score(term_freq, quantized);

        // Reference: BM25 kernel applied by hand with the same kernel
        // we ship (left-associated to stay bit-identical).
        let k1 = config.k1;
        let b = config.b;
        let freq = term_freq as f64;
        let len_norm = quantized as f64 / avg_doc_len;
        let denom = freq + k1 * (1.0 - b + b * len_norm);
        let tf_norm = freq * (k1 + 1.0) / denom;
        let idf = (1.0
            + (doc_count as f64 - doc_freq as f64 + 0.5) / (doc_freq as f64 + 0.5))
            .ln();
        let reference = idf * tf_norm;

        assert!(
            (surch_score - reference).abs() < 1e-12,
            "Surch {surch_score} vs Lucene-quantized reference {reference}"
        );
    }
}
