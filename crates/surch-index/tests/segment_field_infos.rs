use surch_index::field_infos::{DocValuesType, FieldInfo, FieldInfos, IndexOptions};
use surch_index::segment_field_infos::{
    field_infos_file_name, read_segment_field_infos, write_segment_field_infos,
    SegmentFieldInfosError,
};

const SEGMENT_FIELD_INFOS_MANIFEST: &str =
    include_str!("../../../tests/lucene_parity/index/segment_field_infos_manifest.tsv");

#[test]
fn segment_field_infos_formats_lucene_file_name() {
    assert_eq!(field_infos_file_name("_0").unwrap(), "_0.fnm");
}

#[test]
fn segment_field_infos_rejects_invalid_segment_name() {
    let err = field_infos_file_name("0").expect_err("invalid segment name rejected");

    assert!(matches!(
        err,
        SegmentFieldInfosError::InvalidSegmentName { segment_name } if segment_name == "0"
    ));
}

#[test]
fn segment_field_infos_round_trips_manifest_fixture() {
    let (segment_name, expected_file_name, expected_fields) = fixture_segment_field_infos();

    let file =
        write_segment_field_infos(&segment_name, &expected_fields).expect("write field infos file");

    assert_eq!(file.file_name, expected_file_name);

    let decoded =
        read_segment_field_infos(&file.file_name, &file.bytes).expect("read field infos file");

    assert_eq!(decoded, expected_fields);
}

#[test]
fn segment_field_infos_rejects_invalid_file_extension() {
    let (_, _, expected_fields) = fixture_segment_field_infos();
    let file = write_segment_field_infos("_0", &expected_fields).expect("write field infos file");

    let err = read_segment_field_infos("_0.tim", &file.bytes)
        .expect_err("invalid field infos extension rejected");

    assert!(matches!(
        err,
        SegmentFieldInfosError::InvalidFileExtension { file_name } if file_name == "_0.tim"
    ));
}

fn fixture_segment_field_infos() -> (String, String, FieldInfos) {
    let records = SEGMENT_FIELD_INFOS_MANIFEST
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(parse_manifest_record)
        .collect::<Vec<_>>();
    assert!(!records.is_empty(), "manifest should include fields");

    let segment_name = records[0].segment_name.clone();
    let file_name = records[0].file_name.clone();
    let fields = records
        .iter()
        .map(|record| {
            assert_eq!(record.segment_name, segment_name);
            assert_eq!(record.file_name, file_name);
            record.field.clone()
        })
        .collect();

    (
        segment_name,
        file_name,
        FieldInfos::new(fields).expect("manifest field infos"),
    )
}

#[derive(Debug)]
struct ManifestRecord {
    segment_name: String,
    file_name: String,
    field: FieldInfo,
}

fn parse_manifest_record(line: &str) -> ManifestRecord {
    let columns = line.split('\t').collect::<Vec<_>>();
    assert_eq!(columns.len(), 8, "manifest row should have 8 columns");

    ManifestRecord {
        segment_name: columns[0].to_owned(),
        file_name: columns[1].to_owned(),
        field: FieldInfo::new(
            columns[2],
            columns[3].parse().expect("field number"),
            parse_index_options(columns[4]),
            parse_doc_values_type(columns[5]),
            parse_bool(columns[6]),
            parse_bool(columns[7]),
        ),
    }
}

fn parse_index_options(value: &str) -> IndexOptions {
    match value {
        "None" => IndexOptions::None,
        "Docs" => IndexOptions::Docs,
        "DocsAndFreqs" => IndexOptions::DocsAndFreqs,
        "DocsAndFreqsAndPositions" => IndexOptions::DocsAndFreqsAndPositions,
        "DocsAndFreqsAndPositionsAndOffsets" => IndexOptions::DocsAndFreqsAndPositionsAndOffsets,
        other => panic!("unknown index_options fixture value: {other}"),
    }
}

fn parse_doc_values_type(value: &str) -> DocValuesType {
    match value {
        "None" => DocValuesType::None,
        "Numeric" => DocValuesType::Numeric,
        "Binary" => DocValuesType::Binary,
        "Sorted" => DocValuesType::Sorted,
        "SortedNumeric" => DocValuesType::SortedNumeric,
        "SortedSet" => DocValuesType::SortedSet,
        other => panic!("unknown doc_values_type fixture value: {other}"),
    }
}

fn parse_bool(value: &str) -> bool {
    match value {
        "true" => true,
        "false" => false,
        other => panic!("unknown bool fixture value: {other}"),
    }
}
