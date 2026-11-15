//! MVP L6 persistence of a [`DocumentIndex`] (`docs/ops/persistence.md`).
//!
//! Each sealed [`Segment`] becomes up to three immutable files inside the
//! index directory:
//!
//! - `seg-<id>.seg` — framed (`SEGM`), the resident-only state: doc range,
//!   live-docs bitset, per-field BM25 length stats (SmallFloat norms), the
//!   sub-field columns (resident `dict`/`codes`, or the spill descriptors)
//!   and the term dictionary's resident part (FSTs, CSR / descriptor
//!   channels, roaring side tables, P2 digests);
//! - `seg-<id>.pst` — the segment's existing `PostingsSegment` file (FoR
//!   payloads, packed block directories, `TermEntry` tables), linked as-is;
//! - `seg-<id>.sub` — the segment's existing spilled sub-field column file,
//!   linked as-is.
//!
//! A segment already persisted by an earlier generation is never rewritten:
//! [`SegmentPersistCache`] remembers the `Weak` handle of every persisted
//! `Arc<Segment>`. Any in-place mutation goes through `Arc::make_mut`, which
//! moves the value to a NEW allocation whenever a `Weak` exists, so pointer
//! identity with a live `Weak` proves the segment is byte-for-byte the one
//! already on disk (and the held `Weak` keeps the old allocation from being
//! reused by another segment).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Weak};

use surch_store::persist::{
    hash_file_range, link_or_copy, open_framed_file, write_framed_file, FramedReader, FramedWriter,
    PersistError, PersistedFile, Result,
};

use super::{
    DocumentIndex, FieldLengthStats, LiveDocsBitset, Segment, SubfieldColumn, SubfieldColumnDisk,
    SubfieldSegment,
};
use crate::postings::{decode_term_dictionary, encode_term_dictionary, postings_file_for_persist};

/// Version of the `SEGM` framed file (and of the `.pst`/`.sub` layouts it
/// addresses). Bump on any incompatible change: an older or newer file then
/// fails closed with [`PersistError::VersionMismatch`].
pub const SEGMENT_FORMAT_VERSION: u32 = 1;
const SEGMENT_KIND: [u8; 4] = *b"SEGM";

/// Files of one persisted segment, as recorded in the index manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedSegment {
    pub meta: PersistedFile,
    pub postings: Option<PersistedFile>,
    pub subfields: Option<PersistedFile>,
}

impl PersistedSegment {
    /// Every file this segment references.
    pub fn files(&self) -> impl Iterator<Item = &PersistedFile> {
        std::iter::once(&self.meta)
            .chain(self.postings.as_ref())
            .chain(self.subfields.as_ref())
    }

    pub fn encode(&self, out: &mut FramedWriter) -> Result<()> {
        self.meta.encode(out)?;
        for file in [&self.postings, &self.subfields] {
            match file {
                Some(file) => {
                    out.put_u8(1)?;
                    file.encode(out)?;
                }
                None => out.put_u8(0)?,
            }
        }
        Ok(())
    }

    pub fn decode(input: &mut FramedReader) -> Result<Self> {
        let meta = PersistedFile::decode(input)?;
        let mut optional = || -> Result<Option<PersistedFile>> {
            match input.get_u8()? {
                0 => Ok(None),
                1 => Ok(Some(PersistedFile::decode(input)?)),
                other => Err(input.corrupt(format!("invalid option tag {other}"))),
            }
        };
        let postings = optional()?;
        let subfields = optional()?;
        Ok(Self {
            meta,
            postings,
            subfields,
        })
    }
}

/// Which `Arc<Segment>` generations are already on disk — see the module doc.
#[derive(Debug, Default)]
pub struct SegmentPersistCache {
    entries: Vec<(Weak<Segment>, PersistedSegment)>,
}

impl SegmentPersistCache {
    fn lookup(&self, segment: &Arc<Segment>) -> Option<&PersistedSegment> {
        self.entries
            .iter()
            .find(|(weak, _)| std::ptr::eq(weak.as_ptr(), Arc::as_ptr(segment)))
            .map(|(_, persisted)| persisted)
    }

    /// Number of segments this cache tracks (diagnostics / tests).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn link_data_file(
    dir: &Path,
    name: String,
    source: Option<(std::path::PathBuf, u64)>,
) -> Result<Option<PersistedFile>> {
    let Some((path, len)) = source else {
        return Ok(None);
    };
    let target = dir.join(&name);
    link_or_copy(&path, &target)?;
    let hash = hash_file_range(&target, 0, len)?;
    Ok(Some(PersistedFile { name, len, hash }))
}

fn encode_segment(segment: &Segment, out: &mut FramedWriter) -> Result<()> {
    out.put_u32(segment.doc_base)?;
    out.put_u32(segment.doc_count)?;
    out.put_u64s(&segment.live_docs.bits)?;
    out.put_u64(segment.live_docs.count as u64)?;

    out.put_u64(segment.field_stats.len() as u64)?;
    for (name, stats) in &segment.field_stats {
        out.put_str(name)?;
        out.put_u64(stats.doc_count)?;
        out.put_u64(stats.total_terms)?;
        out.put_bytes(&stats.doc_len_dense)?;
        out.put_u64(stats.min_doc_len)?;
    }

    out.put_u64(segment.subfield_values.len() as u64)?;
    for (path, column) in &segment.subfield_values {
        out.put_str(path)?;
        out.put_u64(column.dict.len() as u64)?;
        for value in &column.dict {
            out.put_str(value)?;
        }
        out.put_u32s(&column.codes)?;
        match column.disk {
            Some(disk) => {
                out.put_u8(1)?;
                out.put_u64(disk.dict_offsets_base)?;
                out.put_u32(disk.dict_len)?;
                out.put_u64(disk.dict_bytes_base)?;
                out.put_u32(disk.dict_bytes_len)?;
                out.put_u64(disk.codes_base)?;
                out.put_u32(disk.codes_len)?;
            }
            None => out.put_u8(0)?,
        }
    }
    out.put_u64(
        segment
            .subfield_segment
            .as_ref()
            .map_or(0, |store| store.bytes_written()),
    )?;
    encode_term_dictionary(&segment.terms, out)
}

fn decode_segment(dir: &Path, persisted: &PersistedSegment) -> Result<Segment> {
    let mut input = open_framed_file(dir, &persisted.meta, SEGMENT_KIND, SEGMENT_FORMAT_VERSION)?;
    let doc_base = input.get_u32()?;
    let doc_count = input.get_u32()?;
    let bits = input.get_u64s()?;
    let count = usize::try_from(input.get_u64()?)
        .map_err(|_| input.corrupt("live-docs count overflows usize"))?;
    let popcount: usize = bits.iter().map(|word| word.count_ones() as usize).sum();
    if popcount != count {
        return Err(input.corrupt("live-docs bitset popcount disagrees with its count"));
    }
    let live_docs = LiveDocsBitset { bits, count };

    let stats_count = input.get_count(8)?;
    let mut field_stats = BTreeMap::new();
    for _ in 0..stats_count {
        let name = input.get_str()?;
        let stats = FieldLengthStats {
            doc_count: input.get_u64()?,
            total_terms: input.get_u64()?,
            doc_len_dense: input.get_bytes()?,
            min_doc_len: input.get_u64()?,
        };
        field_stats.insert(name, stats);
    }

    let column_count = input.get_count(8)?;
    let mut subfield_values = BTreeMap::new();
    for _ in 0..column_count {
        let path = input.get_str()?;
        let dict_len = input.get_count(8)?;
        let mut dict = Vec::with_capacity(dict_len);
        for _ in 0..dict_len {
            dict.push(input.get_str()?.into_boxed_str());
        }
        let codes = input.get_u32s()?;
        if codes
            .iter()
            .any(|&code| code != SubfieldColumn::ABSENT && code as usize >= dict.len())
        {
            return Err(input.corrupt(format!("sub-field column {path:?} code out of its dict")));
        }
        let disk = match input.get_u8()? {
            0 => None,
            1 => Some(SubfieldColumnDisk {
                dict_offsets_base: input.get_u64()?,
                dict_len: input.get_u32()?,
                dict_bytes_base: input.get_u64()?,
                dict_bytes_len: input.get_u32()?,
                codes_base: input.get_u64()?,
                codes_len: input.get_u32()?,
            }),
            other => return Err(input.corrupt(format!("invalid option tag {other}"))),
        };
        subfield_values.insert(
            path,
            SubfieldColumn {
                dict,
                codes,
                intern_index: Default::default(),
                disk,
            },
        );
    }
    let subfield_len = input.get_u64()?;
    let subfield_segment = match (&persisted.subfields, subfield_len) {
        (None, 0) => None,
        (Some(file), len) if file.len == len => {
            let path = dir.join(&file.name);
            let store = SubfieldSegment::open_persisted(&path, len)
                .map_err(|source| PersistError::io(&path, source))?;
            Some(Arc::new(store))
        }
        _ => {
            return Err(input.corrupt(
                "segment sub-field file and its recorded length disagree with the manifest",
            ))
        }
    };
    let postings_path = persisted
        .postings
        .as_ref()
        .map(|file| (dir.join(&file.name), file.len));
    let terms = decode_term_dictionary(
        &mut input,
        postings_path
            .as_ref()
            .map(|(path, len)| (path.as_path(), *len)),
    )?;
    input.finish()?;
    Ok(Segment {
        terms,
        field_stats,
        subfield_values,
        live_docs,
        doc_base,
        doc_count,
        subfield_segment,
    })
}

impl DocumentIndex {
    /// MVP L6: write every segment not yet on disk into `dir` (immutable
    /// files named from `next_file_id`, incremented per new segment) and
    /// return the full ordered segment list plus the cache describing it.
    /// The caller commits the list in its manifest and only THEN installs
    /// the returned cache: on failure the new files are unreferenced
    /// orphans, removed by the next open.
    ///
    /// Must be called on a sealed index (`terms_dirty() == false`, i.e.
    /// right after `materialize_terms_and_finalize_postings`), otherwise
    /// pending writes would be missing from the persisted term dictionary.
    pub fn persist_segments(
        &self,
        dir: &Path,
        cache: &SegmentPersistCache,
        next_file_id: &mut u64,
    ) -> Result<(Vec<PersistedSegment>, SegmentPersistCache)> {
        debug_assert!(
            !self.terms_dirty,
            "persist_segments requires a materialized term dictionary"
        );
        let mut persisted = Vec::with_capacity(self.segments.len());
        let mut next_cache = SegmentPersistCache::default();
        for segment in &self.segments {
            let entry = match cache.lookup(segment) {
                Some(existing) => existing.clone(),
                None => {
                    let id = *next_file_id;
                    *next_file_id += 1;
                    let postings_source = postings_file_for_persist(&segment.terms)
                        .map_err(|source| PersistError::io(dir, source))?;
                    let postings =
                        link_data_file(dir, format!("seg-{id:08}.pst"), postings_source)?;
                    let subfield_source = match segment.subfield_segment.as_ref() {
                        Some(store) => {
                            let Some(path) = store.backing_path() else {
                                return Err(PersistError::io(
                                    dir,
                                    std::io::Error::new(
                                        std::io::ErrorKind::Unsupported,
                                        "sub-field segment has no backing file on this platform",
                                    ),
                                ));
                            };
                            store
                                .trim_to_written()
                                .map_err(|source| PersistError::io(path, source))?;
                            Some((path.to_path_buf(), store.bytes_written()))
                        }
                        None => None,
                    };
                    let subfields =
                        link_data_file(dir, format!("seg-{id:08}.sub"), subfield_source)?;
                    let meta = write_framed_file(
                        dir,
                        &format!("seg-{id:08}.seg"),
                        SEGMENT_KIND,
                        SEGMENT_FORMAT_VERSION,
                        |out| encode_segment(segment, out),
                    )?;
                    PersistedSegment {
                        meta,
                        postings,
                        subfields,
                    }
                }
            };
            next_cache
                .entries
                .push((Arc::downgrade(segment), entry.clone()));
            persisted.push(entry);
        }
        Ok((persisted, next_cache))
    }

    /// MVP L6: index-wide resident state that does not belong to a segment
    /// (the `index_prefixes` side postings and the next doc-id hint).
    pub fn encode_index_state(&self, out: &mut FramedWriter) -> Result<()> {
        out.put_u32(self.next_doc_id_hint)?;
        out.put_u64(self.prefix_postings.len() as u64)?;
        for (field, prefixes) in &self.prefix_postings {
            out.put_str(field)?;
            out.put_u64(prefixes.len() as u64)?;
            for (prefix, doc_ids) in prefixes {
                out.put_str(prefix)?;
                let ids: Vec<u32> = doc_ids.iter().copied().collect();
                out.put_u32s(&ids)?;
            }
        }
        Ok(())
    }

    /// MVP L6: reopen an index from its verified manifest entries, without
    /// re-analysing any document. `state` is positioned on the bytes
    /// written by [`Self::encode_index_state`]. The returned cache marks
    /// every reopened segment as already persisted.
    pub fn open_persisted(
        dir: &Path,
        segments: &[PersistedSegment],
        state: &mut FramedReader,
    ) -> Result<(Self, SegmentPersistCache)> {
        let next_doc_id_hint = state.get_u32()?;
        let field_count = state.get_count(8)?;
        let mut prefix_postings = BTreeMap::new();
        for _ in 0..field_count {
            let field = state.get_str()?;
            let prefix_count = state.get_count(8)?;
            let mut prefixes = BTreeMap::new();
            for _ in 0..prefix_count {
                let prefix = state.get_str()?;
                let ids: BTreeSet<u32> = state.get_u32s()?.into_iter().collect();
                prefixes.insert(prefix, ids);
            }
            prefix_postings.insert(field, prefixes);
        }

        if segments.is_empty() {
            return Err(state.corrupt("an index manifest must list at least one segment"));
        }
        let mut loaded: Vec<Arc<Segment>> = Vec::with_capacity(segments.len());
        let mut cache = SegmentPersistCache::default();
        for persisted in segments {
            let segment = Arc::new(decode_segment(dir, persisted)?);
            if let Some(previous) = loaded.last() {
                if segment.doc_base < previous.doc_base {
                    return Err(PersistError::corrupt(
                        dir.join(&persisted.meta.name),
                        "segments are not in ascending doc_base order",
                    ));
                }
            }
            cache
                .entries
                .push((Arc::downgrade(&segment), persisted.clone()));
            loaded.push(segment);
        }
        let index = DocumentIndex {
            segments: loaded,
            prefix_postings,
            next_doc_id_hint,
            ..DocumentIndex::default()
        };
        Ok((index, cache))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::{AnalyzerName, FieldMapping, FieldPrefixes, FieldType, IndexMapping};
    use surch_store::persist::atomic_replace_framed;

    const STATE_KIND: [u8; 4] = *b"TIDX";

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "surch-l6-index-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mapping() -> IndexMapping {
        let mut subfields = BTreeMap::new();
        subfields.insert(
            "raw".to_owned(),
            FieldMapping::new(FieldType::Keyword, None).with_normalizer(Some(AnalyzerName::Norm)),
        );
        let mut mapping = IndexMapping::new();
        mapping.set_field_mapping(
            "NOM",
            FieldMapping::new(FieldType::Text, Some(AnalyzerName::Norm)).with_subfields(subfields),
        );
        mapping.set_field_mapping(
            "PRENOM",
            FieldMapping::new(FieldType::Text, Some(AnalyzerName::Standard))
                .with_index_prefixes(Some(FieldPrefixes::defaults())),
        );
        mapping.set_field_mapping("BODY", FieldMapping::new(FieldType::Text, None));
        mapping
    }

    const NAMES: [&str; 5] = ["Dupont", "Martin", "Bernard", "Petit Durand", "Dupré"];
    const FIRST: [&str; 4] = ["Marie", "Jean", "Mariette", "Paul"];

    /// Three chunks: 5 000 docs (high-df `commun` -> roaring side table),
    /// 700 docs, then a 300-doc tail sealed by the final refresh.
    fn build(disk_enabled: bool) -> DocumentIndex {
        let mapping = mapping();
        let mut index = DocumentIndex::new();
        index.set_flush_budget_bytes_override(Some(1));
        index.set_postings_disk_enabled(disk_enabled);
        index.set_merge_fanin_override(0);
        let mut next = 0u32;
        for chunk in [5_000u32, 700, 300] {
            let docs: Vec<(u32, Vec<(String, String)>)> = (next..next + chunk)
                .map(|doc_id| {
                    let i = doc_id as usize;
                    (
                        doc_id,
                        vec![
                            ("NOM".to_owned(), NAMES[i % NAMES.len()].to_owned()),
                            ("PRENOM".to_owned(), FIRST[i % FIRST.len()].to_owned()),
                            ("BODY".to_owned(), format!("commun doc{}", i % 97)),
                        ],
                    )
                })
                .collect();
            next += chunk;
            index
                .add_documents_with_mapping_deferred(docs, &mapping)
                .expect("chunk indexes");
            index.maybe_flush_by_budget();
        }
        index.materialize_terms_and_finalize_postings();
        index
    }

    fn fingerprint(index: &DocumentIndex) -> Vec<String> {
        let mut out = vec![
            format!("segments={}", index.segment_count()),
            format!("live={}", index.live_doc_count()),
            format!("doc_ids={:?}", index.doc_ids()),
            format!("disk={}", index.postings_disk_backed()),
            format!("prefix={:?}", index.prefix_postings_map()),
            format!("raw={:?}", index.subfield_projection("NOM.raw")),
            format!("codec={:?}", index.postings_codec_stats()),
        ];
        for field in ["NOM", "NOM.raw", "PRENOM", "BODY"] {
            out.push(format!(
                "{field} stats={:?}",
                index.field_stats_aggregated(field)
            ));
            for (base, stats) in index.field_stats_segments(field) {
                out.push(format!(
                    "{field}@{base} doc_len={:?}",
                    stats.doc_len_dense()
                ));
            }
        }
        let probes = [
            ("NOM", "dupont"),
            ("NOM", "durand"),
            ("NOM", "dupre"),
            ("NOM.raw", "petit durand"),
            ("PRENOM", "marie"),
            ("PRENOM", "jean"),
            ("BODY", "commun"),
            ("BODY", "doc42"),
            ("BODY", "absent"),
        ];
        for (field, term) in probes {
            out.push(format!(
                "{field}:{term} ids={:?} decoded={:?}",
                index.term_doc_ids(field, term),
                index.decode_from_segment(field, term)
            ));
        }
        out
    }

    fn persist(
        index: &DocumentIndex,
        dir: &Path,
        cache: &SegmentPersistCache,
        next: &mut u64,
    ) -> (Vec<PersistedSegment>, SegmentPersistCache) {
        let (segments, cache) = index.persist_segments(dir, cache, next).expect("persist");
        atomic_replace_framed(dir, "STATE", STATE_KIND, 1, |out| {
            index.encode_index_state(out)
        })
        .expect("state");
        (segments, cache)
    }

    fn reopen(
        dir: &Path,
        segments: &[PersistedSegment],
    ) -> Result<(DocumentIndex, SegmentPersistCache)> {
        let mut state = FramedReader::open(&dir.join("STATE"), STATE_KIND, 1)?;
        let reopened = DocumentIndex::open_persisted(dir, segments, &mut state)?;
        state.finish()?;
        Ok(reopened)
    }

    #[test]
    fn multi_segment_round_trip_ram_and_disk_modes() {
        for disk_enabled in [false, true] {
            let dir = scratch(if disk_enabled { "disk" } else { "ram" });
            let index = build(disk_enabled);
            assert!(
                index.segment_count() >= 3,
                "test needs a multi-segment index"
            );
            let mut next = 1;
            let (segments, _) = persist(&index, &dir, &SegmentPersistCache::default(), &mut next);
            let before = fingerprint(&index);
            drop(index);

            let (reopened, cache) = reopen(&dir, &segments).expect("reopen");
            assert_eq!(
                fingerprint(&reopened),
                before,
                "disk_enabled={disk_enabled}"
            );

            // Already-persisted segments are recognised, never rewritten.
            let id_before = next;
            let (again, _) = reopened
                .persist_segments(&dir, &cache, &mut next)
                .expect("re-persist");
            assert_eq!(next, id_before);
            assert_eq!(again, segments);
            drop(reopened);
            // Dropping a reopened index must not unlink its persisted files.
            for file in segments.iter().flat_map(PersistedSegment::files) {
                assert!(dir.join(&file.name).exists(), "{} vanished", file.name);
            }
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn reopen_fails_closed_on_corrupt_or_truncated_segment_files() {
        let dir = scratch("corrupt");
        let index = build(true);
        let mut next = 1;
        let (segments, _) = persist(&index, &dir, &SegmentPersistCache::default(), &mut next);
        drop(index);

        let meta = dir.join(&segments[0].meta.name);
        let original = std::fs::read(&meta).unwrap();
        let mut flipped = original.clone();
        let middle = flipped.len() / 2;
        flipped[middle] ^= 0x40;
        std::fs::write(&meta, &flipped).unwrap();
        assert!(
            reopen(&dir, &segments).is_err(),
            "bit flip must fail closed"
        );
        std::fs::write(&meta, &original[..original.len() - 1]).unwrap();
        assert!(matches!(
            reopen(&dir, &segments),
            Err(PersistError::LengthMismatch { .. })
        ));
        std::fs::write(&meta, &original).unwrap();
        reopen(&dir, &segments).expect("restored file reopens");

        let postings = segments[0].postings.as_ref().expect("postings file");
        let pst = dir.join(&postings.name);
        assert!(postings.verify(&dir, true).is_ok());
        let bytes = std::fs::read(&pst).unwrap();
        std::fs::write(&pst, &bytes[..bytes.len() / 2]).unwrap();
        assert!(postings.verify(&dir, false).is_err());
        assert!(
            reopen(&dir, &segments).is_err(),
            "short postings file fails closed"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
