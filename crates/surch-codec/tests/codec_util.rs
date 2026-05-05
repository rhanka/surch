use surch_codec::codec_util::{
    check_footer, check_header, checksum_entire_file, crc32_zlib, footer_length, header_length,
    retrieve_checksum, write_footer, write_header, CodecUtilError, CODEC_MAGIC, FOOTER_MAGIC,
};

#[test]
fn codec_util_writes_and_checks_lucene_header() {
    let mut bytes = Vec::new();

    write_header(&mut bytes, "Lucene90", 7).expect("write header");

    assert_eq!(header_length("Lucene90"), 17);
    assert_eq!(&bytes[..4], &CODEC_MAGIC.to_be_bytes());
    assert_eq!(
        &bytes[4..13],
        &[0x08, b'L', b'u', b'c', b'e', b'n', b'e', b'9', b'0']
    );
    assert_eq!(&bytes[13..17], &7_i32.to_be_bytes());

    let header = check_header(&bytes, "Lucene90", 6, 8).expect("check header");

    assert_eq!(header.version, 7);
    assert_eq!(header.length, bytes.len());
}

#[test]
fn codec_util_rejects_invalid_header_inputs() {
    let mut bytes = Vec::new();
    write_header(&mut bytes, "Lucene90", 7).expect("write header");

    bytes[0] = 0;
    let err = check_header(&bytes, "Lucene90", 6, 8).expect_err("bad magic");
    assert!(matches!(err, CodecUtilError::HeaderMismatch { .. }));

    let mut bytes = Vec::new();
    write_header(&mut bytes, "Lucene90", 7).expect("write header");
    let err = check_header(&bytes, "Other90", 6, 8).expect_err("bad codec");
    assert!(matches!(err, CodecUtilError::CodecMismatch { .. }));

    let err = check_header(&bytes, "Lucene90", 8, 9).expect_err("old version");
    assert!(matches!(
        err,
        CodecUtilError::VersionTooOld { actual: 7, .. }
    ));

    let err = check_header(&bytes, "Lucene90", 1, 6).expect_err("new version");
    assert!(matches!(
        err,
        CodecUtilError::VersionTooNew { actual: 7, .. }
    ));
}

#[test]
fn codec_util_rejects_non_ascii_or_long_codec_names() {
    let mut bytes = Vec::new();

    let err = write_header(&mut bytes, "Luceneé", 1).expect_err("non ascii");
    assert!(matches!(err, CodecUtilError::InvalidCodecName { .. }));

    let long_name = "a".repeat(128);
    let err = write_header(&mut bytes, &long_name, 1).expect_err("too long");
    assert!(matches!(err, CodecUtilError::InvalidCodecName { .. }));
}

#[test]
fn codec_util_crc32_matches_zlib_standard_vector() {
    assert_eq!(crc32_zlib(b"123456789"), 0xcbf4_3926);
}

#[test]
fn codec_util_writes_and_checks_lucene_footer() {
    let mut bytes = b"body".to_vec();

    write_footer(&mut bytes);

    let footer_start = bytes.len() - footer_length();
    assert_eq!(
        &bytes[footer_start..footer_start + 4],
        &FOOTER_MAGIC.to_be_bytes()
    );
    assert_eq!(&bytes[footer_start + 4..footer_start + 8], &[0, 0, 0, 0]);
    assert_eq!(&bytes[footer_start + 8..footer_start + 12], &[0, 0, 0, 0]);

    let expected = crc32_zlib(&bytes[..bytes.len() - 8]);
    assert_eq!(
        retrieve_checksum(&bytes).expect("retrieve checksum"),
        expected
    );
    assert_eq!(check_footer(&bytes).expect("check footer"), expected);
    assert_eq!(
        checksum_entire_file(&bytes).expect("checksum file"),
        expected
    );
}

#[test]
fn codec_util_rejects_corrupt_lucene_footer() {
    let mut bytes = b"body".to_vec();
    write_footer(&mut bytes);

    let mut corrupt_magic = bytes.clone();
    let footer_start = corrupt_magic.len() - footer_length();
    corrupt_magic[footer_start] = 0;
    let err = check_footer(&corrupt_magic).expect_err("bad footer magic");
    assert!(matches!(err, CodecUtilError::FooterMismatch { .. }));

    let mut corrupt_algorithm = bytes.clone();
    let footer_start = corrupt_algorithm.len() - footer_length();
    corrupt_algorithm[footer_start + 7] = 1;
    let err = check_footer(&corrupt_algorithm).expect_err("bad algorithm");
    assert!(matches!(
        err,
        CodecUtilError::UnknownChecksumAlgorithm { algorithm_id: 1 }
    ));

    let mut corrupt_checksum = bytes.clone();
    let last = corrupt_checksum.len() - 1;
    corrupt_checksum[last] ^= 1;
    let err = check_footer(&corrupt_checksum).expect_err("bad checksum");
    assert!(matches!(err, CodecUtilError::ChecksumMismatch { .. }));
}
