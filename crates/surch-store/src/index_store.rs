//! In-memory API façade over WAL + immutable on-disk segments.

use std::path::PathBuf;

use thiserror::Error;

use crate::segment_store::{SegmentMetadata, SegmentStore, SegmentStoreConfig, SegmentStoreError};
use crate::wal::WriteAheadLog;
use crate::wal::{WalError, WalRecord};

pub type Result<T> = std::result::Result<T, IndexStoreError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStoreConfig {
    pub merge_threshold: usize,
    pub keep_recent_segments: usize,
}

impl Default for IndexStoreConfig {
    fn default() -> Self {
        Self {
            merge_threshold: 8,
            keep_recent_segments: 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IndexStore {
    wal: WriteAheadLog,
    segments: SegmentStore,
    config: IndexStoreConfig,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum IndexStoreError {
    #[error("index store I/O error: {0}")]
    Io(String),
    #[error(transparent)]
    Wal(#[from] WalError),
    #[error(transparent)]
    Segment(#[from] SegmentStoreError),
}

impl IndexStore {
    pub fn open(root: impl Into<PathBuf>, config: IndexStoreConfig) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|error| {
            IndexStoreError::Io(format!("create index store {}: {error}", root.display()))
        })?;

        let wal = WriteAheadLog::new(root.join("wal")).map_err(IndexStoreError::Wal)?;
        let segments = SegmentStore::open_with_config(
            root.join("segments"),
            SegmentStoreConfig {
                keep_recent_segments: config.keep_recent_segments,
            },
        )
        .map_err(IndexStoreError::Segment)?;

        let mut store = Self {
            wal,
            segments,
            config,
        };
        store.recover_from_disk()?;
        Ok(store)
    }

    pub fn append_index(&mut self, index: &str, id: &str, source: Vec<u8>) -> Result<u64> {
        self.wal
            .append_index(index, id, source)
            .map_err(IndexStoreError::Wal)
    }

    pub fn append_delete(&mut self, index: &str, id: &str) -> Result<u64> {
        self.wal
            .append_delete(index, id)
            .map_err(IndexStoreError::Wal)
    }

    pub fn flush_wal(&mut self) -> Result<Option<SegmentMetadata>> {
        self.wal.flush().map_err(IndexStoreError::Wal)?;
        let entries = self.wal.drain_entries();
        if entries.is_empty() {
            return Ok(None);
        }

        let mut merged = self.segments.append_entries(entries)?;
        if self.segments.segment_count() > self.config.merge_threshold {
            let to_merge = self
                .segments
                .segment_count()
                .saturating_sub(self.config.keep_recent_segments)
                .max(1);
            let merge_count = to_merge.min(self.segments.segment_count().saturating_sub(1));
            if merge_count > 0 {
                merged = self
                    .segments
                    .merge_oldest_segments(merge_count)
                    .map_err(IndexStoreError::Segment)?
                    .or(merged);
            }
        }

        Ok(merged)
    }

    pub fn segment_count(&self) -> usize {
        self.segments.segment_count()
    }

    pub fn all_segment_records(&self) -> Result<Vec<WalRecord>> {
        self.segments
            .all_records()
            .map_err(IndexStoreError::Segment)
    }

    fn recover_from_disk(&mut self) -> Result<()> {
        let persisted_records = self
            .segments
            .all_records()
            .map_err(IndexStoreError::Segment)?;
        let mut max_persisted_sequence = persisted_records.iter().map(|entry| entry.sequence).max();

        let wal_records = self.wal.entries();
        let replay_records = match max_persisted_sequence {
            Some(max_sequence) => wal_records
                .iter()
                .filter(|record| record.sequence > max_sequence)
                .cloned()
                .collect::<Vec<_>>(),
            None => wal_records.to_vec(),
        };

        if !replay_records.is_empty() {
            if let Some(max_replay_sequence) =
                replay_records.iter().map(|entry| entry.sequence).max()
            {
                max_persisted_sequence = Some(max_replay_sequence);
            }
            self.segments
                .append_entries(replay_records)
                .map_err(IndexStoreError::Segment)?;
        }

        self.wal
            .retain_entries_greater_than(max_persisted_sequence.unwrap_or(0))?;
        Ok(())
    }
}
