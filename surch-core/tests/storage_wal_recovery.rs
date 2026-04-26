use surch_core::common::{Document, FieldValue};
use surch_core::storage::{WalOperation, WriteAheadLog};

#[test]
fn wal_recovery_reloads_entries_from_disk() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let wal = WriteAheadLog::new(temp_dir.path()).expect("create wal");

    wal.append(
        "books",
        "doc-1",
        WalOperation::Index(
            Document::new("doc-1").with_field("title", FieldValue::Text("Recovered".to_string())),
        ),
    )
    .expect("append entry");

    wal.flush().expect("flush wal");

    let reopened = WriteAheadLog::new(temp_dir.path()).expect("reopen wal");
    let entries = reopened.read_all();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].index, "books");
    assert_eq!(entries[0].doc_id, "doc-1");
}
