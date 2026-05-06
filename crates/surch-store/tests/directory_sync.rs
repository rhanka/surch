use surch_store::directory::{DirectoryError, MemoryDirectory};

const SYNC_MANIFEST: &str =
    include_str!("../../../tests/lucene_parity/store/directory_sync_manifest.tsv");

#[test]
fn memory_directory_sync_tracks_file_state_from_manifest_sequence() {
    let mut directory = MemoryDirectory::new();

    for step in sync_manifest_steps() {
        match step {
            SyncStep::Write { name, bytes } => {
                directory
                    .write_file(name, bytes)
                    .expect("write manifest file");
                assert!(!directory.is_synced(name).expect("sync state"));
                assert!(!directory.is_metadata_synced());
            }
            SyncStep::Sync { name } => {
                directory.sync([name]).expect("sync manifest file");
                assert!(directory.is_synced(name).expect("sync state"));
            }
            SyncStep::MetaSync => {
                directory.sync_meta_data();
                assert!(directory.is_metadata_synced());
            }
            SyncStep::Rename { source, target } => {
                directory
                    .rename(source, target)
                    .expect("rename manifest file");
                assert!(!directory.contains_file(source).expect("source absent"));
                assert!(!directory
                    .is_synced(source)
                    .expect("source sync state removed"));
                assert!(!directory.is_synced(target).expect("target sync state"));
                assert!(!directory.is_metadata_synced());
            }
            SyncStep::Delete { name } => {
                directory.delete_file(name).expect("delete manifest file");
                assert!(!directory.contains_file(name).expect("deleted absent"));
                assert!(!directory
                    .is_synced(name)
                    .expect("deleted sync state removed"));
                assert!(!directory.is_metadata_synced());
            }
        }
    }
}

#[test]
fn memory_directory_sync_rejects_missing_files_without_marking_batch() {
    let mut directory = MemoryDirectory::new();

    directory
        .write_file("_0.si", b"segment info")
        .expect("write file");

    assert!(matches!(
        directory.sync(["_0.si", "_missing.si"]),
        Err(DirectoryError::FileNotFound { name }) if name == "_missing.si"
    ));
    assert!(!directory.is_synced("_0.si").expect("sync state"));
}

#[test]
fn memory_directory_sync_rejects_empty_names() {
    let mut directory = MemoryDirectory::new();

    directory
        .write_file("_0.si", b"segment info")
        .expect("write file");

    assert!(matches!(
        directory.sync([""]),
        Err(DirectoryError::InvalidEmptyName)
    ));
    assert!(matches!(
        directory.is_synced(""),
        Err(DirectoryError::InvalidEmptyName)
    ));
}

enum SyncStep<'a> {
    Write { name: &'a str, bytes: &'a [u8] },
    Sync { name: &'a str },
    MetaSync,
    Rename { source: &'a str, target: &'a str },
    Delete { name: &'a str },
}

fn sync_manifest_steps() -> Vec<SyncStep<'static>> {
    SYNC_MANIFEST
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let columns = line.split('\t').collect::<Vec<_>>();
            match columns.as_slice() {
                ["write", name, bytes] => SyncStep::Write {
                    name,
                    bytes: bytes.as_bytes(),
                },
                ["sync", name] => SyncStep::Sync { name },
                ["sync_meta_data"] => SyncStep::MetaSync,
                ["rename", source, target] => SyncStep::Rename { source, target },
                ["delete", name] => SyncStep::Delete { name },
                _ => panic!("invalid sync manifest line: {line}"),
            }
        })
        .collect()
}
