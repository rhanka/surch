//! RAM accounting for a [`DocumentIndex`].
//!
//! Surch keeps the inverted index fully in memory, so sizing a cluster
//! (notably the matchID INSEE indexer, ~1.3 M docs) hinges on knowing
//! how many bytes the postings, the prefix-postings side table, the
//! field-length stats, and the term dictionary actually consume.
//!
//! [`document_index_memory_usage`] walks the in-memory structures and
//! returns an **approximate** byte count. It is intentionally cheap: we
//! sum `std::mem::size_of_val` plus the heap capacity of every `Vec` /
//! `String` we own, but we do not chase down internal allocator
//! padding, hash-table load factors, or FST compression ratios. The
//! numbers are designed to feed Prometheus gauges and capacity-planning
//! dashboards, not to drive a leak detector.
//!
//! Two MVP simplifications are worth noting. `BTreeMap` overhead is
//! approximated as `entries * (sizeof::<K>() + sizeof::<V>())`; real
//! B-trees allocate node arrays of 11 entries each so the constant
//! factor is off, but the relative ranking between gauges (postings
//! ≫ prefix-postings ≫ field stats) holds. And the FST term
//! dictionary built by `PostingsBuilder::build` lives inside
//! `TermDictionary` and is counted as zero bytes here — the per-term
//! postings vectors and block-meta vectors dominate the RAM cost in
//! practice, so omitting the FST is acceptable for the first version
//! of `/_surch/stats`.
//!
//! Stored fields (the original `_source` JSON) live in
//! `surch-api::InMemoryIndex`, not in `DocumentIndex`, so the
//! `stored_fields_bytes` field is reported by the API layer through
//! [`stored_fields_bytes_for`] which sums [`Value`] payloads field by
//! field.

use std::mem::size_of;

use serde_json::Value;

use crate::document_index::{DocumentIndex, FieldLengthStats};

/// Per-component memory usage of a [`DocumentIndex`].
///
/// All values are byte counts and are best-effort approximations
/// computed in `O(terms + docs)` time. See the module docs for the
/// caveats.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MemoryUsage {
    /// Inverted index: per-term `Vec<Posting>` chained from the FST.
    pub postings_bytes: u64,
    /// A6 prefix side-table: `field -> prefix -> BTreeSet<doc_id>`.
    pub prefix_postings_bytes: u64,
    /// A10 multi-field side-table: `parent.sub -> doc_id -> stored token`,
    /// the write-time fan-out projections read by sort / agg on `.raw`.
    pub subfield_values_bytes: u64,
    /// Stored `_source` payloads — populated by the API layer through
    /// [`stored_fields_bytes_for`]; [`document_index_memory_usage`]
    /// always returns 0 here because the documents live outside
    /// [`DocumentIndex`].
    pub stored_fields_bytes: u64,
    /// Per-field BM25 length stats (`doc_count`, `avg_doc_len`, and
    /// the per-doc length map when norms are enabled).
    pub field_stats_bytes: u64,
    /// Per-term block-meta vectors that back the Block-Max WAND
    /// scoring path (one `Vec<BlockMeta>` per term).
    pub term_stats_bytes: u64,
}

impl MemoryUsage {
    /// Sum of every accounted component.
    pub fn total_bytes(&self) -> u64 {
        self.postings_bytes
            .saturating_add(self.prefix_postings_bytes)
            .saturating_add(self.subfield_values_bytes)
            .saturating_add(self.stored_fields_bytes)
            .saturating_add(self.field_stats_bytes)
            .saturating_add(self.term_stats_bytes)
    }
}

/// Returns an approximate per-component byte count for `doc_index`.
///
/// The cost is one pass over every `(field, term)` pair plus one pass
/// over `prefix_postings`. The numbers are stable across calls when
/// the index does not change.
pub fn document_index_memory_usage(doc_index: &DocumentIndex) -> MemoryUsage {
    let DocumentIndexAccounting {
        postings_bytes,
        term_stats_bytes,
    } = accounting_from_postings(doc_index);

    MemoryUsage {
        postings_bytes,
        prefix_postings_bytes: prefix_postings_bytes(doc_index),
        subfield_values_bytes: subfield_values_bytes(doc_index),
        stored_fields_bytes: 0,
        field_stats_bytes: field_stats_bytes(doc_index),
        term_stats_bytes,
    }
}

/// Returns the byte count of the `_source` payloads for an iterator
/// over `serde_json::Value` documents. Used by the API layer (where
/// stored documents actually live) to fill
/// [`MemoryUsage::stored_fields_bytes`].
///
/// Each value contributes `approximate_value_bytes` which sums string
/// payloads, the size of the JSON node, and (for objects) the size of
/// the map keys.
pub fn stored_fields_bytes_for<'a, I>(documents: I) -> u64
where
    I: IntoIterator<Item = &'a Value>,
{
    documents.into_iter().map(approximate_value_bytes).sum()
}

/// Approximate the heap+inline footprint of a [`serde_json::Value`].
///
/// Strings and object keys count their UTF-8 length plus the inline
/// `String` / `Value` header. Numbers and booleans count only the
/// `Value` header. Arrays and objects recurse.
pub fn approximate_value_bytes(value: &Value) -> u64 {
    let head = size_of::<Value>() as u64;
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => head,
        Value::String(s) => head + s.len() as u64,
        Value::Array(items) => head + items.iter().map(approximate_value_bytes).sum::<u64>(),
        Value::Object(map) => {
            let mut total = head;
            for (key, value) in map {
                total += key.len() as u64;
                total += size_of::<String>() as u64;
                total += approximate_value_bytes(value);
            }
            total
        }
    }
}

struct DocumentIndexAccounting {
    postings_bytes: u64,
    term_stats_bytes: u64,
}

fn accounting_from_postings(doc_index: &DocumentIndex) -> DocumentIndexAccounting {
    use crate::postings::BlockMeta;
    let mut postings_bytes: u64 = 0;
    let mut term_stats_bytes: u64 = 0;
    let posting_size = size_of::<crate::postings::Posting>() as u64;
    let block_meta_size = size_of::<BlockMeta>() as u64;

    for field in doc_index.field_names() {
        for term in doc_index.terms(&field) {
            postings_bytes += term.len() as u64;
            if let Some(postings) = doc_index.postings(&field, &term) {
                // Optimisation #9: postings are flat `Copy` structs
                // (`{doc_id, freq}`) — no per-posting heap `Vec<u32>` of
                // positions anymore, so the whole posting list cost is just
                // count * posting_size.
                let count = postings.count() as u64;
                postings_bytes += count.saturating_mul(posting_size);
                // Parallel compact doc_id channel (the conjunction leapfrog
                // walks it): `count * size_of::<u32>()`, index-aligned.
                postings_bytes += count.saturating_mul(size_of::<u32>() as u64);
            }
            if let Some(metas) = doc_index.block_metas(&field, &term) {
                term_stats_bytes += (metas.len() as u64).saturating_mul(block_meta_size);
            }
        }
    }

    DocumentIndexAccounting {
        postings_bytes,
        term_stats_bytes,
    }
}

fn prefix_postings_bytes(doc_index: &DocumentIndex) -> u64 {
    let entry_overhead = (size_of::<String>() + size_of::<u32>()) as u64;
    let mut total: u64 = 0;
    for (field, by_prefix) in doc_index.prefix_postings_map().iter() {
        total += field.len() as u64;
        total += size_of::<String>() as u64;
        for (prefix, doc_ids) in by_prefix.iter() {
            total += prefix.len() as u64;
            total += entry_overhead;
            total += (doc_ids.len() as u64).saturating_mul(size_of::<u32>() as u64);
        }
    }
    total
}

fn subfield_values_bytes(doc_index: &DocumentIndex) -> u64 {
    let pair_overhead = (size_of::<u32>() + size_of::<String>()) as u64;
    let mut total: u64 = 0;
    for (field, by_doc) in doc_index.subfield_values_map().iter() {
        total += field.len() as u64;
        total += size_of::<String>() as u64;
        for value in by_doc.values() {
            total += pair_overhead;
            total += value.len() as u64;
        }
    }
    total
}

fn field_stats_bytes(doc_index: &DocumentIndex) -> u64 {
    let stats_header = size_of::<FieldLengthStats>() as u64;
    let entry_size = size_of::<u64>() as u64;
    let mut total: u64 = 0;
    for (field, stats) in doc_index.field_stats_map().iter() {
        total += field.len() as u64;
        total += stats_header;
        total += (stats.doc_len_dense().len() as u64).saturating_mul(entry_size);
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_index::DocumentIndex;
    use crate::mapping::{FieldMapping, FieldPrefixes, FieldType, IndexMapping};

    #[test]
    fn empty_index_has_zero_postings_bytes() {
        let index = DocumentIndex::new();
        let usage = document_index_memory_usage(&index);
        assert_eq!(usage.postings_bytes, 0);
        assert_eq!(usage.prefix_postings_bytes, 0);
        assert_eq!(usage.subfield_values_bytes, 0);
        assert_eq!(usage.term_stats_bytes, 0);
        assert_eq!(usage.field_stats_bytes, 0);
        assert_eq!(usage.total_bytes(), 0);
    }

    #[test]
    fn indexed_documents_increase_postings_bytes() {
        let mut index = DocumentIndex::new();
        index
            .add_document_with_mapping(
                1,
                [("name", "dupont martin"), ("city", "paris")],
                &IndexMapping::default(),
            )
            .expect("doc 1");
        index
            .add_document_with_mapping(
                2,
                [("name", "dupre"), ("city", "lyon")],
                &IndexMapping::default(),
            )
            .expect("doc 2");
        let usage = document_index_memory_usage(&index);
        assert!(usage.postings_bytes > 0);
        assert!(usage.term_stats_bytes > 0);
        assert!(usage.field_stats_bytes > 0);
        // Stored fields live outside DocumentIndex.
        assert_eq!(usage.stored_fields_bytes, 0);
    }

    #[test]
    fn prefix_postings_are_counted_when_field_carries_index_prefixes() {
        let mut mapping = IndexMapping::new();
        let field_mapping =
            FieldMapping::new(FieldType::Text, None).with_index_prefixes(Some(FieldPrefixes {
                min_chars: 2,
                max_chars: 5,
            }));
        mapping.set_field_mapping("name", field_mapping);

        let mut index = DocumentIndex::new();
        index
            .add_document_with_mapping(1, [("name", "DUPONT")], &mapping)
            .expect("doc 1");
        index
            .add_document_with_mapping(2, [("name", "DUPRE")], &mapping)
            .expect("doc 2");

        let usage = document_index_memory_usage(&index);
        assert!(usage.prefix_postings_bytes > 0, "{:?}", usage);
    }

    #[test]
    fn subfield_values_are_counted_when_field_declares_subfields() {
        // A10: a parent field with a `fields.raw` keyword/normalizer
        // sub-field fans out at write time, so the side-table is non-empty
        // and contributes to both `subfield_values_bytes` and the total.
        let mut subfields = std::collections::BTreeMap::new();
        subfields.insert(
            "raw".to_owned(),
            FieldMapping::new(FieldType::Keyword, None)
                .with_normalizer(Some(crate::mapping::AnalyzerName::Norm)),
        );
        let nom = FieldMapping::new(FieldType::Text, None).with_subfields(subfields);
        let mut mapping = IndexMapping::new();
        mapping.set_field_mapping("NOM", nom);

        let mut index = DocumentIndex::new();
        index
            .add_document_with_mapping(1, [("NOM", "DUPONT")], &mapping)
            .expect("doc 1");

        let usage = document_index_memory_usage(&index);
        assert!(usage.subfield_values_bytes > 0, "{usage:?}");
        assert!(usage.total_bytes() >= usage.subfield_values_bytes);
    }

    #[test]
    fn stored_fields_bytes_grows_with_payload_size() {
        let short = serde_json::json!({ "name": "a" });
        let long = serde_json::json!({ "name": "a much longer payload than the previous one" });
        let small = stored_fields_bytes_for([&short]);
        let big = stored_fields_bytes_for([&long]);
        assert!(big > small, "long payload should outweigh short one");
    }
}
