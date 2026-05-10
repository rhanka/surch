use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use surch_store::{
    index_store::{IndexStore, IndexStoreConfig, IndexStoreError},
    segment_store::SegmentStore,
    wal::{WalOperation, WalRecord, WriteAheadLog},
};

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let mut root = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is monotonic enough for test IDs")
        .as_nanos();
    root.push(format!("{prefix}-{nanos}-{}", std::process::id()));
    fs::create_dir_all(&root).expect("create temp dir");
    root
}

fn remove_dir(path: &PathBuf) {
    let _ = fs::remove_dir_all(path);
}

#[test]
fn wal_round_trip_reopens_with_sequence_continuity() {
    let root = unique_temp_dir("surch-wal-roundtrip");

    let mut wal = WriteAheadLog::new(root.join("wal")).expect("create WAL");
    assert!(wal.is_empty());

    let seq1 = wal
        .append_index("products", "doc-1", b"{\"title\":\"first\"}".to_vec())
        .expect("append first index op");
    let seq2 = wal
        .append_delete("products", "doc-1")
        .expect("append delete op");
    let seq3 = wal
        .append_index("products", "doc-2", b"{\"title\":\"second\"}".to_vec())
        .expect("append second index op");

    assert_eq!(seq1, 1);
    assert_eq!(seq2, 2);
    assert_eq!(seq3, 3);
    assert_eq!(wal.len(), 3);

    wal.flush().expect("flush WAL");
    assert_eq!(wal.len(), 3);

    let mut reopened = WriteAheadLog::new(root.join("wal")).expect("re-open WAL");
    assert_eq!(reopened.len(), 3);

    let entries = reopened.entries();
    assert_eq!(entries[0].sequence, 1);
    assert_eq!(entries[1].sequence, 2);
    assert_eq!(entries[2].sequence, 3);
    assert_eq!(
        entries[0].operation,
        WalOperation::Index {
            source: b"{\"title\":\"first\"}".to_vec()
        }
    );
    assert_eq!(entries[1].operation, WalOperation::Delete);

    let append_after_reopen_seq = reopened
        .append_index("products", "doc-3", b"{\"title\":\"third\"}".to_vec())
        .expect("append after reopen");
    assert_eq!(append_after_reopen_seq, 4);

    remove_dir(&root);
}

#[test]
fn wal_corruption_detected_on_open() {
    let root = unique_temp_dir("surch-wal-corrupt");
    let mut wal = WriteAheadLog::new(root.join("wal")).expect("create WAL");

    wal.append_index("products", "doc-1", b"ok".to_vec())
        .expect("append index");
    wal.flush().expect("flush WAL");
    drop(wal);

    let mut path = root.join("wal");
    path.push("wal.log");
    let mut bytes = fs::read(&path).expect("read WAL file");
    assert!(!bytes.is_empty());
    bytes.truncate(bytes.len().saturating_sub(1));
    fs::write(&path, &bytes).expect("corrupt WAL file");

    let reopen = WriteAheadLog::new(root.join("wal"));
    assert!(reopen.is_err(), "corrupted WAL file must fail to reopen");

    remove_dir(&root);
}

#[test]
fn segment_store_persists_segments_and_reloads() {
    let root = unique_temp_dir("surch-segment-store");
    let mut segment_store = SegmentStore::open(root.join("segments")).expect("open segment store");

    let source_ops = vec![
        WalRecord {
            sequence: 1,
            timestamp_millis: 10_000,
            index: "products".to_owned(),
            doc_id: "doc-1".to_owned(),
            operation: WalOperation::Index {
                source: b"{\"title\":\"one\"}".to_vec(),
            },
        },
        WalRecord {
            sequence: 2,
            timestamp_millis: 11_000,
            index: "products".to_owned(),
            doc_id: "doc-2".to_owned(),
            operation: WalOperation::Index {
                source: b"{\"title\":\"two\"}".to_vec(),
            },
        },
        WalRecord {
            sequence: 3,
            timestamp_millis: 12_000,
            index: "products".to_owned(),
            doc_id: "doc-2".to_owned(),
            operation: WalOperation::Delete,
        },
    ];

    let created = segment_store
        .append_entries(source_ops.clone())
        .expect("append source operations");
    assert!(created.is_some());
    let first = created.unwrap();

    assert_eq!(segment_store.segments(), std::slice::from_ref(&first));

    let replayed_records = segment_store
        .read_segment_records(&first)
        .expect("read segment records");
    assert_eq!(replayed_records, source_ops);

    let reopened = SegmentStore::open(root.join("segments")).expect("re-open segment store");
    let segments = reopened.segments();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0], first);

    let reopened_records = reopened
        .read_segment_records(&segments[0])
        .expect("read reopened segment records");
    assert_eq!(reopened_records, source_ops);

    remove_dir(&root);
}

#[test]
fn segment_store_performs_minimal_merge_of_old_segments() {
    let root = unique_temp_dir("surch-segment-merge");
    let mut segment_store = SegmentStore::open(root.join("segments")).expect("open segment store");

    let mut sequence = 1_u64;
    for index in 0..3 {
        let entry = WalRecord {
            sequence,
            timestamp_millis: 10_000 + index as u64 * 100,
            index: "products".to_owned(),
            doc_id: format!("doc-{index}"),
            operation: WalOperation::Index {
                source: format!("{{\"title\":\"{index}\"}}").as_bytes().to_vec(),
            },
        };
        sequence += 1;

        let _ = segment_store
            .append_entries(vec![entry])
            .expect("append one-segment batch")
            .expect("segment created");
    }

    assert_eq!(segment_store.segments().len(), 3);

    let merged = segment_store
        .merge_oldest_segments(2)
        .expect("merge oldest segments");
    assert!(merged.is_some());

    let segments = segment_store.segments();
    assert_eq!(segments.len(), 2);

    let mut ids = HashSet::new();
    for segment in segments {
        ids.insert(segment.file_name.clone());
    }
    assert_eq!(ids.len(), 2);

    let total_records = segments
        .iter()
        .map(|segment| {
            segment_store
                .read_segment_records(segment)
                .expect("read merged segment records")
                .len() as u64
        })
        .sum::<u64>();
    assert_eq!(total_records, 3);

    remove_dir(&root);
}

#[test]
fn index_store_round_trips_state_and_respects_merge_policy() -> Result<(), IndexStoreError> {
    let root = unique_temp_dir("surch-index-store");

    let config = IndexStoreConfig {
        merge_threshold: 3,
        keep_recent_segments: 1,
    };

    let mut store = IndexStore::open(root.clone(), config.clone())?;

    let _ = store.append_index("products", "doc-1", b"{\"title\":\"one\"}".to_vec())?;
    let _ = store.append_delete("products", "doc-1")?;
    store.flush_wal()?;

    let _ = store.append_index("products", "doc-2", b"{\"title\":\"two\"}".to_vec())?;
    let _ = store.append_index("products", "doc-3", b"{\"title\":\"three\"}".to_vec())?;
    let merged = store.flush_wal()?;
    assert!(merged.is_some());
    assert_eq!(store.segment_count(), 2);

    drop(store);

    let reopened = IndexStore::open(root.clone(), config)?;
    assert_eq!(reopened.segment_count(), 2);
    let records = reopened.all_segment_records()?;
    let unique_sequences: HashSet<u64> = records.iter().map(|entry| entry.sequence).collect();
    assert_eq!(unique_sequences.len(), 4);

    remove_dir(&root);
    Ok(())
}

#[test]
fn index_store_recovers_stale_wal_entries_after_restart() -> Result<(), IndexStoreError> {
    let root = unique_temp_dir("surch-index-store-recover-stale");
    let config = IndexStoreConfig {
        merge_threshold: 8,
        keep_recent_segments: 1,
    };

    {
        let mut store = IndexStore::open(root.clone(), config.clone())?;
        store.append_index("products", "doc-1", b"{\"title\":\"one\"}".to_vec())?;
        store.append_index("products", "doc-2", b"{\"title\":\"two\"}".to_vec())?;
        store.flush_wal()?;
    }

    let wal_before_restart = WriteAheadLog::new(root.join("wal"))?;
    assert_eq!(wal_before_restart.len(), 2);

    {
        let reopened = IndexStore::open(root.clone(), config.clone())?;
        let records = reopened.all_segment_records()?;
        let mut sequences = records
            .iter()
            .map(|record| record.sequence)
            .collect::<Vec<_>>();
        sequences.sort_unstable();
        assert_eq!(sequences, vec![1, 2]);

        let wal_after_restart = WriteAheadLog::new(root.join("wal"))?;
        assert_eq!(wal_after_restart.len(), 0);
        assert_eq!(reopened.segment_count(), 1);
    }

    remove_dir(&root);
    Ok(())
}

#[test]
fn index_store_replays_pending_wal_entries() -> Result<(), IndexStoreError> {
    let root = unique_temp_dir("surch-index-store-replay-pending");
    let config = IndexStoreConfig {
        merge_threshold: 8,
        keep_recent_segments: 1,
    };

    {
        let mut store = IndexStore::open(root.clone(), config.clone())?;
        store.append_index("products", "doc-1", b"{\"title\":\"one\"}".to_vec())?;
        store.flush_wal()?;
    }

    let mut pending_wal = WriteAheadLog::new(root.join("wal"))?;
    pending_wal.append_index("products", "doc-2", b"{\"title\":\"two\"}".to_vec())?;
    pending_wal.flush()?;

    {
        let reopened = IndexStore::open(root.clone(), config.clone())?;
        let records = reopened.all_segment_records()?;
        let mut sequences = records
            .iter()
            .map(|record| record.sequence)
            .collect::<Vec<_>>();
        sequences.sort_unstable();
        assert_eq!(sequences, vec![1, 2]);
        let wal_after_restart = WriteAheadLog::new(root.join("wal"))?;
        assert_eq!(wal_after_restart.len(), 0);
    }

    remove_dir(&root);
    Ok(())
}

#[test]
fn segment_store_returns_stable_error_on_invalid_identifier() {
    let root = unique_temp_dir("surch-store-invalid-id");
    let mut wal = WriteAheadLog::new(root.join("wal")).expect("create WAL");

    assert!(wal.append_index("products", "", b"{}".to_vec()).is_err());
    assert!(wal.append_delete("", "doc-1").is_err());

    remove_dir(&root);
}
