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
}
