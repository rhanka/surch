use surch_store::index_io::{
    ByteArrayIndexInput, ByteArrayIndexOutput, IndexInput, IndexInputError, IndexOutput,
};

const CLASSIC_BYTES: &[u8] =
    include_bytes!("../../../tests/lucene_parity/store/index_io_classic.bytes");
const CLASSIC_CRC32: u32 = 0xe011_7757;

#[test]
fn index_io_output_writes_bytes_and_tracks_pointer_and_checksum() {
    let mut output = ByteArrayIndexOutput::new();

    output
        .write_byte(CLASSIC_BYTES[0])
        .expect("write first byte");
    output
        .write_bytes(&CLASSIC_BYTES[1..])
        .expect("write remaining bytes");

    assert_eq!(output.file_pointer(), CLASSIC_BYTES.len() as u64);
    assert_eq!(output.as_slice(), CLASSIC_BYTES);
    assert_eq!(output.checksum(), CLASSIC_CRC32);
}

#[test]
fn index_io_input_round_trips_fixture_sequentially() {
    let mut input = ByteArrayIndexInput::new(CLASSIC_BYTES);
    let mut actual = vec![0; CLASSIC_BYTES.len()];

    input.read_bytes(&mut actual).expect("read fixture bytes");

    assert_eq!(actual, CLASSIC_BYTES);
    assert_eq!(input.file_pointer(), CLASSIC_BYTES.len() as u64);
    assert_eq!(input.length(), CLASSIC_BYTES.len() as u64);
}

#[test]
fn index_io_input_seek_repositions_reads() {
    let mut input = ByteArrayIndexInput::new(CLASSIC_BYTES);

    input.seek(4).expect("seek to fifth byte");

    assert_eq!(input.file_pointer(), 4);
    assert_eq!(input.read_byte().expect("read after seek"), b'5');
    assert_eq!(input.file_pointer(), 5);
}

#[test]
fn index_io_input_slice_isolated_to_requested_range() {
    let input = ByteArrayIndexInput::new(CLASSIC_BYTES);
    let mut slice = input.slice("middle", 3, 4).expect("create slice");
    let mut actual = [0; 4];

    slice.read_bytes(&mut actual).expect("read slice bytes");

    assert_eq!(&actual, b"4567");
    assert_eq!(slice.length(), 4);
    assert_eq!(slice.file_pointer(), 4);
    assert!(matches!(
        slice.read_byte(),
        Err(IndexInputError::UnexpectedEof { position: 4 })
    ));
}

#[test]
fn index_io_input_reports_eof_and_invalid_seek() {
    let mut input = ByteArrayIndexInput::new(CLASSIC_BYTES);
    input
        .seek(CLASSIC_BYTES.len() as u64)
        .expect("seek to eof is valid");

    assert!(matches!(
        input.read_byte(),
        Err(IndexInputError::UnexpectedEof { position }) if position == CLASSIC_BYTES.len() as u64
    ));
    assert!(matches!(
        input.seek(CLASSIC_BYTES.len() as u64 + 1),
        Err(IndexInputError::SeekPastEof { position, length })
            if position == CLASSIC_BYTES.len() as u64 + 1
                && length == CLASSIC_BYTES.len() as u64
    ));
}

#[test]
fn index_io_input_rejects_slices_outside_bounds() {
    let input = ByteArrayIndexInput::new(CLASSIC_BYTES);

    assert!(matches!(
        input.slice("past-end", CLASSIC_BYTES.len() as u64 - 1, 2),
        Err(IndexInputError::SliceOutOfBounds {
            offset,
            length: 2,
            input_length,
        }) if offset == CLASSIC_BYTES.len() as u64 - 1
            && input_length == CLASSIC_BYTES.len() as u64
    ));
}
