use surch_store::directory::{DirectoryError, MemoryDirectory};
use surch_store::index_io::{ByteArrayIndexOutput, IndexInput, IndexOutput};

const CLASSIC_MANIFEST: &str =
    include_str!("../../../tests/lucene_parity/store/directory_classic_manifest.tsv");
const INDEX_IO_MANIFEST: &str =
    include_str!("../../../tests/lucene_parity/store/directory_io_manifest.tsv");

#[test]
fn memory_directory_writes_reads_and_lists_classic_manifest_files() {
    let mut directory = MemoryDirectory::new();
    let entries = classic_manifest_entries();

    for (name, length) in &entries {
        directory
            .write_file(name, &fixture_bytes(name, *length))
            .expect("write manifest file");
    }

    let expected_names = entries
        .iter()
        .map(|(name, _)| name.to_string())
        .collect::<Vec<_>>();

    assert_eq!(directory.list_all(), expected_names);

    for (name, length) in entries {
        assert_eq!(
            directory.file_length(name).expect("manifest file length"),
            length as u64
        );
        assert!(directory
            .contains_file(name)
            .expect("manifest file contains check"));
        assert_eq!(
            directory.read_file(name).expect("read manifest file"),
            fixture_bytes(name, length)
        );
    }
}

#[test]
fn memory_directory_writes_index_output_and_reads_index_input_manifest_files() {
    let mut directory = MemoryDirectory::new();

    for (name, bytes) in index_io_manifest_entries() {
        let mut output = directory.create_output(name).expect("create index output");
        output.write_bytes(bytes).expect("write manifest bytes");
        let checksum = output.checksum();

        directory
            .write_output(name, output)
            .expect("persist index output");

        assert_eq!(
            directory.file_length(name).expect("index output length"),
            bytes.len() as u64
        );

        let mut input = directory.open_input(name).expect("open index input");
        assert_eq!(input.length(), bytes.len() as u64);

        let mut actual = vec![0; bytes.len()];
        input.read_bytes(&mut actual).expect("read index input");

        assert_eq!(actual, bytes);
        assert_eq!(input.file_pointer(), bytes.len() as u64);

        let mut replay = ByteArrayIndexOutput::new();
        replay.write_bytes(&actual).expect("replay persisted bytes");
        assert_eq!(replay.checksum(), checksum);
    }
}

#[test]
fn memory_directory_open_input_seeks_within_persisted_output() {
    let mut directory = MemoryDirectory::new();
    let (name, bytes) = index_io_manifest_entries()
        .into_iter()
        .find(|(name, _)| *name == "_0.si")
        .expect("manifest contains segment info");

    let mut output = directory.create_output(name).expect("create index output");
    output.write_bytes(bytes).expect("write manifest bytes");
    directory
        .write_output(name, output)
        .expect("persist index output");

    let mut input = directory.open_input(name).expect("open index input");
    input.seek(8).expect("seek into segment info");

    assert_eq!(input.file_pointer(), 8);
    assert_eq!(input.read_byte().expect("read after seek"), bytes[8]);
    assert_eq!(input.file_pointer(), 9);
}

#[test]
fn memory_directory_open_input_reports_file_not_found() {
    let directory = MemoryDirectory::new();

    assert!(matches!(
        directory.open_input("_missing.si"),
        Err(DirectoryError::FileNotFound { name }) if name == "_missing.si"
    ));
}

#[test]
fn memory_directory_rename_moves_file() {
    let mut directory = MemoryDirectory::new();

    directory
        .write_file("_0.fnm", b"field metadata")
        .expect("write source file");

    directory
        .rename("_0.fnm", "_0.fdx")
        .expect("rename source file");

    assert!(!directory
        .contains_file("_0.fnm")
        .expect("source contains check"));
    assert_eq!(
        directory.read_file("_0.fdx").expect("read renamed file"),
        b"field metadata"
    );
}

#[test]
fn memory_directory_delete_removes_file() {
    let mut directory = MemoryDirectory::new();

    directory
        .write_file("segments_1", b"commit")
        .expect("write file");

    directory.delete_file("segments_1").expect("delete file");

    assert_eq!(directory.list_all(), Vec::<String>::new());
    assert!(!directory
        .contains_file("segments_1")
        .expect("deleted contains check"));
    assert!(matches!(
        directory.read_file("segments_1"),
        Err(DirectoryError::FileNotFound { name }) if name == "segments_1"
    ));
}

#[test]
fn memory_directory_reports_file_not_found() {
    let mut directory = MemoryDirectory::new();

    assert!(matches!(
        directory.read_file("_0.si"),
        Err(DirectoryError::FileNotFound { name }) if name == "_0.si"
    ));
    assert!(matches!(
        directory.file_length("_0.si"),
        Err(DirectoryError::FileNotFound { name }) if name == "_0.si"
    ));
    assert!(matches!(
        directory.delete_file("_0.si"),
        Err(DirectoryError::FileNotFound { name }) if name == "_0.si"
    ));
    assert!(matches!(
        directory.rename("_0.si", "_1.si"),
        Err(DirectoryError::FileNotFound { name }) if name == "_0.si"
    ));
}

#[test]
fn memory_directory_rejects_existing_file_names() {
    let mut directory = MemoryDirectory::new();

    directory
        .write_file("_0.si", b"source")
        .expect("write source");
    directory
        .write_file("segments_1", b"target")
        .expect("write target");

    assert!(matches!(
        directory.write_file("_0.si", b"duplicate"),
        Err(DirectoryError::AlreadyExists { name }) if name == "_0.si"
    ));
    assert!(matches!(
        directory.rename("_0.si", "segments_1"),
        Err(DirectoryError::AlreadyExists { name }) if name == "segments_1"
    ));
}

#[test]
fn memory_directory_rejects_empty_names() {
    let mut directory = MemoryDirectory::new();

    assert!(matches!(
        directory.write_file("", b"bytes"),
        Err(DirectoryError::InvalidEmptyName)
    ));
    assert!(matches!(
        directory.read_file(""),
        Err(DirectoryError::InvalidEmptyName)
    ));
    assert!(matches!(
        directory.file_length(""),
        Err(DirectoryError::InvalidEmptyName)
    ));
    assert!(matches!(
        directory.contains_file(""),
        Err(DirectoryError::InvalidEmptyName)
    ));
    assert!(matches!(
        directory.delete_file(""),
        Err(DirectoryError::InvalidEmptyName)
    ));
    assert!(matches!(
        directory.rename("", "_0.si"),
        Err(DirectoryError::InvalidEmptyName)
    ));

    directory
        .write_file("_0.si", b"segment info")
        .expect("write valid source");

    assert!(matches!(
        directory.rename("_0.si", ""),
        Err(DirectoryError::InvalidEmptyName)
    ));
}

fn classic_manifest_entries() -> Vec<(&'static str, usize)> {
    CLASSIC_MANIFEST
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (name, length) = line.split_once('\t').expect("manifest line has tab");
            let length = length.parse().expect("manifest length is usize");
            (name, length)
        })
        .collect()
}

fn index_io_manifest_entries() -> Vec<(&'static str, &'static [u8])> {
    INDEX_IO_MANIFEST
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (name, bytes) = line.split_once('\t').expect("manifest line has tab");
            (name, bytes.as_bytes())
        })
        .collect()
}

fn fixture_bytes(name: &str, length: usize) -> Vec<u8> {
    name.as_bytes()
        .iter()
        .copied()
        .cycle()
        .take(length)
        .collect()
}
