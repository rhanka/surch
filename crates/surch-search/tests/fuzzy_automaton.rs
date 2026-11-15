//! MVP L1: the FST Damerau-Levenshtein automaton
//! (`surch_index::fuzzy::DamerauLevenshteinAutomaton`) must accept exactly
//! the terms the reference oracle (`bounded_damerau_levenshtein`, plus the
//! exact-char prefix rule of `expand_fuzzy_terms`) accepts, with the same
//! distance — over a French name list (accents, transpositions) and
//! deterministic random mutations of it, for k = 0..=2 and with
//! transpositions on and off. Then the same contract is checked through the
//! real term dictionary: single segment, multi-segment, disk-backed.

use std::collections::BTreeMap;

use surch_index::document_index::DocumentIndex;
use surch_index::fuzzy::{automaton_distance, DamerauLevenshteinAutomaton, FuzzyTermHit};
use surch_search::fuzzy::{
    bounded_damerau_levenshtein, fuzzy_token_plan, lucene_fuzzy_similarity, select_top_fuzzy_terms,
    Fuzziness, FuzzyTermMatch,
};

const NAMES: &[&str] = &[
    "dupont",
    "dupond",
    "dupon",
    "dupontt",
    "dpuont",
    "durand",
    "durant",
    "martin",
    "martine",
    "martins",
    "matrin",
    "marten",
    "bernard",
    "bernadr",
    "petit",
    "petti",
    "jean",
    "jaen",
    "jeanne",
    "jeann",
    "joan",
    "hélène",
    "helene",
    "hélèn",
    "éloïse",
    "eloise",
    "éolïse",
    "françois",
    "francois",
    "franços",
    "çaïd",
    "zoé",
    "zoe",
    "ozé",
    "noël",
    "noel",
    "léa",
    "lea",
    "ael",
    "a",
    "ab",
    "ba",
    "abc",
    "acb",
    "bac",
    "",
    "œuvre",
    "oeuvre",
    "straße",
    "strasse",
    "müller",
    "muller",
    "mülller",
    "日本",
    "日本語",
    "本日",
    "x",
    "xy",
    "yx",
    "lefèvre",
    "lefevre",
    "lefébvre",
    "lefebvre",
    "le",
    "el",
    "dubois",
    "duboiss",
    "dubios",
    "ubois",
];

/// Oracle: `expand_fuzzy_terms`-style prefix rule + bounded OSA distance.
fn oracle(query: &str, term: &str, k: u8, prefix: usize, transpositions: bool) -> Option<u8> {
    let q: Vec<char> = query.chars().collect();
    let t: Vec<char> = term.chars().collect();
    if prefix > 0 && (t.len() < prefix || q.len() < prefix || q[..prefix] != t[..prefix]) {
        return None;
    }
    bounded_damerau_levenshtein(query, term, k, transpositions).expect("k <= 2")
}

/// Deterministic xorshift so the "property" sweep is reproducible.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const ALPHABET: &[char] = &['a', 'e', 'é', 'è', 'n', 'r', 't', 'ç', 'ï', 'ß', '本', 'o'];

fn mutate(rng: &mut Rng, word: &str, edits: usize) -> String {
    let mut chars: Vec<char> = word.chars().collect();
    for _ in 0..edits {
        match rng.below(4) {
            0 => {
                let at = rng.below(chars.len() + 1);
                chars.insert(at, ALPHABET[rng.below(ALPHABET.len())]);
            }
            1 if !chars.is_empty() => {
                let at = rng.below(chars.len());
                chars.remove(at);
            }
            2 if !chars.is_empty() => {
                let at = rng.below(chars.len());
                chars[at] = ALPHABET[rng.below(ALPHABET.len())];
            }
            _ if chars.len() >= 2 => {
                let at = rng.below(chars.len() - 1);
                chars.swap(at, at + 1);
            }
            _ => {}
        }
    }
    chars.into_iter().collect()
}

#[test]
fn automaton_matches_oracle_over_french_name_pairs() {
    let mut checked = 0usize;
    for &query in NAMES {
        for &term in NAMES {
            for k in 0..=2u8 {
                for transpositions in [true, false] {
                    for prefix in [0usize, 1, 2] {
                        let automaton =
                            DamerauLevenshteinAutomaton::new(query, k, prefix, transpositions);
                        let expected = if prefix > query.chars().count() {
                            // The automaton clamps the prefix to the query
                            // length; the Lucene layer never builds this
                            // case (it switches to an exact lookup).
                            continue;
                        } else {
                            oracle(query, term, k, prefix, transpositions)
                        };
                        assert_eq!(
                            automaton_distance(&automaton, term),
                            expected,
                            "query={query:?} term={term:?} k={k} prefix={prefix} \
                             transpositions={transpositions}"
                        );
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 10_000, "sweep too small: {checked}");
}

#[test]
fn automaton_matches_oracle_over_random_mutations() {
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    for round in 0..20_000 {
        let base = NAMES[rng.below(NAMES.len())];
        let query_edits = rng.below(2);
        let query = mutate(&mut rng, base, query_edits);
        let term_edits = rng.below(4);
        let term = mutate(&mut rng, base, term_edits);
        let k = (round % 3) as u8;
        let transpositions = round % 2 == 0;
        let automaton = DamerauLevenshteinAutomaton::new(&query, k, 0, transpositions);
        assert_eq!(
            automaton_distance(&automaton, &term),
            oracle(&query, &term, k, 0, transpositions),
            "query={query:?} term={term:?} k={k} transpositions={transpositions}"
        );
    }
}

#[test]
fn transposition_is_one_edit_only_when_enabled() {
    let with = DamerauLevenshteinAutomaton::new("martin", 1, 0, true);
    let without = DamerauLevenshteinAutomaton::new("martin", 1, 0, false);
    assert_eq!(automaton_distance(&with, "matrin"), Some(1));
    assert_eq!(automaton_distance(&without, "matrin"), None);
}

fn brute_force(terms: &[String], query: &str, k: u8, prefix: usize) -> Vec<FuzzyTermHit> {
    let mut out: Vec<FuzzyTermHit> = terms
        .iter()
        .filter_map(|term| {
            oracle(query, term, k, prefix, true).map(|edits| FuzzyTermHit {
                term: term.clone(),
                edits,
            })
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn name_docs() -> Vec<(u32, String)> {
    NAMES
        .iter()
        .enumerate()
        .filter(|(_, name)| !name.is_empty())
        .map(|(i, name)| (i as u32, (*name).to_owned()))
        .collect()
}

fn assert_fst_walk_matches_brute_force(index: &DocumentIndex, universe: &[String]) {
    for &query in NAMES.iter().filter(|name| !name.is_empty()) {
        for k in 0..=2u8 {
            for prefix in [0usize, 1] {
                if prefix >= query.chars().count() {
                    continue;
                }
                let automaton = DamerauLevenshteinAutomaton::new(query, k, prefix, true);
                let mut got = index.fuzzy_terms("name", &automaton);
                got.sort();
                assert_eq!(
                    got,
                    brute_force(universe, query, k, prefix),
                    "query={query:?} k={k} prefix={prefix}"
                );
            }
        }
    }
}

#[test]
fn fst_walk_matches_brute_force_on_a_single_segment() {
    let mut index = DocumentIndex::new();
    for (doc_id, name) in name_docs() {
        index
            .add_document(doc_id, [("name", name)])
            .expect("add document");
    }
    let universe: Vec<String> = index.terms("name").collect();
    assert!(universe.len() > 40, "indexed universe: {universe:?}");
    assert_fst_walk_matches_brute_force(&index, &universe);
}

/// Builds the same corpus split across several sealed segments (budget of
/// one byte: every `maybe_flush_by_budget` seals), optionally disk-backed.
fn segmented_index(disk: bool) -> DocumentIndex {
    let mut index = DocumentIndex::new();
    index.set_flush_budget_bytes_override(Some(1));
    index.set_merge_fanin_override(0);
    if disk {
        index.set_postings_disk_enabled(true);
    }
    for chunk in name_docs().chunks(9) {
        index
            .add_documents_with_mapping(
                chunk
                    .iter()
                    .map(|(doc_id, name)| (*doc_id, [("name", name.clone())])),
                &Default::default(),
            )
            .expect("add chunk");
        index.maybe_flush_by_budget();
    }
    index.materialize_terms_and_finalize_postings();
    index
}

#[test]
fn fst_walk_unions_every_segment_ram_and_disk() {
    let mut reference = DocumentIndex::new();
    for (doc_id, name) in name_docs() {
        reference
            .add_document(doc_id, [("name", name)])
            .expect("add document");
    }
    let universe: Vec<String> = reference.terms("name").collect();

    for disk in [false, true] {
        let index = segmented_index(disk);
        assert!(
            index.segment_count() > 1,
            "disk={disk}: expected a multi-segment index, got {}",
            index.segment_count()
        );
        assert!(index.postings_disk_backed());
        assert_fst_walk_matches_brute_force(&index, &universe);
        // Postings of every expanded term resolve across segments too.
        for term in &universe {
            assert_eq!(
                index.term_doc_ids("name", term),
                reference.term_doc_ids("name", term),
                "disk={disk} term={term:?}"
            );
        }
    }
}

#[test]
fn lucene_plan_degrades_to_exact_term() {
    let auto = Fuzziness::Auto { low: 3, high: 6 };
    assert_eq!(fuzzy_token_plan("le", auto, 0).expect("plan").max_edits, 0);
    assert_eq!(
        fuzzy_token_plan("jean", auto, 0).expect("plan").max_edits,
        1
    );
    assert_eq!(
        fuzzy_token_plan("dupont", auto, 0).expect("plan").max_edits,
        2
    );
    // prefix_length covering the whole token: exact lookup (Lucene
    // `SingleTermsEnum`).
    let plan = fuzzy_token_plan("jean", Fuzziness::Edits(2), 4).expect("plan");
    assert_eq!((plan.max_edits, plan.prefix_chars), (0, 0));
    let plan = fuzzy_token_plan("jean", Fuzziness::Edits(2), 2).expect("plan");
    assert_eq!((plan.max_edits, plan.prefix_chars), (2, 2));
}

#[test]
fn top_terms_follow_lucene_similarity_then_term_order() {
    // query "surch" (5 chars): surch=1.0, sruch=surgh=0.8, such=0.75
    // (min(4, 5) = 4), so `such` is evicted before `surgh` at N = 3 even
    // though both are one edit away.
    let hits = ["such", "surgh", "sruch", "surch"].map(|term| FuzzyTermMatch {
        term: term.to_owned(),
        edits: u8::from(term != "surch"),
    });
    let top = select_top_fuzzy_terms("surch", hits, 3);
    let terms: Vec<&str> = top.iter().map(|t| t.term.as_str()).collect();
    assert_eq!(terms, ["surch", "sruch", "surgh"]);
    assert_eq!(top[0].boost, 1.0);
    assert_eq!(top[1].boost, lucene_fuzzy_similarity(5, "sruch", 1));
    assert!((top[1].boost - 0.8).abs() < 1e-6);
}

#[test]
fn accented_terms_resolve_through_the_dictionary() {
    let mut index = DocumentIndex::new();
    let mut by_term = BTreeMap::new();
    for (doc_id, name) in name_docs() {
        index
            .add_document(doc_id, [("name", name.clone())])
            .expect("add document");
        by_term.insert(name, doc_id);
    }
    let universe: Vec<String> = index.terms("name").collect();
    // Whatever the default analyzer does to accents, a 1-edit expansion of
    // an indexed accented term finds the term itself.
    for term in universe.iter().filter(|t| !t.is_ascii()) {
        let automaton = DamerauLevenshteinAutomaton::new(term, 1, 0, true);
        let hits = index.fuzzy_terms("name", &automaton);
        assert!(
            hits.iter().any(|hit| &hit.term == term && hit.edits == 0),
            "{term:?} not found in {hits:?}"
        );
    }
    assert!(!by_term.is_empty());
}
