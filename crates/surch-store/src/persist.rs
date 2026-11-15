//! Durable file primitives for the opt-in data directory (`SURCH_DATA_DIR`,
//! plan `plan/mvp-matchid.md` lot L6, design
//! `docs/paper/persistence-iceberg-architecture.md` §1-§2, layout
//! `docs/ops/persistence.md`).
//!
//! Three building blocks, all safe Rust (`#![forbid(unsafe_code)]`):
//!
//! - **Framed files** ([`FramedWriter`] / [`FramedReader`]): every file whose
//!   content is resident-only in RAM today (FST term dictionaries, term
//!   entries, id maps, field stats, live docs, mapping, settings, aliases,
//!   manifests) is written behind a fixed 52-byte header
//!   `magic "SRCH" | kind [u8; 4] | format version u32 LE | payload length
//!   u64 LE | blake3-256 of the payload`. The reader streams the payload,
//!   hashes it on the fly and fails closed on any magic / kind / version /
//!   length / checksum mismatch. Payload decoding is bounded by the declared
//!   length, so a corrupt length prefix can never trigger a huge allocation.
//! - **Atomic replace** ([`atomic_replace_framed`]): write `<name>.tmp.<pid>.<nonce>`,
//!   `fsync` it, `rename(2)` it over `<name>`, `fsync` the directory — the
//!   Iceberg / Lucene `segments_N` pointer pattern. A reader sees the old or
//!   the new file, never a mix.
//! - **Immutable data files** ([`PersistedFile`]): files written by the
//!   existing on-disk encodings (FoR postings segments, sub-field columns,
//!   block `_source`) are referenced from a manifest by name, length and the
//!   blake3 of the referenced byte range; [`PersistedFile::verify`] re-checks
//!   them at open time.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use thiserror::Error;

/// Magic of every framed file.
pub const FRAME_MAGIC: [u8; 4] = *b"SRCH";
/// `magic(4) + kind(4) + version(4) + payload_len(8) + blake3(32)`.
pub const FRAME_HEADER_LEN: u64 = 52;
/// Suffix marker of transient files written before an atomic rename.
pub const TMP_MARKER: &str = ".tmp.";

const IO_BUF: usize = 1 << 20;

/// Every way opening persisted state can fail. The `Display` text is meant to
/// be printed as-is by the server before it refuses to start.
#[derive(Debug, Error)]
pub enum PersistError {
    #[error("persistence I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path}: not a Surch persisted file (bad magic)")]
    BadMagic { path: PathBuf },
    #[error("{path}: file kind {found:?}, expected {expected:?}")]
    KindMismatch {
        path: PathBuf,
        found: String,
        expected: String,
    },
    #[error(
        "{path}: on-disk format version {found}, this binary only reads version {expected} \
         (re-index or use a matching Surch build)"
    )]
    VersionMismatch {
        path: PathBuf,
        found: u32,
        expected: u32,
    },
    #[error("{path}: truncated or resized file (expected {expected} bytes, found {found})")]
    LengthMismatch {
        path: PathBuf,
        expected: u64,
        found: u64,
    },
    #[error("{path}: blake3 checksum mismatch, the file is corrupt")]
    ChecksumMismatch { path: PathBuf },
    #[error("{path}: corrupt payload: {reason}")]
    Corrupt { path: PathBuf, reason: String },
}

impl PersistError {
    pub fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub fn corrupt(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::Corrupt {
            path: path.into(),
            reason: reason.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, PersistError>;

/// `fsync` a directory so a `rename`/`create`/`unlink` inside it is durable.
pub fn fsync_dir(dir: &Path) -> Result<()> {
    File::open(dir)
        .and_then(|handle| handle.sync_all())
        .map_err(|source| PersistError::io(dir, source))
}

/// `true` for the transient name of a not-yet-renamed file
/// (`<name>.tmp.<pid>.<nonce>`), which recovery deletes unconditionally.
pub fn is_tmp_name(name: &str) -> bool {
    name.contains(TMP_MARKER)
}

/// Unique transient sibling of `final_name` inside `dir`.
pub fn tmp_path_for(dir: &Path, final_name: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    dir.join(format!(
        "{final_name}{TMP_MARKER}{}.{nanos}.{seq}",
        std::process::id()
    ))
}

/// Hard-link `src` to `dst` (same filesystem: zero copy, the usual case when
/// the spill directory lives under the data directory) or copy it when the
/// link is refused (`EXDEV`, unsupported filesystem), then `fsync` `dst`.
/// The caller `fsync`s the destination directory once per batch.
pub fn link_or_copy(src: &Path, dst: &Path) -> Result<()> {
    if fs::hard_link(src, dst).is_err() {
        fs::copy(src, dst).map_err(|source| PersistError::io(dst, source))?;
    }
    File::open(dst)
        .and_then(|handle| handle.sync_all())
        .map_err(|source| PersistError::io(dst, source))
}

/// blake3 of `path[start..end)`. Fails with [`PersistError::LengthMismatch`]
/// when the file is shorter than `end`.
pub fn hash_file_range(path: &Path, start: u64, end: u64) -> Result<[u8; 32]> {
    let mut file = File::open(path).map_err(|source| PersistError::io(path, source))?;
    let found = file
        .metadata()
        .map_err(|source| PersistError::io(path, source))?
        .len();
    if found < end {
        return Err(PersistError::LengthMismatch {
            path: path.to_path_buf(),
            expected: end,
            found,
        });
    }
    file.seek(SeekFrom::Start(start))
        .map_err(|source| PersistError::io(path, source))?;
    let mut hasher = blake3::Hasher::new();
    let mut remaining = end.saturating_sub(start);
    let mut buf = vec![0u8; IO_BUF];
    while remaining > 0 {
        let want = remaining.min(IO_BUF as u64) as usize;
        file.read_exact(&mut buf[..want])
            .map_err(|source| PersistError::io(path, source))?;
        hasher.update(&buf[..want]);
        remaining -= want as u64;
    }
    Ok(*hasher.finalize().as_bytes())
}

/// Whether [`PersistedFile::verify`] re-hashes data files at open time.
/// `SURCH_DATA_VERIFY=size` limits the check to existence + length (faster
/// restart on very large indices, at the cost of detecting bit rot only when
/// a page is actually decoded); anything else, including unset, verifies the
/// full blake3.
pub fn full_verification_enabled() -> bool {
    !matches!(
        std::env::var("SURCH_DATA_VERIFY").as_deref(),
        Ok("size") | Ok("SIZE")
    )
}

/// A data file referenced by a manifest: the first `len` bytes of `name`
/// (relative to the index directory) must hash to `hash`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedFile {
    pub name: String,
    pub len: u64,
    pub hash: [u8; 32],
}

impl PersistedFile {
    /// Describe `dir/name` as it is on disk right now (`len` = file length).
    pub fn describe(dir: &Path, name: &str) -> Result<Self> {
        let path = dir.join(name);
        let len = fs::metadata(&path)
            .map_err(|source| PersistError::io(&path, source))?
            .len();
        let hash = hash_file_range(&path, 0, len)?;
        Ok(Self {
            name: name.to_owned(),
            len,
            hash,
        })
    }

    /// Fail closed unless `dir/name` exists, is at least `len` bytes long
    /// and (when `full`) its first `len` bytes hash to `hash`.
    pub fn verify(&self, dir: &Path, full: bool) -> Result<()> {
        let path = dir.join(&self.name);
        let found = fs::metadata(&path)
            .map_err(|source| PersistError::io(&path, source))?
            .len();
        if found < self.len {
            return Err(PersistError::LengthMismatch {
                path,
                expected: self.len,
                found,
            });
        }
        if full && hash_file_range(&path, 0, self.len)? != self.hash {
            return Err(PersistError::ChecksumMismatch { path });
        }
        Ok(())
    }

    pub fn encode(&self, out: &mut FramedWriter) -> Result<()> {
        out.put_str(&self.name)?;
        out.put_u64(self.len)?;
        out.put_raw(&self.hash)
    }

    pub fn decode(input: &mut FramedReader) -> Result<Self> {
        let name = input.get_str()?;
        if name.is_empty() || name.contains('/') || name.contains("..") {
            return Err(input.corrupt(format!("invalid referenced file name {name:?}")));
        }
        let len = input.get_u64()?;
        let mut hash = [0u8; 32];
        input.get_raw(&mut hash)?;
        Ok(Self { name, len, hash })
    }
}

/// Streaming writer of a framed file. The header is written as a placeholder
/// first and patched by [`Self::finish`] once the payload hash is known, so
/// arbitrarily large payloads (id maps at 29 M docs) never need a second
/// in-RAM copy.
pub struct FramedWriter {
    path: PathBuf,
    out: BufWriter<File>,
    hasher: blake3::Hasher,
    payload_len: u64,
    kind: [u8; 4],
    version: u32,
}

impl FramedWriter {
    /// Create (truncating) `path` and reserve its header.
    pub fn create(path: &Path, kind: [u8; 4], version: u32) -> Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(|source| PersistError::io(path, source))?;
        let mut out = BufWriter::with_capacity(IO_BUF, file);
        out.write_all(&[0u8; FRAME_HEADER_LEN as usize])
            .map_err(|source| PersistError::io(path, source))?;
        Ok(Self {
            path: path.to_path_buf(),
            out,
            hasher: blake3::Hasher::new(),
            payload_len: 0,
            kind,
            version,
        })
    }

    pub fn put_raw(&mut self, bytes: &[u8]) -> Result<()> {
        self.hasher.update(bytes);
        self.payload_len += bytes.len() as u64;
        self.out
            .write_all(bytes)
            .map_err(|source| PersistError::io(&self.path, source))
    }

    pub fn put_u8(&mut self, value: u8) -> Result<()> {
        self.put_raw(&[value])
    }

    pub fn put_bool(&mut self, value: bool) -> Result<()> {
        self.put_u8(u8::from(value))
    }

    pub fn put_u32(&mut self, value: u32) -> Result<()> {
        self.put_raw(&value.to_le_bytes())
    }

    pub fn put_u64(&mut self, value: u64) -> Result<()> {
        self.put_raw(&value.to_le_bytes())
    }

    /// Length-prefixed (`u64`) byte string.
    pub fn put_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.put_u64(bytes.len() as u64)?;
        self.put_raw(bytes)
    }

    pub fn put_str(&mut self, value: &str) -> Result<()> {
        self.put_bytes(value.as_bytes())
    }

    pub fn put_opt_str(&mut self, value: Option<&str>) -> Result<()> {
        match value {
            Some(value) => {
                self.put_u8(1)?;
                self.put_str(value)
            }
            None => self.put_u8(0),
        }
    }

    /// Count-prefixed `u32` LE array, encoded in bounded chunks.
    pub fn put_u32s(&mut self, values: &[u32]) -> Result<()> {
        self.put_u64(values.len() as u64)?;
        let mut buf = Vec::with_capacity(IO_BUF.min(values.len() * 4));
        for chunk in values.chunks(IO_BUF / 4) {
            buf.clear();
            for value in chunk {
                buf.extend_from_slice(&value.to_le_bytes());
            }
            self.put_raw(&buf)?;
        }
        Ok(())
    }

    /// Count-prefixed `u64` LE array, encoded in bounded chunks.
    pub fn put_u64s(&mut self, values: &[u64]) -> Result<()> {
        self.put_u64(values.len() as u64)?;
        let mut buf = Vec::with_capacity(IO_BUF.min(values.len() * 8));
        for chunk in values.chunks(IO_BUF / 8) {
            buf.clear();
            for value in chunk {
                buf.extend_from_slice(&value.to_le_bytes());
            }
            self.put_raw(&buf)?;
        }
        Ok(())
    }

    /// Flush the payload, patch the header, `fsync` the file. Returns the
    /// total file length and the payload hash.
    pub fn finish(self) -> Result<(u64, [u8; 32])> {
        let Self {
            path,
            out,
            hasher,
            payload_len,
            kind,
            version,
        } = self;
        let mut file = out
            .into_inner()
            .map_err(|error| PersistError::io(&path, error.into_error()))?;
        let hash = *hasher.finalize().as_bytes();
        let mut header = Vec::with_capacity(FRAME_HEADER_LEN as usize);
        header.extend_from_slice(&FRAME_MAGIC);
        header.extend_from_slice(&kind);
        header.extend_from_slice(&version.to_le_bytes());
        header.extend_from_slice(&payload_len.to_le_bytes());
        header.extend_from_slice(&hash);
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.write_all(&header))
            .and_then(|()| file.sync_all())
            .map_err(|source| PersistError::io(&path, source))?;
        Ok((FRAME_HEADER_LEN + payload_len, hash))
    }
}

/// Streaming reader of a framed file: validates the header up front, hashes
/// the payload while it is decoded, and [`Self::finish`] fails closed unless
/// the whole payload was consumed and its hash matches.
pub struct FramedReader {
    path: PathBuf,
    input: BufReader<File>,
    hasher: blake3::Hasher,
    remaining: u64,
    expected_hash: [u8; 32],
}

impl FramedReader {
    pub fn open(path: &Path, kind: [u8; 4], version: u32) -> Result<Self> {
        let file = File::open(path).map_err(|source| PersistError::io(path, source))?;
        let file_len = file
            .metadata()
            .map_err(|source| PersistError::io(path, source))?
            .len();
        if file_len < FRAME_HEADER_LEN {
            return Err(PersistError::LengthMismatch {
                path: path.to_path_buf(),
                expected: FRAME_HEADER_LEN,
                found: file_len,
            });
        }
        let mut input = BufReader::with_capacity(IO_BUF, file);
        let mut header = [0u8; FRAME_HEADER_LEN as usize];
        input
            .read_exact(&mut header)
            .map_err(|source| PersistError::io(path, source))?;
        if header[0..4] != FRAME_MAGIC {
            return Err(PersistError::BadMagic {
                path: path.to_path_buf(),
            });
        }
        if header[4..8] != kind {
            return Err(PersistError::KindMismatch {
                path: path.to_path_buf(),
                found: String::from_utf8_lossy(&header[4..8]).into_owned(),
                expected: String::from_utf8_lossy(&kind).into_owned(),
            });
        }
        let found_version = u32::from_le_bytes(header[8..12].try_into().expect("4 bytes"));
        if found_version != version {
            return Err(PersistError::VersionMismatch {
                path: path.to_path_buf(),
                found: found_version,
                expected: version,
            });
        }
        let payload_len = u64::from_le_bytes(header[12..20].try_into().expect("8 bytes"));
        if FRAME_HEADER_LEN.checked_add(payload_len) != Some(file_len) {
            return Err(PersistError::LengthMismatch {
                path: path.to_path_buf(),
                expected: FRAME_HEADER_LEN.saturating_add(payload_len),
                found: file_len,
            });
        }
        let mut expected_hash = [0u8; 32];
        expected_hash.copy_from_slice(&header[20..52]);
        Ok(Self {
            path: path.to_path_buf(),
            input,
            hasher: blake3::Hasher::new(),
            remaining: payload_len,
            expected_hash,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Payload bytes not yet consumed.
    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    pub fn corrupt(&self, reason: impl Into<String>) -> PersistError {
        PersistError::corrupt(&self.path, reason)
    }

    pub fn get_raw(&mut self, buf: &mut [u8]) -> Result<()> {
        if buf.len() as u64 > self.remaining {
            return Err(self.corrupt("payload ends before the declared structure"));
        }
        self.input
            .read_exact(buf)
            .map_err(|source| PersistError::io(&self.path, source))?;
        self.hasher.update(buf);
        self.remaining -= buf.len() as u64;
        Ok(())
    }

    pub fn get_u8(&mut self) -> Result<u8> {
        let mut buf = [0u8; 1];
        self.get_raw(&mut buf)?;
        Ok(buf[0])
    }

    pub fn get_bool(&mut self) -> Result<bool> {
        match self.get_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(self.corrupt(format!("invalid boolean tag {other}"))),
        }
    }

    pub fn get_u32(&mut self) -> Result<u32> {
        let mut buf = [0u8; 4];
        self.get_raw(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    pub fn get_u64(&mut self) -> Result<u64> {
        let mut buf = [0u8; 8];
        self.get_raw(&mut buf)?;
        Ok(u64::from_le_bytes(buf))
    }

    /// A count read from the payload, bounded by what the remaining payload
    /// can possibly hold at `unit` bytes per element.
    pub fn get_count(&mut self, unit: u64) -> Result<usize> {
        let count = self.get_u64()?;
        if count.saturating_mul(unit.max(1)) > self.remaining {
            return Err(self.corrupt(format!(
                "declared count {count} exceeds the remaining payload"
            )));
        }
        usize::try_from(count).map_err(|_| self.corrupt("count does not fit usize"))
    }

    pub fn get_bytes(&mut self) -> Result<Vec<u8>> {
        let len = self.get_count(1)?;
        let mut buf = vec![0u8; len];
        self.get_raw(&mut buf)?;
        Ok(buf)
    }

    pub fn get_str(&mut self) -> Result<String> {
        let bytes = self.get_bytes()?;
        String::from_utf8(bytes).map_err(|_| self.corrupt("invalid UTF-8 string"))
    }

    pub fn get_opt_str(&mut self) -> Result<Option<String>> {
        match self.get_u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.get_str()?)),
            other => Err(self.corrupt(format!("invalid option tag {other}"))),
        }
    }

    pub fn get_u32s(&mut self) -> Result<Vec<u32>> {
        let count = self.get_count(4)?;
        let mut values = Vec::with_capacity(count);
        let mut buf = vec![0u8; IO_BUF.min(count * 4)];
        let mut left = count;
        while left > 0 {
            let take = left.min(IO_BUF / 4);
            self.get_raw(&mut buf[..take * 4])?;
            values.extend(
                buf[..take * 4]
                    .chunks_exact(4)
                    .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("4 bytes"))),
            );
            left -= take;
        }
        Ok(values)
    }

    pub fn get_u64s(&mut self) -> Result<Vec<u64>> {
        let count = self.get_count(8)?;
        let mut values = Vec::with_capacity(count);
        let mut buf = vec![0u8; IO_BUF.min(count * 8)];
        let mut left = count;
        while left > 0 {
            let take = left.min(IO_BUF / 8);
            self.get_raw(&mut buf[..take * 8])?;
            values.extend(
                buf[..take * 8]
                    .chunks_exact(8)
                    .map(|chunk| u64::from_le_bytes(chunk.try_into().expect("8 bytes"))),
            );
            left -= take;
        }
        Ok(values)
    }

    /// Fail closed unless the whole payload was consumed and hashes to the
    /// header's checksum.
    pub fn finish(self) -> Result<()> {
        if self.remaining != 0 {
            return Err(PersistError::corrupt(
                &self.path,
                format!("{} trailing payload bytes", self.remaining),
            ));
        }
        if *self.hasher.finalize().as_bytes() != self.expected_hash {
            return Err(PersistError::ChecksumMismatch { path: self.path });
        }
        Ok(())
    }
}

/// Write a framed file under a transient name, `fsync` it, then atomically
/// rename it to `dir/name` and `fsync` `dir`. `fill` streams the payload.
pub fn atomic_replace_framed(
    dir: &Path,
    name: &str,
    kind: [u8; 4],
    version: u32,
    fill: impl FnOnce(&mut FramedWriter) -> Result<()>,
) -> Result<u64> {
    let tmp = tmp_path_for(dir, name);
    let result = (|| {
        let mut writer = FramedWriter::create(&tmp, kind, version)?;
        fill(&mut writer)?;
        let (len, _) = writer.finish()?;
        let target = dir.join(name);
        fs::rename(&tmp, &target).map_err(|source| PersistError::io(&target, source))?;
        fsync_dir(dir)?;
        Ok(len)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Write a brand new framed file `dir/name` (not atomically replaced: the
/// name is unique and only becomes reachable once a manifest references it).
/// Returns its [`PersistedFile`] descriptor (`hash` = payload hash).
pub fn write_framed_file(
    dir: &Path,
    name: &str,
    kind: [u8; 4],
    version: u32,
    fill: impl FnOnce(&mut FramedWriter) -> Result<()>,
) -> Result<PersistedFile> {
    let path = dir.join(name);
    let mut writer = FramedWriter::create(&path, kind, version)?;
    fill(&mut writer)?;
    let (len, hash) = writer.finish()?;
    Ok(PersistedFile {
        name: name.to_owned(),
        len,
        hash,
    })
}

/// Open a framed file referenced by `file` (relative to `dir`): checks the
/// recorded length and payload hash against the header before decoding.
pub fn open_framed_file(
    dir: &Path,
    file: &PersistedFile,
    kind: [u8; 4],
    version: u32,
) -> Result<FramedReader> {
    let path = dir.join(&file.name);
    let reader = FramedReader::open(&path, kind, version)?;
    let found = FRAME_HEADER_LEN + reader.remaining();
    if found != file.len {
        return Err(PersistError::LengthMismatch {
            path,
            expected: file.len,
            found,
        });
    }
    if reader.expected_hash != file.hash {
        return Err(PersistError::ChecksumMismatch { path });
    }
    Ok(reader)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "surch-persist-unit-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_sample(dir: &Path) -> u64 {
        atomic_replace_framed(dir, "MANIFEST", *b"TEST", 3, |out| {
            out.put_str("hello")?;
            out.put_u32s(&[1, 2, 3, u32::MAX])?;
            out.put_u64s(&[7, u64::MAX])?;
            out.put_opt_str(None)?;
            out.put_bool(true)
        })
        .unwrap()
    }

    #[test]
    fn framed_round_trip() {
        let dir = scratch("rt");
        write_sample(&dir);
        let mut input = FramedReader::open(&dir.join("MANIFEST"), *b"TEST", 3).unwrap();
        assert_eq!(input.get_str().unwrap(), "hello");
        assert_eq!(input.get_u32s().unwrap(), vec![1, 2, 3, u32::MAX]);
        assert_eq!(input.get_u64s().unwrap(), vec![7, u64::MAX]);
        assert_eq!(input.get_opt_str().unwrap(), None);
        assert!(input.get_bool().unwrap());
        input.finish().unwrap();
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(leftovers, vec!["MANIFEST".to_owned()]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn framed_rejects_version_kind_truncation_and_bitflip() {
        let dir = scratch("bad");
        write_sample(&dir);
        let path = dir.join("MANIFEST");
        assert!(matches!(
            FramedReader::open(&path, *b"TEST", 4),
            Err(PersistError::VersionMismatch {
                found: 3,
                expected: 4,
                ..
            })
        ));
        assert!(matches!(
            FramedReader::open(&path, *b"NOPE", 3),
            Err(PersistError::KindMismatch { .. })
        ));

        let original = fs::read(&path).unwrap();
        let mut flipped = original.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 0x01;
        fs::write(&path, &flipped).unwrap();
        let mut input = FramedReader::open(&path, *b"TEST", 3).unwrap();
        input.get_str().unwrap();
        input.get_u32s().unwrap();
        input.get_u64s().unwrap();
        input.get_opt_str().unwrap();
        // The flipped byte is the bool tag: decoding or the checksum fails.
        let outcome = input.get_bool().and_then(|_| input.finish());
        assert!(outcome.is_err());

        fs::write(&path, &original[..original.len() - 3]).unwrap();
        assert!(matches!(
            FramedReader::open(&path, *b"TEST", 3),
            Err(PersistError::LengthMismatch { .. })
        ));
        fs::write(
            &path,
            b"garbage-not-a-frame-garbage-not-a-frame-garbage-not-a-frame",
        )
        .unwrap();
        assert!(matches!(
            FramedReader::open(&path, *b"TEST", 3),
            Err(PersistError::BadMagic { .. })
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn persisted_file_detects_truncation_and_corruption() {
        let dir = scratch("data");
        fs::write(dir.join("seg.dat"), vec![42u8; 5000]).unwrap();
        let described = PersistedFile::describe(&dir, "seg.dat").unwrap();
        described.verify(&dir, true).unwrap();
        // Appending past the recorded length is allowed (append-only files).
        let mut grown = vec![42u8; 5000];
        grown.extend_from_slice(b"tail");
        fs::write(dir.join("seg.dat"), &grown).unwrap();
        described.verify(&dir, true).unwrap();
        grown[10] = 0;
        fs::write(dir.join("seg.dat"), &grown).unwrap();
        assert!(matches!(
            described.verify(&dir, true),
            Err(PersistError::ChecksumMismatch { .. })
        ));
        described.verify(&dir, false).unwrap();
        fs::write(dir.join("seg.dat"), vec![42u8; 10]).unwrap();
        assert!(matches!(
            described.verify(&dir, false),
            Err(PersistError::LengthMismatch { .. })
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_count_never_allocates_past_the_payload() {
        let dir = scratch("count");
        atomic_replace_framed(&dir, "F", *b"TEST", 1, |out| out.put_u64(u64::MAX / 2)).unwrap();
        let mut input = FramedReader::open(&dir.join("F"), *b"TEST", 1).unwrap();
        assert!(matches!(
            input.get_u32s(),
            Err(PersistError::Corrupt { .. })
        ));
        fs::remove_dir_all(dir).unwrap();
    }
}
