//! Minimal persistent segment store used as an intermediate WAL sink.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::wal::WalRecord;

pub type Result<T> = std::result::Result<T, SegmentStoreError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentStoreConfig {
    pub keep_recent_segments: usize,
}

impl Default for SegmentStoreConfig {
    fn default() -> Self {
        Self {
            keep_recent_segments: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentMetadata {
    pub file_name: String,
    pub records: usize,
}

#[derive(Debug, Clone, Default)]
pub struct SegmentStore {
    path: PathBuf,
    segments: Vec<SegmentMetadata>,
    next_segment_id: u64,
    _keep_recent_segments: usize,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SegmentStoreError {
    #[error("segment store I/O error: {0}")]
    Io(String),
    #[error("invalid segment file `{file_name}`")]
    InvalidSegmentFile { file_name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedSegment {
    metadata: SegmentMetadata,
    records: Vec<WalRecord>,
}

const SEGMENT_PREFIX: &str = "segment_";
const SEGMENT_SUFFIX: &str = ".json";

impl SegmentStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        Self::open_with_config(path, SegmentStoreConfig::default())
    }

    pub fn open_with_config(path: impl Into<PathBuf>, config: SegmentStoreConfig) -> Result<Self> {
        let path = path.into();
        fs::create_dir_all(&path).map_err(|error| {
            SegmentStoreError::Io(format!(
                "create segment store directory {}: {error}",
                path.display()
            ))
        })?;

        let mut segments = Self::read_segment_metadata(&path)?;
        Self::sort_segments(&mut segments);
        let next_segment_id = Self::next_segment_id(&segments);

        Ok(Self {
            path,
            segments,
            next_segment_id,
            _keep_recent_segments: config.keep_recent_segments,
        })
    }

    pub fn append_entries(&mut self, entries: Vec<WalRecord>) -> Result<Option<SegmentMetadata>> {
        if entries.is_empty() {
            return Ok(None);
        }

        let metadata = SegmentMetadata {
            file_name: self.next_segment_name(),
            records: entries.len(),
        };
        let payload = PersistedSegment {
            metadata: metadata.clone(),
            records: entries,
        };
        let bytes = serde_json::to_vec(&payload).map_err(|_error| {
            SegmentStoreError::InvalidSegmentFile {
                file_name: metadata.file_name.clone(),
            }
        })?;
        let path = self.path.join(&metadata.file_name);
        write_atomic(&path, &bytes)?;

        self.segments.push(metadata.clone());
        Ok(Some(metadata))
    }

    pub fn read_segment_records(&self, metadata: &SegmentMetadata) -> Result<Vec<WalRecord>> {
        let path = self.path.join(&metadata.file_name);
        read_segment_records(&path)
    }

    pub fn merge_oldest_segments(&mut self, count: usize) -> Result<Option<SegmentMetadata>> {
        if count < 2 || self.segments.len() <= 1 {
            return Ok(None);
        }

        let to_merge = count.min(self.segments.len() - 1);
        if to_merge == 0 {
            return Ok(None);
        }

        let merged_records = self.segments[..to_merge]
            .iter()
            .map(|segment| self.read_segment_records(segment))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flat_map(|records| records.into_iter())
            .collect::<Vec<_>>();

        for segment in &self.segments[..to_merge] {
            let _ = fs::remove_file(self.path.join(&segment.file_name));
        }

        let metadata = SegmentMetadata {
            file_name: self.next_segment_name(),
            records: merged_records.len(),
        };
        let payload = PersistedSegment {
            metadata: metadata.clone(),
            records: merged_records,
        };
        let bytes =
            serde_json::to_vec(&payload).map_err(|_| SegmentStoreError::InvalidSegmentFile {
                file_name: metadata.file_name.clone(),
            })?;
        write_atomic(&self.path.join(&metadata.file_name), &bytes)?;

        let mut segments = self.segments.split_off(to_merge);
        segments.insert(0, metadata.clone());
        self.segments = segments;
        Ok(Some(metadata))
    }

    pub fn segments(&self) -> &[SegmentMetadata] {
        &self.segments
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub fn all_records(&self) -> Result<Vec<WalRecord>> {
        self.segments
            .iter()
            .map(|segment| self.read_segment_records(segment))
            .collect::<Result<Vec<_>>>()
            .map(|segments| segments.into_iter().flatten().collect())
    }

    fn read_segment_metadata(path: &Path) -> Result<Vec<SegmentMetadata>> {
        let mut segments = Vec::new();
        let entries = fs::read_dir(path).map_err(|error| {
            SegmentStoreError::Io(format!(
                "read segment directory {}: {error}",
                path.display()
            ))
        })?;

        for entry in entries {
            let entry = entry.map_err(|error| {
                SegmentStoreError::Io(format!(
                    "list segment directory {}: {error}",
                    path.display()
                ))
            })?;
            let path = entry.path();

            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }

            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                return Err(SegmentStoreError::InvalidSegmentFile {
                    file_name: String::from("unknown"),
                });
            };

            if !name.starts_with(SEGMENT_PREFIX) || !name.ends_with(SEGMENT_SUFFIX) {
                continue;
            }

            let file_id = parse_segment_id(name)?;
            let records = read_segment_records(&path)?;
            segments.push((
                file_id,
                SegmentMetadata {
                    file_name: name.to_owned(),
                    records: records.len(),
                },
            ));
        }

        segments.sort_by_key(|(file_id, _)| *file_id);
        Ok(segments.into_iter().map(|(_, metadata)| metadata).collect())
    }

    fn sort_segments(segments: &mut [SegmentMetadata]) {
        segments.sort_by_key(|segment| segment.file_name.clone());
    }

    fn next_segment_id(segments: &[SegmentMetadata]) -> u64 {
        segments
            .iter()
            .filter_map(|segment| parse_segment_id(&segment.file_name).ok())
            .max()
            .unwrap_or(0)
            + 1
    }

    fn next_segment_name(&mut self) -> String {
        let file_name = format!(
            "{SEGMENT_PREFIX}{:012}{SEGMENT_SUFFIX}",
            self.next_segment_id
        );
        self.next_segment_id += 1;
        file_name
    }
}

fn parse_segment_id(name: &str) -> Result<u64> {
    let mut remainder =
        name.strip_prefix(SEGMENT_PREFIX)
            .ok_or_else(|| SegmentStoreError::InvalidSegmentFile {
                file_name: name.to_owned(),
            })?;
    if let Some(stripped) = remainder.strip_suffix(SEGMENT_SUFFIX) {
        remainder = stripped;
    } else {
        return Err(SegmentStoreError::InvalidSegmentFile {
            file_name: name.to_owned(),
        });
    }

    remainder
        .parse::<u64>()
        .map_err(|_| SegmentStoreError::InvalidSegmentFile {
            file_name: name.to_owned(),
        })
}

fn read_segment_records(path: &Path) -> Result<Vec<WalRecord>> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|error| {
            SegmentStoreError::Io(format!("read segment {}: {error}", path.display()))
        })?
        .read_to_end(&mut bytes)
        .map_err(|error| {
            SegmentStoreError::Io(format!("read segment {}: {error}", path.display()))
        })?;

    if bytes.is_empty() {
        return Ok(Vec::new());
    }

    let payload: PersistedSegment =
        serde_json::from_slice(&bytes).map_err(|_| SegmentStoreError::InvalidSegmentFile {
            file_name: path.display().to_string(),
        })?;
    Ok(payload.records)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut path_tmp = path.to_path_buf();
    path_tmp.set_extension("tmp");
    let mut file = File::create(&path_tmp).map_err(|error| {
        SegmentStoreError::Io(format!("create segment {}: {error}", path_tmp.display()))
    })?;
    file.write_all(bytes).map_err(|error| {
        SegmentStoreError::Io(format!("write segment {}: {error}", path_tmp.display()))
    })?;
    file.flush()
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            SegmentStoreError::Io(format!("sync segment {}: {error}", path_tmp.display()))
        })?;
    fs::rename(path_tmp, path).map_err(|error| {
        SegmentStoreError::Io(format!("rename segment {}: {error}", path.display()))
    })?;

    Ok(())
}
