//! Lot 3 integration tests: the skip-list block-leapfrog MaxScore
//! executor must produce ranking output byte-for-byte identical to a
//! brute-force OR-match (score every doc of every token, sum per token),
//! and must demonstrably skip whole 128-blocks on a sparse top-K.
//!
//! These tests mirror the real `surch-api::search::maxscore_match` data
//! shape: per-token `(doc_id, term_freq)` postings + per-block BM25 upper
//! bounds, scored with the production `bm25_score`. The skip is a pure
//! performance optimisation, so any divergence from the brute-force
//! ranking is a correctness regression and fails the test.

use std::collections::BTreeMap;

use surch_search::maxscore::{MaxScoreExecutor, MaxScoreToken, BLOCK_SIZE};
use surch_search::scoring::{bm25_score, Bm25Config};

const EPSILON: f64 = 1e-9;

/// A token described the way the production OR-match path describes it:
/// postings `(doc_id, tf)`, a per-doc `doc_freq`, a repeat `boost`, and a
/// `max_term_freq` per block so we can compute the block-max upper bound.
struct Token {
    postings: Vec<(u32, u64)>,
    doc_freq: u64,
    boost: f64,
}

/// Full inputs for one OR-match scenario.
struct Scenario {
    tokens: Vec<Token>,
    doc_count: u64,
    avg_doc_len: f64,
    doc_len_by_id: BTreeMap<u32, u64>,
}

impl Scenario {
    fn config(&self) -> Bm25Config {
        Bm25Config::default()
    }

    /// Per-block BM25 upper bound for a token: bm25(block_max_tf,
    /// min_doc_len, …) * boost. `min_doc_len` is the smallest doc length
    /// across the corpus (tightest, still-valid upper bound), matching the
    /// production path.
    fn block_max_contribs(&self, token: &Token) -> Vec<f64> {
        let config = self.config();
        let min_doc_len = self
            .doc_len_by_id
            .values()
            .copied()
            .min()
            .unwrap_or(1)
            .max(1);
        token
            .postings
            .chunks(BLOCK_SIZE)
            .map(|chunk| {
                let block_max_tf = chunk.iter().map(|(_, tf)| *tf).max().unwrap_or(1).max(1);
                let score = bm25_score(
                    config,
                    self.doc_count,
                    token.doc_freq,
                    block_max_tf,
                    min_doc_len,
                    self.avg_doc_len,
                )
                .unwrap_or(0.0);
                score * token.boost
            })
            .collect()
    }

    fn max_contrib(&self, block_maxes: &[f64]) -> f64 {
        block_maxes
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
    }

    /// The per-doc contribution closure used by the executor and by the
    /// brute-force oracle — identical scoring so any difference is purely
    /// the skip logic.
    fn score_doc(&self, token: &Token, doc_id: u32, tf: u64) -> Option<f64> {
        let doc_len = self.doc_len_by_id.get(&doc_id).copied()?;
        if doc_len == 0 {
            return None;
        }
        bm25_score(
            self.config(),
            self.doc_count,
            token.doc_freq,
            tf,
            doc_len,
            self.avg_doc_len,
        )
        .ok()
        .map(|score| score * token.boost)
    }

    /// Brute-force OR-match: sum every token's contribution for every doc
    /// it contains, no skipping. This is the ground truth ranking.
    fn brute_force(&self) -> BTreeMap<u32, f64> {
        let mut scored: BTreeMap<u32, f64> = BTreeMap::new();
        for token in &self.tokens {
            for &(doc_id, tf) in &token.postings {
                if tf == 0 {
                    continue;
                }
                if let Some(contrib) = self.score_doc(token, doc_id, tf) {
                    *scored.entry(doc_id).or_insert(0.0) += contrib;
                }
            }
        }
        scored
    }
}

/// Compare two scored maps restricted to the top-`limit` docs (the only
/// docs the executor is contractually required to score correctly): same
/// doc ids in the same order, same scores within EPSILON.
fn assert_topk_identical(actual: &BTreeMap<u32, f64>, expected: &BTreeMap<u32, f64>, limit: usize) {
    let top = |m: &BTreeMap<u32, f64>| -> Vec<(u32, f64)> {
        let mut v: Vec<(u32, f64)> = m.iter().map(|(&id, &s)| (id, s)).collect();
        // Sort by score desc, doc id asc (Lucene tie-break).
        v.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        v.truncate(limit);
        v
    };
    let a = top(actual);
    let e = top(expected);
    assert_eq!(
        a.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        e.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        "top-{limit} doc ids must match brute force"
    );
    for ((aid, asc), (eid, esc)) in a.iter().zip(e.iter()) {
        assert_eq!(aid, eid);
        assert!(
            (asc - esc).abs() <= EPSILON,
            "score for doc {aid}: skip={asc} brute={esc}"
        );
    }
}

fn run(scenario: &Scenario, limit: usize) -> (BTreeMap<u32, f64>, usize) {
    // Build per-token block maxes (kept alive for the borrow).
    let block_maxes: Vec<Vec<f64>> = scenario
        .tokens
        .iter()
        .map(|t| scenario.block_max_contribs(t))
        .collect();
    let tokens: Vec<MaxScoreToken<'_>> = scenario
        .tokens
        .iter()
        .enumerate()
        .map(|(i, t)| MaxScoreToken {
            postings: t.postings.as_slice(),
            block_max_contribs: block_maxes[i].as_slice(),
            max_contrib: scenario.max_contrib(&block_maxes[i]),
        })
        .collect();

    let outcome = MaxScoreExecutor::new(limit)
        .run(&tokens, |token_idx, doc_id, tf| {
            scenario.score_doc(&scenario.tokens[token_idx], doc_id, tf)
        })
        .expect("maxscore run");
    (outcome.scored, outcome.blocks_skipped)
}

/// Build a corpus where one rare, high-weight token seeds top-K and one
/// common, low-weight token spans 8 blocks but only contributes near the
/// rare doc ids. The executor must leapfrog the empty stretches.
fn rare_common_scenario() -> Scenario {
    let n_blocks = 8u32;
    let n_docs = n_blocks * BLOCK_SIZE as u32;
    let common: Vec<(u32, u64)> = (0..n_docs).map(|d| (d, 1)).collect();
    // Rare token in 3 well-separated blocks, high tf.
    let rare: Vec<(u32, u64)> = vec![
        (1, 12),
        (3 * BLOCK_SIZE as u32 + 7, 12),
        (7 * BLOCK_SIZE as u32 + 3, 12),
    ];
    let doc_len_by_id: BTreeMap<u32, u64> = (0..n_docs).map(|d| (d, 4)).collect();
    Scenario {
        tokens: vec![
            Token {
                postings: rare,
                doc_freq: 3,
                boost: 1.0,
            },
            Token {
                postings: common,
                doc_freq: n_docs as u64,
                boost: 1.0,
            },
        ],
        doc_count: n_docs as u64,
        avg_doc_len: 4.0,
        doc_len_by_id,
    }
}

#[test]
fn skip_executor_matches_brute_force_and_skips_blocks() {
    let scenario = rare_common_scenario();
    let limit = 3;
    let (scored, blocks_skipped) = run(&scenario, limit);
    let brute = scenario.brute_force();

    // Top-K must be byte-for-byte identical to brute force.
    assert_topk_identical(&scored, &brute, limit);

    // And the skip list must have actually leapfrogged blocks: the
    // common token's blocks far from any rare doc are below the
    // threshold set by the rare token, so they are skipped.
    assert!(
        blocks_skipped > 0,
        "expected the common token to leapfrog skippable blocks, got {blocks_skipped}"
    );
}

#[test]
fn skip_executor_matches_brute_force_on_seeded_multi_token_corpus() {
    // Deterministic xorshift corpus: 4 tokens of varied density and
    // weight over a 6*128 doc-id space, with non-uniform doc lengths and
    // term freqs. The skip executor must equal brute force at every
    // limit — this is the seeded before/after ranking guardrail.
    let mut state: u32 = 0x1234_5678;
    let mut next = || -> u32 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };

    let n_docs = 6 * BLOCK_SIZE as u32;
    let doc_len_by_id: BTreeMap<u32, u64> =
        (0..n_docs).map(|d| (d, (next() % 20) as u64 + 1)).collect();

    let mut make_token = |modulo: u32, n: &mut dyn FnMut() -> u32, boost: f64| {
        let postings: Vec<(u32, u64)> = (0..n_docs)
            .filter(|d| d % modulo == 0)
            .map(|d| (d, (n() % 9) as u64 + 1))
            .collect();
        let doc_freq = postings.len() as u64;
        Token {
            postings,
            doc_freq,
            boost,
        }
    };

    let tokens = vec![
        make_token(1, &mut next, 0.4),  // dense, low weight
        make_token(3, &mut next, 1.1),  // medium
        make_token(11, &mut next, 2.7), // sparse
        make_token(64, &mut next, 5.0), // very rare, high weight
    ];

    let scenario = Scenario {
        tokens,
        doc_count: n_docs as u64,
        avg_doc_len: 10.0,
        doc_len_by_id,
    };

    let brute = scenario.brute_force();
    for limit in [1usize, 2, 3, 5, 10, 25] {
        let (scored, _skipped) = run(&scenario, limit);
        assert_topk_identical(&scored, &brute, limit);
    }
}

#[test]
fn skip_executor_full_score_set_matches_brute_force_when_limit_is_large() {
    // When limit exceeds the number of distinct docs, NOTHING is
    // skippable (threshold never gates), so the executor must reproduce
    // the brute-force scored set in full — not just the top-K slice.
    let scenario = rare_common_scenario();
    let total_docs = scenario.doc_len_by_id.len();
    let (scored, blocks_skipped) = run(&scenario, total_docs + 100);
    let brute = scenario.brute_force();

    assert_eq!(scored.len(), brute.len(), "same doc set");
    for (id, expected) in &brute {
        let actual = scored.get(id).copied().expect("doc present");
        assert!(
            (actual - expected).abs() <= EPSILON,
            "doc {id}: skip={actual} brute={expected}"
        );
    }
    assert_eq!(
        blocks_skipped, 0,
        "no block is skippable when the whole corpus fits in top-K"
    );
}
