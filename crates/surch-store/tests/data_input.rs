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
