use surch_index::field_infos::{DocValuesType, FieldInfo, FieldInfos, IndexOptions};
use surch_index::segment_field_infos::{read_segment_field_infos, SegmentFieldInfosError};
use surch_index::segment_infos::SegmentInfos;
use surch_index::segment_manifest::{
    build_single_segment_manifest, SegmentFile, SegmentManifestError,
};

const SEGMENT_MANIFEST_CLASSIC: &str =
    include_str!("../../../tests/lucene_parity/index/segment_manifest_classic.tsv");

#[test]
fn segment_manifest_lists_classic_bundle_files() {
    let manifest = build_single_segment_manifest("_0", &fixture_field_infos(), &[0x4d; 16])
        .expect("build segment manifest");

    assert_eq!(
        manifest_file_names(&manifest.files),
        expected_manifest_file_names()
    );
}

#[test]
fn segment_manifest_round_trips_field_infos_and_segment_infos() {
    let field_infos = fixture_field_infos();
    let commit_id = [0x29; 16];

    let manifest = build_single_segment_manifest("_0", &field_infos, &commit_id)
        .expect("build segment manifest");

    let field_infos_file = find_file(&manifest.files, "_0.fnm");
    let decoded_field_infos =
        read_segment_field_infos(&field_infos_file.name, &field_infos_file.bytes)
            .expect("read field infos");
    assert_eq!(decoded_field_infos, field_infos);

    let commit_file = find_file(&manifest.files, "segments_1");
    let decoded_commit =
        SegmentInfos::read_commit(&commit_file.name, &commit_file.bytes).expect("read commit");

    assert_eq!(decoded_commit.id(), Some(commit_id));
    assert_eq!(decoded_commit.segments.len(), 1);
    let segment = &decoded_commit.segments[0];
    assert_eq!(segment.metadata.name, "_0");
    assert!(segment.field_infos_files.contains("_0.fnm"));
}

#[test]
fn segment_manifest_rejects_invalid_segment_name() {
    let err = build_single_segment_manifest("0", &fixture_field_infos(), &[0x18; 16])
        .expect_err("invalid segment name rejected");

    assert!(matches!(
        err,
        SegmentManifestError::FieldInfos(SegmentFieldInfosError::InvalidSegmentName {
            segment_name
        }) if segment_name == "0"
    ));
}

fn expected_manifest_file_names() -> Vec<String> {
    SEGMENT_MANIFEST_CLASSIC
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

fn manifest_file_names(files: &[SegmentFile]) -> Vec<String> {
    files.iter().map(|file| file.name.clone()).collect()
}

fn find_file<'a>(files: &'a [SegmentFile], name: &str) -> &'a SegmentFile {
    files
        .iter()
        .find(|file| file.name == name)
        .unwrap_or_else(|| panic!("missing manifest file: {name}"))
}

fn fixture_field_infos() -> FieldInfos {
    FieldInfos::new(vec![
        FieldInfo::new(
            "title",
            0,
            IndexOptions::DocsAndFreqsAndPositions,
            DocValuesType::Sorted,
            false,
            true,
        ),
        FieldInfo::new(
            "body",
            1,
            IndexOptions::DocsAndFreqsAndPositionsAndOffsets,
            DocValuesType::None,
            false,
            true,
        ),
    ])
    .expect("fixture field infos")
}
