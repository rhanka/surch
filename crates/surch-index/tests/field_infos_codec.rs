use surch_index::field_infos::{DocValuesType, FieldInfo, FieldInfos, IndexOptions};
use surch_index::field_infos_codec::{
    decode_field_infos, encode_field_infos, FieldInfosCodecError,
};
use surch_store::data_io::{ByteArrayDataOutput, DataOutput};

const FIELD_INFOS_CLASSIC: &str =
    include_str!("../../../tests/lucene_parity/index/field_infos_classic.tsv");

#[test]
fn field_infos_codec_round_trips_classic_fixture() {
    let expected = fixture_field_infos();

    let bytes = encode_field_infos(&expected).expect("encode field infos");
    let decoded = decode_field_infos(&bytes).expect("decode field infos");

    assert_eq!(decoded, expected);
    assert_eq!(&bytes[..4], b"SFI0");
}

#[test]
fn field_infos_codec_rejects_invalid_magic() {
    let mut bytes = encode_field_infos(&fixture_field_infos()).expect("encode field infos");
    bytes[0] = b'X';

    let err = decode_field_infos(&bytes).expect_err("invalid magic rejected");

    assert!(matches!(err, FieldInfosCodecError::InvalidMagic { actual } if actual == *b"XFI0"));
}

#[test]
fn field_infos_codec_rejects_duplicate_field_from_bytes() {
    let mut output = ByteArrayDataOutput::new();
    output.write_bytes(b"SFI0").expect("write magic");
    output.write_vint(2).expect("write count");
    write_field(&mut output, "title", 0, 1, 0, false, false);
    write_field(&mut output, "title", 1, 2, 0, false, true);

    let err = decode_field_infos(&output.into_inner()).expect_err("duplicate rejected");

    assert!(matches!(err, FieldInfosCodecError::FieldInfos(_)));
}

fn fixture_field_infos() -> FieldInfos {
    let fields = FIELD_INFOS_CLASSIC
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(parse_fixture_field)
        .collect();

    FieldInfos::new(fields).expect("fixture field infos")
}

fn parse_fixture_field(line: &str) -> FieldInfo {
    let columns = line.split('\t').collect::<Vec<_>>();
    assert_eq!(columns.len(), 6, "fixture row should have 6 columns");

    FieldInfo::new(
        columns[0],
        columns[1].parse().expect("field number"),
        parse_index_options(columns[2]),
        parse_doc_values_type(columns[3]),
        parse_bool(columns[4]),
        parse_bool(columns[5]),
    )
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

fn write_field(
    output: &mut ByteArrayDataOutput,
    name: &str,
    number: i32,
    index_options: u8,
    doc_values_type: u8,
    omit_norms: bool,
    store_payloads: bool,
) {
    output.write_string(name).expect("write name");
    output.write_vint(number).expect("write number");
    output
        .write_byte(index_options)
        .expect("write index options");
    output
        .write_byte(doc_values_type)
        .expect("write doc values type");
    output
        .write_byte(u8::from(omit_norms))
        .expect("write omit norms");
    output
        .write_byte(u8::from(store_payloads))
        .expect("write store payloads");
}
