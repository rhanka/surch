//! Block-Max WAND / MaxScore OR-match top-K with skip-list block
//! leapfrogging (Track A, Lot 3).
//!
//! Lot 1 shipped per-128 Block-Max WAND contribution skipping inside the
//! OR-match top-K loop: once the running top-K threshold exceeds a
//! token's per-block upper-bound contribution, a block that contains no
//! already-scored doc contributes nothing and is `continue`-d over. But
//! that loop still *iterates* every block linearly — on a long posting
//! list (TREC-COVID: thousands of 128-blocks per common term) the skip
//! decision is paid once per block even when hundreds of consecutive
//! blocks are all skippable.
//!
//! Lot 3 keeps the exact same skip *decision* but replaces the linear
//! block walk with the Lot 2 codec [`BlockSkipList`] cursor
//! (`advance_to`). When the current block is skippable
//! (`!allow_new_docs && block_max < threshold` and no already-scored doc
//! falls in the block's `[min_doc_id, max_doc_id]` range), we ask the
//! BTreeMap of already-scored docs for the *next* scored doc id past this
//! block, then leapfrog the cursor straight to the block that contains
//! it — jumping over every intervening block in `O(log N_blocks)` instead
//! of touching each one. If no scored doc remains past the current block,
//! the rest of the token's posting list cannot change top-K at all and the
//! token loop terminates early.
//!
//! The ranking output is **byte-for-byte identical** to the linear-walk
//! Lot 1 path: skipping a block is only ever done when that block
//! provably cannot enter or improve a top-K candidate, so the set of
//! `(doc_id, score)` pairs accumulated is unchanged. The skip is a pure
//! performance optimisation. A perf counter ([`MaxScoreOutcome::blocks_skipped`])
//! exposes how many 128-blocks the cursor leapfrogged so tests can assert
//! the skip list is doing work.

use std::collections::BTreeMap;
use surch_codec::postings_block::{BlockSkipEntry, BlockSkipList, PostingsBlockError};

/// Number of postings per logical block. Must match
/// `surch_index::postings::BLOCK_SIZE` and the codec `FOR_BLOCK_SIZE`.
pub const BLOCK_SIZE: usize = surch_codec::postings_block::FOR_BLOCK_SIZE;

/// One query token's contribution to the OR-match, expressed as the data
/// the MaxScore loop needs: its postings (already sorted by ascending
/// `doc_id`), the per-block upper-bound contribution
/// (`block_max_contribs[i]` bounds block `i = doc_ids[i*128 ..
/// (i+1)*128]`), and the token-level max contribution used to order
/// tokens and gate `allow_new_docs`.
///
/// `doc_ids` / `freqs` borrow the live, index-aligned SoA slices (Lot C
/// Phase 1 levier 5: `surch_index::postings::PostingsList::doc_ids` /
/// `::freqs`, sorted by ascending `doc_id`) straight from the in-memory
/// term dictionary — the OR-match loop reads `doc_id` + `freq` directly,
/// so the search path no longer deep-copies the posting list into an
/// owned `Vec<(u32, u64)>` per query (optimisation #7). The per-doc score
/// is computed by the caller-supplied closure passed to
/// [`MaxScoreExecutor::run`], which widens `freq` (`u32`) to the `u64`
/// term-frequency the BM25 scorer expects, so this module stays free of
/// BM25 / norms specifics and is reusable across single-field `Match` and
/// per-field `multi_match`.
#[derive(Debug, Clone)]
pub struct MaxScoreToken<'a> {
    /// `doc_id` channel, sorted by ascending `doc_id`.
    pub doc_ids: &'a [u32],
    /// `freq` channel, index-aligned with `doc_ids` (posting `i` is
    /// `(doc_ids[i], freqs[i])`).
    pub freqs: &'a [u32],
    /// Per-block contribution upper bound, aligned with
    /// `doc_ids.chunks(BLOCK_SIZE)`. `block_max_contribs.len()` must be
    /// `doc_ids.len().div_ceil(BLOCK_SIZE)`.
    pub block_max_contribs: &'a [f64],
    /// Token-level max contribution (the max over all blocks, including
    /// the repeat boost). Used to sort tokens descending and to decide
    /// whether new docs may still enter top-K for this token.
    pub max_contrib: f64,
}

/// Errors raised while running the MaxScore loop. Only fires on a
/// programmer-bug-level invariant violation (block metadata misaligned
/// with postings, or non-monotonic blocks), surfaced as a typed error so
/// the skip path never panics.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MaxScoreError {
    /// `block_max_contribs.len()` did not equal
    /// `postings.len().div_ceil(BLOCK_SIZE)` for some token.
    #[error("token {token}: block_max_contribs len {got} != expected {expected}")]
    BlockMetaMisaligned {
        token: usize,
        got: usize,
        expected: usize,
    },
    /// The codec [`BlockSkipList`] builder rejected the per-block ranges
    /// (postings not strictly increasing across blocks).
    #[error("block skip list invariant violated: {0}")]
    BlockSkip(String),
}

/// Result of a MaxScore OR-match run: the accumulated `(doc_id, score)`
/// map plus the perf counter that proves the skip list leapfrogged whole
/// blocks rather than walking them one by one.
#[derive(Debug, Clone, PartialEq)]
pub struct MaxScoreOutcome {
    /// Final scored docs, keyed by doc id (ascending). Identical to the
    /// linear-walk Lot 1 path for the same inputs.
    pub scored: BTreeMap<u32, f64>,
    /// Number of 128-blocks the skip-list cursor leapfrogged over,
    /// summed across tokens. Zero when no token triggered a block skip
    /// (e.g. a single token, or a threshold that never gates anything).
    pub blocks_skipped: usize,
}

/// MaxScore OR-match executor with skip-list block leapfrogging.
///
/// `limit` is the top-K size; the running threshold is the K-th best
/// score seen so far (or `-inf` until `limit` docs are scored).
#[derive(Debug)]
pub struct MaxScoreExecutor {
    limit: usize,
}

impl MaxScoreExecutor {
    /// Build an executor for a top-`limit` OR-match.
    pub fn new(limit: usize) -> Self {
        Self { limit }
    }

    /// Run the MaxScore loop over `tokens` (which the caller has NOT
    /// necessarily sorted — this method sorts a working copy by
    /// descending `max_contrib`). `score_doc(token_idx, doc_id,
    /// term_freq)` returns the per-doc contribution for that token, or
    /// `None` to skip the doc (e.g. missing/zero doc length). The closure
    /// must be deterministic and side-effect-free; it is called at most
    /// once per `(token, doc)` pair that survives the skip gate.
    ///
    /// Returns the accumulated scores and the blocks-skipped counter.
    pub fn run<F>(
        &self,
        tokens: &[MaxScoreToken<'_>],
        mut score_doc: F,
    ) -> Result<MaxScoreOutcome, MaxScoreError>
    where
        F: FnMut(usize, u32, u64) -> Option<f64>,
    {
        // Validate block-metadata alignment up front, and pre-build the
        // skip list per token so the inner loop only leapfrogs.
        let mut ordered: Vec<(usize, &MaxScoreToken<'_>)> = tokens.iter().enumerate().collect();
        let mut skip_lists: Vec<Option<BlockSkipList>> = Vec::with_capacity(tokens.len());
        for (token_idx, token) in tokens.iter().enumerate() {
            let expected = token.doc_ids.len().div_ceil(BLOCK_SIZE);
            if token.block_max_contribs.len() != expected {
                return Err(MaxScoreError::BlockMetaMisaligned {
                    token: token_idx,
                    got: token.block_max_contribs.len(),
                    expected,
                });
            }
            skip_lists.push(
                build_skip_list(token.doc_ids)
                    .map_err(|err| MaxScoreError::BlockSkip(err.to_string()))?,
            );
        }

        // Tokens drive in descending max_contrib order: the most
        // selective term first so the threshold rises early and gates
        // the cheaper tokens hard.
        ordered.sort_by(|a, b| {
            b.1.max_contrib
                .partial_cmp(&a.1.max_contrib)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut scored: BTreeMap<u32, f64> = BTreeMap::new();
        let mut threshold = f64::NEG_INFINITY;
        let mut blocks_skipped = 0usize;

        for (token_idx, token) in ordered {
            let allow_new_docs = token.max_contrib >= threshold;
            let skip_list = skip_lists[token_idx].as_ref();

            blocks_skipped += self.run_token(
                token_idx,
                token,
                skip_list,
                allow_new_docs,
                threshold,
                &mut scored,
                &mut score_doc,
            )?;

            // Refresh the threshold to the K-th best score so the next
            // (cheaper) token is gated against the tightest bound.
            if scored.len() >= self.limit && self.limit > 0 {
                let mut values: Vec<f64> = scored.values().copied().collect();
                let k = values.len() - self.limit;
                values.select_nth_unstable_by(k, |a, b| {
                    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                });
                threshold = values[k];
            }
        }

        Ok(MaxScoreOutcome {
            scored,
            blocks_skipped,
        })
    }

    /// Walk a single token's postings, leapfrogging skippable blocks via
    /// the skip-list cursor. Returns the number of blocks the cursor
    /// jumped over.
    #[allow(clippy::too_many_arguments)]
    fn run_token<F>(
        &self,
        token_idx: usize,
        token: &MaxScoreToken<'_>,
        skip_list: Option<&BlockSkipList>,
        allow_new_docs: bool,
        threshold: f64,
        scored: &mut BTreeMap<u32, f64>,
        score_doc: &mut F,
    ) -> Result<usize, MaxScoreError>
    where
        F: FnMut(usize, u32, u64) -> Option<f64>,
    {
        let doc_ids = token.doc_ids;
        let freqs = token.freqs;
        let num_blocks = token.block_max_contribs.len();
        let mut blocks_skipped = 0usize;
        let mut block_idx = 0usize;

        while block_idx < num_blocks {
            let block_start = block_idx * BLOCK_SIZE;
            let block_end = ((block_idx + 1) * BLOCK_SIZE).min(doc_ids.len());
            let block_doc_ids = &doc_ids[block_start..block_end];
            let block_freqs = &freqs[block_start..block_end];
            if block_doc_ids.is_empty() {
                block_idx += 1;
                continue;
            }
            let block_max = token.block_max_contribs[block_idx];
            let block_first = block_doc_ids[0];
            let block_last = block_doc_ids[block_doc_ids.len() - 1];

            // Block-level skip decision — identical to the Lot 1 linear
            // path: if new docs are gated off AND this block's tightest
            // upper bound is below the running threshold AND no
            // already-scored doc falls inside the block's doc-id range,
            // the block contributes nothing.
            let skippable = !allow_new_docs
                && block_max < threshold
                && scored.range(block_first..=block_last).next().is_none();

            if skippable {
                // Lot 3 leapfrog: the next doc that *could* still change
                // top-K (a doc already scored from a rarer token) is the
                // first scored doc id strictly past this block's
                // max_doc_id. If none exists, no later block in this
                // token can contribute either — stop the token.
                let Some((&next_scored, _)) = scored.range(block_last + 1..).next() else {
                    break;
                };

                // Jump straight to the block holding `next_scored` using
                // the skip-list cursor (O(log N_blocks)) instead of
                // re-evaluating every intervening block.
                let target_block = match skip_list {
                    Some(list) => seek_block(list, next_scored, block_idx),
                    // No skip list (<=1 block) cannot happen here because
                    // we already advanced past a block; fall back to a
                    // linear bump.
                    None => block_idx + 1,
                };
                let target_block = target_block.max(block_idx + 1).min(num_blocks);
                // Count the blocks we leapfrogged (everything strictly
                // between the current block and the target).
                blocks_skipped += target_block.saturating_sub(block_idx + 1);
                block_idx = target_block;
                continue;
            }

            // Score the block. `allow_new_docs` controls whether a doc
            // not yet in `scored` may be inserted; docs already scored
            // are always updated (a rarer token may have seeded them).
            for (&doc_id, &freq) in block_doc_ids.iter().zip(block_freqs) {
                // Widen the stored `u32` freq to the `u64` term-frequency
                // the scoring closure consumes — identical value, so the
                // BM25 arithmetic downstream is bit-for-bit unchanged.
                let tf = u64::from(freq);
                if tf == 0 {
                    continue;
                }
                let in_scored = scored.contains_key(&doc_id);
                if !in_scored && !allow_new_docs {
                    continue;
                }
                let Some(contrib) = score_doc(token_idx, doc_id, tf) else {
                    continue;
                };
                let entry = scored.entry(doc_id).or_insert(0.0);
                *entry += contrib;
            }

            block_idx += 1;
        }

        Ok(blocks_skipped)
    }
}

/// Build a [`BlockSkipList`] over a token's doc_ids, deriving per-block
/// `(min_doc_id, max_doc_id)` ranges from the 128-chunking. Returns
/// `Ok(None)` when the posting list has 0 or 1 block (no skip benefit).
fn build_skip_list(doc_ids: &[u32]) -> Result<Option<BlockSkipList>, PostingsBlockError> {
    if doc_ids.len() <= BLOCK_SIZE {
        return Ok(None);
    }
    let entries = doc_ids
        .chunks(BLOCK_SIZE)
        .enumerate()
        .map(|(idx, chunk)| BlockSkipEntry {
            block_index: idx,
            min_doc_id: chunk[0],
            max_doc_id: chunk[chunk.len() - 1],
        });
    Ok(Some(BlockSkipList::from_block_ranges(entries)?))
}

/// Use the skip-list cursor to find the index of the first block whose
/// `max_doc_id >= target`, starting the cursor's search from
/// `from_block`. Falls back to `from_block + 1` if the cursor cannot
/// position past the current block (degenerate input).
fn seek_block(skip_list: &BlockSkipList, target: u32, from_block: usize) -> usize {
    let mut cursor = surch_codec::postings_block::BlockSkipCursor::resume(skip_list, from_block);
    match cursor.advance_to(target) {
        Some(entry) => entry.block_index,
        // Target past the last block: jump to the end so the token loop
        // terminates.
        None => skip_list.entries().len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Linear-walk reference implementation of the exact Lot 1 MaxScore
    /// semantics (no skip-list leapfrog) used as the correctness oracle.
    fn linear_reference<F>(
        tokens: &[MaxScoreToken<'_>],
        limit: usize,
        mut score_doc: F,
    ) -> BTreeMap<u32, f64>
    where
        F: FnMut(usize, u32, u64) -> Option<f64>,
    {
        let mut ordered: Vec<(usize, &MaxScoreToken<'_>)> = tokens.iter().enumerate().collect();
        ordered.sort_by(|a, b| {
            b.1.max_contrib
                .partial_cmp(&a.1.max_contrib)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut scored: BTreeMap<u32, f64> = BTreeMap::new();
        let mut threshold = f64::NEG_INFINITY;
        for (token_idx, token) in ordered {
            let allow_new_docs = token.max_contrib >= threshold;
            let blocks = token
                .doc_ids
                .chunks(BLOCK_SIZE)
                .zip(token.freqs.chunks(BLOCK_SIZE));
            for (block_idx, (doc_id_block, freq_block)) in blocks.enumerate() {
                if doc_id_block.is_empty() {
                    continue;
                }
                let block_max = token.block_max_contribs[block_idx];
                if !allow_new_docs && block_max < threshold {
                    let block_first = doc_id_block[0];
                    let block_last = doc_id_block[doc_id_block.len() - 1];
                    if scored.range(block_first..=block_last).next().is_none() {
                        continue;
                    }
                }
                for (&doc_id, &freq) in doc_id_block.iter().zip(freq_block) {
                    let tf = u64::from(freq);
                    if tf == 0 {
                        continue;
                    }
                    let in_scored = scored.contains_key(&doc_id);
                    if !in_scored && !allow_new_docs {
                        continue;
                    }
                    let Some(contrib) = score_doc(token_idx, doc_id, tf) else {
                        continue;
                    };
                    *scored.entry(doc_id).or_insert(0.0) += contrib;
                }
            }
            if scored.len() >= limit && limit > 0 {
                let mut values: Vec<f64> = scored.values().copied().collect();
                let k = values.len() - limit;
                values.select_nth_unstable_by(k, |a, b| {
                    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                });
                threshold = values[k];
            }
        }
        scored
    }

    /// A simple deterministic per-doc contribution: a token's contrib is
    /// `weight / (1 + term_freq fraction)` — monotone, finite, and
    /// bounded by `block_max_contribs` so the skip decision is sound.
    fn make_token<'a>(
        doc_ids: &'a [u32],
        freqs: &'a [u32],
        block_max: &'a mut Vec<f64>,
        weight: f64,
    ) -> MaxScoreToken<'a> {
        // The per-doc contribution we use below is exactly `weight` for
        // every doc, so every block's upper bound is `weight`.
        block_max.clear();
        block_max.extend(doc_ids.chunks(BLOCK_SIZE).map(|_| weight));
        MaxScoreToken {
            doc_ids,
            freqs,
            block_max_contribs: block_max,
            max_contrib: weight,
        }
    }

    fn score_const(weight_by_token: &[f64]) -> impl FnMut(usize, u32, u64) -> Option<f64> + '_ {
        move |token_idx, _doc_id, _tf| Some(weight_by_token[token_idx])
    }

    #[test]
    fn single_token_no_skip() {
        let doc_ids: Vec<u32> = (0..300).collect();
        let freqs: Vec<u32> = vec![1; doc_ids.len()];
        let mut bm = Vec::new();
        let token = make_token(&doc_ids, &freqs, &mut bm, 2.0);
        let weights = [2.0];
        let outcome = MaxScoreExecutor::new(10)
            .run(&[token], score_const(&weights))
            .expect("run");
        // Single token => every doc scored, no skip.
        assert_eq!(outcome.scored.len(), 300);
        assert_eq!(outcome.blocks_skipped, 0);
    }

    #[test]
    fn rare_then_common_skips_blocks_and_matches_reference() {
        // Rare token: 3 docs far apart (different blocks of the common
        // token). Common token: every doc in 0..(8*128). The rare token
        // has the higher max_contrib so it drives first; once K docs are
        // scored, the common token's blocks far from any rare doc are
        // skippable and the cursor must leapfrog them.
        let common_doc_ids: Vec<u32> = (0..(8 * BLOCK_SIZE as u32)).collect();
        let common_freqs: Vec<u32> = vec![1; common_doc_ids.len()];
        let rare_doc_ids: Vec<u32> = vec![0, 3 * BLOCK_SIZE as u32, 7 * BLOCK_SIZE as u32];
        let rare_freqs: Vec<u32> = vec![5, 5, 5];

        let mut rare_bm = Vec::new();
        let mut common_bm = Vec::new();
        // rare weight high so it sorts first and lifts the threshold;
        // common weight low so its blocks fall below threshold.
        let rare = make_token(&rare_doc_ids, &rare_freqs, &mut rare_bm, 10.0);
        let common = make_token(&common_doc_ids, &common_freqs, &mut common_bm, 0.5);
        let weights = [10.0, 0.5];
        let tokens = [rare, common];

        let outcome = MaxScoreExecutor::new(3)
            .run(&tokens, score_const(&weights))
            .expect("run");

        let reference = linear_reference(&tokens, 3, score_const(&weights));
        assert_eq!(
            outcome.scored, reference,
            "skip-list MaxScore must match the linear reference exactly"
        );
        assert!(
            outcome.blocks_skipped > 0,
            "common token must leapfrog skippable blocks, got {}",
            outcome.blocks_skipped
        );
    }

    #[test]
    fn skip_matches_reference_on_seeded_corpus() {
        // Seeded xorshift corpus: many tokens with overlapping postings,
        // varied weights. The skip path MUST equal the linear reference
        // for every limit we try.
        let mut state: u32 = 0x9e37_79b9;
        let mut next = || -> u32 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };

        // Three tokens of decreasing density over a 4*128 doc id space.
        // `doc_ids` is built first (ascending, by construction), then
        // `freqs` is derived by walking it in that same ascending order —
        // same RNG draw sequence/count as the historical single-pass
        // `Posting::new(d, (n() % 7) + 1)` map.
        let make = |modulo: u32, n: &mut dyn FnMut() -> u32| -> (Vec<u32>, Vec<u32>) {
            let doc_ids: Vec<u32> = (0..(4 * BLOCK_SIZE as u32))
                .filter(|d| d.is_multiple_of(modulo))
                .collect();
            let freqs: Vec<u32> = doc_ids.iter().map(|_| (n() % 7) + 1).collect();
            (doc_ids, freqs)
        };
        let (t0_ids, t0_freqs) = make(1, &mut next); // every doc
        let (t1_ids, t1_freqs) = make(5, &mut next); // sparse
        let (t2_ids, t2_freqs) = make(50, &mut next); // rarest

        let mut bm0 = Vec::new();
        let mut bm1 = Vec::new();
        let mut bm2 = Vec::new();
        let tok0 = make_token(&t0_ids, &t0_freqs, &mut bm0, 0.3);
        let tok1 = make_token(&t1_ids, &t1_freqs, &mut bm1, 1.5);
        let tok2 = make_token(&t2_ids, &t2_freqs, &mut bm2, 4.0);
        let weights = [0.3, 1.5, 4.0];
        let tokens = [tok0, tok1, tok2];

        for limit in [1usize, 2, 5, 10, 50] {
            let outcome = MaxScoreExecutor::new(limit)
                .run(&tokens, score_const(&weights))
                .expect("run");
            let reference = linear_reference(&tokens, limit, score_const(&weights));
            assert_eq!(
                outcome.scored, reference,
                "skip-list MaxScore must match the linear reference at limit {limit}"
            );
        }
    }

    #[test]
    fn rejects_misaligned_block_metas() {
        let doc_ids: Vec<u32> = (0..300).collect();
        let freqs: Vec<u32> = vec![1; doc_ids.len()];
        // 300 postings => 3 blocks, but provide only 2 block maxes.
        let bad = MaxScoreToken {
            doc_ids: &doc_ids,
            freqs: &freqs,
            block_max_contribs: &[1.0, 1.0],
            max_contrib: 1.0,
        };
        let weights = [1.0];
        let err = MaxScoreExecutor::new(10)
            .run(&[bad], score_const(&weights))
            .unwrap_err();
        assert_eq!(
            err,
            MaxScoreError::BlockMetaMisaligned {
                token: 0,
                got: 2,
                expected: 3
            }
        );
    }
}
