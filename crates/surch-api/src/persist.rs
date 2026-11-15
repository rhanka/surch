//! MVP L6 — opt-in data directory (`SURCH_DATA_DIR`), see
//! `docs/ops/persistence.md` for the operator view and
//! `docs/paper/persistence-iceberg-architecture.md` for the design.
//!
//! This module owns the on-disk *catalog*: the data-directory layout, the
//! per-index manifest (atomically replaced, Iceberg-style), the binary
//! codec of the index mapping, the root alias file and the crash-recovery
//! sweeps. The per-index payload (id maps, `_source` store, segments) is
//! written by `state.rs` (`InMemoryIndex::persist`) and `surch-index`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use serde_json::Value;
use surch_index::document_index::{PersistedSegment, SegmentPersistCache};
use surch_index::mapping::{
    AnalysisSettings, AnalyzerDefinition, AnalyzerName, EdgeNgramTokenizerDefinition, FieldMapping,
    FieldPrefixes, FieldType, IndexMapping, NormalizerDefinition,
};
use surch_store::persist::{
    atomic_replace_framed, fsync_dir, is_tmp_name, FramedReader, FramedWriter, PersistError,
    PersistedFile, Result,
};

/// Version of every framed file written by this module (`MANI`, `META`,
/// `IDMP`, `ALIA`). Any incompatible change bumps it; older/newer files
/// then fail closed with an explicit version error at startup.
pub(crate) const FORMAT_VERSION: u32 = 1;
pub(crate) const MANIFEST_KIND: [u8; 4] = *b"MANI";
pub(crate) const META_KIND: [u8; 4] = *b"META";
pub(crate) const IDMAP_KIND: [u8; 4] = *b"IDMP";
const ALIASES_KIND: [u8; 4] = *b"ALIA";

pub(crate) const MANIFEST_NAME: &str = "MANIFEST";
pub(crate) const SOURCE_NAME: &str = "source.dat";
const ALIASES_NAME: &str = "ALIASES";
const LOCK_NAME: &str = "LOCK";
const INDICES_DIR: &str = "indices";
const SPILL_DIR: &str = "tmp";
const INDEX_DIR_PREFIX: &str = "i-";
const DELETED_PREFIX: &str = ".deleted-";

/// Environment variable enabling persistence. Unset = historical in-memory
/// behaviour, bit-for-bit unchanged.
pub const DATA_DIR_ENV: &str = "SURCH_DATA_DIR";

/// Root of an opened data directory. Holds the exclusive `LOCK` for the
/// lifetime of the process state (two servers on one directory would
/// interleave manifests).
#[derive(Debug)]
pub struct DataDir {
    root: PathBuf,
    _lock: File,
}

impl DataDir {
    /// Create the layout if needed and take the exclusive lock.
    pub(crate) fn open(root: &Path) -> Result<Self> {
        for dir in [
            root.to_path_buf(),
            root.join(INDICES_DIR),
            root.join(SPILL_DIR),
        ] {
            fs::create_dir_all(&dir).map_err(|source| PersistError::io(&dir, source))?;
        }
        let lock_path = root.join(LOCK_NAME);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| PersistError::io(&lock_path, source))?;
        lock.try_lock().map_err(|error| {
            PersistError::io(
                &lock_path,
                std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    format!("data directory already in use by another Surch process ({error})"),
                ),
            )
        })?;
        Ok(Self {
            root: root.to_path_buf(),
            _lock: lock,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Spill directory for segment tempfiles: same filesystem as the index
    /// directories, so sealing a segment is a hard link, not a copy.
    pub fn spill_dir(&self) -> PathBuf {
        self.root.join(SPILL_DIR)
    }

    pub(crate) fn indices_dir(&self) -> PathBuf {
        self.root.join(INDICES_DIR)
    }

    pub(crate) fn index_dir(&self, name: &str) -> PathBuf {
        self.indices_dir().join(encode_index_dir_name(name))
    }

    /// Crash recovery, step 1: drop every leftover of an interrupted
    /// process — spill tempfiles, half-deleted index directories, and
    /// transient alias files. None of them is ever referenced by a
    /// committed manifest.
    pub(crate) fn sweep_orphans(&self) -> Result<()> {
        let spill = self.spill_dir();
        for entry in read_dir(&spill)? {
            let path = entry.path();
            let _ = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
        }
        let indices = self.indices_dir();
        for entry in read_dir(&indices)? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(DELETED_PREFIX) {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
        for entry in read_dir(&self.root)? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_tmp_name(&name) {
                let _ = fs::remove_file(entry.path());
            }
        }
        fsync_dir(&indices)?;
        fsync_dir(&self.root)
    }

    /// Index directories present on disk (committed or not).
    pub(crate) fn index_dirs(&self) -> Result<Vec<PathBuf>> {
        let mut dirs: Vec<PathBuf> = read_dir(&self.indices_dir())?
            .into_iter()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(INDEX_DIR_PREFIX)
            })
            .map(|entry| entry.path())
            .collect();
        dirs.sort();
        Ok(dirs)
    }

    /// Delete an index directory crash-safely: one atomic rename out of the
    /// `i-*` namespace (after which the index is gone for any future open),
    /// then the recursive removal (finished by [`Self::sweep_orphans`] if
    /// the process dies in between).
    pub(crate) fn remove_index_dir(&self, name: &str) -> Result<()> {
        let dir = self.index_dir(name);
        if !dir.exists() {
            return Ok(());
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let graveyard = self.indices_dir().join(format!(
            "{DELETED_PREFIX}{nanos}-{}",
            encode_index_dir_name(name)
        ));
        fs::rename(&dir, &graveyard).map_err(|source| PersistError::io(&dir, source))?;
        fsync_dir(&self.indices_dir())?;
        fs::remove_dir_all(&graveyard).map_err(|source| PersistError::io(&graveyard, source))
    }

    pub(crate) fn write_aliases(
        &self,
        aliases: &BTreeMap<String, BTreeMap<String, Value>>,
    ) -> Result<()> {
        atomic_replace_framed(
            &self.root,
            ALIASES_NAME,
            ALIASES_KIND,
            FORMAT_VERSION,
            |out| {
                out.put_u64(aliases.len() as u64)?;
                for (alias, targets) in aliases {
                    out.put_str(alias)?;
                    out.put_u64(targets.len() as u64)?;
                    for (index, definition) in targets {
                        out.put_str(index)?;
                        put_json(out, definition)?;
                    }
                }
                Ok(())
            },
        )
        .map(|_| ())
    }

    pub(crate) fn read_aliases(&self) -> Result<BTreeMap<String, BTreeMap<String, Value>>> {
        let path = self.root.join(ALIASES_NAME);
        if !path.exists() {
            return Ok(BTreeMap::new());
        }
        let mut input = FramedReader::open(&path, ALIASES_KIND, FORMAT_VERSION)?;
        let count = input.get_count(8)?;
        let mut aliases = BTreeMap::new();
        for _ in 0..count {
            let alias = input.get_str()?;
            let target_count = input.get_count(8)?;
            let mut targets = BTreeMap::new();
            for _ in 0..target_count {
                let index = input.get_str()?;
                targets.insert(index, get_json(&mut input)?);
            }
            aliases.insert(alias, targets);
        }
        input.finish()?;
        Ok(aliases)
    }
}

fn read_dir(dir: &Path) -> Result<Vec<fs::DirEntry>> {
    fs::read_dir(dir)
        .map_err(|source| PersistError::io(dir, source))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|source| PersistError::io(dir, source))
}

/// Injective, filesystem-safe directory name for an index: `i-` prefix
/// (never `.`/`..`, never hidden), `[a-z0-9._-]` kept, every other byte
/// `%XX`-escaped. The manifest stores the real name; the directory name is
/// only a locator.
pub(crate) fn encode_index_dir_name(name: &str) -> String {
    let mut out = String::from(INDEX_DIR_PREFIX);
    for byte in name.bytes() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub(crate) fn put_json(out: &mut FramedWriter, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value).expect("serde_json::Value always serialises");
    out.put_bytes(&bytes)
}

pub(crate) fn get_json(input: &mut FramedReader) -> Result<Value> {
    let bytes = input.get_bytes()?;
    serde_json::from_slice(&bytes).map_err(|error| input.corrupt(format!("invalid JSON: {error}")))
}

// ---------------------------------------------------------------------------
// Mapping codec — structural and exhaustive (no lossy JSON round trip): every
// `FieldMapping` attribute, sub-fields and the `settings.analysis` block.
// ---------------------------------------------------------------------------

fn field_type_tag(field_type: FieldType) -> u8 {
    match field_type {
        FieldType::Text => 0,
        FieldType::Keyword => 1,
        FieldType::Integer => 2,
        FieldType::Long => 3,
        FieldType::Short => 4,
        FieldType::Byte => 5,
        FieldType::Float => 6,
        FieldType::Double => 7,
        FieldType::Boolean => 8,
        FieldType::Date => 9,
        FieldType::GeoPoint => 10,
        FieldType::Object => 11,
        FieldType::Array => 12,
        FieldType::Unknown => 13,
    }
}

fn field_type_from_tag(input: &FramedReader, tag: u8) -> Result<FieldType> {
    Ok(match tag {
        0 => FieldType::Text,
        1 => FieldType::Keyword,
        2 => FieldType::Integer,
        3 => FieldType::Long,
        4 => FieldType::Short,
        5 => FieldType::Byte,
        6 => FieldType::Float,
        7 => FieldType::Double,
        8 => FieldType::Boolean,
        9 => FieldType::Date,
        10 => FieldType::GeoPoint,
        11 => FieldType::Object,
        12 => FieldType::Array,
        13 => FieldType::Unknown,
        other => return Err(input.corrupt(format!("unknown field type tag {other}"))),
    })
}

fn analyzer_tag(analyzer: Option<AnalyzerName>) -> u8 {
    match analyzer {
        None => 0,
        Some(AnalyzerName::Standard) => 1,
        Some(AnalyzerName::Simple) => 2,
        Some(AnalyzerName::Stop) => 3,
        Some(AnalyzerName::Keyword) => 4,
        Some(AnalyzerName::Whitespace) => 5,
        Some(AnalyzerName::Norm) => 6,
    }
}

fn analyzer_from_tag(input: &FramedReader, tag: u8) -> Result<Option<AnalyzerName>> {
    Ok(match tag {
        0 => None,
        1 => Some(AnalyzerName::Standard),
        2 => Some(AnalyzerName::Simple),
        3 => Some(AnalyzerName::Stop),
        4 => Some(AnalyzerName::Keyword),
        5 => Some(AnalyzerName::Whitespace),
        6 => Some(AnalyzerName::Norm),
        other => return Err(input.corrupt(format!("unknown analyzer tag {other}"))),
    })
}

fn put_strings(out: &mut FramedWriter, values: &[String]) -> Result<()> {
    out.put_u64(values.len() as u64)?;
    for value in values {
        out.put_str(value)?;
    }
    Ok(())
}

fn get_strings(input: &mut FramedReader) -> Result<Vec<String>> {
    let count = input.get_count(8)?;
    (0..count).map(|_| input.get_str()).collect()
}

fn put_field_mapping(out: &mut FramedWriter, field: &FieldMapping) -> Result<()> {
    out.put_u8(field_type_tag(field.field_type))?;
    out.put_u8(analyzer_tag(field.analyzer))?;
    out.put_opt_str(field.custom_analyzer.as_deref())?;
    out.put_u8(analyzer_tag(field.search_analyzer))?;
    out.put_opt_str(field.custom_search_analyzer.as_deref())?;
    out.put_u8(match field.norms {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    })?;
    out.put_u8(analyzer_tag(field.normalizer))?;
    match field.index_prefixes {
        Some(prefixes) => {
            out.put_u8(1)?;
            out.put_u64(prefixes.min_chars as u64)?;
            out.put_u64(prefixes.max_chars as u64)?;
        }
        None => out.put_u8(0)?,
    }
    out.put_opt_str(field.date_format.as_deref())?;
    out.put_u64(field.fields.len() as u64)?;
    for (name, sub) in &field.fields {
        out.put_str(name)?;
        put_field_mapping(out, sub)?;
    }
    Ok(())
}

fn get_field_mapping(input: &mut FramedReader, depth: u32) -> Result<FieldMapping> {
    if depth > 16 {
        return Err(input.corrupt("mapping nesting too deep"));
    }
    let tag = input.get_u8()?;
    let field_type = field_type_from_tag(input, tag)?;
    let tag = input.get_u8()?;
    let analyzer = analyzer_from_tag(input, tag)?;
    let custom_analyzer = input.get_opt_str()?;
    let tag = input.get_u8()?;
    let search_analyzer = analyzer_from_tag(input, tag)?;
    let custom_search_analyzer = input.get_opt_str()?;
    let norms = match input.get_u8()? {
        0 => None,
        1 => Some(false),
        2 => Some(true),
        other => return Err(input.corrupt(format!("invalid norms tag {other}"))),
    };
    let tag = input.get_u8()?;
    let normalizer = analyzer_from_tag(input, tag)?;
    let index_prefixes = match input.get_u8()? {
        0 => None,
        1 => Some(FieldPrefixes {
            min_chars: input.get_u64()? as usize,
            max_chars: input.get_u64()? as usize,
        }),
        other => return Err(input.corrupt(format!("invalid option tag {other}"))),
    };
    let date_format = input.get_opt_str()?;
    let count = input.get_count(8)?;
    let mut fields = BTreeMap::new();
    for _ in 0..count {
        let name = input.get_str()?;
        fields.insert(name, get_field_mapping(input, depth + 1)?);
    }
    Ok(FieldMapping {
        field_type,
        analyzer,
        custom_analyzer,
        search_analyzer,
        custom_search_analyzer,
        norms,
        normalizer,
        index_prefixes,
        date_format,
        fields,
    })
}

pub(crate) fn put_mapping(out: &mut FramedWriter, mapping: &IndexMapping) -> Result<()> {
    let fields: Vec<(&str, &FieldMapping)> = mapping.fields().collect();
    out.put_u64(fields.len() as u64)?;
    for (name, field) in fields {
        out.put_str(name)?;
        put_field_mapping(out, field)?;
    }
    let analysis = mapping.analysis();
    out.put_u64(analysis.tokenizers.len() as u64)?;
    for (name, tokenizer) in &analysis.tokenizers {
        out.put_str(name)?;
        out.put_u64(tokenizer.min_gram as u64)?;
        out.put_u64(tokenizer.max_gram as u64)?;
        put_strings(out, &tokenizer.token_chars)?;
    }
    out.put_u64(analysis.analyzers.len() as u64)?;
    for (name, analyzer) in &analysis.analyzers {
        out.put_str(name)?;
        out.put_str(&analyzer.tokenizer)?;
        put_strings(out, &analyzer.filter)?;
    }
    out.put_u64(analysis.normalizers.len() as u64)?;
    for (name, normalizer) in &analysis.normalizers {
        out.put_str(name)?;
        out.put_str(&normalizer.kind)?;
        put_strings(out, &normalizer.filter)?;
    }
    Ok(())
}

pub(crate) fn get_mapping(input: &mut FramedReader) -> Result<IndexMapping> {
    let mut mapping = IndexMapping::new();
    let count = input.get_count(8)?;
    for _ in 0..count {
        let name = input.get_str()?;
        let field = get_field_mapping(input, 0)?;
        mapping.set_field_mapping(name, field);
    }
    let mut analysis = AnalysisSettings::default();
    let count = input.get_count(8)?;
    for _ in 0..count {
        let name = input.get_str()?;
        let min_gram = input.get_u64()? as usize;
        let max_gram = input.get_u64()? as usize;
        let token_chars = get_strings(input)?;
        analysis.tokenizers.insert(
            name,
            EdgeNgramTokenizerDefinition {
                min_gram,
                max_gram,
                token_chars,
            },
        );
    }
    let count = input.get_count(8)?;
    for _ in 0..count {
        let name = input.get_str()?;
        let tokenizer = input.get_str()?;
        let filter = get_strings(input)?;
        analysis
            .analyzers
            .insert(name, AnalyzerDefinition { tokenizer, filter });
    }
    let count = input.get_count(8)?;
    for _ in 0..count {
        let name = input.get_str()?;
        let kind = input.get_str()?;
        let filter = get_strings(input)?;
        analysis
            .normalizers
            .insert(name, NormalizerDefinition { kind, filter });
    }
    mapping.set_analysis(analysis);
    Ok(mapping)
}

// ---------------------------------------------------------------------------
// Index manifest
// ---------------------------------------------------------------------------

/// One committed generation of an index. Lists every file that is alive;
/// anything else in the index directory is an orphan of an interrupted
/// persist and is deleted at open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexManifest {
    pub name: String,
    pub generation: u64,
    pub next_file_id: u64,
    /// `META`: mapping, settings, counters, `_source` block directory,
    /// index-wide side postings.
    pub meta: PersistedFile,
    /// `IDMP`: dense uid <-> doc_id maps and packed `_source` slots.
    pub ids: PersistedFile,
    pub segments: Vec<PersistedSegment>,
    /// Valid prefix of the append-only `source.dat`.
    pub source_len: u64,
    /// `(end_offset, blake3 of [previous end, end_offset))`, one entry per
    /// generation that grew `source.dat` — verification stays O(new bytes)
    /// per persist instead of re-hashing the whole store.
    pub source_chunks: Vec<(u64, [u8; 32])>,
}

impl IndexManifest {
    pub(crate) fn write(&self, dir: &Path) -> Result<()> {
        atomic_replace_framed(dir, MANIFEST_NAME, MANIFEST_KIND, FORMAT_VERSION, |out| {
            out.put_str(&self.name)?;
            out.put_u64(self.generation)?;
            out.put_u64(self.next_file_id)?;
            self.meta.encode(out)?;
            self.ids.encode(out)?;
            out.put_u64(self.segments.len() as u64)?;
            for segment in &self.segments {
                segment.encode(out)?;
            }
            out.put_u64(self.source_len)?;
            out.put_u64(self.source_chunks.len() as u64)?;
            for (end, hash) in &self.source_chunks {
                out.put_u64(*end)?;
                out.put_raw(hash)?;
            }
            Ok(())
        })
        .map(|_| ())
    }

    pub(crate) fn read(dir: &Path) -> Result<Self> {
        let mut input =
            FramedReader::open(&dir.join(MANIFEST_NAME), MANIFEST_KIND, FORMAT_VERSION)?;
        let name = input.get_str()?;
        let generation = input.get_u64()?;
        let next_file_id = input.get_u64()?;
        let meta = PersistedFile::decode(&mut input)?;
        let ids = PersistedFile::decode(&mut input)?;
        let count = input.get_count(8)?;
        let mut segments = Vec::with_capacity(count);
        for _ in 0..count {
            segments.push(PersistedSegment::decode(&mut input)?);
        }
        let source_len = input.get_u64()?;
        let count = input.get_count(40)?;
        let mut source_chunks = Vec::with_capacity(count);
        let mut previous = 0u64;
        for _ in 0..count {
            let end = input.get_u64()?;
            let mut hash = [0u8; 32];
            input.get_raw(&mut hash)?;
            if end <= previous {
                return Err(input.corrupt("source chunks are not strictly increasing"));
            }
            previous = end;
            source_chunks.push((end, hash));
        }
        if previous != source_len {
            return Err(input.corrupt("source chunks do not cover the recorded source length"));
        }
        input.finish()?;
        if encode_index_dir_name(&name) != dir.file_name().unwrap_or_default().to_string_lossy() {
            return Err(PersistError::corrupt(
                dir.join(MANIFEST_NAME),
                format!("manifest names index {name:?}, which does not match its directory"),
            ));
        }
        Ok(Self {
            name,
            generation,
            next_file_id,
            meta,
            ids,
            segments,
            source_len,
            source_chunks,
        })
    }

    /// Every file name the manifest keeps alive (besides itself and
    /// `source.dat`).
    pub(crate) fn live_files(&self) -> impl Iterator<Item = &PersistedFile> {
        [&self.meta, &self.ids]
            .into_iter()
            .chain(self.segments.iter().flat_map(PersistedSegment::files))
    }

    /// Verify every immutable data file (`.pst` / `.sub`) and the source
    /// chunks. Framed files (`.seg`, `META`, `IDMP`) are verified while
    /// they are decoded.
    pub(crate) fn verify_data_files(&self, dir: &Path, full: bool) -> Result<()> {
        for segment in &self.segments {
            for file in segment.postings.iter().chain(segment.subfields.iter()) {
                file.verify(dir, full)?;
            }
        }
        let source = dir.join(SOURCE_NAME);
        if self.source_len > 0 {
            let found = fs::metadata(&source)
                .map_err(|error| PersistError::io(&source, error))?
                .len();
            if found < self.source_len {
                return Err(PersistError::LengthMismatch {
                    path: source,
                    expected: self.source_len,
                    found,
                });
            }
            if full {
                let mut start = 0;
                for (end, hash) in &self.source_chunks {
                    if surch_store::persist::hash_file_range(&source, start, *end)? != *hash {
                        return Err(PersistError::ChecksumMismatch { path: source });
                    }
                    start = *end;
                }
            }
        }
        Ok(())
    }

    /// Crash recovery, step 2: delete every file of the index directory the
    /// committed manifest does not reference (segments written by a persist
    /// whose manifest rename never happened, transient `*.tmp.*` files).
    pub(crate) fn collect_garbage(&self, dir: &Path) -> Result<()> {
        let live: std::collections::BTreeSet<&str> =
            self.live_files().map(|file| file.name.as_str()).collect();
        let mut removed = false;
        for entry in read_dir(dir)? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == MANIFEST_NAME || name == SOURCE_NAME || live.contains(name.as_str()) {
                continue;
            }
            fs::remove_file(entry.path())
                .map_err(|source| PersistError::io(entry.path(), source))?;
            removed = true;
        }
        if removed {
            fsync_dir(dir)?;
        }
        Ok(())
    }
}

/// Per-index persistence bookkeeping, owned by `InMemoryIndex` once the
/// index has been persisted at least once (or reopened).
#[derive(Debug)]
pub(crate) struct IndexPersistState {
    pub dir: PathBuf,
    pub generation: u64,
    pub next_file_id: u64,
    pub segment_cache: SegmentPersistCache,
    /// `(dense epoch, file)` of the last persisted id maps — rewritten only
    /// when the dense maps changed since.
    pub ids: Option<(u64, PersistedFile)>,
    pub source_len: u64,
    pub source_chunks: Vec<(u64, [u8; 32])>,
}

impl IndexPersistState {
    pub(crate) fn fresh(dir: PathBuf) -> Self {
        Self {
            dir,
            generation: 0,
            next_file_id: 1,
            segment_cache: SegmentPersistCache::default(),
            ids: None,
            source_len: 0,
            source_chunks: Vec::new(),
        }
    }

    pub(crate) fn file_name(&mut self, extension: &str) -> String {
        let id = self.next_file_id;
        self.next_file_id += 1;
        format!("{extension}-{id:08}.{extension}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_dir_names_are_injective_and_safe() {
        assert_eq!(encode_index_dir_name("deces-2026"), "i-deces-2026");
        assert_eq!(encode_index_dir_name(".."), "i-..");
        assert_eq!(encode_index_dir_name("a/b"), "i-a%2Fb");
        assert_eq!(encode_index_dir_name("A%"), "i-%41%25");
        assert_ne!(encode_index_dir_name("a%2Fb"), encode_index_dir_name("a/b"));
    }
}
