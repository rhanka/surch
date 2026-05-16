//! Varint + delta encoding for sorted `doc_id` blocks.
//!
//! This module provides the first building block of the future block-128
//! FoR codec described in `docs/poc/perf-optimization-plan.md` ("Phase 2 C").
//! It is a pure, additive utility: no existing call site references it yet
//! — `TermDictionary` integration ships in a later commit on `wp/a-optim`.
//!
//! The format is intentionally minimal:
//!
//! 1. The slice length is written as an unsigned LEB128 varint.
//! 2. Each element is written as the unsigned LEB128 varint of
//!    `sorted[i] - sorted[i-1]` (with `sorted[-1] = 0`).
//!
//! Inputs are required to be **strictly increasing** `u32` doc identifiers,
//! except that the very first element MAY be `0` (delta = 0 is therefore
//! only permitted as the first written delta).

use thiserror::Error;

/// Errors returned by [`decode_doc_ids_delta_varint`] and
/// [`encode_doc_ids_delta_varint`].
///
/// The variants mirror the style of [`crate::codec_util::CodecUtilError`]
/// (Lucene-flavoured `thiserror` enum, `Clone + PartialEq + Eq`) so that
/// upstream error plumbing in `surch-store` / `surch-index` can treat the
/// two consistently.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PostingsBlockError {
    /// Encountered end of input before a varint or the announced number of
    /// deltas could be fully read.
    #[error("postings block: unexpected end of input")]
    UnexpectedEof,
    /// A varint occupied more than the 5 bytes required to hold a `u32`,
    /// or its bits would not fit in `u32` after shifting.
    #[error("postings block: varint overflow (does not fit in u32)")]
    VarintOverflow,
    /// Two consecutive doc ids would not be strictly increasing once
    /// reconstructed: either a `0` delta past the first element, or an
    /// overflow when accumulating.
    #[error("postings block: doc ids are not strictly monotonic")]
    NotMonotonic,
}

/// Encode `sorted` (a strictly increasing slice of `u32` doc ids) as a
/// length-prefixed sequence of delta-varint values.
///
/// Returns [`PostingsBlockError::NotMonotonic`] if `sorted` is not strictly
/// increasing. An empty input encodes to a single byte (`0x00`, the varint
/// for length `0`).
pub fn encode_doc_ids_delta_varint(sorted: &[u32]) -> Result<Vec<u8>, PostingsBlockError> {
    validate_strictly_increasing(sorted)?;

    let mut out = Vec::with_capacity(sorted.len() + 1);
    write_varint_u32(&mut out, sorted.len() as u32);

    let mut previous: u32 = 0;
    for &value in sorted {
        let delta = value - previous; // safe: validate_* above guarantees value >= previous.
        write_varint_u32(&mut out, delta);
        previous = value;
    }
    Ok(out)
}

/// Decode the output of [`encode_doc_ids_delta_varint`] back into the
/// original `Vec<u32>` of strictly increasing doc ids.
pub fn decode_doc_ids_delta_varint(bytes: &[u8]) -> Result<Vec<u32>, PostingsBlockError> {
    let mut position = 0;
    let length = read_varint_u32(bytes, &mut position)? as usize;

    let mut out = Vec::with_capacity(length);
    let mut previous: u32 = 0;
    for i in 0..length {
        let delta = read_varint_u32(bytes, &mut position)?;
        if i > 0 && delta == 0 {
            return Err(PostingsBlockError::NotMonotonic);
        }
        let value = previous
            .checked_add(delta)
            .ok_or(PostingsBlockError::NotMonotonic)?;
        out.push(value);
        previous = value;
    }
    Ok(out)
}

/// Convenience helper used by tests: encode then decode and return the
/// reconstructed vector. Panics on encode/decode errors — callers in
/// production code should use the two functions above directly.
#[doc(hidden)]
pub fn round_trip(sorted: &[u32]) -> Vec<u32> {
    let encoded = encode_doc_ids_delta_varint(sorted).expect("encode failed");
    decode_doc_ids_delta_varint(&encoded).expect("decode failed")
}

fn validate_strictly_increasing(sorted: &[u32]) -> Result<(), PostingsBlockError> {
    for window in sorted.windows(2) {
        if window[0] >= window[1] {
            return Err(PostingsBlockError::NotMonotonic);
        }
    }
    Ok(())
}

/// Write `value` as an unsigned LEB128 varint (1..=5 bytes for any `u32`).
fn write_varint_u32(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push(((value & 0x7f) as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Read an unsigned LEB128 varint. Returns [`PostingsBlockError::UnexpectedEof`]
/// if input is exhausted before the terminating byte, and
/// [`PostingsBlockError::VarintOverflow`] if the encoded value would not fit
/// in a `u32`.
fn read_varint_u32(input: &[u8], position: &mut usize) -> Result<u32, PostingsBlockError> {
    let mut result: u32 = 0;
    let mut shift: u32 = 0;
    loop {
        let byte = *input
            .get(*position)
            .ok_or(PostingsBlockError::UnexpectedEof)?;
        *position += 1;

        let chunk = u32::from(byte & 0x7f);
        // After 4 full 7-bit groups (shift = 28) only the low 4 bits of the
        // 5th byte may be set, otherwise the value would not fit in u32.
        if shift == 28 && byte & 0x80 != 0 {
            return Err(PostingsBlockError::VarintOverflow);
        }
        if shift == 28 && chunk > 0x0f {
            return Err(PostingsBlockError::VarintOverflow);
        }
        if shift >= 32 {
            return Err(PostingsBlockError::VarintOverflow);
        }

        result |= chunk << shift;

        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
    }
}

/// Encode a parallel pair of slices `(doc_ids, freqs)` as a compact
/// `Vec<u8>` payload suitable for cold-tier posting list storage.
///
/// The layout is intentionally simple — it is the building block of
/// the future block-128 FoR codec described in
/// `docs/poc/for-integration-plan.md` (Phase 1):
///
/// 1. The number of postings `n` is written as a varint.
/// 2. `n` delta-varint doc ids follow (same scheme as
///    [`encode_doc_ids_delta_varint`]).
/// 3. `n` raw varint frequencies follow (no delta — frequencies are
///    not monotonic; varint already compresses the typical 1..16 range
///    to a single byte).
///
/// `doc_ids` and `freqs` must have the same length, otherwise
/// [`PostingsBlockError::NotMonotonic`] is returned (the variant name
/// is reused to avoid a breaking enum change — see the integration
/// plan for the dedicated `LengthMismatch` variant scheduled in Phase
/// 2).
pub fn encode_postings_doc_id_freq(
    doc_ids: &[u32],
    freqs: &[u32],
) -> Result<Vec<u8>, PostingsBlockError> {
    if doc_ids.len() != freqs.len() {
        return Err(PostingsBlockError::NotMonotonic);
    }
    validate_strictly_increasing(doc_ids)?;

    // Rough capacity heuristic: 1 byte length header + ~1.5 bytes per
    // delta + ~1 byte per freq. Avoids the first 1–2 grow cycles of
    // `Vec::push` on the BAN 25 k corpus (mean term length ≈ 8 postings).
    let mut out = Vec::with_capacity(1 + doc_ids.len() * 3);
    write_varint_u32(&mut out, doc_ids.len() as u32);

    let mut previous: u32 = 0;
    for &value in doc_ids {
        let delta = value - previous;
        write_varint_u32(&mut out, delta);
        previous = value;
    }
    for &freq in freqs {
        write_varint_u32(&mut out, freq);
    }
    Ok(out)
}

/// Decode the output of [`encode_postings_doc_id_freq`] back into the
/// two parallel `Vec<u32>` slices.
pub fn decode_postings_doc_id_freq(
    bytes: &[u8],
) -> Result<(Vec<u32>, Vec<u32>), PostingsBlockError> {
    let mut position = 0;
    let length = read_varint_u32(bytes, &mut position)? as usize;

    let mut doc_ids = Vec::with_capacity(length);
    let mut previous: u32 = 0;
    for i in 0..length {
        let delta = read_varint_u32(bytes, &mut position)?;
        if i > 0 && delta == 0 {
            return Err(PostingsBlockError::NotMonotonic);
        }
        let value = previous
            .checked_add(delta)
            .ok_or(PostingsBlockError::NotMonotonic)?;
        doc_ids.push(value);
        previous = value;
    }

    let mut freqs = Vec::with_capacity(length);
    for _ in 0..length {
        freqs.push(read_varint_u32(bytes, &mut position)?);
    }

    Ok((doc_ids, freqs))
}

/// Streaming decoder for the doc-id channel of a payload produced by
/// [`encode_postings_doc_id_freq`]. Yields `(doc_id, position_in_bytes)`
/// pairs, **without allocating** a `Vec<u32>`. Used by the future
/// "decode-on-demand" wire-up where the scoring loop walks doc ids one
/// by one and only materialises the matching slice into RAM.
///
/// Phase 1 wire-up plan: callers use `DocIdDeltaCursor::new(encoded)`,
/// then `cursor.next()` returns `Ok(Some(doc_id))` until exhaustion;
/// `Ok(None)` on the natural end, `Err(_)` on a malformed payload.
#[derive(Debug)]
pub struct DocIdDeltaCursor<'a> {
    bytes: &'a [u8],
    position: usize,
    remaining: u32,
    previous: u32,
    /// First delta seen? `0` is only legal for `index == 0`.
    started: bool,
}

impl<'a> DocIdDeltaCursor<'a> {
    /// Build a cursor over `encoded`, reading the length header
    /// eagerly so the caller learns about a truncated payload before
    /// the first `next()` call.
    pub fn new(encoded: &'a [u8]) -> Result<Self, PostingsBlockError> {
        let mut position = 0;
        let remaining = read_varint_u32(encoded, &mut position)?;
        Ok(Self {
            bytes: encoded,
            position,
            remaining,
            previous: 0,
            started: false,
        })
    }

    /// Number of doc ids that have not been yielded yet.
    pub fn remaining(&self) -> u32 {
        self.remaining
    }

    /// Advance one step. Returns `Ok(None)` once the payload is
    /// exhausted, `Err(_)` if the next varint is malformed.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<u32>, PostingsBlockError> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let delta = read_varint_u32(self.bytes, &mut self.position)?;
        if self.started && delta == 0 {
            return Err(PostingsBlockError::NotMonotonic);
        }
        let value = self
            .previous
            .checked_add(delta)
            .ok_or(PostingsBlockError::NotMonotonic)?;
        self.previous = value;
        self.started = true;
        self.remaining -= 1;
        Ok(Some(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let input: [u32; 0] = [];
        let encoded = encode_doc_ids_delta_varint(&input).unwrap();
        // Just the length varint = 0.
        assert_eq!(encoded, vec![0x00]);
        assert_eq!(
            decode_doc_ids_delta_varint(&encoded).unwrap(),
            input.to_vec()
        );
    }

    #[test]
    fn round_trip_single_zero() {
        let input = [0_u32];
        assert_eq!(round_trip(&input), input.to_vec());
    }

    #[test]
    fn round_trip_sequential_small() {
        let input: Vec<u32> = (1..=100).collect();
        assert_eq!(round_trip(&input), input);
    }

    #[test]
    fn round_trip_gapped() {
        let input = [0_u32, 128, 256, 384];
        assert_eq!(round_trip(&input), input.to_vec());
    }

    #[test]
    fn round_trip_extreme_values() {
        let input = [0_u32, u32::MAX / 2, u32::MAX];
        assert_eq!(round_trip(&input), input.to_vec());
    }

    #[test]
    fn round_trip_random_2k_seeded() {
        // Deterministic xorshift32 with a fixed seed so the test is
        // reproducible without pulling in an `rand` dependency.
        let mut state: u32 = 0x1234_5678;
        let mut next = || -> u32 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };

        let mut ids: Vec<u32> = (0..2000).map(|_| next() % 10_000_000).collect();
        ids.sort_unstable();
        ids.dedup();

        let encoded = encode_doc_ids_delta_varint(&ids).unwrap();
        let decoded = decode_doc_ids_delta_varint(&encoded).unwrap();
        assert_eq!(decoded, ids);
    }

    #[test]
    fn encode_rejects_non_monotonic() {
        let err = encode_doc_ids_delta_varint(&[1, 0]).unwrap_err();
        assert_eq!(err, PostingsBlockError::NotMonotonic);
    }

    #[test]
    fn encode_rejects_equal_neighbors() {
        let err = encode_doc_ids_delta_varint(&[3, 3]).unwrap_err();
        assert_eq!(err, PostingsBlockError::NotMonotonic);
    }

    #[test]
    fn decode_rejects_truncated_input() {
        // Announce 3 deltas but only provide 1 (plus the length byte).
        let bytes = [0x03_u8, 0x01];
        let err = decode_doc_ids_delta_varint(&bytes).unwrap_err();
        assert_eq!(err, PostingsBlockError::UnexpectedEof);
    }

    #[test]
    fn decode_rejects_empty_input() {
        // Even the length varint is missing.
        let bytes: [u8; 0] = [];
        let err = decode_doc_ids_delta_varint(&bytes).unwrap_err();
        assert_eq!(err, PostingsBlockError::UnexpectedEof);
    }

    #[test]
    fn decode_rejects_varint_overflow() {
        // Length=1, then a 6-byte varint where every byte has the continuation
        // bit set: this overflows u32 well before terminating.
        let bytes = [0x01_u8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        let err = decode_doc_ids_delta_varint(&bytes).unwrap_err();
        assert_eq!(err, PostingsBlockError::VarintOverflow);
    }

    #[test]
    fn decode_rejects_non_monotonic_zero_delta() {
        // Length=2, first delta=1 (value=1), second delta=0 → not strictly
        // increasing.
        let bytes = [0x02_u8, 0x01, 0x00];
        let err = decode_doc_ids_delta_varint(&bytes).unwrap_err();
        assert_eq!(err, PostingsBlockError::NotMonotonic);
    }

    #[test]
    fn postings_round_trip_empty() {
        let doc_ids: [u32; 0] = [];
        let freqs: [u32; 0] = [];
        let encoded = encode_postings_doc_id_freq(&doc_ids, &freqs).unwrap();
        let (out_ids, out_freqs) = decode_postings_doc_id_freq(&encoded).unwrap();
        assert!(out_ids.is_empty() && out_freqs.is_empty());
    }

    #[test]
    fn postings_round_trip_small() {
        let doc_ids = [1_u32, 3, 7, 9, 42];
        let freqs = [1_u32, 5, 2, 1, 17];
        let encoded = encode_postings_doc_id_freq(&doc_ids, &freqs).unwrap();
        let (out_ids, out_freqs) = decode_postings_doc_id_freq(&encoded).unwrap();
        assert_eq!(out_ids, doc_ids);
        assert_eq!(out_freqs, freqs);
    }

    #[test]
    fn postings_round_trip_2k_seeded_matches_ground_truth() {
        // Same xorshift32 seed as `round_trip_random_2k_seeded` so we
        // share the doc-id corpus with the doc-id-only path.
        let mut state: u32 = 0x1234_5678;
        let mut next = || -> u32 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };

        let mut doc_ids: Vec<u32> = (0..2000).map(|_| next() % 10_000_000).collect();
        doc_ids.sort_unstable();
        doc_ids.dedup();
        // Synthetic freqs in [1..=16] — covers the BAN/INSEE token range.
        let freqs: Vec<u32> = doc_ids.iter().map(|d| (d % 16) + 1).collect();

        let encoded = encode_postings_doc_id_freq(&doc_ids, &freqs).unwrap();
        let (out_ids, out_freqs) = decode_postings_doc_id_freq(&encoded).unwrap();
        assert_eq!(out_ids, doc_ids, "decoded doc_ids must equal ground truth");
        assert_eq!(out_freqs, freqs, "decoded freqs must equal ground truth");
    }

    #[test]
    fn postings_rejects_length_mismatch() {
        let err = encode_postings_doc_id_freq(&[1, 2], &[1]).unwrap_err();
        // Reused variant — see doc comment on `encode_postings_doc_id_freq`.
        assert_eq!(err, PostingsBlockError::NotMonotonic);
    }

    #[test]
    fn cursor_yields_same_sequence_as_decode() {
        let doc_ids = [1_u32, 3, 7, 9, 42, 100, 128, 5000];
        let freqs = [1_u32; 8];
        let encoded = encode_postings_doc_id_freq(&doc_ids, &freqs).unwrap();

        let mut cursor = DocIdDeltaCursor::new(&encoded).unwrap();
        assert_eq!(cursor.remaining(), doc_ids.len() as u32);
        let mut collected = Vec::with_capacity(doc_ids.len());
        while let Some(v) = cursor.next().unwrap() {
            collected.push(v);
        }
        assert_eq!(collected, doc_ids);
        // Subsequent calls keep returning None without erroring.
        assert!(cursor.next().unwrap().is_none());
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn cursor_rejects_truncated_payload() {
        // Announce 3 deltas but only deliver 1.
        let bytes = [0x03_u8, 0x01];
        let mut cursor = DocIdDeltaCursor::new(&bytes).unwrap();
        let _first = cursor.next().unwrap();
        // Second delta read should fail — EOF before continuation.
        let err = cursor.next().unwrap_err();
        assert_eq!(err, PostingsBlockError::UnexpectedEof);
    }

    /// Sanity: the codec payload is meaningfully smaller than the
    /// naive `Vec<u32>` layout for a realistic dense posting list.
    /// This isn't a perf bench (see `benches/for_decode.rs`) — just a
    /// guardrail so regressions in `encode_postings_doc_id_freq` fail
    /// loudly.
    #[test]
    fn postings_compression_ratio_better_than_naive() {
        let doc_ids: Vec<u32> = (0..1024).map(|i| i * 3 + 7).collect();
        let freqs: Vec<u32> = vec![1; 1024];
        let encoded = encode_postings_doc_id_freq(&doc_ids, &freqs).unwrap();
        let naive_bytes = doc_ids.len() * 4 + freqs.len() * 4; // raw u32 + u32
        assert!(
            encoded.len() < naive_bytes / 2,
            "codec={} naive={} — expected at least 2× compression",
            encoded.len(),
            naive_bytes
        );
    }
}
