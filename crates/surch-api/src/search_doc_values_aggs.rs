//! MVP L1c: aggregations served from doc values instead of `_source`.
//!
//! The general path ([`super::run_search_paths`]) hydrates every matched
//! document (`/agg` without criteria: the whole index, 29.3 M documents on
//! matchID) and reads each aggregated field from `_source`. This path
//! resolves the matched set as internal ids only (a bitset), applies
//! `min_score` with the source-independent scorer, and reads each field
//! from the index:
//!
//! * a multi-field sub-field with a write-time column (`NOM.raw`): the
//!   column codes, segment by segment (the values `subfield_projection`
//!   returns);
//! * a top-level scalar field whose every indexed value provably rebuilds
//!   from its single index term ([`DocValueKind`]): the term dictionary x
//!   postings.
//!
//! Values are then counted once per distinct value and fed to the SAME
//! finishing code as the `_source` path (`finish_terms_aggregation`,
//! `finish_date_histogram_aggregation`, `finish_composite_aggregation`), so
//! the response bytes are identical. Any shape it cannot serve exactly
//! returns `None` and keeps the general path.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Instant;

use serde_json::Value;
use surch_index::mapping::IndexMapping;

use super::{
    below_min_score, build_search_response_with_total, candidate_set_is_exact,
    date_value_epoch_days, finish_composite_aggregation, finish_date_histogram_aggregation,
    finish_terms_aggregation, matches_every_document, posting_candidates, record_terms_count,
    resolve_total_hits, round_days_to_interval, score_for_query, scoring_needs_source, AggResult,
    AggSpec, AggregationResults, CandidateCtx, CompositeCounts, CompositeSource,
    CompositeSourceKind, FieldValueView, ScoringArena, SearchRequest, SearchResponse,
    SearchScoringContext, SubfieldProjections, TermsKey, MILLIS_PER_DAY,
};
use crate::state::{AppState, DocBitSet, DocValueKind, IndexReader};

/// Up to this many matched documents the general path (hydrate + read
/// `_source`) is cheaper than walking a field's whole column / postings.
const DOC_VALUES_MIN_MATCHED: u64 = 2_048;

/// Composite sources served here (the tuple of per-source key ids is packed
/// in a `u128`).
const MAX_COMPOSITE_SOURCES: usize = 4;

/// Dense tuple-count table bound for a composite (`product of per-source key
/// counts`); above it the counts go to a hash map.
const MAX_DENSE_COMPOSITE_CELLS: usize = 1 << 22;

#[cfg(test)]
thread_local! {
    /// Test hooks: `Some(false)` disables this path (brute-force reference),
    /// `Some(true)` forces it even below [`DOC_VALUES_MIN_MATCHED`].
    static DOC_VALUES_OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    /// Number of requests this path answered on this thread.
    static DOC_VALUES_SERVED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn set_doc_values_aggs_override_for_test(mode: Option<bool>) {
    DOC_VALUES_OVERRIDE.with(|cell| cell.set(mode));
}

#[cfg(test)]
pub(crate) fn take_doc_values_aggs_served_for_test() -> u64 {
    DOC_VALUES_SERVED.with(|cell| cell.replace(0))
}

fn override_mode() -> Option<bool> {
    #[cfg(test)]
    {
        DOC_VALUES_OVERRIDE.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        None
    }
}

fn record_served() {
    metrics::counter!("surch_aggs_doc_values_total").increment(1);
    #[cfg(test)]
    DOC_VALUES_SERVED.with(|cell| cell.set(cell.get() + 1));
}

/// Where an aggregated field's values are read from: the sub-field column
/// (`None`) or the term dictionary of a top-level field (`Some(kind)`).
type DocValuesSource = Option<DocValueKind>;

fn doc_values_source(reader: &IndexReader<'_>, field: &str) -> Option<DocValuesSource> {
    if field.contains('.') {
        // `collect_subfield_projections`: only dotted paths are sub-fields;
        // without a column the general path reads the parent `_source`.
        return reader.has_subfield_column(field).then_some(None);
    }
    reader.doc_value_kind(field).map(Some)
}

fn agg_fields(spec: &AggSpec) -> Vec<&str> {
    match spec {
        AggSpec::Terms { field, .. }
        | AggSpec::DateHistogram { field, .. }
        | AggSpec::Cardinality { field } => vec![field.as_str()],
        AggSpec::Composite { sources, .. } => sources.iter().map(source_field).collect(),
    }
}

fn source_field(source: &CompositeSource) -> &str {
    match &source.kind {
        CompositeSourceKind::Terms { field } | CompositeSourceKind::DateHistogram { field, .. } => {
            field
        }
    }
}

/// `size: 0` search with aggregations over one index, served without
/// hydrating `_source`. `constant_score` / `scoring_enabled` are the ones
/// [`super::run_search_paths`] computed for the request.
pub(super) fn run_doc_values_aggregations(
    state: &AppState,
    indices: &[String],
    request: &SearchRequest,
    constant_score: Option<f64>,
    scoring_enabled: bool,
    started_at: Instant,
) -> Option<SearchResponse> {
    let forced = match override_mode() {
        Some(false) => return None,
        Some(true) => true,
        None => false,
    };
    if indices.len() != 1 || request.aggs.is_empty() || request.size != Some(0) {
        return None;
    }
    let index = indices[0].as_str();
    let query = request.query.as_ref();
    let min_score = request.min_score.filter(|_| scoring_enabled);

    // Candidate ids (internal), `None` = every live document.
    let candidate_ids: Option<Vec<u32>> = match query {
        None => None,
        Some(query) if matches_every_document(query) => None,
        Some(query) => {
            if !candidate_set_is_exact(query) {
                return None;
            }
            let ctx = CandidateCtx::new(state, index);
            let candidates = posting_candidates(&ctx, query, None)?;
            if !candidates.exact {
                return None;
            }
            Some(candidates.ids.into_iter().collect())
        }
    };
    let score_each = min_score.is_some() && constant_score.is_none();
    if score_each && query.is_some_and(scoring_needs_source) {
        return None;
    }
    // A source-independent score is a sum of non-negative terms (boosts
    // are parsed `>= 0`, BM25 weights and fuzzy similarities are clamped
    // `>= 0`), so `min_score <= 0` (deces-backend `/agg` with criteria
    // sends `0`) filters nothing: no per-document scoring then.
    let min_score = min_score.filter(|min| !(score_each && (*min as f32) <= 0.0));

    state.with_search_reader(index, |reader| -> Option<SearchResponse> {
        let reader = reader?;
        if !reader.index_covers_live_documents() {
            return None;
        }
        let mut sources: BTreeMap<&str, DocValuesSource> = BTreeMap::new();
        for (_, spec) in &request.aggs {
            for field in agg_fields(spec) {
                sources.insert(field, doc_values_source(&reader, field)?);
            }
        }
        let mut matched = match &candidate_ids {
            None => reader.live_doc_bitset(),
            Some(ids) => reader.live_bitset_of(ids),
        };
        if !forced && matched.count() <= DOC_VALUES_MIN_MATCHED {
            return None;
        }

        if let Some(min) = min_score {
            match (constant_score, query) {
                (Some(constant), _) => {
                    if below_min_score(constant, min) {
                        matched = DocBitSet::with_len(matched.len());
                    }
                }
                (None, Some(query)) => {
                    let ids: Vec<u32> = matched.iter().collect();
                    let mut arena = ScoringArena::default();
                    let mut scoring_context = SearchScoringContext::new(&reader, query, &mut arena);
                    scoring_context.prepare_batch(query, &ids);
                    let mut kept = DocBitSet::with_len(matched.len());
                    for (slot, &id) in ids.iter().enumerate() {
                        scoring_context.set_batch_slot(slot);
                        let score = score_for_query(query, id, &scoring_context, None);
                        if !below_min_score(score, min) {
                            kept.insert(id);
                        }
                    }
                    matched = kept;
                }
                (None, None) => {}
            }
        }

        let mapping = reader.mapping();
        let mut results: Vec<(String, AggResult)> = Vec::with_capacity(request.aggs.len());
        for (name, spec) in &request.aggs {
            let result = aggregate(&reader, spec, &sources, &matched, mapping)?;
            results.push((name.clone(), result));
        }

        record_served();
        let total = matched.count();
        let mut response = build_search_response_with_total(
            Vec::new(),
            resolve_total_hits(total, request.track_total_hits.as_ref()),
            started_at.elapsed().as_millis() as u64,
        );
        response.hits.max_score = None;
        response.aggregations = Some(AggregationResults(results));
        Some(response)
    })
}

/// Per ordinal value of `field` over `matched`, with its document count.
fn value_counts(
    reader: &IndexReader<'_>,
    field: &str,
    source: DocValuesSource,
    matched: &DocBitSet,
) -> Option<(Vec<Value>, Vec<u64>)> {
    let mut counts: Vec<u64> = Vec::new();
    let values = reader.for_each_doc_value(field, source, matched, |_, ordinal| {
        let ordinal = ordinal as usize;
        if ordinal >= counts.len() {
            counts.resize(ordinal + 1, 0);
        }
        counts[ordinal] += 1;
    })?;
    counts.resize(values.len(), 0);
    Some((values, counts))
}

fn aggregate(
    reader: &IndexReader<'_>,
    spec: &AggSpec,
    sources: &BTreeMap<&str, DocValuesSource>,
    matched: &DocBitSet,
    mapping: &IndexMapping,
) -> Option<AggResult> {
    let no_projections = SubfieldProjections::default();
    match spec {
        AggSpec::Terms { field, size } => {
            let (values, counts) =
                value_counts(reader, field, *sources.get(field.as_str())?, matched)?;
            let mut by_key: BTreeMap<TermsKey, (Value, u64)> = BTreeMap::new();
            for (value, count) in values.iter().zip(counts) {
                if count > 0 {
                    record_terms_count(&mut by_key, value, count);
                }
            }
            Some(finish_terms_aggregation(by_key, *size))
        }
        AggSpec::DateHistogram {
            field,
            calendar_interval,
            format,
        } => {
            let view = FieldValueView::new(field, Some(mapping), &no_projections);
            let field_format = view.date_format_or_default().to_owned();
            let (values, counts) =
                value_counts(reader, field, *sources.get(field.as_str())?, matched)?;
            let mut by_bucket: BTreeMap<i64, u64> = BTreeMap::new();
            for (value, count) in values.iter().zip(counts) {
                if count == 0 {
                    continue;
                }
                if let Some(days) = date_value_epoch_days(value, &field_format) {
                    *by_bucket
                        .entry(round_days_to_interval(days, *calendar_interval))
                        .or_insert(0) += count;
                }
            }
            Some(finish_date_histogram_aggregation(
                &by_bucket,
                *calendar_interval,
                format.as_deref().unwrap_or(&field_format),
            ))
        }
        AggSpec::Cardinality { field } => {
            let (values, counts) =
                value_counts(reader, field, *sources.get(field.as_str())?, matched)?;
            let distinct: BTreeSet<TermsKey> = values
                .iter()
                .zip(counts)
                .filter(|(value, count)| *count > 0 && !value.is_null())
                .map(|(value, _)| TermsKey::from_value(value))
                .collect();
            Some(AggResult::Value {
                value: Value::from(distinct.len() as u64),
            })
        }
        AggSpec::Composite {
            sources: composite_sources,
            size,
            after,
        } => composite(
            reader,
            composite_sources,
            *size,
            after.as_ref(),
            sources,
            matched,
            mapping,
        ),
    }
}

/// One composite source read over the matched set: the distinct response
/// keys and, per document, the key id plus one (`0` = the document has no
/// key for this source and is dropped, ES `missing_bucket: false`).
struct SourceKeys {
    keys: Vec<(TermsKey, Value)>,
    doc_keys: Vec<u32>,
}

fn source_keys(
    reader: &IndexReader<'_>,
    source: &CompositeSource,
    doc_values: DocValuesSource,
    view: &FieldValueView<'_>,
    matched: &DocBitSet,
) -> Option<SourceKeys> {
    let mut doc_ordinals = vec![0u32; matched.len() as usize];
    let values =
        reader.for_each_doc_value(source_field(source), doc_values, matched, |doc, ordinal| {
            doc_ordinals[doc as usize] = ordinal + 1;
        })?;
    // Ordinal -> key id + 1 (`0`: no key, e.g. an unparsable date). Two
    // ordinals with the same key (dates of one month) share a key id.
    let mut key_ids: BTreeMap<TermsKey, u32> = BTreeMap::new();
    let mut keys: Vec<(TermsKey, Value)> = Vec::new();
    let mut ordinal_keys: Vec<u32> = Vec::with_capacity(values.len());
    for value in &values {
        let key = match &source.kind {
            CompositeSourceKind::Terms { .. } => {
                (!value.is_null()).then(|| (TermsKey::from_value(value), value.clone()))
            }
            CompositeSourceKind::DateHistogram {
                calendar_interval,
                format,
                ..
            } => date_value_epoch_days(value, view.date_format_or_default()).map(|days| {
                let millis = round_days_to_interval(days, *calendar_interval) * MILLIS_PER_DAY;
                let shown = match format {
                    Some(format) => Value::String(crate::es_json::format_es_date(millis, format)),
                    None => Value::from(millis),
                };
                (TermsKey::number(millis as f64), shown)
            }),
        };
        ordinal_keys.push(match key {
            None => 0,
            Some((key, shown)) => {
                let next = keys.len() as u32;
                let id = *key_ids.entry(key.clone()).or_insert_with(|| {
                    keys.push((key, shown));
                    next
                });
                id + 1
            }
        });
    }
    for doc_key in &mut doc_ordinals {
        if *doc_key != 0 {
            *doc_key = ordinal_keys[(*doc_key - 1) as usize];
        }
    }
    Some(SourceKeys {
        keys,
        doc_keys: doc_ordinals,
    })
}

fn composite(
    reader: &IndexReader<'_>,
    composite_sources: &[CompositeSource],
    size: usize,
    after: Option<&BTreeMap<String, Value>>,
    sources: &BTreeMap<&str, DocValuesSource>,
    matched: &DocBitSet,
    mapping: &IndexMapping,
) -> Option<AggResult> {
    if composite_sources.is_empty() || composite_sources.len() > MAX_COMPOSITE_SOURCES {
        return None;
    }
    let no_projections = SubfieldProjections::default();
    let views: Vec<FieldValueView<'_>> = composite_sources
        .iter()
        .map(|source| FieldValueView::new(source_field(source), Some(mapping), &no_projections))
        .collect();
    let mut per_source: Vec<SourceKeys> = Vec::with_capacity(composite_sources.len());
    for (source, view) in composite_sources.iter().zip(&views) {
        let doc_values = *sources.get(source_field(source))?;
        per_source.push(source_keys(reader, source, doc_values, view, matched)?);
    }

    // Count each tuple of key ids: dense table when small, hash map else.
    let cells = per_source.iter().try_fold(1usize, |acc, source| {
        acc.checked_mul(source.keys.len().max(1))
    });
    let mut tuple_counts: Vec<(u128, u64)> = Vec::new();
    match cells {
        Some(cells) if cells <= MAX_DENSE_COMPOSITE_CELLS => {
            let mut dense = vec![0u64; cells];
            'docs: for doc in matched.iter() {
                let mut cell = 0usize;
                for source in &per_source {
                    let key = source.doc_keys[doc as usize];
                    if key == 0 {
                        continue 'docs;
                    }
                    cell = cell * source.keys.len() + (key - 1) as usize;
                }
                dense[cell] += 1;
            }
            for (mut cell, count) in dense.into_iter().enumerate() {
                if count == 0 {
                    continue;
                }
                let mut packed = 0u128;
                for (position, source) in per_source.iter().enumerate().rev() {
                    let width = source.keys.len();
                    packed |= ((cell % width) as u128) << (32 * position);
                    cell /= width;
                }
                tuple_counts.push((packed, count));
            }
        }
        _ => {
            let mut sparse: HashMap<u128, u64> = HashMap::new();
            'docs: for doc in matched.iter() {
                let mut packed = 0u128;
                for (position, source) in per_source.iter().enumerate() {
                    let key = source.doc_keys[doc as usize];
                    if key == 0 {
                        continue 'docs;
                    }
                    packed |= u128::from(key - 1) << (32 * position);
                }
                *sparse.entry(packed).or_insert(0) += 1;
            }
            tuple_counts.extend(sparse);
        }
    }

    let mut counts: CompositeCounts = BTreeMap::new();
    for (packed, count) in tuple_counts {
        let mut key = Vec::with_capacity(per_source.len());
        let mut shown = Vec::with_capacity(per_source.len());
        for (position, source) in per_source.iter().enumerate() {
            let id = ((packed >> (32 * position)) & u128::from(u32::MAX)) as usize;
            let (terms_key, value) = &source.keys[id];
            key.push(terms_key.clone());
            shown.push(value.clone());
        }
        counts.entry(key).or_insert_with(|| (shown, 0)).1 += count;
    }
    Some(finish_composite_aggregation(
        counts,
        composite_sources,
        &views,
        size,
        after,
    ))
}
