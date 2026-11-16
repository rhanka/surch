//! L6.2 — snapshot format v2: the sealed segment files of an L6 index
//! directory, uploaded as is and content-addressed by blake3
//! (`docs/ops/snapshot-v2.md`).
//!
//! Repository layout added to the v1 one:
//!
//! ```text
//! {root}/
//!   blobs/<blake3 hex>                 one immutable object per distinct content
//!   indices/{index_uuid}/meta-{snap}.dat   IndexSnapshotMetadata, format_version 2,
//!                                          with the `segments` file list below
//! ```
//!
//! A blob is the exact bytes of one file of the index directory
//! (`MANIFEST`, `meta-*.meta`, `ids-*.ids`, `seg-*.seg`, `seg-*.pst`,
//! `seg-*.sub`) or one committed chunk of `source.dat` (the range one
//! `_refresh` appended). The key is the blake3 of those bytes, so a file or
//! chunk already uploaded by an earlier snapshot — of any index — is not
//! uploaded again, and the deletion of a snapshot removes only the blobs no
//! remaining snapshot references.
//!
//! Integrity: at snapshot time every data file (`.pst` / `.sub`) and every
//! `source.dat` chunk is re-hashed and compared with the hash its committed
//! `MANIFEST` records (bit rot fails the snapshot); at restore time every
//! downloaded blob is hashed while it is written and compared with its key
//! and length before the index is opened.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use surch_store::persist::{fsync_dir, hash_file_range};

use super::repository::{RepositoryError, SnapshotRepository};
use super::service::{ServiceResult, SnapshotServiceError};
use crate::state::PinnedIndex;

/// `segments.format` of a v2 index snapshot.
pub const SEGMENT_SNAPSHOT_FORMAT: &str = "surch-segments-v2";
/// Prefix of the content-addressed blobs.
pub const BLOB_PREFIX: &str = "blobs/";
/// Name of the `_source` store inside an index directory.
const SOURCE_FILE: &str = "source.dat";

/// One file of the index directory, stored as the blob `blobs/<blake3>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobFile {
    pub name: String,
    pub len: u64,
    pub blake3: String,
}

/// One committed chunk `[start, end)` of `source.dat`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceChunk {
    pub start: u64,
    pub end: u64,
    pub blake3: String,
}

/// The `segments` block of a v2 [`super::service::IndexSnapshotMetadata`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentSnapshot {
    pub format: String,
    /// `MANIFEST` first, then `META`, `IDMP` and the segment files.
    pub files: Vec<BlobFile>,
    pub source: Vec<SourceChunk>,
    pub source_len: u64,
    pub doc_count: u64,
}

impl SegmentSnapshot {
    /// Every blob key this snapshot needs.
    pub fn blob_keys(&self) -> impl Iterator<Item = String> + '_ {
        self.files
            .iter()
            .map(|file| blob_key(&file.blake3))
            .chain(self.source.iter().map(|chunk| blob_key(&chunk.blake3)))
    }

    /// Total bytes of the snapshotted index directory.
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.len).sum::<u64>() + self.source_len
    }
}

/// Upload accounting of one snapshot (`snapshot.surch` in the response).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct UploadStats {
    pub files_uploaded: u64,
    pub files_reused: u64,
    pub bytes_uploaded: u64,
    pub bytes_reused: u64,
}

pub fn blob_key(hex: &str) -> String {
    format!("{BLOB_PREFIX}{hex}")
}

fn hex(hash: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in hash {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn integrity(message: String) -> SnapshotServiceError {
    SnapshotServiceError::Integrity(message)
}

/// Upload `path[start, end)` as the blob of hash `hash` unless it exists.
fn upload_range(
    repo: &dyn SnapshotRepository,
    path: &Path,
    start: u64,
    end: u64,
    hash: &str,
    stats: &mut UploadStats,
) -> ServiceResult<()> {
    let key = blob_key(hash);
    let len = end - start;
    if repo.object_exists(&key)? {
        stats.files_reused += 1;
        stats.bytes_reused += len;
        return Ok(());
    }
    repo.put_file_range(&key, path, start, len)?;
    stats.files_uploaded += 1;
    stats.bytes_uploaded += len;
    Ok(())
}

/// Upload a pinned index generation; returns its file list.
pub fn upload_pinned(
    repo: &dyn SnapshotRepository,
    pinned: &PinnedIndex,
    stats: &mut UploadStats,
) -> ServiceResult<SegmentSnapshot> {
    let persist = |error: surch_store::persist::PersistError| integrity(error.to_string());
    let mut files = Vec::with_capacity(pinned.files.len());
    for file in &pinned.files {
        let path = pinned.dir.join(&file.name);
        let hash = hash_file_range(&path, 0, file.len).map_err(persist)?;
        if let Some(expected) = file.expected_hash {
            if expected != hash {
                return Err(integrity(format!(
                    "{}: content does not match the blake3 its MANIFEST records; \
                     the index directory is corrupt, refusing to snapshot it",
                    file.name
                )));
            }
        }
        let hash = hex(&hash);
        upload_range(repo, &path, 0, file.len, &hash, stats)?;
        files.push(BlobFile {
            name: file.name.clone(),
            len: file.len,
            blake3: hash,
        });
    }
    let source_path = pinned.dir.join(SOURCE_FILE);
    let mut source = Vec::with_capacity(pinned.source_chunks.len());
    let mut start = 0u64;
    for (end, recorded) in &pinned.source_chunks {
        let hash = hash_file_range(&source_path, start, *end).map_err(persist)?;
        if hash != *recorded {
            return Err(integrity(format!(
                "{SOURCE_FILE}[{start}..{end}): content does not match the blake3 its MANIFEST \
                 records; the index directory is corrupt, refusing to snapshot it"
            )));
        }
        let hash = hex(&hash);
        upload_range(repo, &source_path, start, *end, &hash, stats)?;
        source.push(SourceChunk {
            start,
            end: *end,
            blake3: hash,
        });
        start = *end;
    }
    if start != pinned.source_len {
        return Err(integrity(format!(
            "{SOURCE_FILE}: chunks cover {start} bytes, manifest records {}",
            pinned.source_len
        )));
    }
    Ok(SegmentSnapshot {
        format: SEGMENT_SNAPSHOT_FORMAT.to_owned(),
        files,
        source,
        source_len: pinned.source_len,
        doc_count: pinned.doc_count,
    })
}

/// `Write` adapter hashing and counting what goes through.
struct HashingWriter<'a> {
    inner: &'a mut std::io::BufWriter<File>,
    hasher: blake3::Hasher,
    written: u64,
}

impl Write for HashingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Stream blob `hash` into `out`, then check its length and blake3.
fn download_blob(
    repo: &dyn SnapshotRepository,
    hash: &str,
    expected_len: u64,
    what: &str,
    out: &mut std::io::BufWriter<File>,
) -> ServiceResult<()> {
    let mut writer = HashingWriter {
        inner: out,
        hasher: blake3::Hasher::new(),
        written: 0,
    };
    match repo.get_object_to_writer(&blob_key(hash), &mut writer) {
        Ok(_) => {}
        Err(RepositoryError::NotFound { key }) => {
            return Err(integrity(format!(
                "{what}: blob [{key}] is missing from the repository"
            )))
        }
        Err(other) => return Err(other.into()),
    }
    let found = hex(writer.hasher.finalize().as_bytes());
    if writer.written != expected_len || found != hash {
        return Err(integrity(format!(
            "{what}: downloaded {} bytes with blake3 {found}, snapshot records {expected_len} \
             bytes with blake3 {hash}",
            writer.written
        )));
    }
    Ok(())
}

fn sync_file(out: std::io::BufWriter<File>, path: &Path) -> ServiceResult<()> {
    let file = out
        .into_inner()
        .map_err(|error| integrity(format!("{}: {}", path.display(), error.error())))?;
    file.sync_all()
        .map_err(|error| integrity(format!("{}: {error}", path.display())))
}

/// Download every file of `snapshot` into the (empty) directory `dir`,
/// verified blob by blob. `dir` is a restore staging directory: on error
/// the caller removes it.
pub fn download_into(
    repo: &dyn SnapshotRepository,
    snapshot: &SegmentSnapshot,
    dir: &Path,
) -> ServiceResult<()> {
    if snapshot.format != SEGMENT_SNAPSHOT_FORMAT {
        return Err(integrity(format!(
            "unsupported segment snapshot format [{}] (this node reads [{SEGMENT_SNAPSHOT_FORMAT}])",
            snapshot.format
        )));
    }
    let create = |name: &str| -> ServiceResult<std::io::BufWriter<File>> {
        if name.is_empty() || name.contains('/') || name.contains("..") || name == SOURCE_FILE {
            return Err(integrity(format!("invalid file name [{name}] in snapshot")));
        }
        let path = dir.join(name);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map(|file| std::io::BufWriter::with_capacity(1 << 20, file))
            .map_err(|error| integrity(format!("{}: {error}", path.display())))
    };
    for file in &snapshot.files {
        let mut out = create(&file.name)?;
        download_blob(repo, &file.blake3, file.len, &file.name, &mut out)?;
        sync_file(out, &dir.join(&file.name))?;
    }
    let source_path = dir.join(SOURCE_FILE);
    let source_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&source_path)
        .map_err(|error| integrity(format!("{}: {error}", source_path.display())))?;
    let mut out = std::io::BufWriter::with_capacity(1 << 20, source_file);
    let mut start = 0u64;
    for chunk in &snapshot.source {
        if chunk.start != start || chunk.end <= chunk.start {
            return Err(integrity(format!(
                "{SOURCE_FILE}: chunk [{}..{}) does not follow offset {start}",
                chunk.start, chunk.end
            )));
        }
        let what = format!("{SOURCE_FILE}[{}..{})", chunk.start, chunk.end);
        download_blob(
            repo,
            &chunk.blake3,
            chunk.end - chunk.start,
            &what,
            &mut out,
        )?;
        start = chunk.end;
    }
    if start != snapshot.source_len {
        return Err(integrity(format!(
            "{SOURCE_FILE}: chunks cover {start} bytes, snapshot records {}",
            snapshot.source_len
        )));
    }
    sync_file(out, &source_path)?;
    fsync_dir(dir).map_err(|error| integrity(error.to_string()))
}
