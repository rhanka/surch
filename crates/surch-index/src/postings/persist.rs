//! MVP L6 persistence of a sealed [`TermDictionary`] (`docs/ops/persistence.md`).
//!
//! The FoR postings payload, the packed block directories and the
//! `TermEntry` tables already live in the segment's `PostingsSegment` file
//! (existing on-disk encodings, untouched): that file is linked into the
//! index directory as-is. What is encoded here is everything that is
//! resident-only today — the per-field FST, the resident CSR / descriptor
//! channels (RAM mode, or the disk-mode fallbacks), the high-`df` roaring
//! side tables, the P2 page digests (the resident integrity root, never read
//! back from the file it authenticates) and the codec counters.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fst::Map;
use surch_codec::postings_block::BlockDirEntry;
use surch_store::persist::{FramedReader, FramedWriter, Result};

use super::postings_segment::PostingsSegment;
use super::{
    BlockMeta, BlockedEncodeStats, FieldPostings, P2FieldIntegrity, P2IntegrityTable, P2RegionKind,
    P2VerifiedRegion, TermDictionary,
};
use crate::roaring::RoaringDocSet;

fn put_region(out: &mut FramedWriter, region: &P2VerifiedRegion) -> Result<()> {
    out.put_u64(region.data_base)?;
    out.put_u64(region.data_len)?;
    out.put_u32(region.digest_start)?;
    out.put_u32(region.digest_count)?;
    out.put_u32(region.field_ordinal)?;
    out.put_u8(region.kind as u8)
}

fn get_region(input: &mut FramedReader, digests: usize) -> Result<P2VerifiedRegion> {
    let region = P2VerifiedRegion {
        data_base: input.get_u64()?,
        data_len: input.get_u64()?,
        digest_start: input.get_u32()?,
        digest_count: input.get_u32()?,
        field_ordinal: input.get_u32()?,
        kind: match input.get_u8()? {
            1 => P2RegionKind::Entries,
            2 => P2RegionKind::Directory,
            other => return Err(input.corrupt(format!("unknown P2 region kind {other}"))),
        },
    };
    let end = u64::from(region.digest_start) + u64::from(region.digest_count);
    if end > digests as u64 {
        return Err(input.corrupt("P2 region digests out of the integrity table"));
    }
    Ok(region)
}

fn put_field(out: &mut FramedWriter, field: &FieldPostings) -> Result<()> {
    out.put_bytes(field.fst.as_fst().as_bytes())?;
    out.put_u32s(&field.doc_ids_flat)?;
    out.put_u32s(&field.freqs_flat)?;
    out.put_u32s(&field.postings_counts)?;
    let metas: Vec<u32> = field
        .block_metas_flat
        .iter()
        .map(|meta| meta.max_term_freq)
        .collect();
    out.put_u32s(&metas)?;
    out.put_u32s(&field.offsets)?;
    out.put_u32s(&field.block_offsets)?;

    out.put_u64(field.roaring.len() as u64)?;
    let mut scratch = Vec::new();
    for (term_idx, set) in &field.roaring {
        out.put_u32(*term_idx)?;
        scratch.clear();
        set.encode_into(&mut scratch);
        out.put_bytes(&scratch)?;
    }

    out.put_u64(field.segment_descriptors.len() as u64)?;
    for (offset, len) in field.segment_descriptors.iter() {
        out.put_u64(*offset)?;
        out.put_u32(*len)?;
    }
    match field.term_entries_directory {
        Some((base, count)) => {
            out.put_u8(1)?;
            out.put_u64(base)?;
            out.put_u32(count)?;
        }
        None => out.put_u8(0)?,
    }
    out.put_u64(field.block_directory.len() as u64)?;
    for entry in field.block_directory.iter() {
        out.put_u32(entry.byte_offset_in_term_payload)?;
        out.put_u32(u32::from(entry.count))?;
        out.put_u32(entry.max_doc_id)?;
    }
    out.put_u32s(&field.block_dir_offsets)?;
    match &field.p2_integrity {
        Some(integrity) => {
            out.put_u8(1)?;
            put_region(out, &integrity.entries)?;
            put_region(out, &integrity.directory)
        }
        None => out.put_u8(0),
    }
}

fn get_field(input: &mut FramedReader, digests: usize, segment_len: u64) -> Result<FieldPostings> {
    let fst_bytes = input.get_bytes()?;
    let fst =
        Map::new(fst_bytes).map_err(|error| input.corrupt(format!("invalid FST: {error}")))?;
    let doc_ids_flat = input.get_u32s()?.into_boxed_slice();
    let freqs_flat = input.get_u32s()?.into_boxed_slice();
    let postings_counts = input.get_u32s()?.into_boxed_slice();
    let block_metas_flat: Box<[BlockMeta]> = input
        .get_u32s()?
        .into_iter()
        .map(|max_term_freq| BlockMeta { max_term_freq })
        .collect();
    let offsets = input.get_u32s()?.into_boxed_slice();
    let block_offsets = input.get_u32s()?.into_boxed_slice();
    if doc_ids_flat.len() != freqs_flat.len() {
        return Err(input.corrupt("doc_id and freq channels differ in length"));
    }

    let roaring_count = input.get_count(4)?;
    let mut roaring = Vec::with_capacity(roaring_count);
    let mut previous: Option<u32> = None;
    for _ in 0..roaring_count {
        let term_idx = input.get_u32()?;
        if previous.is_some_and(|previous| previous >= term_idx)
            || u64::from(term_idx) >= fst.len() as u64
        {
            return Err(input.corrupt("roaring side table not sorted by term index"));
        }
        previous = Some(term_idx);
        let bytes = input.get_bytes()?;
        let set = RoaringDocSet::decode_from(&bytes)
            .ok_or_else(|| input.corrupt("invalid roaring container encoding"))?;
        roaring.push((term_idx, set));
    }

    let descriptor_count = input.get_count(12)?;
    let mut segment_descriptors = Vec::with_capacity(descriptor_count);
    for _ in 0..descriptor_count {
        let offset = input.get_u64()?;
        let len = input.get_u32()?;
        if len != 0 && offset.saturating_add(u64::from(len)) > segment_len {
            return Err(input.corrupt("postings descriptor points past the segment file"));
        }
        segment_descriptors.push((offset, len));
    }
    let term_entries_directory = match input.get_u8()? {
        0 => None,
        1 => {
            let base = input.get_u64()?;
            let count = input.get_u32()?;
            if base.saturating_add(u64::from(count).saturating_mul(super::TERM_ENTRY_ENCODED_LEN))
                > segment_len
            {
                return Err(input.corrupt("term entry table points past the segment file"));
            }
            Some((base, count))
        }
        other => return Err(input.corrupt(format!("invalid option tag {other}"))),
    };
    let dir_count = input.get_count(12)?;
    let mut block_directory = Vec::with_capacity(dir_count);
    for _ in 0..dir_count {
        let byte_offset_in_term_payload = input.get_u32()?;
        let count = u16::try_from(input.get_u32()?)
            .map_err(|_| input.corrupt("block directory count overflows u16"))?;
        let max_doc_id = input.get_u32()?;
        block_directory.push(BlockDirEntry {
            byte_offset_in_term_payload,
            count,
            max_doc_id,
        });
    }
    let block_dir_offsets = input.get_u32s()?.into_boxed_slice();
    let p2_integrity = match input.get_u8()? {
        0 => None,
        1 => Some(P2FieldIntegrity {
            entries: get_region(input, digests)?,
            directory: get_region(input, digests)?,
        }),
        other => return Err(input.corrupt(format!("invalid option tag {other}"))),
    };
    Ok(FieldPostings {
        fst,
        doc_ids_flat,
        freqs_flat,
        postings_counts,
        block_metas_flat,
        offsets,
        block_offsets,
        roaring,
        segment_descriptors: segment_descriptors.into_boxed_slice(),
        term_entries_directory,
        block_directory: block_directory.into_boxed_slice(),
        block_dir_offsets,
        p2_integrity,
    })
}

/// The spill file backing `dict`'s postings, trimmed to its written length,
/// ready to be linked into the data directory: `(path, len)`. `None` when
/// the dictionary has no segment (no indexed field, or best-effort I/O
/// failure at build time — every descriptor is then a `(0, 0)` sentinel).
pub(crate) fn postings_file_for_persist(
    dict: &TermDictionary,
) -> std::io::Result<Option<(PathBuf, u64)>> {
    let Some(segment) = dict.postings_segment.as_ref() else {
        return Ok(None);
    };
    let Some(path) = segment.backing_path() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "postings segment has no backing file on this platform",
        ));
    };
    segment.trim_to_written()?;
    Ok(Some((path.to_path_buf(), segment.bytes_written())))
}

/// Encode the resident part of `dict` (see the module doc).
pub(crate) fn encode_term_dictionary(dict: &TermDictionary, out: &mut FramedWriter) -> Result<()> {
    out.put_bool(dict.disk_enabled)?;
    out.put_u64(dict.postings_skipped_terms)?;
    let stats = &dict.postings_codec_stats;
    out.put_u64(stats.blocks)?;
    out.put_u64(stats.doc_id_bytes)?;
    out.put_u64(stats.freq_bytes)?;
    out.put_u64(stats.freq_omitted_blocks)?;

    let table = &dict.p2_integrity;
    out.put_u64(table.digests.len() as u64)?;
    for digest in table.digests.iter() {
        out.put_raw(digest)?;
    }
    out.put_u32(table.protected_fields)?;
    out.put_u64(table.fallback_fields)?;
    out.put_u64(table.term_occurrences)?;
    out.put_u64(table.blocks)?;
    out.put_u64(table.fields)?;

    out.put_u64(
        dict.postings_segment
            .as_ref()
            .map_or(0, |segment| segment.bytes_written()),
    )?;
    out.put_u64(dict.fields.len() as u64)?;
    for (name, field) in &dict.fields {
        out.put_str(name)?;
        put_field(out, field)?;
    }
    Ok(())
}

/// Decode what [`encode_term_dictionary`] wrote. `postings_file` is the
/// verified data-directory file (and its manifest length) backing the
/// dictionary, `None` when it had no segment.
pub(crate) fn decode_term_dictionary(
    input: &mut FramedReader,
    postings_file: Option<(&Path, u64)>,
) -> Result<TermDictionary> {
    let disk_enabled = input.get_bool()?;
    let postings_skipped_terms = input.get_u64()?;
    let postings_codec_stats = BlockedEncodeStats {
        blocks: input.get_u64()?,
        doc_id_bytes: input.get_u64()?,
        freq_bytes: input.get_u64()?,
        freq_omitted_blocks: input.get_u64()?,
    };
    let digest_count = input.get_count(32)?;
    let mut digests = Vec::with_capacity(digest_count);
    for _ in 0..digest_count {
        let mut digest = [0u8; 32];
        input.get_raw(&mut digest)?;
        digests.push(digest);
    }
    let p2_integrity = P2IntegrityTable {
        digests: digests.into_boxed_slice(),
        protected_fields: input.get_u32()?,
        fallback_fields: input.get_u64()?,
        term_occurrences: input.get_u64()?,
        blocks: input.get_u64()?,
        fields: input.get_u64()?,
    };
    let segment_len = input.get_u64()?;
    let postings_segment = match postings_file {
        Some((path, len)) => {
            if len != segment_len {
                return Err(input.corrupt(format!(
                    "postings segment length {len} disagrees with the dictionary ({segment_len})"
                )));
            }
            let segment = PostingsSegment::open_persisted(path, len)
                .map_err(|source| surch_store::persist::PersistError::io(path, source))?;
            Some(Arc::new(segment))
        }
        None => {
            if segment_len != 0 {
                return Err(input.corrupt("dictionary references a missing postings segment"));
            }
            None
        }
    };
    let field_count = input.get_count(8)?;
    let mut fields = std::collections::BTreeMap::new();
    for _ in 0..field_count {
        let name = input.get_str()?;
        let field = get_field(input, p2_integrity.digests.len(), segment_len)?;
        fields.insert(name, field);
    }
    Ok(TermDictionary {
        fields,
        postings_segment,
        postings_skipped_terms,
        disk_enabled,
        p2_integrity: Arc::new(p2_integrity),
        p2_runtime_metrics: Arc::default(),
        postings_codec_stats,
    })
}
