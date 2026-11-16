//! Hybrid roaring-style doc-id set for the high-`df` conjunction path
//! (campaign master plan, lever A1 — the bool/full gap-closer).
//!
//! Two common names (`MARIE ∩ MARTIN`) intersect via a scalar leapfrog walk
//! whose cost is `O(df_rare)` — the measured deces bool/full tail (~2× ES,
//! the only axis Surch trails). Lucene/ES instead AND word-parallel bitmaps
//! (RoaringDocIdSet / BitSetConjunction), the source of their ~2× edge.
//!
//! This is a from-scratch, dependency-free, **SSE2-baseline-safe** equivalent:
//! the hot intersection is `u64 & u64` (64 doc-ids per instruction — a later
//! pass can lift it to `_mm_and_si128`, 128 bits, on the SSE2 baseline; the
//! scalar word AND already captures the bulk of the win). Built ONLY for
//! `df > threshold` terms; low-`df` terms keep the galloping leapfrog.
//!
//! Parity: `intersect_for_each` emits exactly `set(a) ∩ set(b)` in ascending
//! `doc_id` order — bit-identical to a naive sorted intersection (unit-tested
//! against a `BTreeSet` oracle over multi-chunk, mixed-container inputs).

/// doc_ids are split on their high 16 bits into chunks of `2^16`.
const CHUNK_SHIFT: u32 = 16;
const CHUNK_SIZE: u32 = 1 << CHUNK_SHIFT; // 65_536
const WORDS_PER_CHUNK: usize = (CHUNK_SIZE as usize) / 64; // 1_024 (8 KiB)

/// A chunk holding more than this many doc-ids is stored as a dense bitmap
/// (8 KiB flat); below it, a sorted `Vec<u16>` of the low bits is cheaper and
/// keeps rare-term RAM bounded. ~6 % density of 65_536 — the roaring crossover.
const DENSE_THRESHOLD: usize = 4_096;

/// Per-chunk container: dense bitmap for high-density chunks, sorted low-bit
/// array otherwise.
#[derive(Debug, Clone)]
enum Container {
    /// Sorted ascending low-16-bit values.
    Array(Vec<u16>),
    /// One bit per possible low-16 value; bit `i` set ⇔ the chunk holds `i`.
    Bitmap(Box<[u64; WORDS_PER_CHUNK]>),
}

impl Container {
    #[inline]
    fn contains(&self, low: u16) -> bool {
        match self {
            Container::Array(a) => a.binary_search(&low).is_ok(),
            Container::Bitmap(b) => {
                let idx = (low as usize) >> 6;
                let bit = (low as u64) & 63;
                (b[idx] >> bit) & 1 == 1
            }
        }
    }
}

/// A high-`df` term's doc-ids as chunked containers, sorted by chunk key.
#[derive(Debug, Clone)]
pub struct RoaringDocSet {
    /// `(chunk_key /* high 16 bits */, container)`, ascending by `chunk_key`.
    chunks: Vec<(u16, Container)>,
    len: usize,
}

impl RoaringDocSet {
    /// Build from an ascending, de-duplicated `doc_ids` slice (the layout of a
    /// term's posting list / doc-id channel). Panics in debug if not sorted.
    pub fn from_sorted(doc_ids: &[u32]) -> Self {
        debug_assert!(
            doc_ids.windows(2).all(|w| w[0] < w[1]),
            "RoaringDocSet::from_sorted requires strictly ascending doc_ids"
        );
        let mut chunks: Vec<(u16, Container)> = Vec::new();
        let mut i = 0usize;
        while i < doc_ids.len() {
            let key = (doc_ids[i] >> CHUNK_SHIFT) as u16;
            // Gather this chunk's run (doc_ids share the same high 16 bits).
            let start = i;
            while i < doc_ids.len() && (doc_ids[i] >> CHUNK_SHIFT) as u16 == key {
                i += 1;
            }
            let run = &doc_ids[start..i];
            let container = if run.len() > DENSE_THRESHOLD {
                let mut words = Box::new([0u64; WORDS_PER_CHUNK]);
                for &d in run {
                    let low = (d & (CHUNK_SIZE - 1)) as usize;
                    words[low >> 6] |= 1u64 << (low & 63);
                }
                Container::Bitmap(words)
            } else {
                Container::Array(run.iter().map(|&d| (d & (CHUNK_SIZE - 1)) as u16).collect())
            };
            chunks.push((key, container));
        }
        Self {
            chunks,
            len: doc_ids.len(),
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Approximate resident bytes held by this roaring set (memory accounting
    /// #17): the chunk vector's capacity overhead plus, per chunk, either the
    /// dense 8 KiB bitmap or the `Vec<u16>` capacity.
    pub fn memory_bytes(&self) -> usize {
        let mut bytes = self.chunks.capacity() * std::mem::size_of::<(u16, Container)>();
        for (_, c) in &self.chunks {
            bytes += match c {
                Container::Bitmap(_) => WORDS_PER_CHUNK * 8,
                Container::Array(a) => a.capacity() * std::mem::size_of::<u16>(),
            };
        }
        bytes
    }

    /// MVP L6 persistence (`docs/ops/persistence.md`): exact container-level
    /// encoding — `len u64 | chunk count u32 | per chunk: key u16, tag u8
    /// (0 = array, 1 = bitmap), then either `count u32` + `count` low-16
    /// values or the 1 024 bitmap words`, all little-endian. Decoding
    /// rebuilds the identical structure without re-deriving it from doc-ids.
    pub(crate) fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.len as u64).to_le_bytes());
        out.extend_from_slice(&(self.chunks.len() as u32).to_le_bytes());
        for (key, container) in &self.chunks {
            out.extend_from_slice(&key.to_le_bytes());
            match container {
                Container::Array(values) => {
                    out.push(0);
                    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
                    for value in values {
                        out.extend_from_slice(&value.to_le_bytes());
                    }
                }
                Container::Bitmap(words) => {
                    out.push(1);
                    for word in words.iter() {
                        out.extend_from_slice(&word.to_le_bytes());
                    }
                }
            }
        }
    }

    /// Inverse of [`Self::encode_into`]. `None` on any structural
    /// inconsistency (truncation, unsorted keys or values, length mismatch,
    /// trailing bytes) — the caller treats it as a corrupt file.
    pub(crate) fn decode_from(bytes: &[u8]) -> Option<Self> {
        fn take<'a>(bytes: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
            if bytes.len() < n {
                return None;
            }
            let (head, tail) = bytes.split_at(n);
            *bytes = tail;
            Some(head)
        }
        let mut input = bytes;
        let len = u64::from_le_bytes(take(&mut input, 8)?.try_into().ok()?);
        let chunk_count = u32::from_le_bytes(take(&mut input, 4)?.try_into().ok()?) as usize;
        if chunk_count > input.len() {
            return None;
        }
        let mut chunks = Vec::with_capacity(chunk_count);
        let mut total = 0u64;
        let mut previous_key: Option<u16> = None;
        for _ in 0..chunk_count {
            let key = u16::from_le_bytes(take(&mut input, 2)?.try_into().ok()?);
            if previous_key.is_some_and(|previous| previous >= key) {
                return None;
            }
            previous_key = Some(key);
            let container = match take(&mut input, 1)?[0] {
                0 => {
                    let count = u32::from_le_bytes(take(&mut input, 4)?.try_into().ok()?) as usize;
                    let raw = take(&mut input, count.checked_mul(2)?)?;
                    let values: Vec<u16> = raw
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|chunk| u16::from_le_bytes(*chunk))
                        .collect();
                    if count == 0 || values.windows(2).any(|pair| pair[0] >= pair[1]) {
                        return None;
                    }
                    total += count as u64;
                    Container::Array(values)
                }
                1 => {
                    let raw = take(&mut input, WORDS_PER_CHUNK * 8)?;
                    let mut words = Box::new([0u64; WORDS_PER_CHUNK]);
                    for (word, chunk) in words.iter_mut().zip(raw.as_chunks::<8>().0) {
                        *word = u64::from_le_bytes(*chunk);
                        total += u64::from(word.count_ones());
                    }
                    Container::Bitmap(words)
                }
                _ => return None,
            };
            chunks.push((key, container));
        }
        if !input.is_empty() || total != len {
            return None;
        }
        Some(Self {
            chunks,
            len: usize::try_from(len).ok()?,
        })
    }

    /// Call `f(doc_id)` for every doc-id in `self ∩ other`, ascending. The
    /// word-parallel AND of two dense chunks is the hot path (the gap-closer);
    /// mixed/sparse chunks fall back to membership tests / a sorted merge.
    pub fn intersect_for_each(&self, other: &RoaringDocSet, mut f: impl FnMut(u32)) {
        let (mut i, mut j) = (0usize, 0usize);
        while i < self.chunks.len() && j < other.chunks.len() {
            let (ka, ca) = (&self.chunks[i].0, &self.chunks[i].1);
            let (kb, cb) = (&other.chunks[j].0, &other.chunks[j].1);
            match ka.cmp(kb) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    let base = (*ka as u32) << CHUNK_SHIFT;
                    intersect_containers(ca, cb, |low| f(base | low as u32));
                    i += 1;
                    j += 1;
                }
            }
        }
    }
}

/// Intersect two containers of the SAME chunk, emitting matched low-16 values
/// ascending.
#[inline]
fn intersect_containers(a: &Container, b: &Container, mut f: impl FnMut(u16)) {
    match (a, b) {
        // Hot path: word-parallel AND (64 doc-ids per `&`), set bits ascending.
        (Container::Bitmap(wa), Container::Bitmap(wb)) => {
            for w in 0..WORDS_PER_CHUNK {
                let mut and = wa[w] & wb[w];
                let base = (w as u32) << 6;
                while and != 0 {
                    let bit = and.trailing_zeros();
                    f((base + bit) as u16);
                    and &= and - 1; // clear lowest set bit
                }
            }
        }
        // Drive the array, probe the bitmap (array already ascending).
        (Container::Array(arr), Container::Bitmap(_)) => {
            for &low in arr {
                if b.contains(low) {
                    f(low);
                }
            }
        }
        (Container::Bitmap(_), Container::Array(arr)) => {
            for &low in arr {
                if a.contains(low) {
                    f(low);
                }
            }
        }
        // Sorted two-pointer merge.
        (Container::Array(xa), Container::Array(xb)) => {
            let (mut p, mut q) = (0usize, 0usize);
            while p < xa.len() && q < xb.len() {
                match xa[p].cmp(&xb[q]) {
                    std::cmp::Ordering::Less => p += 1,
                    std::cmp::Ordering::Greater => q += 1,
                    std::cmp::Ordering::Equal => {
                        f(xa[p]);
                        p += 1;
                        q += 1;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn naive_intersection(a: &[u32], b: &[u32]) -> Vec<u32> {
        let sa: BTreeSet<u32> = a.iter().copied().collect();
        let sb: BTreeSet<u32> = b.iter().copied().collect();
        sa.intersection(&sb).copied().collect()
    }

    fn roaring_intersection(a: &[u32], b: &[u32]) -> Vec<u32> {
        let ra = RoaringDocSet::from_sorted(a);
        let rb = RoaringDocSet::from_sorted(b);
        let mut out = Vec::new();
        ra.intersect_for_each(&rb, |d| out.push(d));
        out
    }

    /// MVP L6: the persisted container encoding round-trips exactly and
    /// rejects truncation / trailing bytes.
    #[test]
    fn persisted_encoding_round_trips_dense_and_sparse_chunks() {
        let mut ids: Vec<u32> = (0..70_000).step_by(2).collect();
        ids.extend((200_000..200_050).step_by(3));
        let set = RoaringDocSet::from_sorted(&ids);
        let mut bytes = Vec::new();
        set.encode_into(&mut bytes);
        let decoded = RoaringDocSet::decode_from(&bytes).expect("round trip");
        let mut a = Vec::new();
        set.intersect_for_each(&set, |d| a.push(d));
        let mut b = Vec::new();
        decoded.intersect_for_each(&decoded, |d| b.push(d));
        assert_eq!(a, ids);
        assert_eq!(b, ids);
        assert_eq!(decoded.len(), set.len());
        assert!(RoaringDocSet::decode_from(&bytes[..bytes.len() - 1]).is_none());
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(RoaringDocSet::decode_from(&trailing).is_none());
    }

    /// Parity oracle: roaring ∩ == naive set ∩, across multi-chunk inputs that
    /// exercise EVERY container-pair case (dense∩dense, dense∩sparse,
    /// sparse∩sparse) and chunk-key misalignment.
    #[test]
    fn roaring_intersection_matches_naive_oracle() {
        // Chunk 0: both dense (> DENSE_THRESHOLD) and overlapping.
        // Chunk 1: only `a` present (no overlap -> emits nothing).
        // Chunk 2: both sparse arrays, partial overlap.
        // Chunk 3: a dense, b sparse (mixed) -> drives the probe path.
        let mut a: Vec<u32> = Vec::new();
        let mut b: Vec<u32> = Vec::new();

        // chunk 0 dense: a = multiples of 2, b = multiples of 3 (both > 4096 entries)
        for d in 0..CHUNK_SIZE {
            if d.is_multiple_of(2) {
                a.push(d);
            }
            if d.is_multiple_of(3) {
                b.push(d);
            }
        }
        // chunk 1: only a, a few sparse entries
        for d in [10u32, 200, 65000] {
            a.push((1 << CHUNK_SHIFT) | d);
        }
        // chunk 2: both sparse, partial overlap {5,7} shared
        for d in [1u32, 3, 5, 7, 9] {
            a.push((2 << CHUNK_SHIFT) | d);
        }
        for d in [2u32, 5, 7, 11] {
            b.push((2 << CHUNK_SHIFT) | d);
        }
        // chunk 3: a dense, b sparse (mixed-container path)
        for d in 0..CHUNK_SIZE {
            if d % 2 == 1 {
                a.push((3 << CHUNK_SHIFT) | d);
            }
        }
        for d in [4u32, 5, 6, 7, 65535] {
            b.push((3 << CHUNK_SHIFT) | d);
        }
        a.sort_unstable();
        a.dedup();
        b.sort_unstable();
        b.dedup();

        assert_eq!(
            roaring_intersection(&a, &b),
            naive_intersection(&a, &b),
            "roaring intersection must be bit-identical to the naive oracle"
        );
    }

    #[test]
    fn roaring_intersection_edge_cases() {
        // Empty inputs, disjoint, identical, single element.
        assert_eq!(roaring_intersection(&[], &[1, 2, 3]), Vec::<u32>::new());
        assert_eq!(roaring_intersection(&[1, 2, 3], &[]), Vec::<u32>::new());
        assert_eq!(
            roaring_intersection(&[1, 2, 3], &[4, 5, 6]),
            Vec::<u32>::new()
        );
        assert_eq!(roaring_intersection(&[1, 2, 3], &[1, 2, 3]), vec![1, 2, 3]);
        assert_eq!(roaring_intersection(&[7], &[7]), vec![7]);
        // Cross-chunk boundary doc_ids.
        let a = [CHUNK_SIZE - 1, CHUNK_SIZE, CHUNK_SIZE + 1];
        let b = [CHUNK_SIZE - 1, CHUNK_SIZE + 1];
        assert_eq!(
            roaring_intersection(&a, &b),
            vec![CHUNK_SIZE - 1, CHUNK_SIZE + 1]
        );
    }
}
