use std::collections::{BTreeMap, BTreeSet};

use surch_store::data_io::{
    ByteArrayDataInput, ByteArrayDataOutput, DataInput, DataIoError, DataOutput,
};

#[test]
fn data_input_decodes_lucene_vint_vectors() {
    let cases: &[(&[u8], i32)] = &[
        (&[0x00], 0),
        (&[0x01], 1),
        (&[0x02], 2),
        (&[0x7f], 127),
        (&[0x80, 0x01], 128),
        (&[0x81, 0x01], 129),
        (&[0x82, 0x01], 130),
        (&[0xff, 0x7f], 16_383),
        (&[0x80, 0x80, 0x01], 16_384),
        (&[0x81, 0x80, 0x01], 16_385),
        (&[0xff, 0xff, 0xff, 0xff, 0x07], i32::MAX),
        (&[0xff, 0xff, 0xff, 0xff, 0x0f], -1),
    ];

    for (bytes, expected) in cases {
        let mut input = ByteArrayDataInput::new(bytes);

        assert_eq!(input.read_vint().expect("decode vint"), *expected);
    }
}

#[test]
fn data_input_output_encodes_lucene_vint_vectors() {
    let cases: &[(i32, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (127, &[0x7f]),
        (128, &[0x80, 0x01]),
        (16_383, &[0xff, 0x7f]),
        (16_384, &[0x80, 0x80, 0x01]),
        (i32::MAX, &[0xff, 0xff, 0xff, 0xff, 0x07]),
        (-1, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
    ];

    for (value, expected) in cases {
        let mut output = ByteArrayDataOutput::new();

        output.write_vint(*value).expect("encode vint");

        assert_eq!(output.as_slice(), *expected);
    }
}

#[test]
fn data_input_output_round_trips_vlong_boundaries() {
    let cases: &[(i64, &[u8])] = &[
        (0, &[0x00]),
        (127, &[0x7f]),
        (128, &[0x80, 0x01]),
        (
            i64::MAX,
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f],
        ),
    ];

    for (value, expected) in cases {
        let mut output = ByteArrayDataOutput::new();

        output.write_vlong(*value).expect("encode vlong");

        assert_eq!(output.as_slice(), *expected);
        let mut input = ByteArrayDataInput::new(output.as_slice());
        assert_eq!(input.read_vlong().expect("decode vlong"), *value);
    }
}

#[test]
fn data_input_output_rejects_negative_vlong() {
    let mut output = ByteArrayDataOutput::new();

    let err = output.write_vlong(-1).expect_err("negative vlong");

    assert!(matches!(err, DataIoError::NegativeVLong { value: -1 }));
}

#[test]
fn data_input_output_round_trips_zlong_boundaries() {
    let cases: &[(i64, &[u8])] = &[
        (0, &[0x00]),
        (-1, &[0x01]),
        (1, &[0x02]),
        (-2, &[0x03]),
        (
            i64::MIN,
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01],
        ),
    ];

    for (value, expected) in cases {
        let mut output = ByteArrayDataOutput::new();

        output.write_zlong(*value).expect("encode zlong");

        assert_eq!(output.as_slice(), *expected);
        let mut input = ByteArrayDataInput::new(output.as_slice());
        assert_eq!(input.read_zlong().expect("decode zlong"), *value);
    }
}

#[test]
fn data_input_reports_eof_for_truncated_vint() {
    let mut input = ByteArrayDataInput::new(&[0x80]);

    let err = input.read_vint().expect_err("truncated vint");

    assert!(matches!(err, DataIoError::UnexpectedEof { position: 1 }));
}

#[test]
fn data_input_output_round_trips_lucene_utf8_string() {
    let mut output = ByteArrayDataOutput::new();

    output
        .write_string("surch brûle")
        .expect("encode lucene string");

    assert_eq!(output.as_slice(), b"\x0csurch br\xc3\xbble");

    let mut input = ByteArrayDataInput::new(output.as_slice());

    assert_eq!(
        input.read_string().expect("decode lucene string"),
        "surch brûle"
    );
}

#[test]
fn data_input_reports_eof_for_truncated_string_bytes() {
    let mut input = ByteArrayDataInput::new(b"\x05surch"[..4].as_ref());

    let err = input.read_string().expect_err("truncated string");

    assert!(matches!(err, DataIoError::UnexpectedEof { position: 4 }));
}

#[test]
fn data_input_output_round_trips_map_of_strings_in_key_order() {
    let entries = BTreeMap::from([
        ("analyzer".to_string(), "standard".to_string()),
        ("codec".to_string(), "lucene90".to_string()),
        ("field".to_string(), "title".to_string()),
    ]);
    let mut output = ByteArrayDataOutput::new();

    output
        .write_map_of_strings(&entries)
        .expect("encode map of strings");

    assert_eq!(
        output.as_slice(),
        b"\x03\x08analyzer\x08standard\x05codec\x08lucene90\x05field\x05title"
    );

    let mut input = ByteArrayDataInput::new(output.as_slice());

    assert_eq!(
        input.read_map_of_strings().expect("decode map of strings"),
        entries
    );
}

#[test]
fn data_input_output_round_trips_set_of_strings_in_value_order() {
    let values = BTreeSet::from([
        "body".to_string(),
        "keyword".to_string(),
        "title".to_string(),
    ]);
    let mut output = ByteArrayDataOutput::new();

    output
        .write_set_of_strings(&values)
        .expect("encode set of strings");

    assert_eq!(output.as_slice(), b"\x03\x04body\x07keyword\x05title");

    let mut input = ByteArrayDataInput::new(output.as_slice());

    assert_eq!(
        input.read_set_of_strings().expect("decode set of strings"),
        values
    );
}
